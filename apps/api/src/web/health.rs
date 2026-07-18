use super::*;

/// Aggregates app health snapshots into per-status counts. `{filter}` is spliced
/// in to optionally scope the count to a single user; callers that splice a
/// `WHERE` clause must also bind the matching parameters.
const HEALTH_COUNTS_QUERY: &str = r#"
        SELECT CASE
                 WHEN hs.status='healthy' AND bh.status IN ('pending','failed') THEN 'degraded'
                 ELSE COALESCE(hs.status, 'unknown')
               END AS status,
               count(*) AS count
        FROM apps a
        LEFT JOIN app_health_snapshots hs ON hs.app_id = a.id
        LEFT JOIN app_browser_health bh
          ON bh.app_id=a.id AND bh.deployment_id=a.current_deployment_id
        {filter}
        GROUP BY 1
        "#;

pub async fn health_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let rows = sqlx::query(&HEALTH_COUNTS_QUERY.replace("{filter}", "WHERE a.user_id=$1"))
        .bind(user_id)
        .fetch_all(&state.db)
        .await;
    match rows {
        Ok(rows) => Json(health_counts_json(rows)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
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
               rs.network_io, rs.block_io, rs.pids,
               rs.cpu_percent_value, rs.memory_usage_bytes, rs.memory_limit_bytes,
               rs.memory_percent_value, rs.network_rx_bytes, rs.network_tx_bytes,
               rs.block_read_bytes, rs.block_write_bytes, rs.pids_current,
               rs.sampled_at
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
    let container = match resource_container(&row) {
        Ok(container) => container,
        Err(response) => return response,
    };
    let sampled_at = match fresh_sample_time(&row) {
        Ok(sampled_at) => sampled_at,
        Err(response) => return response,
    };
    Json(resource_snapshot_json(&row, &container, sampled_at)).into_response()
}

#[allow(clippy::result_large_err)]
fn fresh_sample_time(
    row: &sqlx::postgres::PgRow,
) -> Result<chrono::DateTime<chrono::Utc>, Response> {
    let sampled_at = row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("sampled_at");
    let Some(sampled_at) = sampled_at else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "resource usage is waiting for the local agent",
        )
            .into_response());
    };
    if chrono::Utc::now().signed_duration_since(sampled_at) > chrono::Duration::seconds(45) {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "resource usage is waiting for a fresh local agent sample",
        )
            .into_response());
    }
    Ok(sampled_at)
}

pub async fn app_health(
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
        SELECT a.id,
               hs.deployment_id,
               hs.container_name,
               COALESCE(hs.status, 'unknown') AS status,
               hs.checked_url,
               hs.http_status,
               hs.latency_ms,
               COALESCE(hs.failure_count, 0) AS failure_count,
               COALESCE(hs.success_count, 0) AS success_count,
               hs.last_error,
               hs.last_checked_at,
               hs.last_healthy_at,
               hs.updated_at,
               bh.status AS browser_status,
               bh.failure AS browser_failure,
               bh.checked_at AS browser_checked_at
        FROM apps a
        LEFT JOIN app_health_snapshots hs ON hs.app_id = a.id
        LEFT JOIN app_browser_health bh
          ON bh.app_id=a.id AND bh.deployment_id=a.current_deployment_id
        WHERE a.id=$1 AND a.user_id=$2
        "#,
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await;
    match row {
        Ok(Some(row)) => Json(health_json(row)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn app_health_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let rows = sqlx::query(
        r#"
        SELECT e.id,
               e.deployment_id,
               e.container_name,
               e.status,
               e.checked_url,
               e.http_status,
               e.latency_ms,
               e.error,
               e.created_at
        FROM app_health_events e
        JOIN apps a ON a.id = e.app_id
        WHERE e.app_id=$1 AND a.user_id=$2
        ORDER BY e.created_at DESC
        LIMIT 100
        "#,
    )
    .bind(id)
    .bind(user_id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => {
            Json(rows.into_iter().map(health_event_json).collect::<Vec<_>>()).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn check_app_health_now(
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
               a.container_port,
               a.domain,
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
        "type": "health_check",
        "app_id": id,
        "deployment_id": deployment_id,
        "container_name": container_name,
        "container_port": row.get::<i32, _>("container_port"),
        "published_port": published_port,
        "health_path": row.get::<String, _>("health_path"),
        "domain": row.get::<String, _>("domain"),
        "route_key": format!("app-{id}"),
    });
    enqueue_interactive_agent_job(
        &state,
        row.get::<Uuid, _>("server_id"),
        id,
        Some(deployment_id),
        "health_check",
        payload,
    )
    .await
}

pub(in crate::web) async fn system_health_counts(
    state: &AppState,
) -> Result<serde_json::Value, sqlx::Error> {
    let rows = sqlx::query(&HEALTH_COUNTS_QUERY.replace("{filter}", ""))
        .fetch_all(&state.db)
        .await?;
    Ok(health_counts_json(rows))
}
