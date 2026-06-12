use super::*;

/// Max bytes retained from a single deployment log line before truncation.
const MAX_LOG_LINE_BYTES: usize = 8 * 1024;
/// Cap on stored log lines per deployment to bound table growth.
const MAX_LOG_LINES_PER_DEPLOYMENT: i64 = 20_000;
/// Cap on retained health events per app (older rows are pruned).
const MAX_HEALTH_EVENTS_PER_APP: i64 = 500;

/// Valid TCP port range for an agent-published container port.
const PORT_RANGE: std::ops::RangeInclusive<i64> = 1..=65_535;
/// Plausible HTTP status codes reported by a health probe.
const HTTP_STATUS_RANGE: std::ops::RangeInclusive<i64> = 100..=599;
/// Upper bound on a reported probe latency, in milliseconds (5 minutes).
const LATENCY_MS_RANGE: std::ops::RangeInclusive<i64> = 0..=300_000;
/// Sanity cap on reported consecutive success/failure counters.
const HEALTH_COUNTER_RANGE: std::ops::RangeInclusive<i64> = 0..=1_000_000;
/// Sanity cap on byte counters reported by Docker resource stats (1 PiB).
const RESOURCE_BYTES_RANGE: std::ops::RangeInclusive<i64> = 0..=1_125_899_906_842_624;
/// Sanity cap on count fields reported by Docker resource stats.
const RESOURCE_COUNT_RANGE: std::ops::RangeInclusive<i64> = 0..=1_000_000;
/// Docker CPU percentage can exceed 100% on multi-core hosts; this only rejects nonsense.
const RESOURCE_PERCENT_MAX: f64 = 1_000_000.0;
/// Max characters kept from short resource-stat fields (e.g. "12.3%").
const RESOURCE_STAT_MAX_CHARS: usize = 128;
/// Max characters kept from free-form health text (checked URL, error).
const HEALTH_TEXT_MAX_CHARS: usize = 512;

/// Parse a UUID-valued field from an agent message.
fn msg_uuid(msg: &serde_json::Value, key: &str) -> Option<Uuid> {
    msg.get(key)
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok())
}

/// Read an integer field and accept it only when it falls inside `range`,
/// returning it narrowed to `i32` for binding to the DB.
fn bounded_i32(
    msg: &serde_json::Value,
    key: &str,
    range: std::ops::RangeInclusive<i64>,
) -> Option<i32> {
    msg.get(key)
        .and_then(|v| v.as_i64())
        .and_then(|v| range.contains(&v).then_some(v as i32))
}

fn bounded_i64(
    msg: &serde_json::Value,
    key: &str,
    range: std::ops::RangeInclusive<i64>,
) -> Option<i64> {
    msg.get(key)
        .and_then(|v| v.as_i64())
        .filter(|v| range.contains(v))
}

fn bounded_f64(msg: &serde_json::Value, key: &str, max: f64) -> Option<f64> {
    msg.get(key)
        .and_then(|v| v.as_f64())
        .filter(|v| v.is_finite() && *v >= 0.0 && *v <= max)
}

/// Read a string field and truncate it to at most `max_chars` characters.
fn capped_str(msg: &serde_json::Value, key: &str, max_chars: usize) -> Option<String> {
    msg.get(key)
        .and_then(|v| v.as_str())
        .map(|value| value.chars().take(max_chars).collect())
}

pub(in crate::agent) async fn handle_agent_message(
    state: &AppState,
    server_id: Uuid,
    msg: serde_json::Value,
) {
    match msg.get("type").and_then(|v| v.as_str()) {
        Some("heartbeat") => handle_heartbeat(state, server_id).await,
        Some("deployment_status") => handle_deployment_status(state, server_id, &msg).await,
        Some("log") => handle_log(state, server_id, &msg).await,
        Some("resource_stats") => handle_resource_stats(state, &msg).await,
        Some("health_status") => handle_health_status(state, server_id, &msg).await,
        Some("job_status") => handle_job_status(state, server_id, &msg).await,
        _ => {}
    }
}

async fn handle_heartbeat(state: &AppState, server_id: Uuid) {
    let _ = sqlx::query("UPDATE servers SET status='online', last_seen_at=now() WHERE id=$1")
        .bind(server_id)
        .execute(&state.db)
        .await;
}

