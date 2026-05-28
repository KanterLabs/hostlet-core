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

async fn enqueue_interactive_agent_job(
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

pub async fn health_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let rows = sqlx::query(
        r#"
        SELECT COALESCE(hs.status, 'unknown') AS status, count(*) AS count
        FROM apps a
        LEFT JOIN app_health_snapshots hs ON hs.app_id = a.id
        WHERE a.user_id=$1
        GROUP BY COALESCE(hs.status, 'unknown')
        "#,
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => Json(health_counts_json(rows)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn system_health_counts(state: &AppState) -> serde_json::Value {
    let rows = sqlx::query(
        r#"
        SELECT COALESCE(hs.status, 'unknown') AS status, count(*) AS count
        FROM apps a
        LEFT JOIN app_health_snapshots hs ON hs.app_id = a.id
        GROUP BY COALESCE(hs.status, 'unknown')
        "#,
    )
    .fetch_all(&state.db)
    .await;
    health_counts_json(rows.unwrap_or_default())
}

fn health_counts_json(rows: Vec<sqlx::postgres::PgRow>) -> serde_json::Value {
    let mut counts = serde_json::json!({
        "healthy": 0,
        "degraded": 0,
        "unhealthy": 0,
        "unknown": 0
    });
    for row in rows {
        let status: String = row.get("status");
        if let Some(value) = counts.get_mut(&status) {
            *value = serde_json::json!(row.get::<i64, _>("count"));
        }
    }
    counts
}

pub async fn system_version(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(_user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let update = cached_update_check(&state).await;
    Json(serde_json::json!({
        "currentVersion": env!("CARGO_PKG_VERSION"),
        "mode": state.mode.as_str(),
        "updateChecksEnabled": state.update_checks_enabled,
        "update": update,
    }))
    .into_response()
}

pub async fn system_update_check(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(_user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !state.update_checks_enabled {
        return (
            StatusCode::BAD_REQUEST,
            "Hostlet update checks are disabled by HOSTLET_UPDATE_CHECKS=false",
        )
            .into_response();
    }
    match refresh_update_check(&state).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => (StatusCode::BAD_GATEWAY, err.to_string()).into_response(),
    }
}

pub async fn operator_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !operator_token_valid(&state, &headers).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let health = system_health_counts(&state).await;
    let servers = sqlx::query("SELECT status,count(*) AS count FROM servers GROUP BY status")
        .fetch_all(&state.db)
        .await;
    let route_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM apps WHERE public_exposure=true AND current_deployment_id IS NOT NULL",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);
    let mut server_counts = serde_json::json!({});
    if let Ok(rows) = servers {
        for row in rows {
            let status: String = row.get("status");
            server_counts[status] = serde_json::json!(row.get::<i64, _>("count"));
        }
    }
    Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "mode": state.mode.as_str(),
        "service": {
            "imageTag": std::env::var("HOSTLET_IMAGE_TAG").ok(),
            "revision": std::env::var("HOSTLET_IMAGE_REVISION")
                .ok()
                .or_else(|| option_env!("HOSTLET_BUILD_REVISION").map(str::to_string)),
            "registry": std::env::var("HOSTLET_IMAGE_REGISTRY").ok(),
        },
        "database": {
            "connected": true,
        },
        "routing": {
            "publicAppRouteCount": route_count,
        },
        "health": health,
        "servers": server_counts,
    }))
    .into_response()
}

pub async fn operator_cleanup_preview(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !operator_token_valid(&state, &headers).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match cleanup_plan(&state, Uuid::nil()).await {
        Ok(plan) => Json(plan).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "failed to build operator cleanup preview");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn operator_run_cleanup(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !operator_token_valid(&state, &headers).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    run_cleanup_inner(&state, None).await
}

async fn operator_token_valid(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(token) = headers
        .get("x-hostlet-agent-token")
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let row = sqlx::query(
        "SELECT agent_token_hash FROM servers WHERE kind='local' ORDER BY created_at ASC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await;
    let Ok(Some(row)) = row else {
        return false;
    };
    let expected: Option<String> = row.get("agent_token_hash");
    expected
        .as_deref()
        .is_some_and(|hash| verify_token(token, hash))
}

pub async fn refresh_update_check_if_stale(state: &AppState) -> anyhow::Result<()> {
    let stale = sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
        "SELECT updated_at FROM settings WHERE key='system_update_check'",
    )
    .fetch_optional(&state.db)
    .await?
    .flatten()
    .map(|updated_at| {
        chrono::Utc::now().signed_duration_since(updated_at) > chrono::Duration::hours(24)
    })
    .unwrap_or(true);
    if stale {
        let _ = refresh_update_check(state).await?;
    }
    Ok(())
}

pub async fn app_resources(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let row = sqlx::query(
        r#"
        SELECT d.container_name, s.kind,
               rs.cpu_percent, rs.memory_usage, rs.memory_percent,
               rs.network_io, rs.block_io, rs.pids, rs.sampled_at
        FROM apps a
        JOIN servers s ON s.id = a.server_id
        LEFT JOIN deployments d ON d.id = a.current_deployment_id
        LEFT JOIN app_resource_snapshots rs ON rs.container_name = d.container_name
        WHERE a.id=$1 AND a.user_id=$2
        "#,
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await;
    let Ok(Some(row)) = row else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if row.get::<String, _>("kind") != "local" {
        return (
            StatusCode::BAD_REQUEST,
            "resource usage is currently available for local apps only",
        )
            .into_response();
    }
    let Some(container) = row.get::<Option<String>, _>("container_name") else {
        return (
            StatusCode::NOT_FOUND,
            "app does not have a running container yet",
        )
            .into_response();
    };

    let sampled_at = row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("sampled_at");
    let Some(sampled_at) = sampled_at else {
        return (
            StatusCode::ACCEPTED,
            "resource usage is waiting for the local agent",
        )
            .into_response();
    };
    if chrono::Utc::now().signed_duration_since(sampled_at) > chrono::Duration::seconds(45) {
        return (
            StatusCode::ACCEPTED,
            "resource usage is waiting for a fresh local agent sample",
        )
            .into_response();
    }
    Json(serde_json::json!({
        "container": container,
        "name": container,
        "cpuPercent": row.get::<Option<String>, _>("cpu_percent").unwrap_or_else(|| "0%".into()),
        "memoryUsage": row.get::<Option<String>, _>("memory_usage").unwrap_or_else(|| "0B / 0B".into()),
        "memoryPercent": row.get::<Option<String>, _>("memory_percent").unwrap_or_else(|| "0%".into()),
        "networkIo": row.get::<Option<String>, _>("network_io").unwrap_or_else(|| "0B / 0B".into()),
        "blockIo": row.get::<Option<String>, _>("block_io").unwrap_or_else(|| "0B / 0B".into()),
        "pids": row.get::<Option<String>, _>("pids").unwrap_or_else(|| "0".into()),
        "sampledAt": sampled_at
    }))
    .into_response()
}

