use super::*;
use sqlx::PgPool;

/// Returns an `UNAUTHORIZED` response unless the request carries a valid user
/// session. Used by handlers that only require an authenticated user.
fn require_user(headers: &HeaderMap, state: &AppState) -> Result<(), Box<Response>> {
    if current_user_id(headers, state).is_some() {
        Ok(())
    } else {
        Err(Box::new(StatusCode::UNAUTHORIZED.into_response()))
    }
}

/// Returns an `UNAUTHORIZED` response unless the request carries a valid
/// operator agent token. Used by operator-only handlers.
async fn require_operator(headers: &HeaderMap, state: &AppState) -> Result<(), Box<Response>> {
    if crate::operator::operator_token_valid(state, headers).await {
        Ok(())
    } else {
        Err(Box::new(StatusCode::UNAUTHORIZED.into_response()))
    }
}

pub async fn system_version(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(response) = require_user(&headers, &state) {
        return *response;
    }
    let update = crate::update_checks::cached_update_check(&state).await;
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
    if let Err(response) = require_user(&headers, &state) {
        return *response;
    }
    if !state.update_checks_enabled {
        return (
            StatusCode::BAD_REQUEST,
            "Hostlet update checks are disabled by HOSTLET_UPDATE_CHECKS=false",
        )
            .into_response();
    }
    match crate::update_checks::refresh_update_check(&state).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => (StatusCode::BAD_GATEWAY, err.to_string()).into_response(),
    }
}

pub async fn operator_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(response) = require_operator(&headers, &state).await {
        return *response;
    }
    let health = match system_health_counts(&state).await {
        Ok(health) => health,
        Err(err) => {
            tracing::warn!(error = %err, "failed to load operator health counts");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let summary = match operator_database_summary(&state.db).await {
        Ok(summary) => summary,
        Err(err) => {
            tracing::warn!(error = %err, "failed to load operator database summary");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
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
            "publicAppRouteCount": summary.route_count,
        },
        "health": health,
        "servers": summary.server_counts,
    }))
    .into_response()
}

struct OperatorDatabaseSummary {
    route_count: i64,
    server_counts: serde_json::Value,
}

async fn operator_database_summary(db: &PgPool) -> Result<OperatorDatabaseSummary, sqlx::Error> {
    let route_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM apps WHERE public_exposure=true AND current_deployment_id IS NOT NULL",
    )
    .fetch_one(db)
    .await?;
    let servers = sqlx::query("SELECT status,count(*) AS count FROM servers GROUP BY status")
        .fetch_all(db)
        .await?;
    let mut server_counts = serde_json::json!({});
    for row in servers {
        let status: String = row.get("status");
        server_counts[status] = serde_json::json!(row.get::<i64, _>("count"));
    }
    Ok(OperatorDatabaseSummary {
        route_count,
        server_counts,
    })
}

pub async fn operator_cleanup_preview(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(response) = require_operator(&headers, &state).await {
        return *response;
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
    if let Err(response) = require_operator(&headers, &state).await {
        return *response;
    }
    run_cleanup_inner(&state, None).await
}

pub async fn backup_metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(response) = require_user(&headers, &state) {
        return *response;
    }
    let row = sqlx::query("SELECT value FROM settings WHERE key='latest_backup_metadata'")
        .fetch_optional(&state.db)
        .await;
    match row {
        Ok(Some(row)) => {
            let value = row.get::<String, _>("value");
            match serde_json::from_str::<serde_json::Value>(&value) {
                Ok(value) => Json(value).into_response(),
                Err(_) => StatusCode::NO_CONTENT.into_response(),
            }
        }
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "failed to load backup metadata");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    #[tokio::test]
    async fn operator_database_summary_reports_query_failures() {
        let pool = PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(10))
            .connect_lazy("postgres://127.0.0.1:1/hostlet")
            .unwrap();

        let result = operator_database_summary(&pool).await;

        assert!(result.is_err());
    }
}
