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

use crate::{deploy, state::AppState};
use anyhow::Context;
use serde::Serialize;
use sqlx::Row;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct CleanupPlan {
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
pub(crate) struct CleanupRetention {
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
pub(crate) struct CleanupDatabasePreview {
    pub(crate) deployment_logs: i64,
    pub(crate) health_events: i64,
    pub(crate) resource_snapshots: i64,
    pub(crate) webhook_events: i64,
    pub(crate) completed_agent_jobs: i64,
    pub(crate) failed_agent_jobs: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CleanupDockerPreview {
    pub(crate) keep_containers: usize,
    pub(crate) keep_images: usize,
    pub(crate) stale_deployment_containers: i64,
    pub(crate) job_will_run: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CleanupDatabaseDeleted {
    pub(crate) deployment_logs: u64,
    pub(crate) health_events: u64,
    pub(crate) resource_snapshots: u64,
    pub(crate) webhook_events: u64,
    pub(crate) completed_agent_jobs: u64,
    pub(crate) failed_agent_jobs: u64,
}

/// Outcome of a full manual cleanup (database purge + Docker job enqueue).
pub(crate) struct CleanupOutcome {
    pub(crate) database_deleted: CleanupDatabaseDeleted,
    pub(crate) docker_cleanup_job_id: Option<Uuid>,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub(crate) const RETENTION: CleanupRetention = CleanupRetention {
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
/// Parameter `$1` is the set of statuses that qualify a deployment as a
/// candidate (protected statuses: active + success + rolled_back).
///
/// The `success_rn` column ranks status='success' deployments that are not the
/// current deployment per app, ordered by `finished_at DESC NULLS LAST,
/// created_at DESC`.  This explicitly protects rollback targets: even when a
/// `rolled_back` row occupies the `rn=2` slot and pushes the most-recent
/// non-current success row to `rn=3`, `success_rn` keeps it protected.
/// See `deploy.rs::create_and_send_rollback` for the target-selection query.
const CANDIDATES_CTE_BODY: &str = r#"SELECT d.id, d.container_name, d.image_tag, d.status,
     row_number() OVER (
       PARTITION BY d.app_id
       ORDER BY CASE WHEN a.current_deployment_id=d.id THEN 0 ELSE 1 END,
                d.finished_at DESC NULLS LAST, d.created_at DESC
     ) AS rn,
     CASE WHEN d.status='success' AND d.id IS DISTINCT FROM a.current_deployment_id THEN
       row_number() OVER (
         PARTITION BY d.app_id,
           (d.status='success' AND d.id IS DISTINCT FROM a.current_deployment_id)
         ORDER BY d.finished_at DESC NULLS LAST, d.created_at DESC
       )
     END AS success_rn
  FROM deployments d JOIN apps a ON a.id=d.app_id
  WHERE d.status = ANY($1)"#;

/// Keep predicate used in both the keep-list query and the stale-count's
/// `protected` sub-select.  `$2` = active statuses, `$3` = keep_previous.
///
/// A deployment is kept when any of:
/// - `status = ANY($2)`: it is actively in progress (never reap live containers)
/// - `rn <= $3 + 1`: it is the current deployment (rn=1) or among the most
///   recent `keep_previous` non-active rows per app
/// - `success_rn <= $3`: it is among the `keep_previous` most-recent
///   rollback targets (success, not current) — protects them even when a
///   `rolled_back` row pushes them past the `rn <= $3 + 1` threshold
const KEEP_PREDICATE: &str = "status = ANY($2) OR rn <= $3 + 1 OR success_rn <= $3";

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
async fn stale_container_count(state: &AppState, keep_previous: i64) -> anyhow::Result<i64> {
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
    .fetch_one(&state.db)
    .await
    .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// Public functions
// ---------------------------------------------------------------------------

/// Build a full cleanup plan: database row counts + Docker keep lists + stale
/// container count.  Used by both the HTTP preview endpoint and [`run_cleanup`].
pub(crate) async fn cleanup_plan(state: &AppState) -> anyhow::Result<CleanupPlan> {
    let database = CleanupDatabasePreview {
        deployment_logs: cleanup_count(state, &cleanup_deployment_logs_sql()).await?,
        health_events: cleanup_count(state, &HEALTH_EVENTS_RULE.count_sql()).await?,
        resource_snapshots: cleanup_count(state, &RESOURCE_SNAPSHOTS_RULE.count_sql()).await?,
        webhook_events: cleanup_count(state, &WEBHOOK_EVENTS_RULE.count_sql()).await?,
        completed_agent_jobs: cleanup_count(state, &COMPLETED_AGENT_JOBS_RULE.count_sql()).await?,
        failed_agent_jobs: cleanup_count(state, &FAILED_AGENT_JOBS_RULE.count_sql()).await?,
    };
    let keep_previous = cleanup_keep_previous();
    let lists = docker_keep_lists(state, keep_previous).await?;
    let stale_deployment_containers = stale_container_count(state, keep_previous).await?;
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
pub(crate) async fn run_cleanup(state: &AppState) -> anyhow::Result<CleanupOutcome> {
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
pub(crate) async fn auto_cleanup_for_server(
    state: &AppState,
    server_id: Uuid,
) -> anyhow::Result<Option<Uuid>> {
    if !auto_cleanup_enabled() {
        return Ok(None);
    }
    let keep_previous = cleanup_keep_previous();
    let lists = docker_keep_lists(state, keep_previous).await?;
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
pub(crate) async fn auto_cleanup_sweep(state: &AppState) {
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
}
