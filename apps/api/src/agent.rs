use crate::{
    crypto::verify_token,
    state::{AgentConnection, AppState},
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
use sqlx::Row;
use tokio::sync::mpsc;
use uuid::Uuid;

const MAX_LOG_LINE_BYTES: usize = 8 * 1024;
const MAX_LOG_LINES_PER_DEPLOYMENT: i64 = 20_000;

pub async fn register() -> impl IntoResponse {
    (
        StatusCode::GONE,
        "remote agent registration is deferred in this release; use the local Hostlet agent",
    )
        .into_response()
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
    let connection_id = Uuid::new_v4();
    let already_connected = {
        let mut agents = state.agents.write().await;
        if agents
            .get(&server_id)
            .is_some_and(|connection| !connection.sender.is_closed())
        {
            true
        } else {
            agents.insert(
                server_id,
                AgentConnection {
                    connection_id,
                    sender: tx,
                },
            );
            false
        }
    };
    if already_connected {
        tracing::warn!(%server_id, "rejected duplicate agent websocket connection");
        let _ = sender.send(Message::Close(None)).await;
        return;
    }
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
    let mut agents = state.agents.write().await;
    if agents
        .get(&server_id)
        .is_some_and(|connection| connection_is_current(connection, connection_id))
    {
        agents.remove(&server_id);
        drop(agents);
        let _ = sqlx::query("UPDATE servers SET status='offline' WHERE id=$1")
            .bind(server_id)
            .execute(&db)
            .await;
    }
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
                if !valid_deployment_status(status) {
                    return;
                }
                let updated = sqlx::query("UPDATE deployments SET status=$1, image_tag=COALESCE($2,image_tag), container_name=COALESCE($3,container_name), published_port=COALESCE($4,published_port), failure_summary=$5, finished_at=CASE WHEN $1 IN ('success','failed','rolled_back') THEN now() ELSE finished_at END WHERE id=$6 AND server_id=$7")
                    .bind(status)
                    .bind(msg.get("image_tag").and_then(|v| v.as_str()))
                    .bind(msg.get("container_name").and_then(|v| v.as_str()))
                    .bind(msg.get("published_port").and_then(|v| v.as_i64()).and_then(|v| {
                        (1..=65_535).contains(&v).then_some(v as i32)
                    }))
                    .bind(msg.get("failure").and_then(|v| v.as_str()))
                    .bind(id)
                    .bind(server_id)
                    .execute(&state.db).await
                    .map(|done| done.rows_affected())
                    .unwrap_or(0);
                if matches!(status, "success" | "rolled_back") && updated == 1 {
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
                if !matches!(stream, "stdout" | "stderr" | "git" | "docker" | "caddy") {
                    return;
                }
                let line = truncate_log_line(line, MAX_LOG_LINE_BYTES);
                let inserted = sqlx::query(
                    "INSERT INTO deployment_logs (deployment_id,stream,line)
                     SELECT $1,$2,$3
                     WHERE EXISTS (SELECT 1 FROM deployments WHERE id=$1 AND server_id=$4)
                       AND (SELECT count(*) FROM deployment_logs WHERE deployment_id=$1) < $5",
                )
                .bind(id)
                .bind(stream)
                .bind(&line)
                .bind(server_id)
                .bind(MAX_LOG_LINES_PER_DEPLOYMENT)
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
                    line,
                });
            }
        }
        Some("resource_stats") => {
            let Some(container) = msg.get("container").and_then(|v| v.as_str()) else {
                return;
            };
            if !valid_container_name(container) {
                return;
            }
            let value = |key: &str, default: &str| {
                msg.get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or(default)
                    .chars()
                    .take(128)
                    .collect::<String>()
            };
            let _ = sqlx::query(
                r#"
                INSERT INTO app_resource_snapshots
                  (container_name,cpu_percent,memory_usage,memory_percent,network_io,block_io,pids,sampled_at)
                VALUES ($1,$2,$3,$4,$5,$6,$7,now())
                ON CONFLICT (container_name) DO UPDATE SET
                  cpu_percent=EXCLUDED.cpu_percent,
                  memory_usage=EXCLUDED.memory_usage,
                  memory_percent=EXCLUDED.memory_percent,
                  network_io=EXCLUDED.network_io,
                  block_io=EXCLUDED.block_io,
                  pids=EXCLUDED.pids,
                  sampled_at=EXCLUDED.sampled_at
                "#,
            )
            .bind(container)
            .bind(value("cpuPercent", "0%"))
            .bind(value("memoryUsage", "0B / 0B"))
            .bind(value("memoryPercent", "0%"))
            .bind(value("networkIo", "0B / 0B"))
            .bind(value("blockIo", "0B / 0B"))
            .bind(value("pids", "0"))
            .execute(&state.db)
            .await;
        }
        Some("job_status") => {
            let Some(job_id) = msg
                .get("job_id")
                .and_then(|v| v.as_str())
                .and_then(|v| Uuid::parse_str(v).ok())
            else {
                return;
            };
            let Some(status) = msg.get("status").and_then(|v| v.as_str()) else {
                return;
            };
            if !valid_agent_job_status(status) {
                return;
            }
            let _ = sqlx::query(
                "UPDATE agent_jobs
                 SET status=$1,
                     failure_summary=$2,
                     updated_at=now(),
                     finished_at=CASE WHEN $1 IN ('success','failed') THEN now() ELSE finished_at END
                 WHERE id=$3 AND server_id=$4",
            )
            .bind(status)
            .bind(msg.get("failure").and_then(|v| v.as_str()))
            .bind(job_id)
            .bind(server_id)
            .execute(&state.db)
            .await;
        }
        _ => {}
    }
}

fn valid_deployment_status(status: &str) -> bool {
    matches!(
        status,
        "queued"
            | "building"
            | "starting"
            | "health_checking"
            | "routing"
            | "running"
            | "success"
            | "failed"
            | "rolled_back"
    )
}

fn valid_agent_job_status(status: &str) -> bool {
    matches!(status, "queued" | "running" | "success" | "failed")
}

fn valid_container_name(value: &str) -> bool {
    value.starts_with("hostlet-")
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn truncate_log_line(line: &str, max_bytes: usize) -> String {
    if line.len() <= max_bytes {
        return line.to_string();
    }
    let mut end = max_bytes;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...[truncated]", &line[..end])
}

fn header_uuid(headers: &HeaderMap, key: &str) -> Option<Uuid> {
    headers
        .get(key)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
}

fn connection_is_current(connection: &AgentConnection, connection_id: Uuid) -> bool {
    connection.connection_id == connection_id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_agent_connection_does_not_match_current_connection() {
        let (sender, _receiver) = mpsc::channel(1);
        let current = Uuid::new_v4();
        let stale = Uuid::new_v4();
        let connection = AgentConnection {
            connection_id: current,
            sender,
        };

        assert!(connection_is_current(&connection, current));
        assert!(!connection_is_current(&connection, stale));
    }
}
