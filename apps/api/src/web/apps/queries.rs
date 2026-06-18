//! Shared SQL for the app read endpoints.
//!
//! `list_apps` and `get_app` return the exact same projected app shape and
//! differ only in their final `WHERE` clause, so the column list and join
//! block live here once instead of being copy-pasted into both handlers.

/// Columns + joins shared by [`APP_LIST_QUERY`] and [`APP_GET_QUERY`].
///
/// The trailing `WHERE`/`ORDER BY` is appended by each query so any schema
/// change only has to be made in this single block.
const APP_SELECT_BODY: &str = r#"
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
          su.used_bytes AS storage_used_bytes
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

/// All apps owned by `$1`, newest first.
pub(super) fn list_query() -> String {
    format!(
        "{APP_SELECT_BODY}        WHERE a.user_id=$1\n        ORDER BY a.created_at DESC\n        "
    )
}

/// A single app `$1` owned by `$2`.
pub(super) fn get_query() -> String {
    format!("{APP_SELECT_BODY}        WHERE a.id=$1 AND a.user_id=$2\n        ")
}
