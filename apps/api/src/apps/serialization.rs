//! App-detail SQL and row-to-wire serialization — a self-hosted-first,
//! Cloud-independent primitive.
//!
//! This module is the single source of truth for two things:
//!
//! 1. [`APP_SELECT_BODY`] — the shared SELECT/JOIN block for the app-detail
//!    shape.  Core's own web handlers append their owner-scoped `WHERE`/`ORDER
//!    BY` in [`crate::web::apps::queries`]; the Hostlet Cloud overlay appends
//!    its tenant-scoped clause instead.
//!
//! 2. [`base_app_json`] — the canonical row-to-JSON serializer.  It accepts an
//!    `include_server` flag so the multi-tenant Cloud caller can omit the host
//!    "server" object from responses, while core's own handlers pass `true` and
//!    preserve the existing wire shape byte-for-byte.
//!
//! The module lives outside `crate::web` precisely so the Hostlet Cloud overlay
//! (which replaces `crate::web` wholesale) can call core instead of forking and
//! drifting.  Core's own web handlers delegate here via thin wrappers in
//! [`crate::web::validation::serialization`].

use sqlx::Row;
use uuid::Uuid;

/// Columns + joins shared by the list and get-app queries.
///
/// The trailing `WHERE`/`ORDER BY` is appended by each caller: core appends
/// its owner-scoped clause in [`crate::web::apps::queries`]; the Cloud overlay
/// appends its tenant-scoped clause.  This single block is the only place a
/// schema change to the app-detail shape needs to be made.
pub const APP_SELECT_BODY: &str = r#"
        SELECT
          a.id,
          a.name,
          a.repo_full_name,
          a.branch,
          a.domain,
          a.current_deployment_id,
          a.root_directory,
          a.runtime_kind,
          a.hostlet_config_path,
          a.runtime_config,
          a.packaging_strategy,
          a.install_command,
          a.build_command,
          a.start_command,
          a.container_port,
          a.health_path,
          a.memory_limit_mb,
          a.cpu_limit,
          a.public_exposure,
          a.auto_deploy,
          a.created_at,
          s.id AS server_id,
          s.name AS server_name,
          s.public_ip AS server_public_ip,
          s.kind AS server_kind,
          s.status AS server_status,
          s.last_seen_at AS server_last_seen_at,
          latest.id AS latest_deployment_id,
          latest.status AS latest_deployment_status,
          latest.commit_sha AS latest_commit_sha,
          latest.failure_summary AS latest_failure_summary,
          latest.started_at AS latest_started_at,
          latest.finished_at AS latest_finished_at,
          latest.runtime_metadata AS latest_runtime_metadata,
          current.status AS current_deployment_status,
          current.published_port AS current_published_port,
          current.finished_at AS current_deployment_finished_at,
          latest_webhook.status AS latest_webhook_status,
          latest_webhook.ignored_reason AS latest_webhook_ignored_reason,
          latest_webhook.commit_sha AS latest_webhook_commit_sha,
          latest_webhook.branch AS latest_webhook_branch,
          latest_webhook.deployment_id AS latest_webhook_deployment_id,
          latest_webhook.created_at AS latest_webhook_created_at,
          hs.status AS health_status,
          hs.http_status AS health_http_status,
          hs.latency_ms AS health_latency_ms,
          hs.failure_count AS health_failure_count,
          hs.success_count AS health_success_count,
          hs.last_error AS health_last_error,
          hs.last_checked_at AS health_last_checked_at,
          hs.last_healthy_at AS health_last_healthy_at,
          hs.updated_at AS health_updated_at,
          COALESCE((
            SELECT jsonb_agg(jsonb_build_object(
              'name', ds.service_name,
              'role', ds.role,
              'containerName', ds.container_name,
              'imageTag', ds.image_tag,
              'targetPort', ds.target_port,
              'publishedPort', ds.published_port,
              'status', ds.status,
              'healthStatus', ds.health_status,
              'lastCheckedAt', ds.last_checked_at,
              'lastHealthyAt', ds.last_healthy_at
            ) ORDER BY (ds.role <> 'web'), ds.service_name)
            FROM deployment_services ds
            WHERE ds.deployment_id = a.current_deployment_id
          ), '[]'::jsonb) AS services,
          su.used_bytes AS storage_used_bytes,
          su.image_bytes AS storage_image_bytes,
          su.container_bytes AS storage_container_bytes
        FROM apps a
        JOIN servers s ON s.id = a.server_id
        LEFT JOIN LATERAL (
          SELECT id,status,commit_sha,failure_summary,started_at,finished_at,runtime_metadata
          FROM deployments
          WHERE app_id = a.id
          ORDER BY created_at DESC
          LIMIT 1
        ) latest ON true
        LEFT JOIN deployments current ON current.id = a.current_deployment_id
        LEFT JOIN LATERAL (
          SELECT status,ignored_reason,commit_sha,branch,deployment_id,created_at
          FROM webhook_app_events
          WHERE app_id = a.id
          ORDER BY created_at DESC
          LIMIT 1
        ) latest_webhook ON true
        LEFT JOIN app_health_snapshots hs ON hs.app_id = a.id
        LEFT JOIN app_storage_usage su ON su.app_id = a.id
