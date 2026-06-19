use crate::{deploy, state::AppState};
use axum::{http::StatusCode, response::IntoResponse};
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
              AND (s.user_id=${user_param} OR s.kind='local')
            )
          )
        "#
    )
}

pub async fn retry_agent_job(
    state: &AppState,
    user_id: Uuid,
    id: Uuid,
    cloud_mode: bool,
) -> axum::response::Response {
    let select = format!(
        r#"
        SELECT j.job_type, j.app_id, j.payload_json
        FROM agent_jobs j
        JOIN servers s ON s.id = j.server_id
        WHERE j.id=$1
          {}
          AND j.status IN ('failed','expired','cancelled')
        "#,
        agent_job_visibility_predicate(2, 3)
    );
    let row = match sqlx::query(&select)
        .bind(id)
        .bind(user_id)
        .bind(cloud_mode)
        .fetch_optional(&state.db)
        .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::warn!(error = %err, job_id = %id, "failed to load agent job for retry");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let job_type = row.get::<String, _>("job_type");
    if retry_creates_fresh_deployment(&job_type) {
        return retry_deployment_job(state, user_id, id, &job_type, &row).await;
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
        FROM servers s
        WHERE j.id=$1
          AND s.id=j.server_id
          {}
          AND j.status IN ('failed','expired','cancelled')
          AND COALESCE(j.payload_json, '{{}}'::jsonb) <> '{{}}'::jsonb
        RETURNING j.app_id,j.deployment_id
        "#,
        agent_job_visibility_predicate(2, 3)
    );
    run_job_mutation(
        state,
        id,
        user_id,
        cloud_mode,
        &update,
        JobMutationOutcome {
            event_type: "agent_job_retried",
            failure_log: "failed to retry agent job",
            success: StatusCode::NO_CONTENT,
        },
    )
    .await
}

pub async fn cancel_agent_job(
    state: &AppState,
    user_id: Uuid,
    id: Uuid,
    cloud_mode: bool,
) -> axum::response::Response {
    let update = cancel_agent_job_update_sql();
    run_job_mutation(
        state,
        id,
        user_id,
        cloud_mode,
        &update,
        JobMutationOutcome {
            event_type: "agent_job_cancelled",
            failure_log: "failed to cancel agent job",
            success: StatusCode::NO_CONTENT,
        },
    )
    .await
}

fn retry_creates_fresh_deployment(job_type: &str) -> bool {
    matches!(job_type, "deploy" | "rollback")
}

/// Retries a `deploy`/`rollback` job by creating a fresh deployment instead of
/// requeuing the original row. The stored secrets were scrubbed on the terminal
/// transition, and the original deployment row is already terminal, so a reused
/// run's reports would not update the right deployment.
async fn retry_deployment_job(
    state: &AppState,
    user_id: Uuid,
    id: Uuid,
    job_type: &str,
    row: &sqlx::postgres::PgRow,
) -> axum::response::Response {
    let Some(app_id) = row.get::<Option<Uuid>, _>("app_id") else {
        return (
            StatusCode::BAD_REQUEST,
            "job no longer has an app to retry against",
        )
            .into_response();
    };
    let result = if job_type == "rollback" {
        deploy::create_and_send_rollback(state, user_id, app_id).await
    } else {
        let payload = row.get::<Option<serde_json::Value>, _>("payload_json");
        let commit_sha = payload
            .as_ref()
            .and_then(|payload| payload.get("commit_sha"))
            .and_then(|value| value.as_str())
            .unwrap_or("HEAD");
        deploy::create_and_send_deploy(state, user_id, app_id, commit_sha).await
    };
    match result {
        Ok(new_deployment_id) => {
            record_agent_job_audit_event(
                state,
                "agent_job_retried",
                Some(app_id),
                Some(new_deployment_id),
                Some(id),
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(err) => (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
    }
}

fn cancel_agent_job_update_sql() -> String {
    format!(
        r#"
        UPDATE agent_jobs j
        SET status='cancelled',
            failure_summary='Cancelled by owner before the agent started work.',
            last_error='Cancelled by owner before the agent started work.',
            payload_json=j.payload_json - 'env' - 'github_token',
            finished_at=now(),
            updated_at=now()
        FROM servers s
        WHERE j.id=$1
          AND s.id=j.server_id
          {}
          AND j.status='queued'
        RETURNING j.app_id,j.deployment_id
        "#,
        agent_job_visibility_predicate(2, 3)
    )
}

struct JobMutationOutcome {
    event_type: &'static str,
    failure_log: &'static str,
    success: StatusCode,
}

async fn run_job_mutation(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
    cloud_mode: bool,
    update_sql: &str,
    outcome: JobMutationOutcome,
) -> axum::response::Response {
    let result = sqlx::query(update_sql)
        .bind(id)
        .bind(user_id)
        .bind(cloud_mode)
        .fetch_optional(&state.db)
        .await;
    match result {
        Ok(Some(row)) => {
            record_agent_job_audit_event(
                state,
                outcome.event_type,
                row.get::<Option<Uuid>, _>("app_id"),
                row.get::<Option<Uuid>, _>("deployment_id"),
                Some(id),
            )
            .await;
            outcome.success.into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::warn!(error = %err, job_id = %id, message = outcome.failure_log);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn record_agent_job_audit_event(
    state: &AppState,
    event_type: &str,
    app_id: Option<Uuid>,
    deployment_id: Option<Uuid>,
    job_id: Option<Uuid>,
) {
    let result = sqlx::query(
        "INSERT INTO audit_events
           (actor_type,actor_id,event_type,app_id,deployment_id,job_id,metadata_json)
         VALUES ('owner',NULL,$1,$2,$3,$4,'{}'::jsonb)",
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deploy_and_rollback_retries_create_fresh_deployments() {
        assert!(retry_creates_fresh_deployment("deploy"));
        assert!(retry_creates_fresh_deployment("rollback"));
        assert!(!retry_creates_fresh_deployment("health_check"));
    }

    #[test]
    fn cancel_scrubs_secret_payload_fields() {
        let sql = cancel_agent_job_update_sql();
        assert!(sql.contains("payload_json=j.payload_json - 'env' - 'github_token'"));
    }
}
