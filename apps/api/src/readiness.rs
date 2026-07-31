use crate::state::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

/// Control-plane readiness: the process is only ready when it can execute a
/// cheap query against its configured Postgres database.
pub async fn readyz(State(state): State<AppState>) -> Response {
    match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.db)
        .await
    {
        Ok(1) => Json(serde_json::json!({"status":"ready"})).into_response(),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"status":"not_ready"})),
        )
            .into_response(),
    }
}