async fn handle_deployment_status(state: &AppState, server_id: Uuid, msg: &serde_json::Value) {
    let (Some(id), Some(status)) = (
        msg_uuid(msg, "deployment_id"),
        msg.get("status").and_then(|v| v.as_str()),
    ) else {
        return;
    };
    if !valid_deployment_status(status) {
        return;
    }
    // Update the deployment in place: COALESCE keeps existing columns when the
    // agent omits a field, runtime_metadata is replaced only when supplied, and
    // finished_at is stamped once the deployment reaches a terminal status.
    // The AND status = ANY($10) guard ensures that once a deployment reaches a
    // terminal state (success/failed/rolled_back/canceled) no late or duplicate
    // agent event can overwrite it — active→active retries still go through.
    let updated = sqlx::query(
        "UPDATE deployments SET \
         status=$1, \
         image_tag=COALESCE($2,image_tag), \
         container_name=COALESCE($3,container_name), \
         published_port=COALESCE($4,published_port), \
         failure_summary=$5, \
         compose_project=COALESCE($6,compose_project), \
         runtime_metadata=CASE WHEN $7::jsonb IS NULL THEN runtime_metadata ELSE $7::jsonb END, \
         finished_at=CASE WHEN $1 IN ('success','failed','rolled_back') THEN now() ELSE finished_at END \
         WHERE id=$8 AND server_id=$9 AND status = ANY($10)",
    )
    .bind(status)
    .bind(msg.get("image_tag").and_then(|v| v.as_str()))
    .bind(msg.get("container_name").and_then(|v| v.as_str()))
    .bind(bounded_i32(msg, "published_port", PORT_RANGE))
    .bind(msg.get("failure").and_then(|v| v.as_str()))
    .bind(msg.get("compose_project").and_then(|v| v.as_str()))
    .bind(msg.get("runtime_metadata").cloned())
    .bind(id)
    .bind(server_id)
    .bind(crate::deploy::ACTIVE_DEPLOYMENT_STATUSES)
    .execute(&state.db)
    .await
    .map(|done| done.rows_affected())
    .unwrap_or(0);
    if matches!(status, "success" | "rolled_back") && updated == 1 {
        let _ = sqlx::query("UPDATE apps SET current_deployment_id=$1, domain=COALESCE($2, domain) WHERE id=(SELECT app_id FROM deployments WHERE id=$1)")
            .bind(id)
            .bind(msg.get("local_url").and_then(|v| v.as_str()))
            .execute(&state.db)
            .await;
        if let Err(err) =
            crate::screenshots::enqueue_auto_screenshot_for_deployment(state, id).await
        {
            tracing::warn!(error = %err, deployment_id = %id, "failed to enqueue automatic screenshot");
        }
        // After the current_deployment_id update (so keep lists rank the new
        // deployment as current), trigger best-effort automatic Docker cleanup.
        if status == "success" {
            if let Err(err) = crate::cleanup::auto_cleanup_for_server(state, server_id).await {
                tracing::warn!(error = %err, %server_id, "failed to enqueue automatic Docker cleanup");
            }
        }
    }
}

async fn handle_log(state: &AppState, server_id: Uuid, msg: &serde_json::Value) {
    let (Some(id), Some(line)) = (
        msg_uuid(msg, "deployment_id"),
        msg.get("line").and_then(|v| v.as_str()),
    ) else {
        return;
    };
    let stream = msg
        .get("stream")
        .and_then(|v| v.as_str())
        .unwrap_or("stdout");
    if !matches!(stream, "stdout" | "stderr" | "git" | "docker" | "caddy") {
        return;
    }
    let line = truncate_log_line(line, MAX_LOG_LINE_BYTES);
    // Insert only if the deployment belongs to this server and the per-deployment
    // log cap has not been reached; the row count tells us whether it was stored.
    let inserted = sqlx::query(
        "INSERT INTO deployment_logs (deployment_id,stream,line)
                     SELECT $1,$2,$3
                     WHERE EXISTS (SELECT 1 FROM deployments WHERE id=$1 AND server_id=$4)
                       AND (SELECT count(*) FROM deployment_logs WHERE deployment_id=$1) < $5",
    )
    .bind(id)
    .bind(stream)
    .bind(&line)
    .bind(server_id)
    .bind(MAX_LOG_LINES_PER_DEPLOYMENT)
    .execute(&state.db)
    .await
    .map(|done| done.rows_affected())
    .unwrap_or(0);
    if inserted == 0 {
        return;
    }
    let _ = state.logs.send(crate::state::LogEvent {
        deployment_id: id,
        stream: stream.into(),
        line,
    });
}

