use super::routes::{ClaimJobRequest, CompleteJobRequest};
use super::*;

/// Deterministic server id shared by every fixture insert in this module.
/// `00000000-0000-0000-0000-000000000001`.
const TEST_SERVER_ID: Uuid = Uuid::from_u128(1);

#[test]
fn stale_agent_connection_does_not_match_current_connection() {
    let (sender, _receiver) = mpsc::channel(1);
    let current = Uuid::new_v4();
    let stale = Uuid::new_v4();
    let connection = AgentConnection {
        connection_id: current,
        sender,
    };

    assert!(connection_is_current(&connection, current));
    assert!(!connection_is_current(&connection, stale));
}

#[test]
fn runtime_health_statuses_are_explicit() {
    for status in ["unknown", "healthy", "degraded", "unhealthy"] {
        assert!(valid_health_status(status));
    }
    for status in ["success", "failed", "offline", "warning", ""] {
        assert!(!valid_health_status(status));
    }
}

#[test]
fn container_names_are_limited_to_managed_hostlet_names() {
    assert!(valid_container_name("hostlet-app-123"));
    assert!(valid_container_name("hostlet-app_123.local"));
    assert!(!valid_container_name("other-app-123"));
    assert!(!valid_container_name("hostlet-app/../../bad"));
    assert!(!valid_container_name(&format!(
        "hostlet-{}",
        "a".repeat(140)
    )));
}

#[test]
fn log_line_truncation_preserves_utf8_boundaries() {
    let line = "ok-".to_string() + &"é".repeat(20);
    let truncated = truncate_log_line(&line, 8);
    assert!(truncated.ends_with("...[truncated]"));
    assert!(truncated.is_char_boundary(truncated.len()));
}

#[tokio::test]
async fn db_agent_jobs_claim_complete_and_ingest_events() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    let failed_deployment_id = insert_deployment(&state, app_id).await;
    let job_id = insert_job(&state, app_id, deployment_id).await;
    let headers = agent_headers(&state, TEST_SERVER_ID);

    // Each phase asserts one behavior so a failure points at the step that broke.
    assert_claim_marks_job_claimed(&state, &headers, job_id).await;
    assert_complete_rejects_unknown_status(&state, &headers, job_id).await;
    assert_complete_success_marks_job_succeeded(&state, &headers, job_id).await;
    assert_failed_deployment_status_records_runtime_metadata(&state, failed_deployment_id).await;
    assert_deployment_status_becomes_current(&state, app_id, deployment_id).await;
    assert_only_valid_log_streams_are_stored(&state, deployment_id).await;
    assert_resource_stats_record_numeric_metrics(&state, user_id, app_id).await;
    assert_resource_stats_reject_invalid_numeric_metrics(&state, app_id).await;
    assert_health_status_is_recorded(&state, app_id, deployment_id).await;
}

