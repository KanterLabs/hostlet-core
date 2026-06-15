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
use serde_json::Value;
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
const CANDIDATES_CTE_BODY: &str = r#"SELECT d.id, d.container_name, d.image_tag, d.runtime_metadata, d.status,
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

fn deployment_image_refs(image_tag: Option<&str>, runtime_metadata: Option<&Value>) -> Vec<String> {
    let mut refs = Vec::new();
    if let Some(image_tag) = image_tag.and_then(clean_image_ref) {
        refs.push(image_tag.to_string());
    }
    if let Some(metadata) = runtime_metadata {
        push_metadata_image_ref(&mut refs, metadata, "imageRef");
        push_metadata_image_ref(&mut refs, metadata, "image_ref");
        push_metadata_image_ref(&mut refs, metadata, "imageDigest");
        push_metadata_image_ref(&mut refs, metadata, "image_digest");
        if let Some(artifact) = metadata.get("buildArtifact") {
            push_metadata_image_ref(&mut refs, artifact, "imageRef");
            push_metadata_image_ref(&mut refs, artifact, "imageDigest");
        }
        if let Some(artifact) = metadata.get("build_artifact") {
            push_metadata_image_ref(&mut refs, artifact, "image_ref");
            push_metadata_image_ref(&mut refs, artifact, "image_digest");
        }
    }
    refs.sort();
    refs.dedup();
    refs
}

fn push_metadata_image_ref(refs: &mut Vec<String>, metadata: &Value, key: &str) {
    if let Some(image_ref) = metadata
        .get(key)
        .and_then(Value::as_str)
        .and_then(clean_image_ref)
    {
        refs.push(image_ref.to_string());
    }
}

fn clean_image_ref(value: &str) -> Option<&str> {
    let image_ref = value.trim();
    if image_ref.is_empty() || image_ref.len() > 512 || image_ref.chars().any(char::is_whitespace) {
        return None;
    }
    Some(image_ref)
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
        SELECT container_name, image_tag, runtime_metadata
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
        .flat_map(|row| {
            deployment_image_refs(
                row.get::<Option<String>, _>("image_tag").as_deref(),
                row.get::<Option<Value>, _>("runtime_metadata").as_ref(),
            )
        })
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

#[cfg(test)]
mod tests;
