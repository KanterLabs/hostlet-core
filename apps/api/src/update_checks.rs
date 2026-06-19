//! Update-availability checks against the GitHub releases API.
//!
//! Extracted from `crate::web::system` so that the Hostlet Cloud overlay
//! (which replaces `web/` wholesale) can call these helpers directly instead
//! of forking them.

use crate::state::AppState;
use hostlet_contracts::version_is_newer;
use sqlx::Row;

/// Cached metadata about the latest available Hostlet release.
pub struct UpdateCheck {
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

pub async fn apply_update_manifest(
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

pub async fn cached_update_check(state: &AppState) -> Option<serde_json::Value> {
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

pub async fn refresh_update_check(state: &AppState) -> anyhow::Result<serde_json::Value> {
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