async fn assert_claim_marks_job_claimed(state: &AppState, headers: &HeaderMap, job_id: Uuid) {
    let response = claim_job(
        State(state.clone()),
        headers.clone(),
        Json(ClaimJobRequest {
            agent_id: Some("ci-agent".into()),
        }),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(job_status(state, job_id).await.as_deref(), Some("claimed"));
}

async fn assert_complete_rejects_unknown_status(
    state: &AppState,
    headers: &HeaderMap,
    job_id: Uuid,
) {
    let status = complete_job_status(
        state,
        headers,
        job_id,
        CompleteJobRequest {
            status: "bogus".into(),
            failure: None,
            result: None,
        },
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

async fn assert_complete_success_marks_job_succeeded(
    state: &AppState,
    headers: &HeaderMap,
    job_id: Uuid,
) {
    let status = complete_job_status(
        state,
        headers,
        job_id,
        CompleteJobRequest {
            status: "success".into(),
            failure: None,
            result: Some(serde_json::json!({"ok": true})),
        },
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(job_status(state, job_id).await.as_deref(), Some("success"));
}

async fn assert_deployment_status_becomes_current(
    state: &AppState,
    app_id: Uuid,
    deployment_id: Uuid,
) {
    handle_agent_message(
        state,
        TEST_SERVER_ID,
        serde_json::json!({
            "type": "deployment_status",
            "deployment_id": deployment_id,
            "status": "success",
            "container_name": format!("hostlet-app-{app_id}"),
            "published_port": 32002
        }),
    )
    .await;
    assert_eq!(
        current_deployment(state, app_id).await.as_ref(),
        Some(&deployment_id)
    );
}

async fn assert_failed_deployment_status_records_runtime_metadata(
    state: &AppState,
    deployment_id: Uuid,
) {
    handle_agent_message(
        state,
        TEST_SERVER_ID,
        serde_json::json!({
            "type": "deployment_status",
            "deployment_id": deployment_id,
            "status": "failed",
            "failure": "Health check failed",
            "runtime_metadata": {
                "gitSyncDurationMs": 350,
                "containerStartDurationMs": 125,
                "healthCheckDurationMs": 4_000,
                "bootDurationMs": 4_125
            }
        }),
    )
    .await;
    let metadata = deployment_runtime_metadata(state, deployment_id).await;
    assert_eq!(metadata["gitSyncDurationMs"], 350);
    assert_eq!(metadata["containerStartDurationMs"], 125);
    assert_eq!(metadata["healthCheckDurationMs"], 4_000);
    assert_eq!(metadata["bootDurationMs"], 4_125);
}

async fn assert_only_valid_log_streams_are_stored(state: &AppState, deployment_id: Uuid) {
    handle_agent_message(
        state,
        TEST_SERVER_ID,
        serde_json::json!({
            "type": "log",
            "deployment_id": deployment_id,
            "stream": "bad-stream",
            "line": "ignored"
        }),
    )
    .await;
    handle_agent_message(
        state,
        TEST_SERVER_ID,
        serde_json::json!({
            "type": "log",
            "deployment_id": deployment_id,
            "stream": "stdout",
            "line": "accepted"
        }),
    )
    .await;
    assert_eq!(deployment_log_count(state, deployment_id).await, 1);
}

async fn assert_resource_stats_record_numeric_metrics(
    state: &AppState,
    user_id: Uuid,
    app_id: Uuid,
) {
    let container = format!("hostlet-app-{app_id}");
    handle_agent_message(
        state,
        TEST_SERVER_ID,
        serde_json::json!({
            "type": "resource_stats",
            "container": container,
            "cpuPercent": "12.5%",
            "cpuPercentValue": 12.5,
            "memoryUsage": "12.5MiB / 1GiB",
            "memoryUsageBytes": 13_107_200,
            "memoryLimitBytes": 1_073_741_824,
            "memoryPercent": "1.22%",
            "memoryPercentValue": 1.22,
            "networkIo": "1.2kB / 0B",
            "networkRxBytes": 1_200,
            "networkTxBytes": 0,
            "blockIo": "4.0MB / 1.0MB",
            "blockReadBytes": 4_000_000,
            "blockWriteBytes": 1_000_000,
            "pids": "7",
            "pidsCurrent": 7
        }),
    )
    .await;
    let row = sqlx::query(
        "SELECT cpu_percent_value,memory_usage_bytes,memory_limit_bytes,memory_percent_value,
                network_rx_bytes,network_tx_bytes,block_read_bytes,block_write_bytes,pids_current
           FROM app_resource_snapshots WHERE container_name=$1",
    )
    .bind(&container)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(row.get::<Option<f64>, _>("cpu_percent_value"), Some(12.5));
    assert_eq!(
        row.get::<Option<i64>, _>("memory_usage_bytes"),
        Some(13_107_200)
    );
    assert_eq!(
        row.get::<Option<i64>, _>("memory_limit_bytes"),
        Some(1_073_741_824)
    );
    assert_eq!(
        row.get::<Option<f64>, _>("memory_percent_value"),
        Some(1.22)
    );
    assert_eq!(row.get::<Option<i64>, _>("network_rx_bytes"), Some(1_200));
    assert_eq!(row.get::<Option<i64>, _>("network_tx_bytes"), Some(0));
    assert_eq!(
        row.get::<Option<i64>, _>("block_read_bytes"),
        Some(4_000_000)
    );
    assert_eq!(
        row.get::<Option<i64>, _>("block_write_bytes"),
        Some(1_000_000)
    );
    assert_eq!(row.get::<Option<i64>, _>("pids_current"), Some(7));

    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::COOKIE,
        crate::auth::test_session_cookie_header(state, user_id)
            .parse()
            .unwrap(),
    );
    let response = crate::web::app_resources(State(state.clone()), headers, Path(app_id))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["container"], container);
    assert_eq!(payload["cpuPercentValue"], 12.5);
    assert_eq!(payload["memoryUsageBytes"], 13_107_200);
    assert_eq!(payload["memoryLimitBytes"], 1_073_741_824);
    assert_eq!(payload["memoryPercentValue"], 1.22);
    assert_eq!(payload["networkRxBytes"], 1_200);
    assert_eq!(payload["networkTxBytes"], 0);
    assert_eq!(payload["blockReadBytes"], 4_000_000);
    assert_eq!(payload["blockWriteBytes"], 1_000_000);
    assert_eq!(payload["pidsCurrent"], 7);
}

async fn assert_resource_stats_reject_invalid_numeric_metrics(state: &AppState, app_id: Uuid) {
    let container = format!("hostlet-app-{app_id}");
    handle_agent_message(
        state,
        TEST_SERVER_ID,
        serde_json::json!({
            "type": "resource_stats",
            "container": container,
            "cpuPercent": "-1%",
            "cpuPercentValue": -1.0,
            "memoryUsage": "-1B / 1PiB",
            "memoryUsageBytes": -1,
            "memoryLimitBytes": 1_125_899_906_842_625i64,
            "memoryPercent": "1000000.1%",
            "memoryPercentValue": 1_000_000.1,
            "networkIo": "-1B / 1PiB",
            "networkRxBytes": -1,
            "networkTxBytes": 1_125_899_906_842_625i64,
            "blockIo": "-1B / 1PiB",
            "blockReadBytes": -1,
            "blockWriteBytes": 1_125_899_906_842_625i64,
            "pids": "-1",
            "pidsCurrent": -1
        }),
    )
    .await;
    let row = sqlx::query(
        "SELECT cpu_percent_value,memory_usage_bytes,memory_limit_bytes,memory_percent_value,
                network_rx_bytes,network_tx_bytes,block_read_bytes,block_write_bytes,pids_current
           FROM app_resource_snapshots WHERE container_name=$1",
    )
    .bind(&container)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(row.get::<Option<f64>, _>("cpu_percent_value"), None);
    assert_eq!(row.get::<Option<i64>, _>("memory_usage_bytes"), None);
    assert_eq!(row.get::<Option<i64>, _>("memory_limit_bytes"), None);
    assert_eq!(row.get::<Option<f64>, _>("memory_percent_value"), None);
    assert_eq!(row.get::<Option<i64>, _>("network_rx_bytes"), None);
    assert_eq!(row.get::<Option<i64>, _>("network_tx_bytes"), None);
    assert_eq!(row.get::<Option<i64>, _>("block_read_bytes"), None);
    assert_eq!(row.get::<Option<i64>, _>("block_write_bytes"), None);
    assert_eq!(row.get::<Option<i64>, _>("pids_current"), None);
}

async fn assert_health_status_is_recorded(state: &AppState, app_id: Uuid, deployment_id: Uuid) {
    handle_agent_message(
        state,
        TEST_SERVER_ID,
        serde_json::json!({
            "type": "health_status",
            "app_id": app_id,
            "deployment_id": deployment_id,
            "container_name": format!("hostlet-app-{app_id}"),
            "status": "healthy",
            "http_status": 200,
            "latency_ms": 12
        }),
    )
    .await;
    assert_eq!(
        health_status(state, app_id).await.as_deref(),
        Some("healthy")
    );
}

#[tokio::test]
async fn db_expired_agent_jobs_retry_then_fail_at_max_attempts() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    let retry_job = insert_expired_job(&state, app_id, deployment_id, 1, 3).await;
    let fail_job = insert_expired_job(&state, app_id, deployment_id, 3, 3).await;

    assert_eq!(recover_stale_agent_jobs(&state).await.unwrap(), 2);
    assert_eq!(
        job_status(&state, retry_job).await.as_deref(),
        Some("queued")
    );
    assert_eq!(
        job_status(&state, fail_job).await.as_deref(),
        Some("failed")
    );

    let headers = agent_headers(&state, TEST_SERVER_ID);
    assert_eq!(
        claim_job(
            State(state.clone()),
            headers,
            Json(ClaimJobRequest {
                agent_id: Some("ci-agent".into()),
            }),
        )
        .await
        .into_response()
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        job_status(&state, retry_job).await.as_deref(),
        Some("claimed")
    );
}

/// Drives `complete_job` and returns just the response status, hiding the
/// `State`/`headers` clone and `into_response` ceremony the callers share.
async fn complete_job_status(
    state: &AppState,
    headers: &HeaderMap,
    job_id: Uuid,
    request: CompleteJobRequest,
) -> StatusCode {
    complete_job(
        State(state.clone()),
        headers.clone(),
        Path(job_id),
        Json(request),
    )
    .await
    .into_response()
    .status()
}

async fn reset_agent_db(state: &AppState) {
    // Truncate app-derived tables first.  Do NOT include `users` in the
    // TRUNCATE: `servers.user_id REFERENCES users`, so `TRUNCATE users CASCADE`
    // would cascade into `servers` and destroy the seeded TEST_SERVER_ID row
    // that `seed_local_server` inserts with `user_id = NULL`.  A plain
    // `DELETE FROM users` is safe because NULL FK values are never matched by
    // the referential-integrity check.
    sqlx::query(
        "TRUNCATE deployment_logs, app_health_events, app_health_snapshots, \
             app_resource_snapshots, agent_jobs, deployments, app_env_vars, apps CASCADE",
    )
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query("DELETE FROM users")
        .execute(&state.db)
        .await
        .unwrap();
}

async fn insert_user(state: &AppState) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO users (github_id, login) VALUES ($1,'agent-user') RETURNING id",
    )
    .bind(rand_id())
    .fetch_one(&state.db)
    .await
    .unwrap()
}

