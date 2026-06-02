//! Persistence for in-progress GitHub device-flow logins.
//!
//! A device flow's transient state is stored as a JSON blob in the `settings`
//! table under a prefixed key. Splitting this storage out of the main auth
//! module keeps the HTTP handlers free of the settings-table plumbing.

use crate::state::AppState;
use serde::{Deserialize, Serialize};
use sqlx::Row;

const DEVICE_FLOW_KEY_PREFIX: &str = "github_device_flow:";

#[derive(Serialize, Deserialize)]
pub(super) struct StoredDeviceFlow {
    pub(super) device_code: String,
    pub(super) web_origin: String,
    pub(super) expires_at: i64,
    pub(super) interval: i64,
}

pub(super) async fn store_device_flow(
    state: &AppState,
    flow_id: &str,
    flow: &StoredDeviceFlow,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO settings (key,value) VALUES ($1,$2)
         ON CONFLICT (key) DO UPDATE SET value=EXCLUDED.value, updated_at=now()",
    )
    .bind(format!("{DEVICE_FLOW_KEY_PREFIX}{flow_id}"))
    .bind(serde_json::to_string(flow)?)
    .execute(&state.db)
    .await?;
    Ok(())
}

pub(super) async fn load_device_flow(
    state: &AppState,
    flow_id: &str,
) -> anyhow::Result<Option<StoredDeviceFlow>> {
    let row = sqlx::query("SELECT value FROM settings WHERE key=$1")
        .bind(format!("{DEVICE_FLOW_KEY_PREFIX}{flow_id}"))
        .fetch_optional(&state.db)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_str(
        row.get::<String, _>("value").as_str(),
    )?))
}

pub(super) async fn delete_device_flow(state: &AppState, flow_id: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM settings WHERE key=$1")
        .bind(format!("{DEVICE_FLOW_KEY_PREFIX}{flow_id}"))
        .execute(&state.db)
        .await?;
    Ok(())
}
