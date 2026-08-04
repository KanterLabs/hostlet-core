use crate::{deploy, state::AppState};
use axum::{http::StatusCode, response::IntoResponse, Json};
use sqlx::Row;
use uuid::Uuid;

/// Builds the SQL fragment that restricts agent-job visibility to jobs the
/// caller is allowed to see.
///
/// `user_param` and `cloud_param` are 1-based bind-parameter indices that the
/// fragment references as `${user_param}` / `${cloud_param}`. They must match
/// the order in which the surrounding query binds the user id and the
/// cloud-mode flag.
pub fn agent_job_visibility_predicate(user_param: usize, cloud_param: usize) -> String {
    format!(
        r#"
          AND (
            EXISTS (SELECT 1 FROM apps a WHERE a.id=j.app_id AND a.user_id=${user_param})
            OR EXISTS (
              SELECT 1 FROM deployments d
              JOIN apps a ON a.id=d.app_id
              WHERE d.id=j.deployment_id AND a.user_id=${user_param}
            )
            OR (
              ${cloud_param} = false
              AND j.app_id IS NULL
              AND j.deployment_id IS NULL
              AND EXISTS (
                SELECT 1 FROM servers s
                WHERE s.id=j.server_id AND (s.user_id=${user_param} OR s.kind='local')
              )
            )
          )
        "#
    )
}

/// Input for recording a structured audit event.
pub struct AuditEventInput<'a> {
    pub actor_type: &'a str,
    pub actor_id: Option<String>,
    pub event_type: &'a str,
    pub app_id: Option<Uuid>,
    pub deployment_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub metadata: serde_json::Value,
}

/// Identifies the principal responsible for a job-control action.
///
/// Job visibility and audit attribution are intentionally separate: an
/// operator may act on an owner's job while the audit record must continue to
/// name the operator rather than the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobAuditActor<'a> {
    pub actor_type: &'a str,
    pub actor_id: Option<&'a str>,
}

impl<'a> JobAuditActor<'a> {
    pub const fn new(actor_type: &'a str, actor_id: Option<&'a str>) -> Self {
        Self {
            actor_type,
            actor_id,
        }
    }

    pub const fn owner() -> Self {
        Self::new("owner", None)
    }
}

/// The ownership scope used to locate a job for a recovery operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentJobVisibility {
    pub user_id: Uuid,
    pub cloud_mode: bool,
}

/// Input for an interactive agent job. Unlike deployments, these jobs are
/// directly enqueued without creating a new deployment record.
pub struct InteractiveAgentJob<'a> {
    pub server_id: Uuid,
    pub app_id: Uuid,
    pub deployment_id: Option<Uuid>,
    pub job_type: &'a str,
    pub payload: serde_json::Value,
}

/// The outcome of a retry attempt. `Rejected` represents a valid request that
/// cannot currently create a replacement deployment (for example, because no
/// prior deployment is available for rollback).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentJobRetryOutcome {
    Retried,
    NotFound,
    Rejected(String),
}

/// The outcome of a cancellation attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentJobCancelOutcome {
    Cancelled,
    NotFound,
}