async fn insert_app(state: &AppState, user_id: Uuid) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO apps
               (user_id,server_id,name,repo_full_name,branch,container_port,health_path,domain,runtime_kind,root_directory,public_exposure,auto_deploy)
	             VALUES ($1,$2,'agent-app','hostlet-ci/node-hello','main',3000,'/health','agent.example.test','single','.',true,false)
             RETURNING id",
        )
        .bind(user_id)
        .bind(TEST_SERVER_ID)
        .fetch_one(&state.db)
        .await
        .unwrap()
}

async fn insert_deployment(state: &AppState, app_id: Uuid) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO deployments (app_id,server_id,status,commit_sha,started_at,runtime_kind)
             VALUES ($1,$2,'running','HEAD',now(),'single')
             RETURNING id",
    )
    .bind(app_id)
    .bind(TEST_SERVER_ID)
    .fetch_one(&state.db)
    .await
    .unwrap()
}

async fn insert_job(state: &AppState, app_id: Uuid, deployment_id: Uuid) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agent_jobs
               (server_id,app_id,deployment_id,job_type,status,payload_json)
             VALUES ($1,$2,$3,'deploy','queued','{\"type\":\"deploy\"}'::jsonb)
             RETURNING id",
    )
    .bind(TEST_SERVER_ID)
    .bind(app_id)
    .bind(deployment_id)
    .fetch_one(&state.db)
    .await
    .unwrap()
}

