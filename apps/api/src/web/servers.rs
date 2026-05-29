use super::*;

pub async fn list_servers(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let Some(_user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match sqlx::query("SELECT id,name,public_ip,kind,status,last_seen_at,created_at FROM servers WHERE kind='local' ORDER BY created_at ASC")
        .fetch_all(&state.db).await {
        Ok(rows) => Json(rows.into_iter().map(|r| serde_json::json!({
            "id": r.get::<Uuid,_>("id"), "name": r.get::<String,_>("name"), "publicIp": r.get::<Option<String>,_>("public_ip"),
            "kind": r.get::<String,_>("kind"), "status": r.get::<String,_>("status"), "lastSeenAt": r.get::<Option<chrono::DateTime<chrono::Utc>>,_>("last_seen_at")
        })).collect::<Vec<_>>()).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
pub async fn create_server() -> impl IntoResponse {
    (
        StatusCode::GONE,
        "remote VPS agents are deferred in this release; deploy to this Hostlet machine",
    )
        .into_response()
}

pub async fn server_install_command() -> impl IntoResponse {
    (
        StatusCode::GONE,
        "remote VPS agents are deferred in this release; deploy to this Hostlet machine",
    )
        .into_response()
}
