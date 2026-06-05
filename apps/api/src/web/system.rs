use super::*;
use hostlet_contracts::version_is_newer;
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
    if operator_token_valid(state, headers).await {
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
    match refresh_update_check(&state).await {
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
struct UpdateCheck {
    latest_version: String,
    release_notes_url: String,
    released_at: Option<String>,
    minimum_supported_version: Option<String>,
    compose_migrations: bool,
    database_migrations: bool,
}

async fn fetch_latest_release(state: &AppState) -> anyhow::Result<UpdateCheck> {
    let value: serde_json::Value = state
        .http
        .get("https://api.github.com/repos/ShaneKanterman04/Hostlet/releases/latest")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let latest_version = value
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0")
        .trim_start_matches('v')
        .to_string();
    let release_notes_url = value
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://github.com/ShaneKanterman04/Hostlet/releases/latest")
        .to_string();
    let mut update = UpdateCheck {
        latest_version,
        release_notes_url,
        released_at: value
            .get("published_at")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        minimum_supported_version: None,
        compose_migrations: false,
        database_migrations: false,
    };
    if let Some(manifest_url) = release_manifest_url(&value) {
        apply_update_manifest(state, &mut update, &manifest_url).await?;
    }
    Ok(update)
}

/// Locates the `browser_download_url` of the `hostlet-release.json` asset in a
/// GitHub release payload, if present.
fn release_manifest_url(release: &serde_json::Value) -> Option<String> {
    let assets = release.get("assets")?.as_array()?;
    for asset in assets {
        let name = asset.get("name").and_then(|v| v.as_str());
        if name != Some("hostlet-release.json") {
            continue;
        }
        if let Some(url) = asset.get("browser_download_url").and_then(|v| v.as_str()) {
            return Some(url.to_string());
        }
    }
    None
}

async fn apply_update_manifest(
    state: &AppState,
    update: &mut UpdateCheck,
    manifest_url: &str,
) -> anyhow::Result<()> {
    let value: serde_json::Value = state
        .http
        .get(manifest_url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if let Some(version) = value.get("version").and_then(|v| v.as_str()) {
        update.latest_version = version.trim_start_matches('v').to_string();
    }
    update.released_at = value
        .get("released_at")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| update.released_at.clone());
    update.minimum_supported_version = value
        .get("minimum_supported_version")
        .and_then(|v| v.as_str())
        .map(|value| value.trim_start_matches('v').to_string());
    update.compose_migrations = value
        .get("compose_migrations")
        .and_then(|v| v.as_bool())
        .unwrap_or(update.compose_migrations);
    update.database_migrations = value
        .get("database_migrations")
        .and_then(|v| v.as_bool())
        .unwrap_or(update.database_migrations);
    if let Some(notes_url) = value.get("notes_url").and_then(|v| v.as_str()) {
        update.release_notes_url = notes_url.to_string();
    }
    Ok(())
}

async fn cached_update_check(state: &AppState) -> Option<serde_json::Value> {
    let row = sqlx::query("SELECT value,updated_at FROM settings WHERE key='system_update_check'")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()?;
    let value: String = row.get("value");
    let mut json = serde_json::from_str::<serde_json::Value>(&value).ok()?;
    if let serde_json::Value::Object(ref mut object) = json {
        object.insert(
            "checkedAt".into(),
            serde_json::json!(row.get::<chrono::DateTime<chrono::Utc>, _>("updated_at")),
        );
    }
    Some(json)
}

async fn refresh_update_check(state: &AppState) -> anyhow::Result<serde_json::Value> {
    let update = fetch_latest_release(state).await?;
    let value = serde_json::json!({
        "latestVersion": update.latest_version,
        "releaseNotesUrl": update.release_notes_url,
        "releasedAt": update.released_at,
        "minimumSupportedVersion": update.minimum_supported_version,
        "composeMigrations": update.compose_migrations,
        "databaseMigrations": update.database_migrations,
        "updateAvailable": version_is_newer(env!("CARGO_PKG_VERSION"), &update.latest_version),
        "unsupportedDirectUpdate": update.minimum_supported_version.as_ref().is_some_and(|minimum| version_is_newer(minimum, env!("CARGO_PKG_VERSION"))),
    });
    let _ = sqlx::query(
        "INSERT INTO settings (key,value,updated_at) VALUES ('system_update_check',$1,now())
         ON CONFLICT (key) DO UPDATE SET value=EXCLUDED.value, updated_at=now()",
    )
    .bind(value.to_string())
    .execute(&state.db)
    .await;
    Ok(value)
}

pub(in crate::web) fn domain_host(value: &str) -> Option<&str> {
    if let Some((host, port)) = value.rsplit_once(':') {
        if port.parse::<u16>().is_ok() {
            return Some(host);
        }
    }
    Some(value)
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