async fn insert_expired_job(
    state: &AppState,
    app_id: Uuid,
    deployment_id: Uuid,
    attempt: i32,
    max_attempts: i32,
) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO agent_jobs
               (server_id,app_id,deployment_id,job_type,status,payload_json,attempt,max_attempts,lease_expires_at)
             VALUES ($1,$2,$3,'deploy','running','{\"type\":\"deploy\"}'::jsonb,$4,$5,now() - interval '1 minute')
             RETURNING id",
        )
        .bind(TEST_SERVER_ID)
        .bind(app_id)
        .bind(deployment_id)
        .bind(attempt)
        .bind(max_attempts)
        .fetch_one(&state.db)
        .await
        .unwrap()
}

fn agent_headers(_state: &AppState, server_id: Uuid) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-hostlet-server-id",
        server_id.to_string().parse().unwrap(),
    );
    headers.insert(
        "x-hostlet-agent-token",
        std::env::var("LOCAL_AGENT_TOKEN")
            .unwrap_or_else(|_| "ci-only-not-a-secret-agent-token-01".into())
            .parse()
            .unwrap(),
    );
    headers
}

async fn job_status(state: &AppState, job_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM agent_jobs WHERE id=$1")
        .bind(job_id)
        .fetch_optional(&state.db)
        .await
        .unwrap()
}

