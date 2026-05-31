use super::*;

pub async fn restart_app_container(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(err) if err.to_string() == "sign in required" => {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(err) => return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response(),
    };
    let row = sqlx::query(
        r#"
        SELECT a.server_id,
               a.health_path,
               d.id AS deployment_id,
               d.container_name,
               d.published_port
        FROM apps a
        LEFT JOIN deployments d ON d.id = a.current_deployment_id
        WHERE a.id=$1 AND a.user_id=$2
        "#,
    )
    .bind(id)
    .bind(context.user_id)
    .fetch_optional(&state.db)
    .await;
    let Ok(Some(row)) = row else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(deployment_id) = row.get::<Option<Uuid>, _>("deployment_id") else {
        return (
            StatusCode::BAD_REQUEST,
            "app does not have a current deployment",
        )
            .into_response();
    };
    let Some(container_name) = row.get::<Option<String>, _>("container_name") else {
        return (
            StatusCode::BAD_REQUEST,
            "app does not have a current container",
        )
            .into_response();
    };
    let Some(published_port) = row.get::<Option<i32>, _>("published_port") else {
        return (
            StatusCode::BAD_REQUEST,
            "app does not have a published runtime port",
        )
            .into_response();
    };
    let payload = serde_json::json!({
        "type": "restart_container",
        "app_id": id,
        "deployment_id": deployment_id,
        "container_name": container_name,
        "published_port": published_port,
        "health_path": row.get::<String, _>("health_path"),
    });
    enqueue_interactive_agent_job(
        &state,
        row.get::<Uuid, _>("server_id"),
        id,
        Some(deployment_id),
        "restart_container",
        payload,
    )
    .await
}