/// Records a structured audit event into the `audit_events` table.
pub async fn record_audit_event(state: &AppState, event: AuditEventInput<'_>) {
    let _ = sqlx::query(
        "INSERT INTO audit_events
           (actor_type,actor_id,event_type,app_id,deployment_id,job_id,metadata_json)
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(event.actor_type)
    .bind(event.actor_id)
    .bind(event.event_type)
    .bind(event.app_id)
    .bind(event.deployment_id)
    .bind(event.job_id)
    .bind(event.metadata)
    .execute(&state.db)
    .await;
}

/// Enqueues an interactive (non-deploy) agent job and returns a `202 Accepted`
/// response carrying the new job ID, or `500` on failure.
pub async fn enqueue_interactive_agent_job(
    state: &AppState,
    server_id: Uuid,
    app_id: Uuid,
    deployment_id: Option<Uuid>,
    job_type: &str,
    payload: serde_json::Value,
) -> axum::response::Response {
    match enqueue_interactive_agent_job_for_actor(
        state,
        JobAuditActor::owner(),
        InteractiveAgentJob {
            server_id,
            app_id,
            deployment_id,
            job_type,
            payload,
        },
    )
    .await
    {
        Ok(job_id) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({"jobId": job_id})),
        )
            .into_response(),
        Err(err) => {
            tracing::warn!(error = %err, app_id = %app_id, job_type, "failed to enqueue agent job");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Enqueues an interactive job and records the request audit event in the
/// same transaction. Callers provide the audit actor explicitly so privileged
/// recovery paths never attribute an operator action to the app owner.
pub async fn enqueue_interactive_agent_job_for_actor(
    state: &AppState,
    actor: JobAuditActor<'_>,
    job: InteractiveAgentJob<'_>,
) -> anyhow::Result<Uuid> {
    let mut transaction = state.db.begin().await?;
    // Keep the same teardown fence protocol as deploy::enqueue_agent_job.
    // The app row lock serializes this enqueue with start_app_teardown.
    let queue_priority_offset = sqlx::query_scalar::<_, i32>(
        "SELECT queue_priority_offset FROM apps WHERE id=$1 FOR KEY SHARE",
    )
    .bind(job.app_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| anyhow::anyhow!("app no longer exists"))?;
    let deletion_fenced = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
           SELECT 1 FROM agent_jobs
           WHERE app_id=$1
             AND job_type='delete_app'
             AND (
               status IN ('queued','claimed','running','success')
               OR payload_json->>'teardown_fence'='true'
             )
         )",
    )
    .bind(job.app_id)
    .fetch_one(&mut *transaction)
    .await?;
    anyhow::ensure!(!deletion_fenced, "app deletion is in progress");

    let job_id = deploy::insert_agent_job_in_transaction(
        &mut transaction,
        job.server_id,
        Some(job.app_id),
        job.deployment_id,
        job.job_type,
        job.payload,
        20 + queue_priority_offset,
    )
    .await?;
    record_agent_job_audit_event_in_transaction(
        &mut transaction,
        actor,
        &format!("{}_requested", job.job_type),
        Some(job.app_id),
        job.deployment_id,
        Some(job_id),
    )
    .await?;
    transaction.commit().await?;
    Ok(job_id)
}