async fn current_deployment(state: &AppState, app_id: Uuid) -> Option<Uuid> {
    sqlx::query_scalar("SELECT current_deployment_id FROM apps WHERE id=$1")
        .bind(app_id)
        .fetch_optional(&state.db)
        .await
        .unwrap()
        .flatten()
}

async fn deployment_log_count(state: &AppState, deployment_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM deployment_logs WHERE deployment_id=$1")
        .bind(deployment_id)
        .fetch_one(&state.db)
        .await
        .unwrap()
}

async fn health_status(state: &AppState, app_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM app_health_snapshots WHERE app_id=$1")
        .bind(app_id)
        .fetch_optional(&state.db)
        .await
        .unwrap()
}

async fn deployment_runtime_metadata(state: &AppState, deployment_id: Uuid) -> serde_json::Value {
    sqlx::query_scalar("SELECT runtime_metadata FROM deployments WHERE id=$1")
        .bind(deployment_id)
        .fetch_one(&state.db)
        .await
        .unwrap()
}

fn rand_id() -> i64 {
    let bytes = *Uuid::new_v4().as_bytes();
    // Mask off the sign bit so the result is always non-negative without the
    // `i64::MIN` overflow that `.abs()` would panic on.
    (u64::from_be_bytes(bytes[..8].try_into().unwrap()) >> 1) as i64
}

// --- FIX 1-3 DB-gated tests ---

/// Inserts a second app owned by `user_id` with a distinct domain so it can
/// coexist with the first `insert_app` row in the same transaction.
async fn insert_app_2(state: &AppState, user_id: Uuid) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO apps
           (user_id,server_id,name,repo_full_name,branch,container_port,health_path,domain,runtime_kind,root_directory,public_exposure,auto_deploy)
         VALUES ($1,$2,'agent-app-2','hostlet-ci/node-hello','main',3001,'/health','agent2.example.test','single','.',true,false)
         RETURNING id",
    )
    .bind(user_id)
    .bind(TEST_SERVER_ID)
    .fetch_one(&state.db)
    .await
    .unwrap()
}

async fn deployment_status_by_id(state: &AppState, deployment_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM deployments WHERE id=$1")
        .bind(deployment_id)
        .fetch_optional(&state.db)
        .await
        .unwrap()
}

async fn deployment_failure_summary(state: &AppState, deployment_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT failure_summary FROM deployments WHERE id=$1")
        .bind(deployment_id)
        .fetch_optional(&state.db)
        .await
        .unwrap()
        .flatten()
}

async fn deployment_finished_at_is_set(state: &AppState, deployment_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT finished_at IS NOT NULL FROM deployments WHERE id=$1")
        .bind(deployment_id)
        .fetch_optional(&state.db)
        .await
        .unwrap()
        .unwrap_or(false)
}

