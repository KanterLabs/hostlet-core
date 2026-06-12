//! Shared cleanup planner and enqueue logic for automatic container reaping.
//!
//! **Why this file exists:** cloud's overlay replaces whole files. Any cleanup
//! helper defined in `web/cleanup.rs` (which cloud overrides) would become an
//! unmanaged fork the moment cloud customises that file. Placing the shared,
//! stable planner here keeps it outside the override boundary so cloud inherits
//! changes automatically via the `vendor/hostlet-core` submodule.
//!
//! Placement rule (mandatory): shared helpers must live here (or in another
//! file not listed in the cloud override set). `web/cleanup.rs` is overridden;
//! this file is inherited. See `AGENTS.md` for the full override inventory.
//!
//! The manual-cleanup surface (`cleanup_plan`, `run_cleanup`, and their types)
//! is `pub`, not `pub(crate)`: the only core caller is `web/cleanup.rs`, which
//! cloud's overlay replaces wholesale — under `pub(crate)` the overlay build
//! flags this surface as dead code (`-D warnings`) until cloud's fork adopts it.

use crate::{deploy, state::AppState};
use anyhow::Context;
use serde::Serialize;
use sqlx::Row;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct CleanupPlan {
    pub(crate) retention: CleanupRetention,
    pub(crate) database: CleanupDatabasePreview,
    pub(crate) docker: CleanupDockerPreview,
    #[serde(skip_serializing)]
    pub(crate) local_server_id: Option<Uuid>,
    #[serde(skip_serializing)]
    pub(crate) keep_containers: Vec<String>,
    #[serde(skip_serializing)]
    pub(crate) keep_images: Vec<String>,
}

#[derive(Serialize)]
pub struct CleanupRetention {
    pub(crate) deployment_log_days: i64,
    pub(crate) deployments_per_app: i64,
    pub(crate) health_event_days: i64,
    pub(crate) health_events_per_app: i64,
    pub(crate) resource_snapshot_days: i64,
    pub(crate) resource_snapshots_per_app: i64,
    pub(crate) webhook_event_days: i64,
    pub(crate) completed_agent_job_days: i64,
    pub(crate) failed_agent_job_days: i64,
}

