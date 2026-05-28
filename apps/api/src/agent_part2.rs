async fn handle_agent_message(state: &AppState, server_id: Uuid, msg: serde_json::Value) {
    match msg.get("type").and_then(|v| v.as_str()) {
        Some("heartbeat") => {
            let _ =
                sqlx::query("UPDATE servers SET status='online', last_seen_at=now() WHERE id=$1")
                    .bind(server_id)
                    .execute(&state.db)
                    .await;
        }
        Some("deployment_status") => {
            if let (Some(id), Some(status)) = (
                msg.get("deployment_id")
                    .and_then(|v| v.as_str())
                    .and_then(|v| Uuid::parse_str(v).ok()),
                msg.get("status").and_then(|v| v.as_str()),
            ) {
                if !valid_deployment_status(status) {
                    return;
                }
                let updated = sqlx::query("UPDATE deployments SET status=$1, image_tag=COALESCE($2,image_tag), container_name=COALESCE($3,container_name), published_port=COALESCE($4,published_port), failure_summary=$5, compose_project=COALESCE($6,compose_project), runtime_metadata=CASE WHEN $7::jsonb IS NULL THEN runtime_metadata ELSE $7::jsonb END, finished_at=CASE WHEN $1 IN ('success','failed','rolled_back') THEN now() ELSE finished_at END WHERE id=$8 AND server_id=$9")
                    .bind(status)
                    .bind(msg.get("image_tag").and_then(|v| v.as_str()))
                    .bind(msg.get("container_name").and_then(|v| v.as_str()))
                    .bind(msg.get("published_port").and_then(|v| v.as_i64()).and_then(|v| {
                        (1..=65_535).contains(&v).then_some(v as i32)
                    }))
                    .bind(msg.get("failure").and_then(|v| v.as_str()))
                    .bind(msg.get("compose_project").and_then(|v| v.as_str()))
                    .bind(msg.get("runtime_metadata").cloned())
                    .bind(id)
                    .bind(server_id)
                    .execute(&state.db).await
                    .map(|done| done.rows_affected())
                    .unwrap_or(0);
                if matches!(status, "success" | "rolled_back") && updated == 1 {
                    let _ = sqlx::query("UPDATE apps SET current_deployment_id=$1, domain=COALESCE($2, domain) WHERE id=(SELECT app_id FROM deployments WHERE id=$1)")
                        .bind(id)
                        .bind(msg.get("local_url").and_then(|v| v.as_str()))
                        .execute(&state.db)
                        .await;
                }
            }
        }
        Some("log") => {
            if let (Some(id), Some(line)) = (
                msg.get("deployment_id")
                    .and_then(|v| v.as_str())
                    .and_then(|v| Uuid::parse_str(v).ok()),
                msg.get("line").and_then(|v| v.as_str()),
            ) {
                let stream = msg
                    .get("stream")
                    .and_then(|v| v.as_str())
                    .unwrap_or("stdout");
                if !matches!(stream, "stdout" | "stderr" | "git" | "docker" | "caddy") {
                    return;
                }
                let line = truncate_log_line(line, MAX_LOG_LINE_BYTES);
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
        }
        Some("resource_stats") => {
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
                    .take(128)
                    .collect::<String>()
            };
            let _ = sqlx::query(
                r#"
                INSERT INTO app_resource_snapshots
                  (container_name,cpu_percent,memory_usage,memory_percent,network_io,block_io,pids,sampled_at)
                VALUES ($1,$2,$3,$4,$5,$6,$7,now())
                ON CONFLICT (container_name) DO UPDATE SET
                  cpu_percent=EXCLUDED.cpu_percent,
                  memory_usage=EXCLUDED.memory_usage,
                  memory_percent=EXCLUDED.memory_percent,
                  network_io=EXCLUDED.network_io,
                  block_io=EXCLUDED.block_io,
                  pids=EXCLUDED.pids,
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
            .execute(&state.db)
            .await;
        }
        Some("health_status") => {
            let Some(app_id) = msg
                .get("app_id")
                .and_then(|v| v.as_str())
                .and_then(|v| Uuid::parse_str(v).ok())
            else {
                return;
            };
            let Some(status) = msg.get("status").and_then(|v| v.as_str()) else {
                return;
            };
            if !valid_health_status(status) {
                return;
            }
            let deployment_id = msg
                .get("deployment_id")
                .and_then(|v| v.as_str())
                .and_then(|v| Uuid::parse_str(v).ok());
            let container = msg.get("container_name").and_then(|v| v.as_str());
            if container.is_some_and(|value| !valid_container_name(value)) {
                return;
            }
            let http_status = msg
                .get("http_status")
                .and_then(|v| v.as_i64())
                .and_then(|v| (100..=599).contains(&v).then_some(v as i32));
            let latency_ms = msg
                .get("latency_ms")
                .and_then(|v| v.as_i64())
                .and_then(|v| (0..=300_000).contains(&v).then_some(v as i32));
            let failure_count = msg
                .get("failure_count")
                .and_then(|v| v.as_i64())
                .and_then(|v| (0..=1_000_000).contains(&v).then_some(v as i32))
                .unwrap_or(0);
            let success_count = msg
                .get("success_count")
                .and_then(|v| v.as_i64())
                .and_then(|v| (0..=1_000_000).contains(&v).then_some(v as i32))
                .unwrap_or(0);
            let checked_url = msg
                .get("checked_url")
                .and_then(|v| v.as_str())
                .map(|value| value.chars().take(512).collect::<String>());
            let error = msg
                .get("error")
                .and_then(|v| v.as_str())
                .map(|value| value.chars().take(512).collect::<String>());
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
        Some("job_status") => {
            let Some(job_id) = msg
                .get("job_id")
                .and_then(|v| v.as_str())
                .and_then(|v| Uuid::parse_str(v).ok())
            else {
                return;
            };
            let Some(status) = msg.get("status").and_then(|v| v.as_str()) else {
                return;
            };
            if !valid_agent_job_status(status) {
                return;
            }
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
        _ => {}
    }
}

async fn prune_health_events(state: &AppState, app_id: Uuid) {
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

async fn authenticated_server_id(state: &AppState, headers: &HeaderMap) -> Option<Uuid> {
    let server_id = header_uuid(headers, "x-hostlet-server-id")?;
    let token = headers
        .get("x-hostlet-agent-token")
        .and_then(|v| v.to_str().ok())?;
    let row = sqlx::query("SELECT agent_token_hash FROM servers WHERE id=$1")
        .bind(server_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()?;
    let expected: Option<String> = row.get("agent_token_hash");
    expected
        .as_deref()
        .filter(|hash| verify_token(token, hash))
        .map(|_| server_id)
}

fn valid_deployment_status(status: &str) -> bool {
    status.parse::<DeploymentStatus>().is_ok() && status != "canceled"
}

fn valid_agent_job_status(status: &str) -> bool {
    status.parse::<AgentJobStatus>().is_ok() && status != "canceled"
}

fn valid_health_status(status: &str) -> bool {
    status.parse::<RuntimeHealthStatus>().is_ok()
}

fn valid_container_name(value: &str) -> bool {
    value.starts_with("hostlet-")
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn truncate_log_line(line: &str, max_bytes: usize) -> String {
    if line.len() <= max_bytes {
        return line.to_string();
    }
    let mut end = max_bytes;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...[truncated]", &line[..end])
}

fn header_uuid(headers: &HeaderMap, key: &str) -> Option<Uuid> {
    headers
        .get(key)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
}

fn connection_is_current(connection: &AgentConnection, connection_id: Uuid) -> bool {
    connection.connection_id == connection_id
}
