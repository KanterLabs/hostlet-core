//! Row-to-JSON serialization for app and health-check responses.
//!
//! These helpers translate `sqlx` rows into the public wire shapes returned by
//! the web API. They live in their own module because they are serialization
//! concerns rather than input validation, even though both are surfaced through
//! the parent `validation` module's re-exports.

use super::*;

/// A column that is present and required: decode it, panicking on a missing or
/// undecodable column (matching the original `row.get::<T, _>(col)` behavior).
fn req<T>(r: &sqlx::postgres::PgRow, col: &str) -> T
where
    T: for<'a> sqlx::Decode<'a, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    r.get::<T, _>(col)
}

/// A column that may be absent, NULL, or fail to decode: yields `None` in every
/// such case, otherwise `Some(value)`. Serializes to JSON `null` on `None`.
fn opt<T>(r: &sqlx::postgres::PgRow, col: &str) -> Option<T>
where
    T: for<'a> sqlx::Decode<'a, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    r.try_get::<T, _>(col).ok()
}

/// A column with a fallback default used when the column is absent or fails to
/// decode (matching `try_get(col).unwrap_or(default)`).
fn or_default<T>(r: &sqlx::postgres::PgRow, col: &str, default: T) -> T
where
    T: for<'a> sqlx::Decode<'a, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    r.try_get::<T, _>(col).unwrap_or(default)
}

pub(in crate::web) fn app_json(r: sqlx::postgres::PgRow) -> serde_json::Value {
    let runtime_config: serde_json::Value = or_default(&r, "runtime_config", serde_json::json!({}));
    let storage_used_bytes = opt::<i64>(&r, "storage_used_bytes").unwrap_or(0);
    let storage_limit_bytes = crate::storage::volume_storage_limit_bytes(&runtime_config);
    serde_json::json!({
        "id": req::<Uuid>(&r, "id"),
        "name": req::<String>(&r, "name"),
        "repoFullName": req::<String>(&r, "repo_full_name"),
        "branch": req::<String>(&r, "branch"),
        "domain": req::<String>(&r, "domain"),
        "currentDeploymentId": req::<Option<Uuid>>(&r, "current_deployment_id"),
        "runtimeKind": or_default(&r, "runtime_kind", "single".to_string()),
        "services": or_default::<serde_json::Value>(&r, "services", serde_json::json!([])),
        "hostletConfigPath": or_default(&r, "hostlet_config_path", "hostlet.yml".to_string()),
        "runtimeConfig": runtime_config,
        "packagingStrategy": or_default(&r, "packaging_strategy", "auto".to_string()),
        "storageUsedBytes": storage_used_bytes,
        "storageLimitBytes": storage_limit_bytes,
        "rootDirectory": or_default(&r, "root_directory", ".".to_string()),
        "installCommand": or_default::<Option<String>>(&r, "install_command", None),
        "buildCommand": or_default::<Option<String>>(&r, "build_command", None),
        "startCommand": or_default::<Option<String>>(&r, "start_command", None),
        "containerPort": opt::<i32>(&r, "container_port"),
        "healthPath": opt::<String>(&r, "health_path"),
        "memoryLimitMb": or_default::<Option<i32>>(&r, "memory_limit_mb", None),
        "cpuLimit": or_default::<Option<f64>>(&r, "cpu_limit", None),
        "publicExposure": or_default(&r, "public_exposure", false),
        "autoDeploy": or_default(&r, "auto_deploy", false),
        "createdAt": opt::<chrono::DateTime<chrono::Utc>>(&r, "created_at"),
        "server": opt::<Uuid>(&r, "server_id").map(|id| serde_json::json!({
            "id": id,
            "name": or_default(&r, "server_name", "Server".to_string()),
            "publicIp": or_default::<Option<String>>(&r, "server_public_ip", None),
            "kind": or_default(&r, "server_kind", "remote".to_string()),
            "status": or_default(&r, "server_status", "offline".to_string()),
            "lastSeenAt": or_default::<Option<chrono::DateTime<chrono::Utc>>>(&r, "server_last_seen_at", None)
        })),
        "latestDeployment": or_default::<Option<Uuid>>(&r, "latest_deployment_id", None).map(|id| serde_json::json!({
            "id": id,
            "status": or_default::<Option<String>>(&r, "latest_deployment_status", None),
            "commitSha": or_default::<Option<String>>(&r, "latest_commit_sha", None),
            "failure": or_default::<Option<String>>(&r, "latest_failure_summary", None),
            "startedAt": or_default::<Option<chrono::DateTime<chrono::Utc>>>(&r, "latest_started_at", None),
            "finishedAt": or_default::<Option<chrono::DateTime<chrono::Utc>>>(&r, "latest_finished_at", None),
            "runtimeMetadata": or_default::<Option<serde_json::Value>>(&r, "latest_runtime_metadata", None).unwrap_or_else(|| serde_json::json!({}))
        })),
        "currentDeployment": or_default::<Option<String>>(&r, "current_deployment_status", None).map(|status| serde_json::json!({
            "status": status,
            "publishedPort": or_default::<Option<i32>>(&r, "current_published_port", None),
            "finishedAt": or_default::<Option<chrono::DateTime<chrono::Utc>>>(&r, "current_deployment_finished_at", None)
        })),
        "latestWebhook": or_default::<Option<String>>(&r, "latest_webhook_status", None).map(|status| serde_json::json!({
            "status": status,
            "ignoredReason": or_default::<Option<String>>(&r, "latest_webhook_ignored_reason", None),
            "commitSha": or_default::<Option<String>>(&r, "latest_webhook_commit_sha", None),
            "branch": or_default::<Option<String>>(&r, "latest_webhook_branch", None),
            "deploymentId": or_default::<Option<Uuid>>(&r, "latest_webhook_deployment_id", None),
            "createdAt": or_default::<Option<chrono::DateTime<chrono::Utc>>>(&r, "latest_webhook_created_at", None)
        })),
        "health": or_default::<Option<String>>(&r, "health_status", None).map(|status| serde_json::json!({
            "status": status,
            "httpStatus": or_default::<Option<i32>>(&r, "health_http_status", None),
            "latencyMs": or_default::<Option<i32>>(&r, "health_latency_ms", None),
            "failureCount": or_default::<Option<i32>>(&r, "health_failure_count", None).unwrap_or(0),
            "successCount": or_default::<Option<i32>>(&r, "health_success_count", None).unwrap_or(0),
            "lastError": or_default::<Option<String>>(&r, "health_last_error", None),
            "lastCheckedAt": or_default::<Option<chrono::DateTime<chrono::Utc>>>(&r, "health_last_checked_at", None),
            "lastHealthyAt": or_default::<Option<chrono::DateTime<chrono::Utc>>>(&r, "health_last_healthy_at", None),
            "updatedAt": or_default::<Option<chrono::DateTime<chrono::Utc>>>(&r, "health_updated_at", None)
        }))
    })
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