/// FIX 1: `fail_deployment_row` transitions a 'queued' row to 'failed' and
/// unblocks `ensure_no_active_deployment` for the same app.
#[tokio::test]
async fn db_fail_deployment_row_unblocks_ensure_no_active_deployment() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;

    // insert_deployment creates a 'running' row; back it down to 'queued' so
    // we test the exact pre-fail_deployment_row state.
    let deployment_id = insert_deployment(&state, app_id).await;
    sqlx::query("UPDATE deployments SET status='queued' WHERE id=$1")
        .bind(deployment_id)
        .execute(&state.db)
        .await
        .unwrap();

    crate::deploy::fail_deployment_row(&state, deployment_id, "test startup failure").await;

    assert_eq!(
        deployment_status_by_id(&state, deployment_id)
            .await
            .as_deref(),
        Some("failed"),
        "deployment should be 'failed'"
    );
    assert_eq!(
        deployment_failure_summary(&state, deployment_id)
            .await
            .as_deref(),
        Some("test startup failure"),
        "failure_summary should be set"
    );
    assert!(
        deployment_finished_at_is_set(&state, deployment_id).await,
        "finished_at should be stamped"
    );
    // The row is now terminal; ensure_no_active_deployment must succeed.
    crate::deploy::ensure_no_active_deployment(&state, app_id)
        .await
        .expect("ensure_no_active_deployment should succeed after fail_deployment_row");
}

/// FIX 2: `mark_deployment_running` is monotonic — it advances 'queued' to
/// 'running' but does NOT downgrade a row that is already further along.
#[tokio::test]
async fn db_mark_deployment_running_is_monotonic() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app1 = insert_app(&state, user_id).await;
    let app2 = insert_app_2(&state, user_id).await;

    // 'building' should be left unchanged.
    let d_building = insert_deployment(&state, app1).await;
    sqlx::query("UPDATE deployments SET status='building' WHERE id=$1")
        .bind(d_building)
        .execute(&state.db)
        .await
        .unwrap();

    // 'queued' should be promoted to 'running'.
    let d_queued = insert_deployment(&state, app2).await;
    sqlx::query("UPDATE deployments SET status='queued' WHERE id=$1")
        .bind(d_queued)
        .execute(&state.db)
        .await
        .unwrap();

    crate::deploy::mark_deployment_running(&state, d_building).await;
    crate::deploy::mark_deployment_running(&state, d_queued).await;

    assert_eq!(
        deployment_status_by_id(&state, d_building).await.as_deref(),
        Some("building"),
        "mark_deployment_running must not downgrade 'building'"
    );
    assert_eq!(
        deployment_status_by_id(&state, d_queued).await.as_deref(),
        Some("running"),
        "mark_deployment_running should promote 'queued' to 'running'"
    );
}

/// FIX 3: once a deployment reaches a terminal status, later agent events
/// (success, building, etc.) must not change it.
#[tokio::test]
async fn db_terminal_deployment_ignores_late_agent_events() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;

    // Post 'failed' — row starts at 'running' which is active, so this fires.
    handle_agent_message(
        &state,
        TEST_SERVER_ID,
        serde_json::json!({
            "type": "deployment_status",
            "deployment_id": deployment_id,
            "status": "failed",
            "failure": "health check timed out"
        }),
    )
    .await;
    assert_eq!(
        deployment_status_by_id(&state, deployment_id)
            .await
            .as_deref(),
        Some("failed"),
        "deployment should be 'failed'"
    );

    // A late 'success' event must be rejected because the row is now terminal.
    handle_agent_message(
        &state,
        TEST_SERVER_ID,
        serde_json::json!({
            "type": "deployment_status",
            "deployment_id": deployment_id,
            "status": "success",
            "container_name": format!("hostlet-app-{app_id}"),
            "published_port": 32100
        }),
    )
    .await;
    assert_eq!(
        deployment_status_by_id(&state, deployment_id)
            .await
            .as_deref(),
        Some("failed"),
        "terminal status must not be overwritten by late 'success'"
    );

    // Likewise a late 'building' event.
    handle_agent_message(
        &state,
        TEST_SERVER_ID,
        serde_json::json!({
            "type": "deployment_status",
            "deployment_id": deployment_id,
            "status": "building"
        }),
    )
    .await;
    assert_eq!(
        deployment_status_by_id(&state, deployment_id)
            .await
            .as_deref(),
        Some("failed"),
        "terminal status must not be overwritten by late 'building'"
    );

    // current_deployment_id must remain NULL — success was never applied.
    assert_eq!(
        current_deployment(&state, app_id).await,
        None,
        "current_deployment_id must not be set when success was blocked"
    );
}