async fn handle_resource_stats(state: &AppState, msg: &serde_json::Value) {
    let Some(container) = msg.get("container").and_then(|v| v.as_str()) else {
        return;
    };
    if !valid_container_name(container) {
        return;
    }
    let value = |key: &str, default: &str| {
        msg.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or(default)
            .chars()
            .take(RESOURCE_STAT_MAX_CHARS)
            .collect::<String>()
    };
    let percent = |key: &str| bounded_f64(msg, key, RESOURCE_PERCENT_MAX);
    let bytes = |key: &str| bounded_i64(msg, key, RESOURCE_BYTES_RANGE);
    let count = |key: &str| bounded_i64(msg, key, RESOURCE_COUNT_RANGE);
    // Keep the latest sample per container (upsert keyed on container_name).
    let _ = sqlx::query(
        r#"
                INSERT INTO app_resource_snapshots
                  (container_name,cpu_percent,memory_usage,memory_percent,network_io,block_io,pids,
                   cpu_percent_value,memory_usage_bytes,memory_limit_bytes,memory_percent_value,
                   network_rx_bytes,network_tx_bytes,block_read_bytes,block_write_bytes,pids_current,
                   sampled_at)
                VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,now())
                ON CONFLICT (container_name) DO UPDATE SET
                  cpu_percent=EXCLUDED.cpu_percent,
                  memory_usage=EXCLUDED.memory_usage,
                  memory_percent=EXCLUDED.memory_percent,
                  network_io=EXCLUDED.network_io,
                  block_io=EXCLUDED.block_io,
                  pids=EXCLUDED.pids,
                  cpu_percent_value=EXCLUDED.cpu_percent_value,
                  memory_usage_bytes=EXCLUDED.memory_usage_bytes,
                  memory_limit_bytes=EXCLUDED.memory_limit_bytes,
                  memory_percent_value=EXCLUDED.memory_percent_value,
                  network_rx_bytes=EXCLUDED.network_rx_bytes,
                  network_tx_bytes=EXCLUDED.network_tx_bytes,
                  block_read_bytes=EXCLUDED.block_read_bytes,
                  block_write_bytes=EXCLUDED.block_write_bytes,
                  pids_current=EXCLUDED.pids_current,
                  sampled_at=EXCLUDED.sampled_at
                "#,
    )
    .bind(container)
    .bind(value("cpuPercent", "0%"))
    .bind(value("memoryUsage", "0B / 0B"))
    .bind(value("memoryPercent", "0%"))
    .bind(value("networkIo", "0B / 0B"))
    .bind(value("blockIo", "0B / 0B"))
    .bind(value("pids", "0"))
    .bind(percent("cpuPercentValue"))
    .bind(bytes("memoryUsageBytes"))
    .bind(bytes("memoryLimitBytes"))
    .bind(percent("memoryPercentValue"))
    .bind(bytes("networkRxBytes"))
    .bind(bytes("networkTxBytes"))
    .bind(bytes("blockReadBytes"))
    .bind(bytes("blockWriteBytes"))
    .bind(count("pidsCurrent"))
    .execute(&state.db)
    .await;
}

