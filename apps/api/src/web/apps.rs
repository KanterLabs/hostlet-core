use super::*;

mod addons;
mod create;
mod queries;
mod request_auth;
mod update;

pub use create::create_app;
pub use update::update_app;

pub(in crate::web) use request_auth::request_context_or_response;

pub async fn list_apps(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let rows = sqlx::query(&queries::list_query())
        .bind(user_id)
        .fetch_all(&state.db)
        .await;
    match rows {
        Ok(rows) => Json(rows.into_iter().map(app_json).collect::<Vec<_>>()).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn get_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let app = sqlx::query(&queries::get_query())
        .bind(id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await;
    match app {
        Ok(Some(row)) => Json(app_json(row)).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}