#[derive(Serialize)]
pub struct CleanupDatabasePreview {
    pub(crate) deployment_logs: i64,
    pub(crate) health_events: i64,
    pub(crate) resource_snapshots: i64,
    pub(crate) webhook_events: i64,
    pub(crate) completed_agent_jobs: i64,
    pub(crate) failed_agent_jobs: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupDockerPreview {
    pub(crate) keep_containers: usize,
    pub(crate) keep_images: usize,
    pub(crate) stale_deployment_containers: i64,
    pub(crate) job_will_run: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupDatabaseDeleted {
    pub(crate) deployment_logs: u64,
    pub(crate) health_events: u64,
    pub(crate) resource_snapshots: u64,
    pub(crate) webhook_events: u64,
    pub(crate) completed_agent_jobs: u64,
    pub(crate) failed_agent_jobs: u64,
}

/// Outcome of a full manual cleanup (database purge + Docker job enqueue).
pub struct CleanupOutcome {
    pub database_deleted: CleanupDatabaseDeleted,
    pub docker_cleanup_job_id: Option<Uuid>,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const RETENTION: CleanupRetention = CleanupRetention {
    deployment_log_days: 30,
    deployments_per_app: 20,
    health_event_days: 7,
    health_events_per_app: 500,
    resource_snapshot_days: 7,
    resource_snapshots_per_app: 1000,
    webhook_event_days: 14,
    completed_agent_job_days: 30,
    failed_agent_job_days: 90,
};

const LIVE_DOCKER_DEPLOYMENT_STATUSES: &[&str] = &["success", "rolled_back"];

/// SQL body of the `candidates` CTE shared by both the keep-list query and the
/// stale-container count query.  Factored here so the two queries cannot diverge.
///
/// `$1` = protected statuses (active + success + rolled_back).  `$4` (`bigint`)
/// = keep-failed-hours knob: when > 0, recently-finished
/// `failed`/`expired`/`cancelled` deployments also enter the CTE.  `rn` is
/// CASE-gated AND partition-gated on `d.status = ANY($1)` so those extra rows
/// get `NULL` rn and cannot displace protected rows from `rn <= keep_previous + 1`
/// slots (same mechanism as `success_rn`).
///
/// `success_rn` ranks non-current success rows per app by `finished_at DESC`.
/// It protects rollback targets even when a `rolled_back` row occupies rn=2 and
/// pushes the most-recent success to rn=3.  See `deploy.rs::create_and_send_rollback`.
const CANDIDATES_CTE_BODY: &str = r#"SELECT d.id, d.container_name, d.image_tag, d.status,
     CASE WHEN d.status = ANY($1) THEN
       row_number() OVER (
         PARTITION BY d.app_id, (d.status = ANY($1))
         ORDER BY CASE WHEN a.current_deployment_id=d.id THEN 0 ELSE 1 END,
                  d.finished_at DESC NULLS LAST, d.created_at DESC
       )
     END AS rn,
     CASE WHEN d.status='success' AND d.id IS DISTINCT FROM a.current_deployment_id THEN
       row_number() OVER (
         PARTITION BY d.app_id,
           (d.status='success' AND d.id IS DISTINCT FROM a.current_deployment_id)
         ORDER BY d.finished_at DESC NULLS LAST, d.created_at DESC
       )
     END AS success_rn
  FROM deployments d JOIN apps a ON a.id=d.app_id
  WHERE d.status = ANY($1)
     OR ($4::bigint > 0
         AND d.status IN ('failed','expired','cancelled')
         AND COALESCE(d.finished_at,d.updated_at,d.created_at)
               >= now() - ($4::bigint * interval '1 hour'))"#;

/// Keep predicate used in both the keep-list query and the stale-count's
/// `protected` sub-select.  `$2` = active statuses, `$3` = keep_previous.
///
/// Keeps a row when any of: (1) `status = ANY($2)` — in-progress, never reap;
/// (2) `rn <= $3 + 1` — current or within the keep_previous window; (3)
/// `success_rn <= $3` — rollback targets protected even when pushed past rn+1
/// by a rolled_back row; (4) `status IN ('failed','expired','cancelled')` —
/// recency enforced by the CTE WHERE, so this is a no-op when `$4 = 0`.
/// Status list here must stay identical to the one in `CANDIDATES_CTE_BODY`.
const KEEP_PREDICATE: &str =
    "status = ANY($2) OR rn <= $3 + 1 OR success_rn <= $3 OR status IN ('failed','expired','cancelled')";

// ---------------------------------------------------------------------------
// Environment knobs
// ---------------------------------------------------------------------------

const DEFAULT_CLEANUP_KEEP_PREVIOUS: i64 = 1;

/// Return the `HOSTLET_CLEANUP_KEEP_PREVIOUS` value, defaulting to
/// [`DEFAULT_CLEANUP_KEEP_PREVIOUS`] when unset or invalid.
///
/// Zero is always rejected: the most-recent previous successful container is
/// the rollback target and must never be reaped.  Valid range: `[1, 100]`.
pub(crate) fn cleanup_keep_previous() -> i64 {
    std::env::var("HOSTLET_CLEANUP_KEEP_PREVIOUS")
        .ok()
        .and_then(|v| cleanup_keep_previous_value(&v))
        .unwrap_or(DEFAULT_CLEANUP_KEEP_PREVIOUS)
}

/// Pure validator for `HOSTLET_CLEANUP_KEEP_PREVIOUS`.
///
/// Returns `None` for empty, non-numeric, zero, or out-of-range input.
fn cleanup_keep_previous_value(value: &str) -> Option<i64> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    v.parse::<i64>().ok().filter(|n| (1..=100).contains(n))
}

const DEFAULT_CLEANUP_KEEP_FAILED_HOURS: i64 = 0;

/// Return the `HOSTLET_CLEANUP_KEEP_FAILED_HOURS` value, defaulting to
/// [`DEFAULT_CLEANUP_KEEP_FAILED_HOURS`] (disabled) when unset or invalid.
///
/// When > 0, keeps containers/images of `failed`/`expired`/`cancelled`
/// deployments whose `COALESCE(finished_at,updated_at,created_at)` is within
/// that many hours, so failed tenants stay debuggable.  Honoured by all
/// cleanup paths.  Valid range: `[0, 720]` (0 = disabled, 720 = 30 days).
pub(crate) fn cleanup_keep_failed_hours() -> i64 {
    std::env::var("HOSTLET_CLEANUP_KEEP_FAILED_HOURS")
        .ok()
        .and_then(|v| cleanup_keep_failed_hours_value(&v))
        .unwrap_or(DEFAULT_CLEANUP_KEEP_FAILED_HOURS)
}

/// Pure validator for `HOSTLET_CLEANUP_KEEP_FAILED_HOURS`.
///
/// Returns `None` for empty, non-numeric, negative, or > 720 input.  Zero is
/// VALID and means disabled.
fn cleanup_keep_failed_hours_value(value: &str) -> Option<i64> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    v.parse::<i64>().ok().filter(|n| (0..=720).contains(n))
}

/// Return `true` unless `HOSTLET_AUTO_CLEANUP` is set to a falsy value.
///
/// This gate **defaults to enabled** — the opposite of [`crate::env::bool_env`]
/// which defaults to `false`.  Do NOT reuse `bool_env` here.
pub(crate) fn auto_cleanup_enabled() -> bool {
    std::env::var("HOSTLET_AUTO_CLEANUP")
        .map(|v| auto_cleanup_enabled_value(&v))
        .unwrap_or(true)
}

/// Pure validator for `HOSTLET_AUTO_CLEANUP`.
///
/// Returns `false` only for trimmed, ASCII-lowercased `"0"`, `"false"`, or
/// `"no"`.  Any other value (including unrecognised strings) is treated as
/// enabled so that opt-out is explicit and forward-compatible.
fn auto_cleanup_enabled_value(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no"
    )
}