/// Marks an agent job as failed with the given summary message.
pub async fn mark_agent_job_failed(state: &AppState, job_id: Uuid, failure: &str) {
    let _ = sqlx::query(
        "UPDATE agent_jobs
         SET status='failed', failure_summary=$2, updated_at=now(), finished_at=now()
         WHERE id=$1",
    )
    .bind(job_id)
    .bind(failure)
    .execute(&state.db)
    .await;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppTeardownStart {
    pub job_id: Uuid,
    pub agent_cleanup_required: bool,
    pub active_jobs: u64,
}

/// Establishes a durable app-deletion marker and fences every other job for
/// the app in one transaction.
///
/// The app row is locked FOR UPDATE. Ordinary enqueues take FOR KEY SHARE and
/// re-check the marker after acquiring that lock, so an enqueue racing this
/// function is either included in the cancellation pass or rejected.
pub async fn start_app_teardown(
    state: &AppState,
    server_id: Uuid,
    app_id: Uuid,
    mut payload: serde_json::Value,
    agent_cleanup_required: bool,
) -> anyhow::Result<AppTeardownStart> {
    let Some(object) = payload.as_object_mut() else {
        anyhow::bail!("delete_app payload must be a JSON object");
    };
    object.insert(
        deploy::TEARDOWN_FENCE_PAYLOAD_KEY.to_string(),
        serde_json::Value::Bool(true),
    );

    let mut transaction = state.db.begin().await?;
    let queue_priority_offset = sqlx::query_scalar::<_, i32>(
        "SELECT queue_priority_offset FROM apps WHERE id=$1 FOR UPDATE",
    )
    .bind(app_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| anyhow::anyhow!("app no longer exists"))?;

    let existing = sqlx::query(
        "SELECT id,status
         FROM agent_jobs
         WHERE app_id=$1 AND job_type='delete_app'
         ORDER BY created_at DESC
         LIMIT 1
         FOR UPDATE",
    )
    .bind(app_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let target_status = if agent_cleanup_required {
        "queued"
    } else {
        "success"
    };
    let job_id = if let Some(existing) = existing {
        let job_id = existing.get::<Uuid, _>("id");
        let status = existing.get::<String, _>("status");
        if matches!(status.as_str(), "failed" | "cancelled" | "expired") {
            sqlx::query(
                "UPDATE agent_jobs
                 SET status=$2,payload_json=$3,priority=$4,attempt=0,
                     claimed_by=NULL,claimed_at=NULL,claim_token=NULL,
                     lease_expires_at=NULL,cancel_requested_at=NULL,
                     failure_summary=NULL,last_error=NULL,result_json=NULL,
                     available_at=now(),started_at=NULL,
                     finished_at=CASE WHEN $2='success' THEN now() ELSE NULL END,
                     updated_at=now()
                 WHERE id=$1",
            )
            .bind(job_id)
            .bind(target_status)
            .bind(&payload)
            .bind(5 + queue_priority_offset)
            .execute(&mut *transaction)
            .await?;
        } else if status == "queued" && agent_cleanup_required {
            sqlx::query(
                "UPDATE agent_jobs
                 SET payload_json=$2,priority=$3,updated_at=now()
                 WHERE id=$1",
            )
            .bind(job_id)
            .bind(&payload)
            .bind(5 + queue_priority_offset)
            .execute(&mut *transaction)
            .await?;
        } else if status == "queued" {
            sqlx::query(
                "UPDATE agent_jobs
                 SET status='success',payload_json=$2,priority=$3,
                     started_at=COALESCE(started_at,now()),finished_at=now(),updated_at=now()
                 WHERE id=$1",
            )
            .bind(job_id)
            .bind(&payload)
            .bind(5 + queue_priority_offset)
            .execute(&mut *transaction)
            .await?;
        }
        job_id
    } else {
        let job_id = deploy::insert_agent_job_in_transaction(
            &mut transaction,
            server_id,
            Some(app_id),
            None,
            "delete_app",
            payload,
            5 + queue_priority_offset,
        )
        .await?;
        if !agent_cleanup_required {
            sqlx::query(
                "UPDATE agent_jobs
                 SET status='success',started_at=now(),finished_at=now(),updated_at=now()
                 WHERE id=$1",
            )
            .bind(job_id)
            .execute(&mut *transaction)
            .await?;
        }
        job_id
    };

    let active_jobs = fence_app_jobs_in_transaction(&mut transaction, app_id).await?;
    transaction.commit().await?;
    Ok(AppTeardownStart {
        job_id,
        agent_cleanup_required,
        active_jobs,
    })
}

/// Reapplies an existing teardown fence before finalization. This catches
/// legacy or already-in-flight work without creating a second delete marker.
pub async fn refresh_app_teardown_fence(state: &AppState, app_id: Uuid) -> anyhow::Result<u64> {
    let mut transaction = state.db.begin().await?;
    let app_exists = sqlx::query_scalar::<_, Uuid>("SELECT id FROM apps WHERE id=$1 FOR UPDATE")
        .bind(app_id)
        .fetch_optional(&mut *transaction)
        .await?
        .is_some();
    if !app_exists {
        return Ok(0);
    }
    let active_jobs = fence_app_jobs_in_transaction(&mut transaction, app_id).await?;
    transaction.commit().await?;
    Ok(active_jobs)
}

async fn fence_app_jobs_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    app_id: Uuid,
) -> anyhow::Result<u64> {
    let active_jobs = sqlx::query_scalar::<_, i64>(
        "WITH fenced AS (
           UPDATE agent_jobs
           SET status=CASE WHEN status='queued' THEN 'cancelled' ELSE status END,
               cancel_requested_at=CASE
                 WHEN status IN ('claimed','running') THEN COALESCE(cancel_requested_at,now())
                 ELSE cancel_requested_at
               END,
               failure_summary=CASE
                 WHEN status='queued' THEN 'App deletion was requested before this job could run.'
                 ELSE failure_summary
               END,
               last_error=CASE
                 WHEN status='queued' THEN 'App deletion was requested before this job could run.'
                 ELSE last_error
               END,
               payload_json=CASE
                 WHEN status='queued' THEN '{}'::jsonb
                 ELSE payload_json - 'env' - 'github_token' - 'artifact_registry'
               END,
               lease_expires_at=CASE WHEN status='queued' THEN NULL ELSE lease_expires_at END,
               finished_at=CASE WHEN status='queued' THEN now() ELSE finished_at END,
               updated_at=now()
           WHERE app_id=$1
             AND job_type<>'delete_app'
             AND status IN ('queued','claimed','running')
           RETURNING deployment_id,status
         ), cancelled_deployments AS (
           UPDATE deployments d
           SET status='canceled',failure_code='cancelled_by_owner',
               failure_summary='App deletion was requested before this deployment could run.',
               finished_at=now()
           FROM fenced f
           WHERE f.status='cancelled'
             AND d.id=f.deployment_id
             AND d.status = ANY($2)
           RETURNING d.id
         ), cleared_pending AS (
           UPDATE apps a
           SET pending_deployment_id=NULL,updated_at=now()
           WHERE a.id=$1
             AND EXISTS (
               SELECT 1
               FROM cancelled_deployments d
               WHERE d.id=a.pending_deployment_id
             )
         )
         SELECT count(*) FILTER (WHERE status IN ('claimed','running')) FROM fenced",
    )
    .bind(app_id)
    .bind(deploy::ACTIVE_DEPLOYMENT_STATUSES)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(active_jobs.max(0) as u64)
}