"#;

/// A column that is present and required: decode it, panicking on a missing or
/// undecodable column (matching the original `row.get::<T, _>(col)` behavior).
pub(crate) fn req<T>(r: &sqlx::postgres::PgRow, col: &str) -> T
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

/// Canonical app row-to-JSON serializer.
///
/// When `include_server` is `true` the response includes a `"server"` sub-object
/// `{id, name, publicIp, kind, status, lastSeenAt}` — core's own web handlers
/// always pass `true` so the existing wire shape is preserved byte-for-byte.
/// When `include_server` is `false` the `"server"` key is omitted entirely,
/// which lets the multi-tenant Cloud caller hide host infrastructure from
/// tenant responses.  Every other field is identical in both cases.
pub fn base_app_json(r: sqlx::postgres::PgRow, include_server: bool) -> serde_json::Value {
    let runtime_config: serde_json::Value = or_default(&r, "runtime_config", serde_json::json!({}));
    // `storage_used_bytes` is the managed volume total — the value the per-plan
    // quota and the over-quota deploy gate are held to. Image and container
    // bytes are the rest of the app's disk footprint, shown but never gated.
    let storage_used_bytes = opt::<i64>(&r, "storage_used_bytes").unwrap_or(0);
    let storage_image_bytes = opt::<i64>(&r, "storage_image_bytes").unwrap_or(0);
    let storage_container_bytes = opt::<i64>(&r, "storage_container_bytes").unwrap_or(0);
    let storage_total_bytes = storage_used_bytes
        .saturating_add(storage_image_bytes)
        .saturating_add(storage_container_bytes);
    let storage_limit_bytes = crate::storage::volume_storage_limit_bytes(&runtime_config);
    let mut value = serde_json::json!({
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
    });
    apply_storage_footprint(
        &mut value,
        storage_image_bytes,
        storage_container_bytes,
        storage_total_bytes,
    );
    apply_server_visibility(&mut value, include_server);
    value
}

/// Adds the disk-footprint breakdown fields onto the app value. Set after the
/// main object is built (rather than inside the `json!` macro) so the large
/// serializer stays under the macro recursion limit. `storageUsedBytes` already
/// carries the managed-volume value; these are the rest of the footprint.
fn apply_storage_footprint(
    value: &mut serde_json::Value,
    image_bytes: i64,
    container_bytes: i64,
    total_bytes: i64,
) {
    if let Some(map) = value.as_object_mut() {
        map.insert("storageImageBytes".into(), serde_json::json!(image_bytes));
        map.insert(
            "storageContainerBytes".into(),
            serde_json::json!(container_bytes),
        );
        map.insert("storageTotalBytes".into(), serde_json::json!(total_bytes));
    }
}

/// Drop the "server" sub-object when the caller hides host infra, leaving every other field untouched.
fn apply_server_visibility(value: &mut serde_json::Value, include_server: bool) {
    if !include_server {
        if let Some(map) = value.as_object_mut() {
            map.remove("server");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_app_value() -> serde_json::Value {
        serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "my-app",
            "domain": "my-app.hostlet.cloud",
            "autoDeploy": true,
            "server": {
                "id": "00000000-0000-0000-0000-000000000002",
                "name": "my-server",
                "publicIp": null,
                "kind": "remote",
                "status": "online",
                "lastSeenAt": null
            }
        })
    }

    #[test]
    fn include_server_true_keeps_server_and_does_not_mutate_payload() {
        let before = sample_app_value();
        let mut v = sample_app_value();
        apply_server_visibility(&mut v, true);
        assert_eq!(v, before);
        assert!(v.get("server").is_some());
    }

    #[test]
    fn include_server_false_removes_exactly_the_server_key() {
        let mut expected = sample_app_value();
        expected.as_object_mut().unwrap().remove("server");

        let mut v = sample_app_value();
        apply_server_visibility(&mut v, false);

        assert_eq!(v, expected);
        assert!(v.get("server").is_none());
        assert!(v.get("id").is_some());
        assert!(v.get("name").is_some());
        assert!(v.get("domain").is_some());
        assert!(v.get("autoDeploy").is_some());
    }
}