// ---------------------------------------------------------------------------
// Private shared helper
// ---------------------------------------------------------------------------

struct DockerKeepLists {
    keep_containers: Vec<String>,
    keep_images: Vec<String>,
}

/// Query the keep lists for Docker cleanup: the containers and images that
/// must NOT be removed by the `docker_cleanup` agent job.
async fn docker_keep_lists(
    state: &AppState,
    keep_previous: i64,
    keep_failed_hours: i64,
) -> anyhow::Result<DockerKeepLists> {
    let active_statuses = deployment_status_strings(deploy::ACTIVE_DEPLOYMENT_STATUSES);
    let protected_statuses = docker_cleanup_keep_statuses();
    let keep_rows = sqlx::query(&format!(
        r#"WITH candidates AS ({CANDIDATES_CTE_BODY})
        SELECT container_name, image_tag
        FROM candidates
        WHERE {KEEP_PREDICATE}"#
    ))
    .bind(&protected_statuses)
    .bind(&active_statuses)
    .bind(keep_previous)
    .bind(keep_failed_hours)
    .fetch_all(&state.db)
    .await?;
    let mut keep_containers = keep_rows
        .iter()
        .filter_map(|row| row.get::<Option<String>, _>("container_name"))
        .collect::<Vec<_>>();
    keep_containers.sort();
    keep_containers.dedup();
    let mut keep_images = keep_rows
        .iter()
        .filter_map(|row| row.get::<Option<String>, _>("image_tag"))
        .collect::<Vec<_>>();
    keep_images.sort();
    keep_images.dedup();
    Ok(DockerKeepLists {
        keep_containers,
        keep_images,
    })
}

