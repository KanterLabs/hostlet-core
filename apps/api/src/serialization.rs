//! Row → JSON serializers for health, resource, and audit events.
//!
//! Extracted from `crate::web::health` and `crate::web::audit` so that the
//! Hostlet Cloud overlay (which replaces `web/` wholesale) can call these
//! helpers directly instead of forking them.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use sqlx::Row;
use uuid::Uuid;

pub fn health_counts_json(rows: Vec<sqlx::postgres::PgRow>) -> serde_json::Value {
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

#[allow(clippy::result_large_err)]
pub fn resource_container(row: &sqlx::postgres::PgRow) -> Result<String, Response> {
    if row.get::<String, _>("kind") != "local" {
        return Err((
            StatusCode::BAD_REQUEST,
            "resource usage is currently available for local apps only",
        )
            .into_response());
    }
    row.get::<Option<String>, _>("container_name")
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "app does not have a running container yet",
            )
                .into_response()
        })
}

pub fn resource_snapshot_json(
    row: &sqlx::postgres::PgRow,
    container: &str,
    sampled_at: chrono::DateTime<chrono::Utc>,
) -> serde_json::Value {
    serde_json::json!({
        "container": container,
        "name": container,
        "cpuPercent": row.get::<Option<String>, _>("cpu_percent").unwrap_or_else(|| "0%".into()),
        "memoryUsage": row.get::<Option<String>, _>("memory_usage").unwrap_or_else(|| "0B / 0B".into()),
        "memoryUsageBytes": row.get::<Option<i64>, _>("memory_usage_bytes"),
        "memoryLimitBytes": row.get::<Option<i64>, _>("memory_limit_bytes"),
        "memoryPercent": row.get::<Option<String>, _>("memory_percent").unwrap_or_else(|| "0%".into()),
        "memoryPercentValue": row.get::<Option<f64>, _>("memory_percent_value"),
        "networkIo": row.get::<Option<String>, _>("network_io").unwrap_or_else(|| "0B / 0B".into()),
        "networkRxBytes": row.get::<Option<i64>, _>("network_rx_bytes"),
        "networkTxBytes": row.get::<Option<i64>, _>("network_tx_bytes"),
        "blockIo": row.get::<Option<String>, _>("block_io").unwrap_or_else(|| "0B / 0B".into()),
        "blockReadBytes": row.get::<Option<i64>, _>("block_read_bytes"),
        "blockWriteBytes": row.get::<Option<i64>, _>("block_write_bytes"),
        "pids": row.get::<Option<String>, _>("pids").unwrap_or_else(|| "0".into()),
        "pidsCurrent": row.get::<Option<i64>, _>("pids_current"),
        "cpuPercentValue": row.get::<Option<f64>, _>("cpu_percent_value"),
        "sampledAt": sampled_at
    })
}

pub fn health_event_json(row: sqlx::postgres::PgRow) -> serde_json::Value {
    serde_json::json!({
        "id": row.get::<Uuid, _>("id"),
        "deploymentId": row.get::<Option<Uuid>, _>("deployment_id"),
        "containerName": row.get::<Option<String>, _>("container_name"),
        "status": row.get::<String, _>("status"),
        "checkedUrl": row.get::<Option<String>, _>("checked_url"),
        "httpStatus": row.get::<Option<i32>, _>("http_status"),
        "latencyMs": row.get::<Option<i32>, _>("latency_ms"),
        "error": row.get::<Option<String>, _>("error"),
        "createdAt": row.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
    })
}

/// Project one `audit_events` row into the camelCase JSON shape the API returns.
pub fn audit_event_json(row: sqlx::postgres::PgRow) -> serde_json::Value {
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
}
