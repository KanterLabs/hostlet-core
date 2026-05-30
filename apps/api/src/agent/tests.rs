use super::routes::{ClaimJobRequest, CompleteJobRequest};
use super::*;

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
    let server_id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    let job_id = insert_job(&state, app_id, deployment_id).await;
    let headers = agent_headers(&state, server_id);

    let claim_response = claim_job(
        State(state.clone()),
        headers.clone(),
        Json(ClaimJobRequest {
            agent_id: Some("ci-agent".into()),
        }),
    )
    .await
    .into_response();
    assert_eq!(claim_response.status(), StatusCode::OK);
    assert_eq!(job_status(&state, job_id).await.as_deref(), Some("claimed"));

    assert_eq!(
        complete_job(
            State(state.clone()),
            headers.clone(),
            Path(job_id),
            Json(CompleteJobRequest {
                status: "bogus".into(),
                failure: None,
                result: None,
            }),
        )
        .await
        .into_response()
        .status(),
        StatusCode::BAD_REQUEST
    );

    assert_eq!(
        complete_job(
            State(state.clone()),
            headers.clone(),
            Path(job_id),
            Json(CompleteJobRequest {
                status: "success".into(),
                failure: None,
                result: Some(serde_json::json!({"ok": true})),
            }),
        )
        .await
        .into_response()
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(job_status(&state, job_id).await.as_deref(), Some("success"));

    handle_agent_message(
        &state,
        server_id,
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
        current_deployment(&state, app_id).await.as_ref(),
        Some(&deployment_id)
    );

    handle_agent_message(
        &state,
        server_id,
        serde_json::json!({
            "type": "log",
            "deployment_id": deployment_id,
            "stream": "bad-stream",
            "line": "ignored"
        }),
    )
    .await;
    handle_agent_message(
        &state,
        server_id,
        serde_json::json!({
            "type": "log",
            "deployment_id": deployment_id,
            "stream": "stdout",
            "line": "accepted"
        }),
    )
    .await;
    assert_eq!(deployment_log_count(&state, deployment_id).await, 1);

    handle_agent_message(
        &state,
        server_id,
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
        health_status(&state, app_id).await.as_deref(),
        Some("healthy")
    );
}

#[tokio::test]
async fn db_expired_agent_jobs_retry_then_fail_at_max_attempts() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let server_id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
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

    let headers = agent_headers(&state, server_id);
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

async fn reset_agent_db(state: &AppState) {
    sqlx::query(
        "TRUNCATE deployment_logs, app_health_events, app_health_snapshots, agent_jobs,
             deployments, app_env_vars, apps, users CASCADE",
    )
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
	             VALUES ($1,'00000000-0000-0000-0000-000000000001','agent-app','hostlet-ci/node-hello','main',3000,'/health','agent.example.test','single','.',true,false)
             RETURNING id",
        )
        .bind(user_id)
        .fetch_one(&state.db)
        .await
        .unwrap()
}

async fn insert_deployment(state: &AppState, app_id: Uuid) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO deployments (app_id,server_id,status,commit_sha,started_at,runtime_kind)
             VALUES ($1,'00000000-0000-0000-0000-000000000001','running','HEAD',now(),'single')
             RETURNING id",
    )
    .bind(app_id)
    .fetch_one(&state.db)
    .await
    .unwrap()
}

async fn insert_job(state: &AppState, app_id: Uuid, deployment_id: Uuid) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO agent_jobs
               (server_id,app_id,deployment_id,job_type,status,payload_json)
             VALUES ('00000000-0000-0000-0000-000000000001',$1,$2,'deploy','queued','{\"type\":\"deploy\"}'::jsonb)
             RETURNING id",
        )
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
             VALUES ('00000000-0000-0000-0000-000000000001',$1,$2,'deploy','running','{\"type\":\"deploy\"}'::jsonb,$3,$4,now() - interval '1 minute')
             RETURNING id",
        )
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

fn rand_id() -> i64 {
    let bytes = *Uuid::new_v4().as_bytes();
    i64::from_be_bytes(bytes[..8].try_into().unwrap()).abs()
}
