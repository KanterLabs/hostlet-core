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
use hostlet_contracts::{AgentJobStatus, DeploymentStatus, RuntimeHealthStatus};
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