pub async fn retry_agent_job(
    state: &AppState,
    user_id: Uuid,
    id: Uuid,
    cloud_mode: bool,
) -> axum::response::Response {
    match retry_agent_job_for_actor(
        state,
        AgentJobVisibility {
            user_id,
            cloud_mode,
        },
        id,
        JobAuditActor::owner(),
    )
    .await
    {
        Ok(AgentJobRetryOutcome::Retried) => StatusCode::NO_CONTENT.into_response(),
        Ok(AgentJobRetryOutcome::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Ok(AgentJobRetryOutcome::Rejected(reason)) => {
            (StatusCode::BAD_REQUEST, reason).into_response()
        }
        Err(err) => {
            tracing::warn!(error = %err, job_id = %id, "failed to retry agent job");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Retries a terminal job within `visibility` and attributes the recovery to
/// `actor`. The owner scope is used solely for authorization; it is never
/// inferred as the actor for the resulting audit event.
pub async fn retry_agent_job_for_actor(
    state: &AppState,
    visibility: AgentJobVisibility,
    id: Uuid,
    actor: JobAuditActor<'_>,
) -> anyhow::Result<AgentJobRetryOutcome> {
    let select = format!(
        r#"
        SELECT j.job_type, j.app_id, j.payload_json
        FROM agent_jobs j
        WHERE j.id=$1
          {}
          AND j.status IN ('failed','expired','cancelled')
        "#,
        agent_job_visibility_predicate(2, 3)
    );
    let row = match sqlx::query(&select)
        .bind(id)
        .bind(visibility.user_id)
        .bind(visibility.cloud_mode)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return Ok(AgentJobRetryOutcome::NotFound),
        Err(err) => return Err(err.into()),
    };
    let job_type = row.get::<String, _>("job_type");
    if retry_creates_fresh_deployment(&job_type) {
        return retry_deployment_job_for_actor(state, visibility, id, &job_type, &row, actor).await;
    }
    let update = format!(
        r#"
        UPDATE agent_jobs j
        SET status='queued',
            failure_summary=NULL,
            last_error=NULL,
            claimed_by=NULL,
            claimed_at=NULL,
            lease_expires_at=NULL,
            finished_at=NULL,
            updated_at=now()
        WHERE j.id=$1
          {}
          AND j.status IN ('failed','expired','cancelled')
          AND COALESCE(j.payload_json, '{{}}'::jsonb) <> '{{}}'::jsonb
        RETURNING j.app_id,j.deployment_id
        "#,
        agent_job_visibility_predicate(2, 3)
    );
    match run_job_mutation_for_actor(
        state,
        id,
        visibility,
        &update,
        JobMutationAudit {
            event_type: "agent_job_retried",
            actor,
        },
    )
    .await?
    {
        JobMutationOutcome::Mutated => Ok(AgentJobRetryOutcome::Retried),
        JobMutationOutcome::NotFound => Ok(AgentJobRetryOutcome::NotFound),
    }
}

pub async fn cancel_agent_job(
    state: &AppState,
    user_id: Uuid,
    id: Uuid,
    cloud_mode: bool,
) -> axum::response::Response {
    match cancel_agent_job_for_actor(
        state,
        AgentJobVisibility {
            user_id,
            cloud_mode,
        },
        id,
        JobAuditActor::owner(),
    )
    .await
    {
        Ok(AgentJobCancelOutcome::Cancelled) => StatusCode::NO_CONTENT.into_response(),
        Ok(AgentJobCancelOutcome::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::warn!(error = %err, job_id = %id, "failed to cancel agent job");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Cancels a queued or active job within `visibility`, recording the supplied
/// actor atomically with the mutation.
pub async fn cancel_agent_job_for_actor(
    state: &AppState,
    visibility: AgentJobVisibility,
    id: Uuid,
    actor: JobAuditActor<'_>,
) -> anyhow::Result<AgentJobCancelOutcome> {
    let update = cancel_agent_job_update_sql(actor);
    match run_job_mutation_for_actor(
        state,
        id,
        visibility,
        &update,
        JobMutationAudit {
            event_type: "agent_job_cancelled",
            actor,
        },
    )
    .await?
    {
        JobMutationOutcome::Mutated => Ok(AgentJobCancelOutcome::Cancelled),
        JobMutationOutcome::NotFound => Ok(AgentJobCancelOutcome::NotFound),
    }
}

fn retry_creates_fresh_deployment(job_type: &str) -> bool {
    matches!(job_type, "deploy" | "rollback" | "build" | "release")
}

/// Retries a `deploy`/`rollback` job by creating a fresh deployment instead of
/// requeuing the original row. The stored secrets were scrubbed on the terminal
/// transition, and the original deployment row is already terminal, so a reused
/// run's reports would not update the right deployment.
async fn retry_deployment_job_for_actor(
    state: &AppState,
    visibility: AgentJobVisibility,
    id: Uuid,
    job_type: &str,
    row: &sqlx::postgres::PgRow,
    actor: JobAuditActor<'_>,
) -> anyhow::Result<AgentJobRetryOutcome> {
    let Some(app_id) = row.get::<Option<Uuid>, _>("app_id") else {
        return Ok(AgentJobRetryOutcome::Rejected(
            "job no longer has an app to retry against".into(),
        ));
    };
    let result = if job_type == "rollback" {
        deploy::create_and_send_rollback(state, visibility.user_id, app_id).await
    } else {
        let payload = row.get::<Option<serde_json::Value>, _>("payload_json");
        let commit_sha = payload
            .as_ref()
            .and_then(|payload| payload.get("commit_sha"))
            .and_then(|value| value.as_str())
            .unwrap_or("HEAD");
        deploy::create_and_send_deploy(state, visibility.user_id, app_id, commit_sha).await
    };
    match result {
        Ok(new_deployment_id) => {
            record_agent_job_audit_event(
                state,
                actor,
                "agent_job_retried",
                Some(app_id),
                Some(new_deployment_id),
                Some(id),
            )
            .await;
            Ok(AgentJobRetryOutcome::Retried)
        }
        Err(err) => Ok(AgentJobRetryOutcome::Rejected(err.to_string())),
    }
}

fn cancel_agent_job_update_sql(actor: JobAuditActor<'_>) -> String {
    // These are fixed SQL literals selected from actor type, never caller
    // supplied text interpolated into the query.
    let (summary, failure_code) = if actor.actor_type == "owner" {
        (
            "Cancelled by owner before the agent started work.",
            "cancelled_by_owner",
        )
    } else {
        (
            "Cancelled by requester before the agent started work.",
            "cancelled_by_requester",
        )
    };
    format!(
        r#"
        WITH updated AS (
          UPDATE agent_jobs j
          SET status=CASE WHEN j.status='queued' THEN 'cancelled' ELSE j.status END,
              cancel_requested_at=CASE WHEN j.status IN ('claimed','running') THEN now() ELSE j.cancel_requested_at END,
              failure_summary=CASE WHEN j.status='queued' THEN '{summary}' ELSE j.failure_summary END,
              last_error=CASE WHEN j.status='queued' THEN '{summary}' ELSE j.last_error END,
              payload_json=CASE WHEN j.status='queued' THEN j.payload_json - 'env' - 'github_token' - 'artifact_registry' ELSE j.payload_json END,
              finished_at=CASE WHEN j.status='queued' THEN now() ELSE j.finished_at END,
              updated_at=now()
          WHERE j.id=$1
            {}
            AND j.status IN ('queued','claimed','running')
          RETURNING j.app_id,j.deployment_id,j.status
        ), cancelled_deployment AS (
          UPDATE deployments d
          SET status='canceled',failure_code='{failure_code}',
              failure_summary='{summary}',finished_at=now()
          FROM updated u
          WHERE u.status='cancelled' AND d.id=u.deployment_id
            AND d.status = ANY(ARRAY['queued','queued_for_build','running','building','publishing','queued_for_release','pulling','starting','health_checking','routing'])
          RETURNING d.id
        )
        SELECT app_id,deployment_id FROM updated
        "#,
        agent_job_visibility_predicate(2, 3)
    )
}

struct JobMutationAudit<'a> {
    event_type: &'static str,
    actor: JobAuditActor<'a>,
}

enum JobMutationOutcome {
    Mutated,
    NotFound,
}

async fn run_job_mutation_for_actor(
    state: &AppState,
    id: Uuid,
    visibility: AgentJobVisibility,
    update_sql: &str,
    audit: JobMutationAudit<'_>,
) -> anyhow::Result<JobMutationOutcome> {
    let mut transaction = state.db.begin().await?;
    let row = sqlx::query(update_sql)
        .bind(id)
        .bind(visibility.user_id)
        .bind(visibility.cloud_mode)
        .fetch_optional(&mut *transaction)
        .await?;
    let Some(row) = row else {
        transaction.commit().await?;
        return Ok(JobMutationOutcome::NotFound);
    };
    record_agent_job_audit_event_in_transaction(
        &mut transaction,
        audit.actor,
        audit.event_type,
        row.get::<Option<Uuid>, _>("app_id"),
        row.get::<Option<Uuid>, _>("deployment_id"),
        Some(id),
    )
    .await?;
    transaction.commit().await?;
    Ok(JobMutationOutcome::Mutated)
}

async fn record_agent_job_audit_event(
    state: &AppState,
    actor: JobAuditActor<'_>,
    event_type: &str,
    app_id: Option<Uuid>,
    deployment_id: Option<Uuid>,
    job_id: Option<Uuid>,
) {
    let result = sqlx::query(
        "INSERT INTO audit_events
           (actor_type,actor_id,event_type,app_id,deployment_id,job_id,metadata_json)
         VALUES ($1,$2,$3,$4,$5,$6,'{}'::jsonb)",
    )
    .bind(actor.actor_type)
    .bind(actor.actor_id)
    .bind(event_type)
    .bind(app_id)
    .bind(deployment_id)
    .bind(job_id)
    .execute(&state.db)
    .await;
    if let Err(err) = result {
        tracing::warn!(error = %err, event_type, "failed to record agent job audit event");
    }
}

async fn record_agent_job_audit_event_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: JobAuditActor<'_>,
    event_type: &str,
    app_id: Option<Uuid>,
    deployment_id: Option<Uuid>,
    job_id: Option<Uuid>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO audit_events
           (actor_type,actor_id,event_type,app_id,deployment_id,job_id,metadata_json)
         VALUES ($1,$2,$3,$4,$5,$6,'{}'::jsonb)",
    )
    .bind(actor.actor_type)
    .bind(actor.actor_id)
    .bind(event_type)
    .bind(app_id)
    .bind(deployment_id)
    .bind(job_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deploy_and_rollback_retries_create_fresh_deployments() {
        assert!(retry_creates_fresh_deployment("deploy"));
        assert!(retry_creates_fresh_deployment("rollback"));
        assert!(retry_creates_fresh_deployment("build"));
        assert!(retry_creates_fresh_deployment("release"));
        assert!(!retry_creates_fresh_deployment("health_check"));
    }

    #[test]
    fn cancel_sql_uses_actor_accurate_fixed_messages() {
        let owner_sql = cancel_agent_job_update_sql(JobAuditActor::owner());
        assert!(owner_sql.contains("Cancelled by owner before the agent started work."));
        assert!(owner_sql.contains("failure_code='cancelled_by_owner'"));
        assert!(owner_sql.contains("j.payload_json - 'env' - 'github_token' - 'artifact_registry'"));
        assert!(owner_sql.contains("cancel_requested_at"));

        let operator_sql =
            cancel_agent_job_update_sql(JobAuditActor::new("operator", Some("admin-123")));
        assert!(operator_sql.contains("Cancelled by requester before the agent started work."));
        assert!(operator_sql.contains("failure_code='cancelled_by_requester'"));
        assert!(!operator_sql.contains("Cancelled by owner"));
        assert!(!operator_sql.contains("cancelled_by_owner"));
    }

    #[test]
    fn audit_actor_keeps_operator_identity_separate_from_owner_scope() {
        let actor = JobAuditActor::new("operator", Some("admin-123"));
        let visibility = AgentJobVisibility {
            user_id: Uuid::nil(),
            cloud_mode: true,
        };

        assert_eq!(actor.actor_type, "operator");
        assert_eq!(actor.actor_id, Some("admin-123"));
        assert_ne!(actor.actor_type, "owner");
        assert!(visibility.cloud_mode);
    }
}
