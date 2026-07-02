use crate::state::AppState;
use sqlx::Row;
use std::time::Duration;
use uuid::Uuid;

/// A stored screenshot older than this is stale and eligible for a fresh
/// capture even when the current deployment already has one. Default 30
/// days; overridable (in whole days) for operators who want a tighter or
/// looser refresh cadence.
const DEFAULT_RECAPTURE_MAX_AGE_DAYS: i64 = 30;

fn recapture_max_age() -> chrono::Duration {
    let days = std::env::var("HOSTLET_SCREENSHOT_RECAPTURE_DAYS")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|days| *days > 0)
        .unwrap_or(DEFAULT_RECAPTURE_MAX_AGE_DAYS);
    chrono::Duration::days(days)
}

/// Whether `enqueue_auto_screenshot_for_deployment` should queue a capture
/// for `deployment_id`: either the current deployment has no `generated`
/// screenshot yet, or the newest screenshot for the app (any deployment or
/// source) has aged past `recapture_max_age`.
pub(super) async fn should_enqueue_capture(
    state: &AppState,
    app_id: Uuid,
    deployment_id: Uuid,
) -> anyhow::Result<bool> {
    let row = sqlx::query(
        "SELECT
           EXISTS(
             SELECT 1 FROM app_screenshots
             WHERE app_id=$1 AND deployment_id=$2 AND source=$3
           ) AS has_current,
           (SELECT MAX(captured_at) FROM app_screenshots WHERE app_id=$1) AS newest_captured_at",
    )
    .bind(app_id)
    .bind(deployment_id)
    .bind(super::GENERATED_SOURCE)
    .fetch_one(&state.db)
    .await?;
    if !row.get::<bool, _>("has_current") {
        return Ok(true);
    }
    let newest_captured_at: Option<chrono::DateTime<chrono::Utc>> = row.get("newest_captured_at");
    Ok(newest_captured_at.is_none_or(|captured_at| {
        chrono::Utc::now().signed_duration_since(captured_at) > recapture_max_age()
    }))
}

/// A 30-day staleness threshold only needs infrequent checking; this keeps
/// the sweep off the hot path while still catching stale portfolio
/// screenshots within a day of crossing the threshold.
const RECAPTURE_SWEEP_INTERVAL: Duration = Duration::from_secs(6 * 3600);

/// Periodic sibling of `sweep_orphaned_screenshot_files`: instead of
/// reaping files with no DB row, it re-queues captures for apps whose
/// newest screenshot has gone stale. Mirrors
/// `runtime_recovery::spawn_runtime_recovery_task`'s ticker shape (an
/// immediate first tick consumed before the loop, `Delay` on missed ticks)
/// since — unlike the orphan sweep — this needs to actually recur.
pub fn spawn_recapture_sweep_task(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(RECAPTURE_SWEEP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            sweep_stale_screenshots(&state).await;
        }
    });
}

#[cfg(test)]
pub(super) async fn sweep_stale_screenshots_for_test(state: &AppState) {
    sweep_stale_screenshots(state).await;
}

async fn sweep_stale_screenshots(state: &AppState) {
    let candidates = match stale_screenshot_deployment_ids(state).await {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(error = %err, "screenshot recapture sweep: failed to query candidates");
            return;
        }
    };

    let mut enqueued: u32 = 0;
    for deployment_id in candidates {
        // Delegates to the same gated enqueue path deploy-time capture uses
        // (screenshot_hooks veto, in-flight job dedupe, staleness check) so
        // this sweep can't bypass those checks.
        match super::enqueue_auto_screenshot_for_deployment(state, deployment_id).await {
            Ok(Some(_)) => enqueued += 1,
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    %deployment_id,
                    "screenshot recapture sweep: failed to enqueue capture"
                );
            }
        }
    }

    if enqueued > 0 {
        tracing::info!(
            enqueued,
            "screenshot recapture sweep: enqueued stale screenshot captures"
        );
    }
}

async fn stale_screenshot_deployment_ids(state: &AppState) -> anyhow::Result<Vec<Uuid>> {
    let cutoff = chrono::Utc::now() - recapture_max_age();
    let rows = sqlx::query(
        "SELECT d.id AS deployment_id
         FROM apps a
         JOIN deployments d ON d.id = a.current_deployment_id
         LEFT JOIN (
           SELECT app_id, MAX(captured_at) AS newest_captured_at
           FROM app_screenshots
           GROUP BY app_id
         ) s ON s.app_id = a.id
         WHERE a.public_exposure = true
           AND d.status = ANY($1)
           AND (s.newest_captured_at IS NULL OR s.newest_captured_at < $2)",
    )
    .bind(super::LIVE_DEPLOYMENT_STATUSES)
    .bind(cutoff)
    .fetch_all(&state.db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| row.get::<Uuid, _>("deployment_id"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recapture_max_age_defaults_to_30_days() {
        std::env::remove_var("HOSTLET_SCREENSHOT_RECAPTURE_DAYS");
        assert_eq!(recapture_max_age(), chrono::Duration::days(30));
    }

    #[test]
    fn recapture_max_age_honors_env_override() {
        std::env::set_var("HOSTLET_SCREENSHOT_RECAPTURE_DAYS", "7");
        assert_eq!(recapture_max_age(), chrono::Duration::days(7));
        std::env::remove_var("HOSTLET_SCREENSHOT_RECAPTURE_DAYS");
    }

    #[test]
    fn recapture_max_age_ignores_invalid_override() {
        std::env::set_var("HOSTLET_SCREENSHOT_RECAPTURE_DAYS", "not-a-number");
        assert_eq!(recapture_max_age(), chrono::Duration::days(30));
        std::env::remove_var("HOSTLET_SCREENSHOT_RECAPTURE_DAYS");
    }
}
