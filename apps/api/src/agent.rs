use crate::{
    crypto::{sign, verify_token},
    state::{AgentConnection, AppState},
};
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
use serde::Deserialize;
use sqlx::Row;
use tokio::sync::mpsc;
use uuid::Uuid;

const MAX_LOG_LINE_BYTES: usize = 8 * 1024;
const MAX_LOG_LINES_PER_DEPLOYMENT: i64 = 20_000;
const MAX_HEALTH_EVENTS_PER_APP: i64 = 500;

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

pub async fn health_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(server_id) = authenticated_server_id(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let rows = sqlx::query(
        r#"
        SELECT a.id AS app_id,
               a.health_path,
               d.id AS deployment_id,
               d.container_name,
               d.published_port
        FROM apps a
        JOIN deployments d ON d.id = a.current_deployment_id
        WHERE a.server_id=$1
          AND d.server_id=$1
          AND d.status IN ('success','rolled_back')
          AND d.container_name IS NOT NULL
          AND d.published_port IS NOT NULL
        ORDER BY a.created_at ASC
        "#,
    )
    .bind(server_id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| {
                    serde_json::json!({
                        "appId": row.get::<Uuid, _>("app_id"),
                        "deploymentId": row.get::<Uuid, _>("deployment_id"),
                        "containerName": row.get::<String, _>("container_name"),
                        "publishedPort": row.get::<i32, _>("published_port"),
                        "healthPath": row.get::<String, _>("health_path"),
                    })
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
pub struct ClaimJobRequest {
    agent_id: Option<String>,
}

pub async fn claim_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ClaimJobRequest>,
) -> impl IntoResponse {
    let Some(server_id) = authenticated_server_id(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let agent_id = request
        .agent_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("local-agent");

    let _ = sqlx::query(
        "UPDATE agent_jobs
         SET status='queued',
             claimed_by=NULL,
             claimed_at=NULL,
             lease_expires_at=NULL,
             updated_at=now()
         WHERE server_id=$1
           AND status IN ('claimed','running')
           AND lease_expires_at < now()
           AND attempt < max_attempts",
    )
    .bind(server_id)
    .execute(&state.db)
    .await;

    let row = sqlx::query(
        r#"
        UPDATE agent_jobs
        SET status='claimed',
            attempt=attempt + 1,
            claimed_by=$2,
            claimed_at=now(),
            lease_expires_at=now() + interval '5 minutes',
            started_at=COALESCE(started_at, now()),
            updated_at=now()
        WHERE id = (
            SELECT id
            FROM agent_jobs
            WHERE server_id=$1
              AND status='queued'
              AND COALESCE(payload_json, '{}'::jsonb) <> '{}'::jsonb
            ORDER BY priority ASC, created_at ASC
            FOR UPDATE SKIP LOCKED
            LIMIT 1
        )
        RETURNING id, job_type, app_id, deployment_id, payload_json, attempt
        "#,
    )
    .bind(server_id)
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(row)) => {
            let mut payload = row.get::<serde_json::Value, _>("payload_json");
            if let Some(object) = payload.as_object_mut() {
                object.insert("job_id".into(), serde_json::json!(row.get::<Uuid, _>("id")));
                object.insert(
                    "job_type".into(),
                    serde_json::json!(row.get::<String, _>("job_type")),
                );
            }
            let secret = match crate::deploy::job_signing_secret_for_server(&state, server_id).await
            {
                Ok(secret) => secret,
                Err(err) => {
                    tracing::warn!(error = %err, %server_id, "failed to load job signing secret");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            };
            let body = match serde_json::to_vec(&payload) {
                Ok(body) => body,
                Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            };
            Json(serde_json::json!({
                "job": {
                    "id": row.get::<Uuid, _>("id"),
                    "type": row.get::<String, _>("job_type"),
                    "appId": row.get::<Option<Uuid>, _>("app_id"),
                    "deploymentId": row.get::<Option<Uuid>, _>("deployment_id"),
                    "attempt": row.get::<i32, _>("attempt"),
                    "payload": payload,
                    "signature": sign(&secret, &body),
                }
            }))
            .into_response()
        }
        Ok(None) => Json(serde_json::json!({"job": null})).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, %server_id, "failed to claim agent job");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct CompleteJobRequest {
    status: String,
    failure: Option<String>,
    result: Option<serde_json::Value>,
}

pub async fn complete_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<CompleteJobRequest>,
) -> impl IntoResponse {
    let Some(server_id) = authenticated_server_id(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !matches!(request.status.as_str(), "success" | "failed") {
        return (StatusCode::BAD_REQUEST, "invalid job status").into_response();
    }
    let result = sqlx::query(
        "UPDATE agent_jobs
         SET status=$1,
             failure_summary=$2,
             last_error=$2,
             result_json=$3,
             lease_expires_at=NULL,
             updated_at=now(),
             finished_at=now()
         WHERE id=$4 AND server_id=$5 AND status IN ('claimed','running')
         RETURNING job_type",
    )
    .bind(&request.status)
    .bind(request.failure.as_deref())
    .bind(request.result.unwrap_or_else(|| serde_json::json!({})))
    .bind(id)
    .bind(server_id)
    .fetch_optional(&state.db)
    .await;

    match result {
        Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::warn!(error = %err, job_id = %id, "failed to complete agent job");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn recover_stale_agent_jobs(state: &AppState) -> anyhow::Result<u64> {
    let retried = sqlx::query(
        "UPDATE agent_jobs
         SET status='queued',
             claimed_by=NULL,
             claimed_at=NULL,
             lease_expires_at=NULL,
             updated_at=now()
         WHERE status IN ('claimed','running')
           AND lease_expires_at < now()
           AND attempt < max_attempts",
    )
    .execute(&state.db)
    .await?
    .rows_affected();

    let failed = sqlx::query(
        "UPDATE agent_jobs
         SET status='failed',
             failure_summary=COALESCE(failure_summary, 'Agent job lease expired and retry limit was reached.'),
             last_error=COALESCE(last_error, 'Agent job lease expired and retry limit was reached.'),
             lease_expires_at=NULL,
             updated_at=now(),
             finished_at=now()
         WHERE status IN ('claimed','running')
           AND lease_expires_at < now()
           AND attempt >= max_attempts",
    )
    .execute(&state.db)
    .await?
    .rows_affected();

    Ok(retried + failed)
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
                let updated = sqlx::query("UPDATE deployments SET status=$1, image_tag=COALESCE($2,image_tag), container_name=COALESCE($3,container_name), published_port=COALESCE($4,published_port), failure_summary=$5, compose_project=COALESCE($6,compose_project), runtime_metadata=CASE WHEN $7::jsonb IS NULL THEN runtime_metadata ELSE $7::jsonb END, finished_at=CASE WHEN $1 IN ('success','failed','rolled_back') THEN now() ELSE finished_at END WHERE id=$8 AND server_id=$9")
                    .bind(status)
                    .bind(msg.get("image_tag").and_then(|v| v.as_str()))
                    .bind(msg.get("container_name").and_then(|v| v.as_str()))
                    .bind(msg.get("published_port").and_then(|v| v.as_i64()).and_then(|v| {
                        (1..=65_535).contains(&v).then_some(v as i32)
                    }))
                    .bind(msg.get("failure").and_then(|v| v.as_str()))
                    .bind(msg.get("compose_project").and_then(|v| v.as_str()))
                    .bind(msg.get("runtime_metadata").cloned())
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
        Some("health_status") => {
            let Some(app_id) = msg
                .get("app_id")
                .and_then(|v| v.as_str())
                .and_then(|v| Uuid::parse_str(v).ok())
            else {
                return;
            };
            let Some(status) = msg.get("status").and_then(|v| v.as_str()) else {
                return;
            };
            if !valid_health_status(status) {
                return;
            }
            let deployment_id = msg
                .get("deployment_id")
                .and_then(|v| v.as_str())
                .and_then(|v| Uuid::parse_str(v).ok());
            let container = msg.get("container_name").and_then(|v| v.as_str());
            if container.is_some_and(|value| !valid_container_name(value)) {
                return;
            }
            let http_status = msg
                .get("http_status")
                .and_then(|v| v.as_i64())
                .and_then(|v| (100..=599).contains(&v).then_some(v as i32));
            let latency_ms = msg
                .get("latency_ms")
                .and_then(|v| v.as_i64())
                .and_then(|v| (0..=300_000).contains(&v).then_some(v as i32));
            let failure_count = msg
                .get("failure_count")
                .and_then(|v| v.as_i64())
                .and_then(|v| (0..=1_000_000).contains(&v).then_some(v as i32))
                .unwrap_or(0);
            let success_count = msg
                .get("success_count")
                .and_then(|v| v.as_i64())
                .and_then(|v| (0..=1_000_000).contains(&v).then_some(v as i32))
                .unwrap_or(0);
            let checked_url = msg
                .get("checked_url")
                .and_then(|v| v.as_str())
                .map(|value| value.chars().take(512).collect::<String>());
            let error = msg
                .get("error")
                .and_then(|v| v.as_str())
                .map(|value| value.chars().take(512).collect::<String>());
            let updated = sqlx::query(
                r#"
                INSERT INTO app_health_snapshots
                  (app_id,deployment_id,container_name,status,checked_url,http_status,latency_ms,
                   failure_count,success_count,last_error,last_checked_at,last_healthy_at,updated_at)
                SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,now(),
                       CASE WHEN $4='healthy' THEN now() ELSE NULL END,
                       now()
                WHERE EXISTS (SELECT 1 FROM apps WHERE id=$1 AND server_id=$11)
                ON CONFLICT (app_id) DO UPDATE SET
                  deployment_id=EXCLUDED.deployment_id,
                  container_name=EXCLUDED.container_name,
                  status=EXCLUDED.status,
                  checked_url=EXCLUDED.checked_url,
                  http_status=EXCLUDED.http_status,
                  latency_ms=EXCLUDED.latency_ms,
                  failure_count=EXCLUDED.failure_count,
                  success_count=EXCLUDED.success_count,
                  last_error=EXCLUDED.last_error,
                  last_checked_at=EXCLUDED.last_checked_at,
                  last_healthy_at=CASE
                    WHEN EXCLUDED.status='healthy' THEN EXCLUDED.last_checked_at
                    ELSE app_health_snapshots.last_healthy_at
                  END,
                  updated_at=EXCLUDED.updated_at
                "#,
            )
            .bind(app_id)
            .bind(deployment_id)
            .bind(container)
            .bind(status)
            .bind(checked_url.as_deref())
            .bind(http_status)
            .bind(latency_ms)
            .bind(failure_count)
            .bind(success_count)
            .bind(error.as_deref())
            .bind(server_id)
            .execute(&state.db)
            .await
            .map(|done| done.rows_affected())
            .unwrap_or(0);
            if updated == 0 {
                return;
            }
            let _ = sqlx::query(
                r#"
                INSERT INTO app_health_events
                  (app_id,deployment_id,container_name,status,checked_url,http_status,latency_ms,error)
                VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
                "#,
            )
            .bind(app_id)
            .bind(deployment_id)
            .bind(container)
            .bind(status)
            .bind(checked_url.as_deref())
            .bind(http_status)
            .bind(latency_ms)
            .bind(error.as_deref())
            .execute(&state.db)
            .await;
            prune_health_events(state, app_id).await;
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

async fn prune_health_events(state: &AppState, app_id: Uuid) {
    let _ = sqlx::query(
        "DELETE FROM app_health_events
         WHERE app_id=$1
           AND created_at < now() - interval '7 days'",
    )
    .bind(app_id)
    .execute(&state.db)
    .await;
    let _ = sqlx::query(
        "DELETE FROM app_health_events
         WHERE id IN (
           SELECT id
           FROM app_health_events
           WHERE app_id=$1
           ORDER BY created_at DESC
           OFFSET $2
         )",
    )
    .bind(app_id)
    .bind(MAX_HEALTH_EVENTS_PER_APP)
    .execute(&state.db)
    .await;
}

async fn authenticated_server_id(state: &AppState, headers: &HeaderMap) -> Option<Uuid> {
    let server_id = header_uuid(headers, "x-hostlet-server-id")?;
    let token = headers
        .get("x-hostlet-agent-token")
        .and_then(|v| v.to_str().ok())?;
    let row = sqlx::query("SELECT agent_token_hash FROM servers WHERE id=$1")
        .bind(server_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()?;
    let expected: Option<String> = row.get("agent_token_hash");
    expected
        .as_deref()
        .filter(|hash| verify_token(token, hash))
        .map(|_| server_id)
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
    matches!(
        status,
        "queued" | "claimed" | "running" | "success" | "failed" | "cancelled" | "expired"
    )
}

fn valid_health_status(status: &str) -> bool {
    matches!(status, "unknown" | "healthy" | "degraded" | "unhealthy")
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

    #[test]
    fn runtime_health_statuses_are_explicit() {
        for status in ["unknown", "healthy", "degraded", "unhealthy"] {
            assert!(valid_health_status(status));
        }
        for status in ["success", "failed", "offline", "warning", ""] {
            assert!(!valid_health_status(status));
        }
    }

    #[test]
    fn container_names_are_limited_to_managed_hostlet_names() {
        assert!(valid_container_name("hostlet-app-123"));
        assert!(valid_container_name("hostlet-app_123.local"));
        assert!(!valid_container_name("other-app-123"));
        assert!(!valid_container_name("hostlet-app/../../bad"));
        assert!(!valid_container_name(&format!(
            "hostlet-{}",
            "a".repeat(140)
        )));
    }
}