/// Count deployments whose container would be reaped by a Docker cleanup with
/// the given `keep_previous` setting.
async fn stale_container_count(
    state: &AppState,
    keep_previous: i64,
    keep_failed_hours: i64,
) -> anyhow::Result<i64> {
    let active_statuses = deployment_status_strings(deploy::ACTIVE_DEPLOYMENT_STATUSES);
    let protected_statuses = docker_cleanup_keep_statuses();
    sqlx::query_scalar::<_, i64>(&format!(
        r#"WITH candidates AS ({CANDIDATES_CTE_BODY}),
        protected AS (
          SELECT id FROM candidates
          WHERE {KEEP_PREDICATE}
        )
        SELECT count(*)::bigint
        FROM deployments d
        WHERE d.container_name IS NOT NULL
          AND NOT EXISTS (SELECT 1 FROM protected p WHERE p.id=d.id)"#
    ))
    .bind(&protected_statuses)
    .bind(&active_statuses)
    .bind(keep_previous)
    .bind(keep_failed_hours)
    .fetch_one(&state.db)
    .await
    .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// Public functions
// ---------------------------------------------------------------------------

/// Build a full cleanup plan: database row counts + Docker keep lists + stale
/// container count.  Used by both the HTTP preview endpoint and [`run_cleanup`].
pub async fn cleanup_plan(state: &AppState) -> anyhow::Result<CleanupPlan> {
    cleanup_plan_with_keep_failed_hours(state, cleanup_keep_failed_hours()).await
}

/// [`cleanup_plan`] with an explicit keep-failed-hours value.  `pub` so cloud's
/// overlay fork and DB-gated tests can inject the knob without env-var side effects.
pub async fn cleanup_plan_with_keep_failed_hours(
    state: &AppState,
    keep_failed_hours: i64,
) -> anyhow::Result<CleanupPlan> {
    let database = CleanupDatabasePreview {
        deployment_logs: cleanup_count(state, &cleanup_deployment_logs_sql()).await?,
        health_events: cleanup_count(state, &HEALTH_EVENTS_RULE.count_sql()).await?,
        resource_snapshots: cleanup_count(state, &RESOURCE_SNAPSHOTS_RULE.count_sql()).await?,
        webhook_events: cleanup_count(state, &WEBHOOK_EVENTS_RULE.count_sql()).await?,
        completed_agent_jobs: cleanup_count(state, &COMPLETED_AGENT_JOBS_RULE.count_sql()).await?,
        failed_agent_jobs: cleanup_count(state, &FAILED_AGENT_JOBS_RULE.count_sql()).await?,
    };
    let keep_previous = cleanup_keep_previous();
    let lists = docker_keep_lists(state, keep_previous, keep_failed_hours).await?;
    let stale_deployment_containers =
        stale_container_count(state, keep_previous, keep_failed_hours).await?;
    let local_server_id =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM servers WHERE kind='local' LIMIT 1")
            .fetch_optional(&state.db)
            .await?;
    Ok(CleanupPlan {
        retention: RETENTION,
        database,
        docker: CleanupDockerPreview {
            keep_containers: lists.keep_containers.len(),
            keep_images: lists.keep_images.len(),
            stale_deployment_containers,
            job_will_run: local_server_id.is_some(),
        },
        local_server_id,
        keep_containers: lists.keep_containers,
        keep_images: lists.keep_images,
    })
}

/// Run a full cleanup: build the plan, purge database rows, and enqueue a
/// Docker cleanup job (best-effort — a job-enqueue failure does NOT fail the
/// call).  Records no audit event; the caller (`web/cleanup.rs`) does that.
pub async fn run_cleanup(state: &AppState) -> anyhow::Result<CleanupOutcome> {
    let plan = cleanup_plan(state)
        .await
        .context("failed to build cleanup plan")?;
    let database_deleted = apply_database_cleanup(state)
        .await
        .context("database cleanup failed")?;
    let docker_cleanup_job_id = if let Some(server_id) = plan.local_server_id {
        match deploy::enqueue_agent_job(
            state,
            server_id,
            None,
            None,
            "docker_cleanup",
            serde_json::json!({
                "type": "docker_cleanup",
                "keep_containers": plan.keep_containers,
                "keep_images": plan.keep_images,
                "dry_run": false,
            }),
            50,
        )
        .await
        {
            Ok(job_id) => Some(job_id),
            Err(err) => {
                tracing::warn!(error = %err, "failed to enqueue Docker cleanup job");
                None
            }
        }
    } else {
        None
    };
    Ok(CleanupOutcome {
        database_deleted,
        docker_cleanup_job_id,
    })
}

/// Enqueue an automatic Docker cleanup for a specific server.
///
/// Returns `Ok(None)` when auto-cleanup is disabled (HOSTLET_AUTO_CLEANUP=0),
/// `Ok(Some(job_id))` on success, or `Err` if the keep-list query or job
/// insert fails.  Never purges database rows (auto path only reaps Docker
/// state); never records an audit event (the `agent_jobs` row is the record).
///
/// Before enqueueing, cancels any existing queued `docker_cleanup` jobs for
/// this server whose keep lists may be stale.
pub async fn auto_cleanup_for_server(
    state: &AppState,
    server_id: Uuid,
) -> anyhow::Result<Option<Uuid>> {
    if !auto_cleanup_enabled() {
        return Ok(None);
    }
    let keep_previous = cleanup_keep_previous();
    let keep_failed_hours = cleanup_keep_failed_hours();
    let lists = docker_keep_lists(state, keep_previous, keep_failed_hours).await?;
    // Supersede any queued docker_cleanup jobs for this server whose keep lists
    // predate the current deployment state.  Only 'queued' jobs are targeted;
    // a job that is already 'claimed' or 'running' is left to complete.
    let _ = sqlx::query(
        "UPDATE agent_jobs
         SET status='cancelled',
             failure_summary='Superseded by a newer automatic Docker cleanup job.',
             last_error='Superseded by a newer automatic Docker cleanup job.',
             finished_at=now(),
             updated_at=now()
         WHERE server_id=$1
           AND job_type='docker_cleanup'
           AND status='queued'",
    )
    .bind(server_id)
    .execute(&state.db)
    .await;
    let job_id = deploy::enqueue_agent_job(
        state,
        server_id,
        None,
        None,
        "docker_cleanup",
        serde_json::json!({
            "type": "docker_cleanup",
            "keep_containers": lists.keep_containers,
            "keep_images": lists.keep_images,
            "dry_run": false,
        }),
        50,
    )
    .await?;
    Ok(Some(job_id))
}

/// Best-effort sweep: enqueue automatic Docker cleanup for every server that
/// has deployments.  Individual failures are logged and skipped; this function
/// never returns `Err` and never panics.
pub async fn auto_cleanup_sweep(state: &AppState) {
    if !auto_cleanup_enabled() {
        return;
    }
    let server_ids: Vec<Uuid> = match sqlx::query_scalar(
        "SELECT DISTINCT server_id FROM deployments WHERE server_id IS NOT NULL",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(ids) => ids,
        Err(err) => {
            tracing::warn!(error = %err, "auto_cleanup_sweep: failed to list server ids");
            return;
        }
    };
    for server_id in server_ids {
        if let Err(err) = auto_cleanup_for_server(state, server_id).await {
            tracing::warn!(
                error = %err,
                %server_id,
                "auto_cleanup_sweep: failed to enqueue cleanup for server"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

pub(crate) fn deployment_status_strings(statuses: &[&str]) -> Vec<String> {
    statuses
        .iter()
        .map(|status| (*status).to_string())
        .collect()
}

pub(crate) fn docker_cleanup_keep_statuses() -> Vec<String> {
    let mut statuses = deployment_status_strings(deploy::ACTIVE_DEPLOYMENT_STATUSES);
    statuses.extend(deployment_status_strings(LIVE_DOCKER_DEPLOYMENT_STATUSES));
    statuses
}

pub(crate) async fn cleanup_count(state: &AppState, sql: &str) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(sql).fetch_one(&state.db).await?)
}

pub(crate) async fn cleanup_delete(state: &AppState, sql: &str) -> anyhow::Result<u64> {
    Ok(sqlx::query(sql).execute(&state.db).await?.rows_affected())
}

pub(crate) async fn apply_database_cleanup(
    state: &AppState,
) -> anyhow::Result<CleanupDatabaseDeleted> {
    Ok(CleanupDatabaseDeleted {
        deployment_logs: cleanup_delete(state, &delete_deployment_logs_sql()).await?,
        health_events: cleanup_delete(state, &HEALTH_EVENTS_RULE.delete_sql()).await?,
        resource_snapshots: cleanup_delete(state, &RESOURCE_SNAPSHOTS_RULE.delete_sql()).await?,
        webhook_events: cleanup_delete(state, &WEBHOOK_EVENTS_RULE.delete_sql()).await?,
        completed_agent_jobs: cleanup_delete(state, &COMPLETED_AGENT_JOBS_RULE.delete_sql())
            .await?,
        failed_agent_jobs: cleanup_delete(state, &FAILED_AGENT_JOBS_RULE.delete_sql()).await?,
    })
}

/// A retention rule expressed as a single SQL predicate that selects the rows
/// to purge from one table.  The matching count and delete statements are then
/// derived from this one definition so a rule change only edits one place.
pub(crate) struct RetentionRule {
    /// `FROM`-clause target including its alias, e.g. `app_health_events e`.
    from: &'static str,
    /// The shared `WHERE` body identifying the rows to remove.
    predicate: &'static str,
}

impl RetentionRule {
    /// `SELECT count(*)::bigint FROM <from> WHERE <predicate>`.
    pub(crate) fn count_sql(&self) -> String {
        format!(
            "SELECT count(*)::bigint\nFROM {}\nWHERE {}\n",
            self.from, self.predicate
        )
    }

    /// `DELETE FROM <from> WHERE <predicate>`.
    pub(crate) fn delete_sql(&self) -> String {
        format!("DELETE FROM {}\nWHERE {}\n", self.from, self.predicate)
    }
}

/// Deployment-log retention cannot share one `from`/`predicate` because the
/// count uses a `JOIN` while the delete uses `USING`; the shared `WHERE` body
/// still lives in one place.
pub(crate) const DEPLOYMENT_LOGS_PREDICATE: &str = r#"l.created_at < now() - interval '30 days'
  AND d.id NOT IN (
    SELECT id FROM (
      SELECT id,row_number() OVER (PARTITION BY app_id ORDER BY created_at DESC) AS rn
      FROM deployments
    ) ranked WHERE rn <= 20
  )
  AND NOT EXISTS (
    SELECT 1 FROM agent_jobs j
    WHERE j.deployment_id=d.id AND j.status IN ('queued','claimed','running')
  )"#;

pub(crate) fn cleanup_deployment_logs_sql() -> String {
    format!(
        "SELECT count(*)::bigint\nFROM deployment_logs l\nJOIN deployments d ON d.id=l.deployment_id\nWHERE {DEPLOYMENT_LOGS_PREDICATE}\n"
    )
}

pub(crate) fn delete_deployment_logs_sql() -> String {
    format!(
        "DELETE FROM deployment_logs l\nUSING deployments d\nWHERE d.id=l.deployment_id\n  AND {DEPLOYMENT_LOGS_PREDICATE}\n"
    )
}

pub(crate) const HEALTH_EVENTS_RULE: RetentionRule = RetentionRule {
    from: "app_health_events e",
    predicate: r#"e.created_at < now() - interval '7 days'
   OR e.id IN (
      SELECT id FROM (
        SELECT id,row_number() OVER (PARTITION BY app_id ORDER BY created_at DESC) AS rn
        FROM app_health_events
      ) ranked WHERE rn > 500
   )"#,
};

pub(crate) const RESOURCE_SNAPSHOTS_RULE: RetentionRule = RetentionRule {
    from: "app_resource_snapshots s",
    predicate: r#"s.sampled_at < now() - interval '7 days'
  AND NOT EXISTS (
    SELECT 1 FROM deployments d
    JOIN apps a ON a.current_deployment_id=d.id
    WHERE d.container_name=s.container_name
  )"#,
};

pub(crate) const WEBHOOK_EVENTS_RULE: RetentionRule = RetentionRule {
    from: "webhook_events e",
    predicate: "e.created_at < now() - interval '14 days'",
};

pub(crate) const COMPLETED_AGENT_JOBS_RULE: RetentionRule = RetentionRule {
    from: "agent_jobs j",
    predicate: r#"j.status IN ('success','cancelled')
  AND COALESCE(j.finished_at,j.updated_at,j.created_at) < now() - interval '30 days'"#,
};

pub(crate) const FAILED_AGENT_JOBS_RULE: RetentionRule = RetentionRule {
    from: "agent_jobs j",
    predicate: r#"j.status IN ('failed','expired')
  AND COALESCE(j.finished_at,j.updated_at,j.created_at) < now() - interval '90 days'"#,
};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Pure-function tests for env parsers ----------------------------------------

    #[test]
    fn cleanup_keep_previous_value_accepts_valid_range() {
        assert_eq!(cleanup_keep_previous_value("1"), Some(1));
        assert_eq!(cleanup_keep_previous_value(" 3 "), Some(3));
        assert_eq!(cleanup_keep_previous_value("100"), Some(100));
    }

    #[test]
    fn cleanup_keep_previous_value_rejects_zero_empty_nonnumeric_over_100() {
        assert_eq!(cleanup_keep_previous_value("0"), None);
        assert_eq!(cleanup_keep_previous_value(""), None);
        assert_eq!(cleanup_keep_previous_value("soon"), None);
        assert_eq!(cleanup_keep_previous_value("101"), None);
        assert_eq!(cleanup_keep_previous_value("-1"), None);
    }

    #[test]
    fn auto_cleanup_enabled_value_false_for_falsy() {
        assert!(!auto_cleanup_enabled_value("0"));
        assert!(!auto_cleanup_enabled_value("false"));
        assert!(!auto_cleanup_enabled_value("no"));
        assert!(!auto_cleanup_enabled_value("FALSE"));
        assert!(!auto_cleanup_enabled_value(" no "));
    }

    #[test]
    fn auto_cleanup_enabled_value_true_for_truthy_and_unknown() {
        assert!(auto_cleanup_enabled_value("1"));
        assert!(auto_cleanup_enabled_value("true"));
        assert!(auto_cleanup_enabled_value("yes"));
        assert!(auto_cleanup_enabled_value("anything"));
    }

    #[test]
    fn cleanup_keep_failed_hours_value_accepts_zero_through_720() {
        assert_eq!(cleanup_keep_failed_hours_value("0"), Some(0));
        assert_eq!(cleanup_keep_failed_hours_value(" 24 "), Some(24));
        assert_eq!(cleanup_keep_failed_hours_value("720"), Some(720));
    }

    #[test]
    fn cleanup_keep_failed_hours_value_rejects_negative_empty_nonnumeric_over_720() {
        assert_eq!(cleanup_keep_failed_hours_value("-1"), None);
        assert_eq!(cleanup_keep_failed_hours_value("721"), None);
        assert_eq!(cleanup_keep_failed_hours_value(""), None);
        assert_eq!(cleanup_keep_failed_hours_value("soon"), None);
    }

    // DB-gated helpers -----------------------------------------------------------

    async fn reset_cleanup_db(state: &AppState) {
        sqlx::query(
            "TRUNCATE deployment_logs, app_health_events, app_health_snapshots, \
             app_resource_snapshots, agent_jobs, deployments, app_env_vars, apps, users CASCADE",
        )
        .execute(&state.db)
        .await
        .unwrap();
    }

    pub(super) async fn insert_cleanup_user(state: &AppState) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO users (github_id, login) VALUES ($1,'cleanup-user') RETURNING id",
        )
        .bind(9_090_001_i64)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    async fn insert_cleanup_app(state: &AppState, user_id: Uuid) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO apps
               (user_id,server_id,name,repo_full_name,branch,container_port,health_path,\
                domain,runtime_kind,root_directory,public_exposure,auto_deploy)
             VALUES ($1,$2,'cleanup-app','hostlet-ci/node-hello','main',3000,'/health',\
               'cleanup.example.test','single','.',true,false)
             RETURNING id",
        )
        .bind(user_id)
        .bind(state.local_server_id)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    async fn insert_cleanup_deployment(
        state: &AppState,
        app_id: Uuid,
        status: &str,
        image_tag: &str,
        container_name: &str,
    ) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO deployments
               (app_id,server_id,status,commit_sha,image_tag,container_name,\
                started_at,finished_at,runtime_kind)
             VALUES ($1,$2,$3,'HEAD',$4,$5,now(),now(),'single')
             RETURNING id",
        )
        .bind(app_id)
        .bind(state.local_server_id)
        .bind(status)
        .bind(image_tag)
        .bind(container_name)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    /// Same as `insert_cleanup_deployment` but with an explicit `finished_at`
    /// offset (in minutes ago) so tests can establish a strict ordering without
    /// relying on wall-clock timing.
    async fn insert_cleanup_deployment_ago(
        state: &AppState,
        app_id: Uuid,
        status: &str,
        image_tag: &str,
        container_name: &str,
        minutes_ago: i32,
    ) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO deployments
               (app_id,server_id,status,commit_sha,image_tag,container_name,\
                started_at,finished_at,runtime_kind)
             VALUES ($1,$2,$3,'HEAD',$4,$5,now(),\
               now() - ($6 * interval '1 minute'),'single')
             RETURNING id",
        )
        .bind(app_id)
        .bind(state.local_server_id)
        .bind(status)
        .bind(image_tag)
        .bind(container_name)
        .bind(minutes_ago)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    // DB-gated tests -------------------------------------------------------------

    #[tokio::test]
    async fn db_cleanup_plan_marks_failed_deployment_containers_stale() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_cleanup_db(&state).await;
        let user_id = insert_cleanup_user(&state).await;
        let app_id = insert_cleanup_app(&state, user_id).await;
        let current_deployment = insert_cleanup_deployment(
            &state,
            app_id,
            "success",
            "hostlet/app-current:latest",
            "hostlet-app-current",
        )
        .await;
        insert_cleanup_deployment(
            &state,
            app_id,
            "health_checking",
            "hostlet/app-active:latest",
            "hostlet-app-active",
        )
        .await;
        insert_cleanup_deployment(
            &state,
            app_id,
            "failed",
            "hostlet/app-failed:latest",
            "hostlet-app-failed",
        )
        .await;
        sqlx::query("UPDATE apps SET current_deployment_id=$1 WHERE id=$2")
            .bind(current_deployment)
            .bind(app_id)
            .execute(&state.db)
            .await
            .unwrap();

        let plan = cleanup_plan(&state).await.unwrap();

        assert_eq!(plan.docker.stale_deployment_containers, 1);
        assert_eq!(
            plan.keep_containers,
            vec!["hostlet-app-active", "hostlet-app-current"]
        );
        assert_eq!(
            plan.keep_images,
            vec!["hostlet/app-active:latest", "hostlet/app-current:latest"]
        );
        assert!(!plan.keep_containers.contains(&"hostlet-app-failed".into()));
        assert!(!plan
            .keep_images
            .contains(&"hostlet/app-failed:latest".into()));
    }

    /// Verifies the rollback-target fix.
    ///
    /// Scenario: A(success, oldest) < B(success) < R(rolled_back) < C(success,
    /// current).  With `keep_previous=1`, the keep list must include C, R, and
    /// B — B is the actual rollback target (most-recent success ≠ current).
    ///
    /// Under the old `rn<=2` rule, R occupied rn=2 and pushed B to rn=3, where
    /// it would have been reaped.  The `success_rn` column fixes this by
    /// protecting B regardless of its overall rank.
    #[tokio::test]
    async fn db_cleanup_plan_protects_rollback_target_behind_rolled_back_row() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_cleanup_db(&state).await;
        let user_id = insert_cleanup_user(&state).await;
        let app_id = insert_cleanup_app(&state, user_id).await;

        // A: oldest success (stale with keep_previous=1)
        insert_cleanup_deployment_ago(
            &state,
            app_id,
            "success",
            "hostlet/app-a:v1",
            "hostlet-app-a",
            40,
        )
        .await;
        // B: second success — the rollback target deploy.rs would pick
        insert_cleanup_deployment_ago(
            &state,
            app_id,
            "success",
            "hostlet/app-b:v2",
            "hostlet-app-b",
            30,
        )
        .await;
        // R: rolled_back — more recent than B, occupies rn=2
        insert_cleanup_deployment_ago(
            &state,
            app_id,
            "rolled_back",
            "hostlet/app-r:v3",
            "hostlet-app-r",
            20,
        )
        .await;
        // C: newest success, set as current deployment
        let c = insert_cleanup_deployment_ago(
            &state,
            app_id,
            "success",
            "hostlet/app-c:v4",
            "hostlet-app-c",
            10,
        )
        .await;
        sqlx::query("UPDATE apps SET current_deployment_id=$1 WHERE id=$2")
            .bind(c)
            .bind(app_id)
            .execute(&state.db)
            .await
            .unwrap();

        let plan = cleanup_plan(&state).await.unwrap();

        assert!(
            plan.keep_containers.contains(&"hostlet-app-c".into()),
            "current container (C) must be kept"
        );
        assert!(
            plan.keep_containers.contains(&"hostlet-app-r".into()),
            "rolled_back container (R) must be kept"
        );
        assert!(
            plan.keep_containers.contains(&"hostlet-app-b".into()),
            "rollback target (B) must be kept via success_rn"
        );
        assert!(
            !plan.keep_containers.contains(&"hostlet-app-a".into()),
            "oldest success (A) must be stale"
        );
        assert_eq!(
            plan.docker.stale_deployment_containers, 1,
            "exactly one container should be stale (A)"
        );
    }

    /// Verifies the keep-failed-hours knob and non-displacement guarantee.
    ///
    /// Scenario: C(success/current,5m) > F(failed,60m) > R(rolled_back,120m) >
    /// O(failed,1500m=25h).  F newer than R so R's rn=2 slot is the stress case.
    /// knob=24: C/R/F kept, O not, stale=1.  knob=0: only C/R kept, stale=2.
    #[tokio::test]
    async fn db_cleanup_plan_protects_recent_failed_deployments_only() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_cleanup_db(&state).await;
        let user_id = insert_cleanup_user(&state).await;
        let app_id = insert_cleanup_app(&state, user_id).await;

        // C: current success (5 min ago)
        let c = insert_cleanup_deployment_ago(
            &state,
            app_id,
            "success",
            "hostlet/app-c:v1",
            "hostlet-app-c",
            5,
        )
        .await;
        sqlx::query("UPDATE apps SET current_deployment_id=$1 WHERE id=$2")
            .bind(c)
            .bind(app_id)
            .execute(&state.db)
            .await
            .unwrap();
        // F: recent failed (60 min ago — newer than R, exercises non-displacement)
        insert_cleanup_deployment_ago(
            &state,
            app_id,
            "failed",
            "hostlet/app-f:bad",
            "hostlet-app-f",
            60,
        )
        .await;
        // R: rolled_back (120 min ago)
        insert_cleanup_deployment_ago(
            &state,
            app_id,
            "rolled_back",
            "hostlet/app-r:v0",
            "hostlet-app-r",
            120,
        )
        .await;
        // O: old failed (1500 min = 25 h ago)
        insert_cleanup_deployment_ago(
            &state,
            app_id,
            "failed",
            "hostlet/app-o:bad",
            "hostlet-app-o",
            1500,
        )
        .await;

        // --- knob = 24 h ---
        let plan = cleanup_plan_with_keep_failed_hours(&state, 24)
            .await
            .unwrap();
        let kc = &plan.keep_containers;
        assert!(kc.contains(&"hostlet-app-c".into()), "C must be kept");
        assert!(
            kc.contains(&"hostlet-app-r".into()),
            "rolled_back row (R) must not be displaced by the recent failed row (F)"
        );
        assert!(kc.contains(&"hostlet-app-f".into()), "F(60m) kept");
        assert!(!kc.contains(&"hostlet-app-o".into()), "O(25h) not kept");
        assert!(plan.keep_images.contains(&"hostlet/app-f:bad".into()));
        assert!(!plan.keep_images.contains(&"hostlet/app-o:bad".into()));
        assert_eq!(plan.docker.stale_deployment_containers, 1);

        // --- knob = 0 (disabled) ---
        let plan = cleanup_plan_with_keep_failed_hours(&state, 0)
            .await
            .unwrap();
        let kc = &plan.keep_containers;
        assert!(kc.contains(&"hostlet-app-c".into()));
        assert!(kc.contains(&"hostlet-app-r".into()));
        assert!(!kc.contains(&"hostlet-app-f".into()), "F not kept(0)");
        assert!(!kc.contains(&"hostlet-app-o".into()), "O not kept(0)");
        assert_eq!(plan.docker.stale_deployment_containers, 2);
    }
}
