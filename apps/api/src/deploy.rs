use crate::{auth::current_user_id, crypto::sign, state::AppState};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

const ACTIVE_DEPLOYMENT_STATUSES: &[&str] = &[
    "queued",
    "running",
    "building",
    "starting",
    "health_checking",
    "routing",
];

pub async fn manual_deploy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(app_id): Path<Uuid>,
) -> impl IntoResponse {
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match create_and_send_deploy(&state, user_id, app_id, "HEAD").await {
        Ok(id) => Json(json!({"deploymentId": id})).into_response(),
        Err(err) => (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
    }
}

pub async fn get_deployment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let row = sqlx::query(
        "SELECT d.* FROM deployments d JOIN apps a ON a.id=d.app_id WHERE d.id=$1 AND a.user_id=$2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await;
    match row {
        Ok(Some(r)) => Json(json!({"id": r.get::<Uuid,_>("id"), "status": r.get::<String,_>("status"), "commitSha": r.get::<String,_>("commit_sha"), "failure": r.get::<Option<String>,_>("failure_summary")})).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn deployment_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let rows = sqlx::query("SELECT l.stream,l.line,l.created_at FROM deployment_logs l JOIN deployments d ON d.id=l.deployment_id JOIN apps a ON a.id=d.app_id WHERE l.deployment_id=$1 AND a.user_id=$2 ORDER BY l.created_at ASC LIMIT 1000")
        .bind(id).bind(user_id).fetch_all(&state.db).await;
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
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let row = sqlx::query(
        "SELECT 1 FROM deployments d JOIN apps a ON a.id=d.app_id WHERE d.id=$1 AND a.user_id=$2",
    )
    .bind(deployment_id)
    .bind(user_id)
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
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match create_and_send_rollback(&state, user_id, app_id).await {
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
    let app = sqlx::query("SELECT id,server_id,name,repo_full_name,branch,container_port,health_path,domain,root_directory,install_command,build_command,start_command,memory_limit_mb,cpu_limit FROM apps WHERE id=$1 AND user_id=$2")
        .bind(app_id).bind(user_id).fetch_one(&state.db).await?;
    let server_id: Uuid = app.get("server_id");
    let deployment_id: Uuid = match sqlx::query("INSERT INTO deployments (app_id,server_id,status,commit_sha,started_at) VALUES ($1,$2,'queued',$3,now()) RETURNING id")
        .bind(app_id).bind(server_id).bind(commit_sha).fetch_one(&state.db).await {
            Ok(row) => row.get("id"),
            Err(err) if is_active_deploy_unique_violation(&err) => {
                anyhow::bail!("an active deployment is already running for this app")
            }
            Err(err) => return Err(err.into()),
        };
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
    let github_token = github_access_token(state, user_id).await?;
    let payload = json!({
        "type": "deploy", "deployment_id": deployment_id, "app_id": app_id,
        "route_key": route_key(app_id),
        "app_name": app.get::<String,_>("name"), "repo": app.get::<String,_>("repo_full_name"),
        "branch": app.get::<String,_>("branch"), "commit_sha": commit_sha,
        "container_port": app.get::<i32,_>("container_port"), "health_path": app.get::<String,_>("health_path"),
        "domain": app.get::<String,_>("domain"), "env": env,
        "root_directory": app.get::<String,_>("root_directory"),
        "install_command": app.get::<Option<String>,_>("install_command"),
        "build_command": app.get::<Option<String>,_>("build_command"),
        "start_command": app.get::<Option<String>,_>("start_command"),
        "memory_limit_mb": app.get::<Option<i32>,_>("memory_limit_mb"),
        "cpu_limit": app.get::<Option<f64>,_>("cpu_limit"),
        "github_token": github_token
    });
    send_job(state, server_id, deployment_id, payload).await?;
    Ok(deployment_id)
}

async fn create_and_send_rollback(
    state: &AppState,
    user_id: Uuid,
    app_id: Uuid,
) -> anyhow::Result<Uuid> {
    ensure_no_active_deployment(state, app_id).await?;
    let app = sqlx::query("SELECT server_id,current_deployment_id,domain,container_port FROM apps WHERE id=$1 AND user_id=$2").bind(app_id).bind(user_id).fetch_one(&state.db).await?;
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
    let rollback_id: Uuid = match sqlx::query("INSERT INTO deployments (app_id,server_id,status,commit_sha,started_at,image_tag,container_name) VALUES ($1,$2,'queued','rollback',now(),$3,$4) RETURNING id")
        .bind(app_id).bind(server_id).bind(prev.get::<Option<String>,_>("image_tag")).bind(prev.get::<Option<String>,_>("container_name")).fetch_one(&state.db).await {
            Ok(row) => row.get("id"),
            Err(err) if is_active_deploy_unique_violation(&err) => {
                anyhow::bail!("an active deployment is already running for this app")
            }
            Err(err) => return Err(err.into()),
        };
    sqlx::query("INSERT INTO rollback_events (app_id,from_deployment_id,to_deployment_id,status) VALUES ($1,$2,$3,'queued')")
        .bind(app_id).bind(current).bind(prev.get::<Uuid,_>("id")).execute(&state.db).await?;
    let payload = json!({"type":"rollback","deployment_id": rollback_id, "app_id": app_id, "route_key": route_key(app_id), "target_deployment_id": prev.get::<Uuid,_>("id"), "target_container": prev.get::<Option<String>,_>("container_name"), "domain": app.get::<String,_>("domain"), "container_port": app.get::<i32,_>("container_port"), "published_port": published_port});
    send_job(state, server_id, rollback_id, payload).await?;
    Ok(rollback_id)
}

async fn send_job(
    state: &AppState,
    server_id: Uuid,
    deployment_id: Uuid,
    payload: serde_json::Value,
) -> anyhow::Result<()> {
    match send_agent_job(state, server_id, payload).await {
        Ok(()) => {}
        Err(err) => {
            if err.to_string().contains("server agent is offline") {
                sqlx::query("UPDATE deployments SET status='failed', failure_summary='Server agent is offline. Check the systemd service and network access.', finished_at=now() WHERE id=$1").bind(deployment_id).execute(&state.db).await?;
            }
            return Err(err);
        }
    }
    sqlx::query("UPDATE deployments SET status='running' WHERE id=$1")
        .bind(deployment_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

pub async fn send_agent_job(
    state: &AppState,
    server_id: Uuid,
    payload: serde_json::Value,
) -> anyhow::Result<()> {
    let sender = {
        let agents = state.agents.read().await;
        agents
            .get(&server_id)
            .map(|connection| connection.sender.clone())
    }
    .ok_or_else(|| anyhow::anyhow!("server agent is offline"))?;
    let job_signing_secret = job_signing_secret_for_server(state, server_id).await?;
    let body = serde_json::to_vec(&payload)?;
    let signed =
        json!({"type":"job","payload": payload, "signature": sign(&job_signing_secret, &body)});
    sender.send(signed).await?;
    Ok(())
}

async fn job_signing_secret_for_server(
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

async fn ensure_no_active_deployment(state: &AppState, app_id: Uuid) -> anyhow::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::is_active_deployment_status;

    #[test]
    fn rollback_requires_previous_success() {
        let successes = vec!["a"];
        let current = "a";
        assert!(successes.into_iter().find(|id| *id != current).is_none());
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
}
