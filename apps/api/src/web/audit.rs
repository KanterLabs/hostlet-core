use super::*;

pub async fn audit_events(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let rows = sqlx::query(
        r#"
        SELECT e.id,
               e.actor_type,
               e.actor_id,
               e.event_type,
               e.app_id,
               e.deployment_id,
               e.job_id,
               e.metadata_json,
               e.created_at
        FROM audit_events e
        WHERE e.app_id IS NULL
           OR EXISTS (
                SELECT 1 FROM apps a
                WHERE a.id=e.app_id AND a.user_id=$1
           )
        ORDER BY e.created_at DESC
        LIMIT 200
        "#,
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| {
                    serde_json::json!({
                        "id": row.get::<Uuid, _>("id"),
                        "actorType": row.get::<String, _>("actor_type"),
                        "actorId": row.get::<Option<String>, _>("actor_id"),
                        "eventType": row.get::<String, _>("event_type"),
                        "appId": row.get::<Option<Uuid>, _>("app_id"),
                        "deploymentId": row.get::<Option<Uuid>, _>("deployment_id"),
                        "jobId": row.get::<Option<Uuid>, _>("job_id"),
                        "metadata": row.get::<serde_json::Value, _>("metadata_json"),
                        "createdAt": row.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
                    })
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "failed to list audit events");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