pub(in crate::web) async fn enqueue_interactive_agent_job(
    state: &AppState,
    server_id: Uuid,
    app_id: Uuid,
    deployment_id: Option<Uuid>,
    job_type: &str,
    payload: serde_json::Value,
) -> axum::response::Response {
    match deploy::enqueue_agent_job(
        state,
        server_id,
        Some(app_id),
        deployment_id,
        job_type,
        payload,
        20,
    )
    .await
    {
        Ok(job_id) => {
            record_audit_event(
                state,
                AuditEventInput {
                    actor_type: "owner",
                    actor_id: None,
                    event_type: &format!("{job_type}_requested"),
                    app_id: Some(app_id),
                    deployment_id,
                    job_id: Some(job_id),
                    metadata: serde_json::json!({}),
                },
            )
            .await;
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({"jobId": job_id})),
            )
                .into_response()
        }
        Err(err) => {
            tracing::warn!(error = %err, app_id = %app_id, job_type, "failed to enqueue agent job");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn agent_job_visibility_predicate(user_param: usize, cloud_param: usize) -> String {
    format!(
        r#"
          AND (
            EXISTS (SELECT 1 FROM apps a WHERE a.id=j.app_id AND a.user_id=${user_param})
            OR EXISTS (
              SELECT 1 FROM deployments d
              JOIN apps a ON a.id=d.app_id
              WHERE d.id=j.deployment_id AND a.user_id=${user_param}
            )
            OR (
              ${cloud_param} = false
              AND j.app_id IS NULL
              AND j.deployment_id IS NULL
              AND (s.user_id=${user_param} OR s.kind='local')
            )
          )
        "#
    )
}

pub async fn agent_job_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let sql = format!(
        r#"
        SELECT j.id,j.job_type,j.app_id,j.status,j.failure_summary,j.finished_at
        FROM agent_jobs j
        JOIN servers s ON s.id = j.server_id
        WHERE j.id=$1
          {}
        "#,
        agent_job_visibility_predicate(2, 3)
    );
    let row = sqlx::query(&sql)
        .bind(id)
        .bind(user_id)
        .bind(false)
        .fetch_optional(&state.db)
        .await;
    match row {
        Ok(Some(row)) => {
            let mut finalized_delete = false;
            if row.get::<String, _>("status") == "success"
                && row.get::<String, _>("job_type") == "delete_app"
                && row.get::<Option<Uuid>, _>("app_id").is_some()
            {
                finalized_delete = finalize_delete_app_from_job(&state, id)
                    .await
                    .unwrap_or(false);
            }
            let mut status = row.get::<String, _>("status");
            if status == "success"
                && row.get::<String, _>("job_type") == "delete_app"
                && row.get::<Option<Uuid>, _>("app_id").is_some()
                && !finalized_delete
            {
                status = "running".into();
            }
            Json(serde_json::json!({
            "id": row.get::<Uuid, _>("id"),
            "status": status,
            "failure": row.get::<Option<String>, _>("failure_summary"),
            "finishedAt": row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("finished_at")
            }))
            .into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn list_agent_jobs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let sql = format!(
        r#"
        SELECT j.id,j.job_type,j.app_id,j.deployment_id,j.status,j.failure_summary,
               j.attempt,j.max_attempts,j.claimed_by,j.created_at,j.updated_at,j.finished_at
        FROM agent_jobs j
        JOIN servers s ON s.id = j.server_id
        WHERE true
          {}
        ORDER BY j.created_at DESC
        LIMIT 200
        "#,
        agent_job_visibility_predicate(1, 2)
    );
    let rows = sqlx::query(&sql)
        .bind(user_id)
        .bind(false)
        .fetch_all(&state.db)
        .await;
    match rows {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| {
                    serde_json::json!({
                        "id": row.get::<Uuid, _>("id"),
                        "type": row.get::<String, _>("job_type"),
                        "appId": row.get::<Option<Uuid>, _>("app_id"),
                        "deploymentId": row.get::<Option<Uuid>, _>("deployment_id"),
                        "status": row.get::<String, _>("status"),
                        "failure": row.get::<Option<String>, _>("failure_summary"),
                        "attempt": row.get::<i32, _>("attempt"),
                        "maxAttempts": row.get::<i32, _>("max_attempts"),
                        "claimedBy": row.get::<Option<String>, _>("claimed_by"),
                        "createdAt": row.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
                        "updatedAt": row.get::<chrono::DateTime<chrono::Utc>, _>("updated_at"),
                        "finishedAt": row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("finished_at"),
                    })
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "failed to list agent jobs");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn retry_agent_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let sql = format!(
        r#"
        UPDATE agent_jobs j
        SET status='queued',
            failure_summary=NULL,
            last_error=NULL,
            claimed_by=NULL,
            claimed_at=NULL,
            lease_expires_at=NULL,
            finished_at=NULL,
            updated_at=now()
        FROM servers s
        WHERE j.id=$1
          AND s.id=j.server_id
          {}
          AND j.status IN ('failed','expired','cancelled')
          AND COALESCE(j.payload_json, '{{}}'::jsonb) <> '{{}}'::jsonb
        RETURNING j.app_id,j.deployment_id
        "#,
        agent_job_visibility_predicate(2, 3)
    );
    let result = sqlx::query(&sql)
        .bind(id)
        .bind(user_id)
        .bind(false)
        .fetch_optional(&state.db)
        .await;
    match result {
        Ok(Some(row)) => {
            record_audit_event(
                &state,
                AuditEventInput {
                    actor_type: "owner",
                    actor_id: None,
                    event_type: "agent_job_retried",
                    app_id: row.get::<Option<Uuid>, _>("app_id"),
                    deployment_id: row.get::<Option<Uuid>, _>("deployment_id"),
                    job_id: Some(id),
                    metadata: serde_json::json!({}),
                },
            )
            .await;
            StatusCode::ACCEPTED.into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::warn!(error = %err, job_id = %id, "failed to retry agent job");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn cancel_agent_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let sql = format!(
        r#"
        UPDATE agent_jobs j
        SET status='cancelled',
            failure_summary='Cancelled by owner before the agent started work.',
            last_error='Cancelled by owner before the agent started work.',
            finished_at=now(),
            updated_at=now()
        FROM servers s
        WHERE j.id=$1
          AND s.id=j.server_id
          {}
          AND j.status='queued'
        RETURNING j.app_id,j.deployment_id
        "#,
        agent_job_visibility_predicate(2, 3)
    );
    let result = sqlx::query(&sql)
        .bind(id)
        .bind(user_id)
        .bind(false)
        .fetch_optional(&state.db)
        .await;
    match result {
        Ok(Some(row)) => {
            record_audit_event(
                &state,
                AuditEventInput {
                    actor_type: "owner",
                    actor_id: None,
                    event_type: "agent_job_cancelled",
                    app_id: row.get::<Option<Uuid>, _>("app_id"),
                    deployment_id: row.get::<Option<Uuid>, _>("deployment_id"),
                    job_id: Some(id),
                    metadata: serde_json::json!({}),
                },
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::warn!(error = %err, job_id = %id, "failed to cancel agent job");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