async fn handle_health_status(state: &AppState, server_id: Uuid, msg: &serde_json::Value) {
    let Some(app_id) = msg_uuid(msg, "app_id") else {
        return;
    };
    let Some(status) = msg.get("status").and_then(|v| v.as_str()) else {
        return;
    };
    if !valid_health_status(status) {
        return;
    }
    let deployment_id = msg_uuid(msg, "deployment_id");
    let container = msg.get("container_name").and_then(|v| v.as_str());
    if container.is_some_and(|value| !valid_container_name(value)) {
        return;
    }
    let http_status = bounded_i32(msg, "http_status", HTTP_STATUS_RANGE);
    let published_port = bounded_i32(msg, "published_port", PORT_RANGE);
    let latency_ms = bounded_i32(msg, "latency_ms", LATENCY_MS_RANGE);
    let failure_count = bounded_i32(msg, "failure_count", HEALTH_COUNTER_RANGE).unwrap_or(0);
    let success_count = bounded_i32(msg, "success_count", HEALTH_COUNTER_RANGE).unwrap_or(0);
    let checked_url = capped_str(msg, "checked_url", HEALTH_TEXT_MAX_CHARS);
    let error = capped_str(msg, "error", HEALTH_TEXT_MAX_CHARS);
    // Upsert the latest health snapshot for the app (one row per app_id), but only
    // when the app belongs to this server. last_healthy_at advances only on a
    // 'healthy' status and is otherwise preserved.
    let updated = sqlx::query(
        r#"
                INSERT INTO app_health_snapshots
                  (app_id,deployment_id,container_name,status,checked_url,http_status,latency_ms,
                   failure_count,success_count,last_error,last_checked_at,last_healthy_at,updated_at)
                SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,now(),
                       CASE WHEN $4='healthy' THEN now() ELSE NULL END,
                       now()
                WHERE EXISTS (SELECT 1 FROM apps WHERE id=$1 AND server_id=$11)
                ON CONFLICT (app_id) DO UPDATE SET
                  deployment_id=EXCLUDED.deployment_id,
                  container_name=EXCLUDED.container_name,
                  status=EXCLUDED.status,
                  checked_url=EXCLUDED.checked_url,
                  http_status=EXCLUDED.http_status,
                  latency_ms=EXCLUDED.latency_ms,
                  failure_count=EXCLUDED.failure_count,
                  success_count=EXCLUDED.success_count,
                  last_error=EXCLUDED.last_error,
                  last_checked_at=EXCLUDED.last_checked_at,
                  last_healthy_at=CASE
                    WHEN EXCLUDED.status='healthy' THEN EXCLUDED.last_checked_at
                    ELSE app_health_snapshots.last_healthy_at
                  END,
                  updated_at=EXCLUDED.updated_at
                "#,
    )
    .bind(app_id)
    .bind(deployment_id)
    .bind(container)
    .bind(status)
    .bind(checked_url.as_deref())
    .bind(http_status)
    .bind(latency_ms)
    .bind(failure_count)
    .bind(success_count)
    .bind(error.as_deref())
    .bind(server_id)
    .execute(&state.db)
    .await
    .map(|done| done.rows_affected())
    .unwrap_or(0);
    if updated == 0 {
        return;
    }
    if let (Some(deployment_id), Some(published_port)) = (deployment_id, published_port) {
        let _ = sqlx::query(
            r#"
                UPDATE deployments d
                SET published_port=$1
                FROM apps a
                WHERE d.id=$2
                  AND d.server_id=$3
                  AND d.app_id=$4
                  AND a.id=d.app_id
                  AND a.current_deployment_id=d.id
                  AND d.status IN ('success','rolled_back')
                "#,
        )
        .bind(published_port)
        .bind(deployment_id)
        .bind(server_id)
        .bind(app_id)
        .execute(&state.db)
        .await;
    }
    // Append an immutable history event, then trim the per-app event log.
    let _ = sqlx::query(
        r#"
                INSERT INTO app_health_events
                  (app_id,deployment_id,container_name,status,checked_url,http_status,latency_ms,error)
                VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
                "#,
    )
    .bind(app_id)
    .bind(deployment_id)
    .bind(container)
    .bind(status)
    .bind(checked_url.as_deref())
    .bind(http_status)
    .bind(latency_ms)
    .bind(error.as_deref())
    .execute(&state.db)
    .await;
    prune_health_events(state, app_id).await;
}

async fn handle_job_status(state: &AppState, server_id: Uuid, msg: &serde_json::Value) {
    let Some(job_id) = msg_uuid(msg, "job_id") else {
        return;
    };
    let Some(status) = msg.get("status").and_then(|v| v.as_str()) else {
        return;
    };
    if !valid_agent_job_status(status) {
        return;
    }
    // Refresh the lease on active statuses, clear it on terminal ones, and stamp
    // finished_at when the job ends.
    let _ = sqlx::query(
        "UPDATE agent_jobs
                 SET status=$1,
                     failure_summary=$2,
                     updated_at=now(),
                     lease_expires_at=CASE
                       WHEN $1 IN ('claimed','running') THEN now() + interval '5 minutes'
                       WHEN $1 IN ('success','failed') THEN NULL
                       ELSE lease_expires_at
                     END,
                     finished_at=CASE WHEN $1 IN ('success','failed') THEN now() ELSE finished_at END
                 WHERE id=$3 AND server_id=$4",
    )
    .bind(status)
    .bind(msg.get("failure").and_then(|v| v.as_str()))
    .bind(job_id)
    .bind(server_id)
    .execute(&state.db)
    .await;
}

pub(in crate::agent) async fn prune_health_events(state: &AppState, app_id: Uuid) {
    let _ = sqlx::query(
        "DELETE FROM app_health_events
         WHERE app_id=$1
           AND created_at < now() - interval '7 days'",
    )
    .bind(app_id)
    .execute(&state.db)
    .await;
    let _ = sqlx::query(
        "DELETE FROM app_health_events
         WHERE id IN (
           SELECT id
           FROM app_health_events
           WHERE app_id=$1
           ORDER BY created_at DESC
           OFFSET $2
         )",
    )
    .bind(app_id)
    .bind(MAX_HEALTH_EVENTS_PER_APP)
    .execute(&state.db)
    .await;
}
