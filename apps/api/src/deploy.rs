mod deploy_app;

use crate::{auth::request_context, state::AppState};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use deploy_app::{DeployApp, DEPLOY_APP_COLUMNS};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

pub(crate) const ACTIVE_DEPLOYMENT_STATUSES: &[&str] = &[
    "queued",
    "running",
    "building",
    "starting",
    "health_checking",
    "routing",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeploymentQueue {
    pub status: String,
    pub position: Option<i64>,
    pub deploys_ahead: i64,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl DeploymentQueue {
    fn not_applicable(updated_at: Option<chrono::DateTime<chrono::Utc>>) -> Self {
        Self {
            status: "not_applicable".to_string(),
            position: None,
            deploys_ahead: 0,
            updated_at,
        }
    }
}

pub async fn manual_deploy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(app_id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(err) if err.to_string() == "sign in required" => {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(err) => return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response(),
    };
    match create_and_send_deploy(&state, context.user_id, app_id, "HEAD").await {
        Ok(id) => Json(json!({"deploymentId": id})).into_response(),
        Err(err) => (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
    }
}

pub async fn get_deployment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let row = sqlx::query(
        "SELECT d.* FROM deployments d JOIN apps a ON a.id=d.app_id WHERE d.id=$1 AND a.user_id=$2",
    )
    .bind(id)
    .bind(context.user_id)
    .fetch_optional(&state.db)
    .await;
    match row {
        Ok(Some(r)) => {
            let status = r.get::<String, _>("status");
            let queue =
                deployment_queue_status(&state, id, r.get::<Uuid, _>("server_id"), &status).await;
            Json(json!({
                "id": r.get::<Uuid,_>("id"),
                "appId": r.get::<Uuid,_>("app_id"),
                "status": status,
                "commitSha": r.get::<String,_>("commit_sha"),
                "failure": r.get::<Option<String>,_>("failure_summary"),
                "runtimeMetadata": r.try_get::<serde_json::Value,_>("runtime_metadata").unwrap_or_else(|_| json!({})),
                "queue": queue
            })).into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

pub(crate) async fn deployment_queue_status(
    state: &AppState,
    deployment_id: Uuid,
    server_id: Uuid,
    deployment_status: &str,
) -> DeploymentQueue {
    let job = match sqlx::query(
        "SELECT id, status, priority, created_at, updated_at
         FROM agent_jobs
         WHERE deployment_id=$1 AND job_type IN ('deploy','rollback')
         ORDER BY created_at DESC
         LIMIT 1",
    )
    .bind(deployment_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(job) => job,
        Err(err) => {
            tracing::warn!(error = %err, deployment_id = %deployment_id, "failed to load deployment queue job");
            return DeploymentQueue::not_applicable(None);
        }
    };

    let Some(job) = job else {
        return DeploymentQueue {
            status: if ACTIVE_DEPLOYMENT_STATUSES.contains(&deployment_status) {
                "building"
            } else {
                "not_applicable"
            }
            .to_string(),
            position: None,
            deploys_ahead: 0,
            updated_at: None,
        };
    };

    let job_status = job.get::<String, _>("status");
    let updated_at = job.get::<chrono::DateTime<chrono::Utc>, _>("updated_at");
    if job_status == "queued" {
        let priority = job.get::<i32, _>("priority");
        let created_at = job.get::<chrono::DateTime<chrono::Utc>, _>("created_at");
        let deploys_ahead = match sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint
             FROM agent_jobs
             WHERE server_id=$1
               AND status='queued'
               AND COALESCE(payload_json, '{}'::jsonb) <> '{}'::jsonb
               AND job_type IN ('deploy','rollback')
               AND (priority < $2 OR (priority = $2 AND created_at < $3))",
        )
        .bind(server_id)
        .bind(priority)
        .bind(created_at)
        .fetch_one(&state.db)
        .await
        {
            Ok(count) => count,
            Err(err) => {
                tracing::warn!(error = %err, deployment_id = %deployment_id, "failed to count deployment queue position");
                return DeploymentQueue::not_applicable(Some(updated_at));
            }
        };
        return DeploymentQueue {
            status: "queued".to_string(),
            position: Some(deploys_ahead + 1),
            deploys_ahead,
            updated_at: Some(updated_at),
        };
    }

    DeploymentQueue {
        status: if ACTIVE_DEPLOYMENT_STATUSES.contains(&deployment_status)
            || matches!(job_status.as_str(), "claimed" | "running")
        {
            "building"
        } else {
            "not_applicable"
        }
        .to_string(),
        position: None,
        deploys_ahead: 0,
        updated_at: Some(updated_at),
    }
}

pub async fn deployment_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let rows = sqlx::query("SELECT l.stream,l.line,l.created_at FROM deployment_logs l JOIN deployments d ON d.id=l.deployment_id JOIN apps a ON a.id=d.app_id WHERE l.deployment_id=$1 AND a.user_id=$2 ORDER BY l.created_at ASC LIMIT 1000")
        .bind(id).bind(context.user_id).fetch_all(&state.db).await;
    match rows {
        Ok(rows) => Json(rows.into_iter().map(|r| json!({"stream": r.get::<String,_>("stream"), "line": r.get::<String,_>("line")})).collect::<Vec<_>>()).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn logs_ws(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(deployment_id): Path<Uuid>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let origin_ok = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .and_then(crate::state::normalize_origin)
        .as_deref()
        .is_some_and(|origin| state.web_origin_allowed(origin));
    if !origin_ok {
        return StatusCode::FORBIDDEN.into_response();
    }
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let row = sqlx::query(
        "SELECT 1 FROM deployments d JOIN apps a ON a.id=d.app_id WHERE d.id=$1 AND a.user_id=$2",
    )
    .bind(deployment_id)
    .bind(context.user_id)
    .fetch_optional(&state.db)
    .await;
    let Ok(Some(_)) = row else {
        return StatusCode::NOT_FOUND.into_response();
    };
    ws.on_upgrade(move |socket| logs_socket(state, deployment_id, socket))
}

async fn logs_socket(state: AppState, deployment_id: Uuid, socket: WebSocket) {
    let (mut tx, _) = socket.split();
    let mut rx = state.logs.subscribe();
    while let Ok(event) = rx.recv().await {
        if event.deployment_id == deployment_id
            && tx
                .send(Message::Text(
                    json!({"stream": event.stream, "line": event.line}).to_string(),
                ))
                .await
                .is_err()
        {
            break;
        }
    }
}

pub async fn rollback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(app_id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(err) if err.to_string() == "sign in required" => {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(err) => return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response(),
    };
    match create_and_send_rollback(&state, context.user_id, app_id).await {
        Ok(id) => Json(json!({"rollbackDeploymentId": id})).into_response(),
        Err(err) => (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
    }
}

pub async fn create_and_send_deploy(
    state: &AppState,
    user_id: Uuid,
    app_id: Uuid,
    commit_sha: &str,
) -> anyhow::Result<Uuid> {
    ensure_no_active_deployment(state, app_id).await?;
    let app_row = sqlx::query(&format!(
        "SELECT {DEPLOY_APP_COLUMNS} FROM apps WHERE id=$1 AND user_id=$2"
    ))
    .bind(app_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await?;
    let app = DeployApp::from_row(&app_row);
    let server_id = app.server_id;
    let insert_deployment = sqlx::query(
        "INSERT INTO deployments (app_id,server_id,status,commit_sha,started_at,runtime_kind) \
         VALUES ($1,$2,'queued',$3,now(),$4) RETURNING id",
    )
    .bind(app_id)
    .bind(server_id)
    .bind(commit_sha)
    .bind(&app.runtime_kind)
    .fetch_one(&state.db)
    .await;
    let deployment_id: Uuid = match insert_deployment {
        Ok(row) => row.get("id"),
        Err(err) if is_active_deploy_unique_violation(&err) => {
            anyhow::bail!("an active deployment is already running for this app")
        }
        Err(err) => return Err(err.into()),
    };
    // Everything from here to just before the audit event is wrapped so that any
    // error marks the newly-created row 'failed' before propagating — preventing
    // it from sitting in 'queued' and blocking the next deploy for 30 minutes.
    let result: anyhow::Result<()> = async {
        let env_rows = sqlx::query("SELECT key,value_ciphertext FROM app_env_vars WHERE app_id=$1")
            .bind(app_id)
            .fetch_all(&state.db)
            .await?;
        let mut env = serde_json::Map::new();
        for row in env_rows {
            env.insert(
                row.get::<String, _>("key"),
                json!(state
                    .crypto
                    .decrypt(row.get::<String, _>("value_ciphertext").as_str())?),
            );
        }
        let github_token = github_access_token(state, user_id).await.ok().flatten();
        let payload = app.deploy_payload(
            deployment_id,
            app_id,
            route_key(app_id),
            commit_sha,
            env,
            github_token,
        );
        send_job(state, server_id, deployment_id, payload).await?;
        Ok(())
    }
    .await;
    if let Err(err) = result {
        fail_deployment_row(
            state,
            deployment_id,
            &format!("Deployment could not be started: {err}"),
        )
        .await;
        return Err(err);
    }
    record_audit_event(
        state,
        "deployment_requested",
        user_id,
        app_id,
        Some(deployment_id),
        None,
    )
    .await;
    Ok(deployment_id)
}

pub(crate) async fn create_and_send_rollback(
    state: &AppState,
    user_id: Uuid,
    app_id: Uuid,
) -> anyhow::Result<Uuid> {
    ensure_no_active_deployment(state, app_id).await?;
    let app = sqlx::query(
        "SELECT server_id,current_deployment_id,domain,container_port,runtime_kind \
         FROM apps WHERE id=$1 AND user_id=$2",
    )
    .bind(app_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await?;
    if !rollback_supported_for_runtime(&app.get::<String, _>("runtime_kind")) {
        anyhow::bail!("Compose rollback is not supported in Hostlet 0.5.0; redeploy the target revision instead");
    }
    let current: Option<Uuid> = app.get("current_deployment_id");
    let prev = sqlx::query("SELECT id,image_tag,container_name,published_port FROM deployments WHERE app_id=$1 AND status='success' AND ($2::uuid IS NULL OR id <> $2) ORDER BY finished_at DESC LIMIT 1")
        .bind(app_id).bind(current).fetch_optional(&state.db).await?;
    let Some(prev) = prev else {
        anyhow::bail!("no previous successful deployment is available");
    };
    let Some(published_port) = prev.get::<Option<i32>, _>("published_port") else {
        anyhow::bail!(
            "previous deployment is missing route metadata; deploy again before rolling back"
        );
    };
    let server_id: Uuid = app.get("server_id");
    let insert_rollback = sqlx::query(
        "INSERT INTO deployments \
         (app_id,server_id,status,commit_sha,started_at,image_tag,container_name) \
         VALUES ($1,$2,'queued','rollback',now(),$3,$4) RETURNING id",
    )
    .bind(app_id)
    .bind(server_id)
    .bind(prev.get::<Option<String>, _>("image_tag"))
    .bind(prev.get::<Option<String>, _>("container_name"))
    .fetch_one(&state.db)
    .await;
    let rollback_id: Uuid = match insert_rollback {
        Ok(row) => row.get("id"),
        Err(err) if is_active_deploy_unique_violation(&err) => {
            anyhow::bail!("an active deployment is already running for this app")
        }
        Err(err) => return Err(err.into()),
    };
    // Same guard as create_and_send_deploy: wrap the section after the INSERT so
    // any error marks the row 'failed' rather than leaving it 'queued' forever.
    let result: anyhow::Result<()> = async {
        sqlx::query(
            "INSERT INTO rollback_events (app_id,from_deployment_id,to_deployment_id,status) \
             VALUES ($1,$2,$3,'queued')",
        )
        .bind(app_id)
        .bind(current)
        .bind(prev.get::<Uuid, _>("id"))
        .execute(&state.db)
        .await?;
        let payload = json!({
            "type": "rollback",
            "deployment_id": rollback_id,
            "app_id": app_id,
            "route_key": route_key(app_id),
            "target_deployment_id": prev.get::<Uuid,_>("id"),
            "target_container": prev.get::<Option<String>,_>("container_name"),
            "domain": app.get::<String,_>("domain"),
            "container_port": app.get::<i32,_>("container_port"),
            "published_port": published_port,
        });
        send_job(state, server_id, rollback_id, payload).await?;
        Ok(())
    }
    .await;
    if let Err(err) = result {
        fail_deployment_row(
            state,
            rollback_id,
            &format!("Deployment could not be started: {err}"),
        )
        .await;
        return Err(err);
    }
    record_audit_event(
        state,
        "rollback_requested",
        user_id,
        app_id,
        Some(rollback_id),
        None,
    )
    .await;
    Ok(rollback_id)
}

async fn send_job(
    state: &AppState,
    server_id: Uuid,
    deployment_id: Uuid,
    payload: serde_json::Value,
) -> anyhow::Result<()> {
    let job_type = payload
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("deployment")
        .to_string();
    let app_id = payload
        .get("app_id")
        .and_then(|value| value.as_str())
        .and_then(|value| Uuid::parse_str(value).ok());
    let job_id = enqueue_agent_job(
        state,
        server_id,
        app_id,
        Some(deployment_id),
        &job_type,
        payload,
        10,
    )
    .await?;
    if let Some(app_id) = app_id {
        record_audit_event(
            state,
            &format!("{job_type}_job_queued"),
            Uuid::nil(),
            app_id,
            Some(deployment_id),
            Some(job_id),
        )
        .await;
    }
    // Best-effort: advance 'queued' → 'running' now that the job is enqueued.
    // If this update fails, the agent's own status report will correct the state;
    // we must NOT fail the call after the job row already exists.
    mark_deployment_running(state, deployment_id).await;
    Ok(())
}

async fn record_audit_event(
    state: &AppState,
    event_type: &str,
    actor_id: Uuid,
    app_id: Uuid,
    deployment_id: Option<Uuid>,
    job_id: Option<Uuid>,
) {
    let _ = sqlx::query(
        "INSERT INTO audit_events
           (actor_type,actor_id,event_type,app_id,deployment_id,job_id,metadata_json)
         VALUES ('owner',$1,$2,$3,$4,$5,'{}'::jsonb)",
    )
    .bind(actor_id.to_string())
    .bind(event_type)
    .bind(app_id)
    .bind(deployment_id)
    .bind(job_id)
    .execute(&state.db)
    .await;
}

pub async fn enqueue_agent_job(
    state: &AppState,
    server_id: Uuid,
    app_id: Option<Uuid>,
    deployment_id: Option<Uuid>,
    job_type: &str,
    payload: serde_json::Value,
    priority: i32,
) -> anyhow::Result<Uuid> {
    let id = sqlx::query(
        "INSERT INTO agent_jobs
           (server_id,app_id,deployment_id,job_type,status,payload_json,priority)
         VALUES ($1,$2,$3,$4,'queued',$5,$6)
         RETURNING id",
    )
    .bind(server_id)
    .bind(app_id)
    .bind(deployment_id)
    .bind(job_type)
    .bind(payload)
    .bind(priority)
    .fetch_one(&state.db)
    .await?
    .get::<Uuid, _>("id");
    Ok(id)
}

pub async fn job_signing_secret_for_server(
    state: &AppState,
    server_id: Uuid,
) -> anyhow::Result<String> {
    let encrypted: Option<String> =
        sqlx::query_scalar("SELECT job_signing_secret_ciphertext FROM servers WHERE id=$1")
            .bind(server_id)
            .fetch_optional(&state.db)
            .await?
            .flatten();
    match encrypted {
        Some(value) => state.crypto.decrypt(&value),
        None => Ok(state.job_signing_secret.clone()),
    }
}

/// Startup housekeeping: recover stale deployments, then run the best-effort
/// automatic Docker cleanup sweep so superseded deployment containers are reaped.
///
/// Only the stale-deployment recovery can fail this call; a cleanup sweep
/// failure is logged and swallowed so it never prevents startup.
pub async fn recover_stale_deployments_and_cleanup(state: &AppState) -> anyhow::Result<u64> {
    let recovered = recover_stale_deployments(state).await?;
    crate::cleanup::auto_cleanup_sweep(state).await;
    Ok(recovered)
}

pub async fn recover_stale_deployments(state: &AppState) -> anyhow::Result<u64> {
    let result = sqlx::query(
        "UPDATE deployments
         SET status='failed',
             failure_summary=COALESCE(failure_summary, 'Deployment was interrupted before completion. Start a new deployment to retry.'),
             finished_at=now()
         WHERE status = ANY($1)
           AND COALESCE(started_at, created_at) < now() - interval '30 minutes'",
    )
    .bind(ACTIVE_DEPLOYMENT_STATUSES)
    .execute(&state.db)
    .await?;
    Ok(result.rows_affected())
}

/// Marks a deployment row 'failed' when startup fails after the INSERT.
/// Prevents the row from sitting in 'queued'/'running' and blocking future deploys
/// for the 30-minute stale-recovery window.  DB errors are swallowed with a warning
/// because this is best-effort cleanup on an already-erroring path.
pub(crate) async fn fail_deployment_row(state: &AppState, deployment_id: Uuid, summary: &str) {
    if let Err(err) = sqlx::query(
        "UPDATE deployments \
         SET status='failed', failure_summary=$2, finished_at=now() \
         WHERE id=$1 AND status = ANY($3)",
    )
    .bind(deployment_id)
    .bind(summary)
    .bind(ACTIVE_DEPLOYMENT_STATUSES)
    .execute(&state.db)
    .await
    {
        tracing::warn!(
            error = %err,
            %deployment_id,
            "failed to mark deployment row as failed during cleanup"
        );
    }
}

/// Advances a deployment from 'queued' to 'running' (best-effort, idempotent).
/// Uses a status guard so the agent's own first report cannot be backtracked.
/// Any DB error is logged and swallowed; `send_job` must not fail after the job
/// is already enqueued.
pub(crate) async fn mark_deployment_running(state: &AppState, deployment_id: Uuid) {
    if let Err(err) =
        sqlx::query("UPDATE deployments SET status='running' WHERE id=$1 AND status='queued'")
            .bind(deployment_id)
            .execute(&state.db)
            .await
    {
        tracing::warn!(
            error = %err,
            %deployment_id,
            "failed to mark deployment running after enqueue"
        );
    }
}

pub(crate) async fn ensure_no_active_deployment(
    state: &AppState,
    app_id: Uuid,
) -> anyhow::Result<()> {
    let active = sqlx::query(
        "SELECT id,status FROM deployments WHERE app_id=$1 AND status = ANY($2) LIMIT 1",
    )
    .bind(app_id)
    .bind(ACTIVE_DEPLOYMENT_STATUSES)
    .fetch_optional(&state.db)
    .await?;
    if let Some(row) = active {
        anyhow::bail!(
            "deployment {} is already {} for this app",
            row.get::<Uuid, _>("id"),
            row.get::<String, _>("status")
        );
    }
    Ok(())
}

fn is_active_deploy_unique_violation(err: &sqlx::Error) -> bool {
    let Some(db_err) = err.as_database_error() else {
        return false;
    };
    db_err.code().as_deref() == Some("23505")
        && db_err
            .message()
            .contains("idx_deployments_one_active_per_app")
}

async fn github_access_token(state: &AppState, user_id: Uuid) -> anyhow::Result<Option<String>> {
    let row = sqlx::query(
        "SELECT access_token_ciphertext
         FROM github_accounts
         WHERE user_id=$1
         ORDER BY updated_at DESC
         LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;
    row.map(|row| {
        state
            .crypto
            .decrypt(row.get::<String, _>("access_token_ciphertext").as_str())
    })
    .transpose()
}

#[cfg(test)]
fn is_active_deployment_status(status: &str) -> bool {
    ACTIVE_DEPLOYMENT_STATUSES.contains(&status)
}

fn route_key(app_id: Uuid) -> String {
    format!("app-{app_id}")
}

fn rollback_supported_for_runtime(runtime_kind: &str) -> bool {
    runtime_kind != "compose"
}

#[cfg(test)]
mod tests {
    use super::{
        deployment_queue_status, is_active_deployment_status, rollback_supported_for_runtime,
        route_key,
    };
    use crate::state::AppState;
    use uuid::Uuid;

    const TEST_SERVER_ID: Uuid = Uuid::from_u128(1);

    #[test]
    fn route_key_is_app_prefixed_id() {
        let app_id = Uuid::parse_str("00000000-0000-0000-0000-0000000000ab").unwrap();
        assert_eq!(
            route_key(app_id),
            "app-00000000-0000-0000-0000-0000000000ab"
        );
    }

    #[test]
    fn active_statuses_match_deploy_lifecycle() {
        for status in [
            "queued",
            "running",
            "building",
            "starting",
            "health_checking",
            "routing",
        ] {
            assert!(is_active_deployment_status(status));
        }
        for status in ["success", "failed", "rolled_back", "canceled"] {
            assert!(!is_active_deployment_status(status));
        }
    }

    #[test]
    fn compose_rollback_is_disabled_for_release() {
        assert!(rollback_supported_for_runtime("single"));
        assert!(!rollback_supported_for_runtime("compose"));
    }

    #[tokio::test]
    async fn db_deployment_queue_reports_deploys_ahead() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_deploy_db(&state).await;
        let user_id = insert_user(&state).await;
        let first_app_id = insert_app(&state, user_id, "queue-first").await;
        let target_app_id = insert_app(&state, user_id, "queue-target").await;
        let first_deployment_id = insert_deployment(&state, first_app_id, "running").await;
        let target_deployment_id = insert_deployment(&state, target_app_id, "running").await;
        insert_deploy_job(
            &state,
            first_app_id,
            first_deployment_id,
            "queued",
            "2 minutes",
        )
        .await;
        insert_deploy_job(
            &state,
            target_app_id,
            target_deployment_id,
            "queued",
            "1 minute",
        )
        .await;

        let queue =
            deployment_queue_status(&state, target_deployment_id, TEST_SERVER_ID, "running").await;

        assert_eq!(queue.status, "queued");
        assert_eq!(queue.deploys_ahead, 1);
        assert_eq!(queue.position, Some(2));
        assert!(queue.updated_at.is_some());
    }

    #[tokio::test]
    async fn db_deployment_queue_without_job_falls_back_to_status() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_deploy_db(&state).await;
        let user_id = insert_user(&state).await;
        let app_id = insert_app(&state, user_id, "queue-no-job").await;
        let deployment_id = insert_deployment(&state, app_id, "building").await;

        let queue =
            deployment_queue_status(&state, deployment_id, TEST_SERVER_ID, "building").await;

        assert_eq!(queue.status, "building");
        assert_eq!(queue.deploys_ahead, 0);
        assert_eq!(queue.position, None);
        assert_eq!(queue.updated_at, None);
    }

    async fn reset_deploy_db(state: &AppState) {
        sqlx::query(
            "TRUNCATE app_screenshots, app_resource_snapshots, agent_jobs, deployments, app_env_vars, apps CASCADE",
        )
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query("DELETE FROM users")
            .execute(&state.db)
            .await
            .unwrap();
    }

    async fn insert_user(state: &AppState) -> Uuid {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO users (github_id, login) VALUES (9601,'deploy-queue-user') RETURNING id",
        )
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    async fn insert_app(state: &AppState, user_id: Uuid, name: &str) -> Uuid {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO apps
               (user_id,server_id,name,repo_full_name,branch,container_port,health_path,domain,runtime_kind,root_directory,public_exposure,auto_deploy)
             VALUES ($1,$2,$3,'hostlet-ci/node-hello','main',3000,'/health',$4,'single','.',true,false)
             RETURNING id",
        )
        .bind(user_id)
        .bind(TEST_SERVER_ID)
        .bind(name)
        .bind(format!("{name}.example.test"))
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    async fn insert_deployment(state: &AppState, app_id: Uuid, status: &str) -> Uuid {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO deployments (app_id,server_id,status,commit_sha,started_at,runtime_kind)
             VALUES ($1,$2,$3,'HEAD',now(),'single')
             RETURNING id",
        )
        .bind(app_id)
        .bind(TEST_SERVER_ID)
        .bind(status)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    async fn insert_deploy_job(
        state: &AppState,
        app_id: Uuid,
        deployment_id: Uuid,
        status: &str,
        age: &str,
    ) -> Uuid {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO agent_jobs
               (server_id,app_id,deployment_id,job_type,status,payload_json,created_at,updated_at)
             VALUES ($1,$2,$3,'deploy',$4,'{\"type\":\"deploy\"}'::jsonb,now() - $5::interval,now() - $5::interval)
             RETURNING id",
        )
        .bind(TEST_SERVER_ID)
        .bind(app_id)
        .bind(deployment_id)
        .bind(status)
        .bind(age)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }
}
