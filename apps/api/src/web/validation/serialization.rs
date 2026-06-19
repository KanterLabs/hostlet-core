//! Row-to-JSON serialization thin wrappers for app and health-check responses.
//!
//! App serialization now lives in [`crate::apps::serialization`] so the Cloud
//! overlay (which replaces `crate::web` wholesale) can call core instead of
//! forking. [`app_json`] is a thin wrapper that delegates to
//! [`crate::apps::serialization::base_app_json`] with `include_server=true`,
//! preserving the existing self-hosted wire shape byte-for-byte. [`health_json`]
//! remains here as it is web-only and has no Cloud-shared counterpart.

use crate::apps::serialization::{base_app_json, req};
use uuid::Uuid;

pub(in crate::web) fn app_json(r: sqlx::postgres::PgRow) -> serde_json::Value {
    base_app_json(r, true)
}

pub(in crate::web) fn health_json(row: sqlx::postgres::PgRow) -> serde_json::Value {
    serde_json::json!({
        "appId": req::<Uuid>(&row, "id"),
        "deploymentId": req::<Option<Uuid>>(&row, "deployment_id"),
        "containerName": req::<Option<String>>(&row, "container_name"),
        "status": req::<String>(&row, "status"),
        "checkedUrl": req::<Option<String>>(&row, "checked_url"),
        "httpStatus": req::<Option<i32>>(&row, "http_status"),
        "latencyMs": req::<Option<i32>>(&row, "latency_ms"),
        "failureCount": req::<i32>(&row, "failure_count"),
        "successCount": req::<i32>(&row, "success_count"),
        "lastError": req::<Option<String>>(&row, "last_error"),
        "lastCheckedAt": req::<Option<chrono::DateTime<chrono::Utc>>>(&row, "last_checked_at"),
        "lastHealthyAt": req::<Option<chrono::DateTime<chrono::Utc>>>(&row, "last_healthy_at"),
        "updatedAt": req::<Option<chrono::DateTime<chrono::Utc>>>(&row, "updated_at"),
    })
}
