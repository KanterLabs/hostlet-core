use super::apps::request_context_or_response;
use super::*;

pub async fn cleanup_preview(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user_id = match request_context_or_response(&headers, &state).await {
        Ok(context) => context.user_id,
        Err(response) => return response,
    };
    match cleanup_plan(&state, user_id).await {
        Ok(plan) => Json(plan).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "failed to build cleanup preview");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn run_cleanup(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let context = match request_context_or_response(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    run_cleanup_inner(&state, Some(context.user_id)).await
}

pub(in crate::web) async fn run_cleanup_inner(
    state: &AppState,
    user_id: Option<Uuid>,
) -> axum::response::Response {
    let outcome = match crate::cleanup::run_cleanup(state).await {
        Ok(outcome) => outcome,
        Err(err) => {
            tracing::warn!(error = %err, "cleanup failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    record_audit_event(
        state,
        AuditEventInput {
            actor_type: user_id.map(|_| "owner").unwrap_or("cli"),
            actor_id: user_id.map(|id| id.to_string()),
            event_type: "cleanup_requested",
            app_id: None,
            deployment_id: None,
            job_id: outcome.docker_cleanup_job_id,
            metadata: serde_json::json!({"databaseDeleted": outcome.database_deleted}),
        },
    )
    .await;
    Json(serde_json::json!({
        "databaseDeleted": outcome.database_deleted,
        "dockerCleanupJobId": outcome.docker_cleanup_job_id,
    }))
    .into_response()
}

pub(in crate::web) async fn cleanup_plan(
    state: &AppState,
    _user_id: Uuid,
) -> anyhow::Result<crate::cleanup::CleanupPlan> {
    crate::cleanup::cleanup_plan(state).await
}
