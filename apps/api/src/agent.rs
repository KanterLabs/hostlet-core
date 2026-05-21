use crate::{
    crypto::{hash_token, random_token, verify_token},
    state::AppState,
};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use sqlx::Row;
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct RegisterBody {
    server_id: Uuid,
    install_token: String,
}

pub async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterBody>,
) -> impl IntoResponse {
    let agent_token = random_token(64);
    let res = sqlx::query(
        "UPDATE servers
         SET agent_token_hash=$1, install_token_hash=NULL, status='online', last_seen_at=now()
         WHERE id=$2 AND install_token_hash=$3",
    )
    .bind(hash_token(&agent_token))
    .bind(body.server_id)
    .bind(hash_token(&body.install_token))
    .execute(&state.db)
    .await;
    match res {
        Ok(done) if done.rows_affected() == 1 => Json(serde_json::json!({
            "agentToken": agent_token,
            "jobSigningSecret": state.job_signing_secret
        }))
        .into_response(),
        Ok(_) => StatusCode::UNAUTHORIZED.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn ws(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let Some(server_id) = header_uuid(&headers, "x-hostlet-server-id") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(token) = headers
        .get("x-hostlet-agent-token")
        .and_then(|v| v.to_str().ok())
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let row = sqlx::query("SELECT agent_token_hash FROM servers WHERE id=$1")
        .bind(server_id)
        .fetch_optional(&state.db)
        .await;
    let Ok(Some(row)) = row else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let expected: Option<String> = row.get("agent_token_hash");
    if !expected
        .as_deref()
        .map(|h| verify_token(token, h))
        .unwrap_or(false)
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(state, server_id, socket))
}

pub async fn event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(value): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(server_id) = header_uuid(&headers, "x-hostlet-server-id") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(token) = headers
        .get("x-hostlet-agent-token")
        .and_then(|v| v.to_str().ok())
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let row = sqlx::query("SELECT agent_token_hash FROM servers WHERE id=$1")
        .bind(server_id)
        .fetch_optional(&state.db)
        .await;
    let Ok(Some(row)) = row else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let expected: Option<String> = row.get("agent_token_hash");
    if !expected
        .as_deref()
        .map(|h| verify_token(token, h))
        .unwrap_or(false)
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    handle_agent_message(&state, server_id, value).await;
    StatusCode::ACCEPTED.into_response()
}

async fn handle_socket(state: AppState, server_id: Uuid, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::channel::<serde_json::Value>(32);
    state.agents.write().await.insert(server_id, tx);
    let _ = sqlx::query("UPDATE servers SET status='online', last_seen_at=now() WHERE id=$1")
        .bind(server_id)
        .execute(&state.db)
        .await;
    let db = state.db.clone();
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(Message::Text(msg.to_string())).await.is_err() {
                break;
            }
        }
    });
    while let Some(Ok(msg)) = receiver.next().await {
        if let Message::Text(text) = msg {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                handle_agent_message(&state, server_id, value).await;
            }
        }
    }
    send_task.abort();
    state.agents.write().await.remove(&server_id);
    let _ = sqlx::query("UPDATE servers SET status='offline' WHERE id=$1")
        .bind(server_id)
        .execute(&db)
        .await;
}

async fn handle_agent_message(state: &AppState, server_id: Uuid, msg: serde_json::Value) {
    match msg.get("type").and_then(|v| v.as_str()) {
        Some("heartbeat") => {
            let _ =
                sqlx::query("UPDATE servers SET status='online', last_seen_at=now() WHERE id=$1")
                    .bind(server_id)
                    .execute(&state.db)
                    .await;
        }
        Some("deployment_status") => {
            if let (Some(id), Some(status)) = (
                msg.get("deployment_id")
                    .and_then(|v| v.as_str())
                    .and_then(|v| Uuid::parse_str(v).ok()),
                msg.get("status").and_then(|v| v.as_str()),
            ) {
                let updated = sqlx::query("UPDATE deployments SET status=$1, image_tag=COALESCE($2,image_tag), container_name=COALESCE($3,container_name), failure_summary=$4, finished_at=CASE WHEN $1 IN ('success','failed','rolled_back') THEN now() ELSE finished_at END WHERE id=$5 AND server_id=$6")
                    .bind(status).bind(msg.get("image_tag").and_then(|v| v.as_str())).bind(msg.get("container_name").and_then(|v| v.as_str())).bind(msg.get("failure").and_then(|v| v.as_str())).bind(id).bind(server_id).execute(&state.db).await
                    .map(|done| done.rows_affected())
                    .unwrap_or(0);
                if status == "success" && updated == 1 {
                    let _ = sqlx::query("UPDATE apps SET current_deployment_id=$1, domain=COALESCE($2, domain) WHERE id=(SELECT app_id FROM deployments WHERE id=$1)")
                        .bind(id)
                        .bind(msg.get("local_url").and_then(|v| v.as_str()))
                        .execute(&state.db)
                        .await;
                }
            }
        }
        Some("log") => {
            if let (Some(id), Some(line)) = (
                msg.get("deployment_id")
                    .and_then(|v| v.as_str())
                    .and_then(|v| Uuid::parse_str(v).ok()),
                msg.get("line").and_then(|v| v.as_str()),
            ) {
                let stream = msg
                    .get("stream")
                    .and_then(|v| v.as_str())
                    .unwrap_or("stdout");
                let inserted = sqlx::query(
                    "INSERT INTO deployment_logs (deployment_id,stream,line)
                     SELECT $1,$2,$3
                     WHERE EXISTS (SELECT 1 FROM deployments WHERE id=$1 AND server_id=$4)",
                )
                .bind(id)
                .bind(stream)
                .bind(line)
                .bind(server_id)
                .execute(&state.db)
                .await
                .map(|done| done.rows_affected())
                .unwrap_or(0);
                if inserted == 0 {
                    return;
                }
                let _ = state.logs.send(crate::state::LogEvent {
                    deployment_id: id,
                    stream: stream.into(),
                    line: line.into(),
                });
            }
        }
        _ => {}
    }
}

fn header_uuid(headers: &HeaderMap, key: &str) -> Option<Uuid> {
    headers
        .get(key)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
}
