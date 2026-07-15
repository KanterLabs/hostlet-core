use super::*;

const REMOTE_AGENTS_DEFERRED: &str =
    "remote VPS agents are deferred in this release; deploy to this Hostlet machine";

pub async fn list_servers(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let Some(_user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let rows = sqlx::query(
        "SELECT id,name,public_ip,kind,status,last_seen_at,created_at,capabilities,draining,max_concurrent_apps,max_concurrent_builds,agent_protocol_version \
         FROM servers WHERE kind='local' ORDER BY created_at ASC",
    )
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|r| {
                    serde_json::json!({
                        "id": r.get::<Uuid, _>("id"),
                        "name": r.get::<String, _>("name"),
                        "publicIp": r.get::<Option<String>, _>("public_ip"),
                        "kind": r.get::<String, _>("kind"),
                        "status": r.get::<String, _>("status"),
                        "lastSeenAt": r.get::<Option<chrono::DateTime<chrono::Utc>>, _>("last_seen_at"),
                        "capabilities": r.get::<Vec<String>, _>("capabilities"),
                        "agentProtocolVersion": r.get::<i32, _>("agent_protocol_version"),
                        "draining": r.get::<bool, _>("draining"),
                        "maxConcurrentApps": r.get::<i32, _>("max_concurrent_apps"),
                        "maxConcurrentBuilds": r.get::<i32, _>("max_concurrent_builds"),
                    })
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn create_server() -> impl IntoResponse {
    (StatusCode::GONE, REMOTE_AGENTS_DEFERRED).into_response()
}

pub async fn server_install_command() -> impl IntoResponse {
    (StatusCode::GONE, REMOTE_AGENTS_DEFERRED).into_response()
}
