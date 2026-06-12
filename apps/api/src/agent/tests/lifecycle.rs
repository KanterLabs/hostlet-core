use super::*;

/// A deploy payload carrying the two decrypted secret keys that terminal-state
/// transitions must strip from `agent_jobs.payload_json`.
fn secret_deploy_payload() -> serde_json::Value {
    serde_json::json!({
        "type": "deploy",
        "commit_sha": "HEAD",
        "env": {"K": "v"},
        "github_token": "tok"
    })
}

/// Inserts an agent job with an explicit `status` and `payload`, binding the
/// payload as jsonb. Mirrors `insert_job` but lets each lifecycle test seed the
/// exact terminal/active state and payload it needs.
async fn insert_job_with_payload(
    state: &AppState,
    app_id: Uuid,
    deployment_id: Uuid,
    job_type: &str,
    status: &str,
    payload: serde_json::Value,
) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agent_jobs
           (server_id,app_id,deployment_id,job_type,status,payload_json)
         VALUES ($1,$2,$3,$4,$5,$6)
         RETURNING id",
    )
    .bind(TEST_SERVER_ID)
    .bind(app_id)
    .bind(deployment_id)
    .bind(job_type)
    .bind(status)
    .bind(payload)
    .fetch_one(&state.db)
    .await
    .unwrap()
}

/// Inserts a deployment with an explicit `status`. The shared `insert_deployment`
/// hardcodes 'running', which would trip `idx_deployments_one_active_per_app`
/// when a retry creates a second active deployment for the same app.
async fn insert_deployment_with_status(state: &AppState, app_id: Uuid, status: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO deployments (app_id,server_id,status,commit_sha,started_at,finished_at,runtime_kind)
             VALUES ($1,$2,$3,'HEAD',now(),now(),'single')
             RETURNING id",
    )
    .bind(app_id)
    .bind(TEST_SERVER_ID)
    .bind(status)
    .fetch_one(&state.db)
    .await
    .unwrap()
}

async fn job_payload(state: &AppState, job_id: Uuid) -> serde_json::Value {
    sqlx::query_scalar("SELECT payload_json FROM agent_jobs WHERE id=$1")
        .bind(job_id)
        .fetch_one(&state.db)
        .await
        .unwrap()
}

async fn job_finished_at_is_set(state: &AppState, job_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT finished_at IS NOT NULL FROM agent_jobs WHERE id=$1")
        .bind(job_id)
        .fetch_one(&state.db)
        .await
        .unwrap()
}

async fn job_lease_is_null(state: &AppState, job_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT lease_expires_at IS NULL FROM agent_jobs WHERE id=$1")
        .bind(job_id)
        .fetch_one(&state.db)
        .await
        .unwrap()
}

/// Asserts the stored payload has neither secret key, mirroring the post-scrub
/// invariant every terminal transition must uphold.
async fn assert_payload_scrubbed(state: &AppState, job_id: Uuid) {
    let payload = job_payload(state, job_id).await;
    let object = payload.as_object().expect("payload must be a json object");
    assert!(!object.contains_key("env"), "env must be stripped");
    assert!(
        !object.contains_key("github_token"),
        "github_token must be stripped"
    );
}

/// Claims the single queued job for `TEST_SERVER_ID` and asserts the claim
/// succeeded, leaving the job in the 'claimed' state.
async fn claim_only_queued_job(state: &AppState) {
    let response = claim_job(
        State(state.clone()),
        agent_headers(state, TEST_SERVER_ID),
        Json(ClaimJobRequest {
            agent_id: Some("ci-agent".into()),
        }),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
}

/// Cookie-authenticated owner headers for driving the customer-facing handlers.
fn owner_headers(state: &AppState, user_id: Uuid) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::COOKIE,
        crate::auth::test_session_cookie_header(state, user_id)
            .parse()
            .unwrap(),
    );
    headers
}

/// The REST terminal transition strips the secret keys while preserving the rest
/// of the payload.
#[tokio::test]
async fn db_complete_job_scrubs_payload_secrets() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    let job_id = insert_job_with_payload(
        &state,
        app_id,
        deployment_id,
        "deploy",
        "queued",
        secret_deploy_payload(),
    )
    .await;

    claim_only_queued_job(&state).await;
    let headers = agent_headers(&state, TEST_SERVER_ID);
    let status = complete_job_status(
        &state,
        &headers,
        job_id,
        CompleteJobRequest {
            status: "success".into(),
            failure: None,
            result: None,
        },
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert_eq!(job_status(&state, job_id).await.as_deref(), Some("success"));
    assert_payload_scrubbed(&state, job_id).await;
    assert_eq!(
        job_payload(&state, job_id).await["commit_sha"],
        "HEAD",
        "non-secret payload keys must survive the scrub"
    );
}

/// The WS terminal branch scrubs secrets and stamps finished_at.
#[tokio::test]
async fn db_ws_terminal_job_status_scrubs_payload() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    let job_id = insert_job_with_payload(
        &state,
        app_id,
        deployment_id,
        "deploy",
        "queued",
        secret_deploy_payload(),
    )
    .await;

    claim_only_queued_job(&state).await;
    handle_agent_message(
        &state,
        TEST_SERVER_ID,
        serde_json::json!({
            "type": "job_status",
            "job_id": job_id,
            "status": "failed",
            "failure": "boom"
        }),
    )
    .await;

    assert_eq!(job_status(&state, job_id).await.as_deref(), Some("failed"));
    assert!(
        job_finished_at_is_set(&state, job_id).await,
        "finished_at must be stamped on the terminal WS transition"
    );
    assert_payload_scrubbed(&state, job_id).await;
}

/// A late/replayed 'running' WS event must not reopen a job that already reached
/// a terminal state.
#[tokio::test]
async fn db_ws_job_status_cannot_resurrect_terminal_job() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    let job_id = insert_job_with_payload(
        &state,
        app_id,
        deployment_id,
        "deploy",
        "queued",
        secret_deploy_payload(),
    )
    .await;

    claim_only_queued_job(&state).await;
    let headers = agent_headers(&state, TEST_SERVER_ID);
    let status = complete_job_status(
        &state,
        &headers,
        job_id,
        CompleteJobRequest {
            status: "success".into(),
            failure: None,
            result: None,
        },
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    handle_agent_message(
        &state,
        TEST_SERVER_ID,
        serde_json::json!({
            "type": "job_status",
            "job_id": job_id,
            "status": "running"
        }),
    )
    .await;

    assert_eq!(
        job_status(&state, job_id).await.as_deref(),
        Some("success"),
        "terminal job must not be reopened by a late 'running' event"
    );
    assert!(
        job_lease_is_null(&state, job_id).await,
        "a resurrected job would have re-acquired a lease"
    );
}

/// The recover sweep scrubs rows that reached a terminal state without an inline
/// scrub, alongside the exhaustion path that fails (and scrubs) expired jobs.
#[tokio::test]
async fn db_recover_sweep_scrubs_already_terminal_payloads() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;

    // Already-terminal 'failed' row whose payload predates the inline scrub.
    let terminal_job = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agent_jobs
           (server_id,app_id,deployment_id,job_type,status,payload_json,finished_at)
         VALUES ($1,$2,$3,'deploy','failed',$4,now())
         RETURNING id",
    )
    .bind(TEST_SERVER_ID)
    .bind(app_id)
    .bind(deployment_id)
    .bind(secret_deploy_payload())
    .fetch_one(&state.db)
    .await
    .unwrap();

    // Expired, attempts-exhausted job: the exhaustion UPDATE fails it inline.
    let expired_job = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agent_jobs
           (server_id,app_id,deployment_id,job_type,status,payload_json,attempt,max_attempts,lease_expires_at)
         VALUES ($1,$2,$3,'deploy','running',$4,3,3,now() - interval '1 minute')
         RETURNING id",
    )
    .bind(TEST_SERVER_ID)
    .bind(app_id)
    .bind(deployment_id)
    .bind(secret_deploy_payload())
    .fetch_one(&state.db)
    .await
    .unwrap();

    recover_stale_agent_jobs(&state).await.unwrap();

    assert_eq!(
        job_status(&state, expired_job).await.as_deref(),
        Some("failed"),
        "exhausted expired job must be marked failed"
    );
    assert_payload_scrubbed(&state, terminal_job).await;
    assert_payload_scrubbed(&state, expired_job).await;
}

/// Retrying a deploy job creates a FRESH deployment that re-decrypts the current
/// env vars, leaves the original job untouched, and returns 204.
#[tokio::test]
async fn db_retry_deploy_job_creates_fresh_deployment_with_fresh_secrets() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    sqlx::query("INSERT INTO app_env_vars (app_id,key,value_ciphertext) VALUES ($1,'K',$2)")
        .bind(app_id)
        .bind(state.crypto.encrypt("fresh-value").unwrap())
        .execute(&state.db)
        .await
        .unwrap();
    let old_deployment = insert_deployment_with_status(&state, app_id, "failed").await;
    let old_job = insert_job_with_payload(
        &state,
        app_id,
        old_deployment,
        "deploy",
        "failed",
        serde_json::json!({"type": "deploy", "commit_sha": "HEAD"}),
    )
    .await;

    let response = crate::web::retry_agent_job(
        State(state.clone()),
        owner_headers(&state, user_id),
        Path(old_job),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let new_deployment = sqlx::query(
        "SELECT commit_sha FROM deployments
         WHERE app_id=$1 AND status IN ('queued','running')
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(app_id)
    .fetch_optional(&state.db)
    .await
    .unwrap()
    .expect("a fresh deployment must exist");
    assert_eq!(new_deployment.get::<String, _>("commit_sha"), "HEAD");

    let new_job = sqlx::query(
        "SELECT payload_json FROM agent_jobs
         WHERE app_id=$1 AND job_type='deploy' AND status='queued'
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(app_id)
    .fetch_optional(&state.db)
    .await
    .unwrap()
    .expect("a fresh deploy job must exist");
    let new_payload: serde_json::Value = new_job.get("payload_json");
    assert_eq!(
        new_payload["env"]["K"], "fresh-value",
        "retried deploy must carry freshly decrypted env vars"
    );

    assert_eq!(
        job_status(&state, old_job).await.as_deref(),
        Some("failed"),
        "the original deploy job must stay terminal"
    );
}

/// Retrying a non-deploy job requeues the original row in place with its payload
/// intact and returns 204.
#[tokio::test]
async fn db_retry_non_deploy_job_requeues_in_place() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    let job_id = insert_job_with_payload(
        &state,
        app_id,
        deployment_id,
        "restart_container",
        "failed",
        serde_json::json!({"type": "restart_container"}),
    )
    .await;

    let response = crate::web::retry_agent_job(
        State(state.clone()),
        owner_headers(&state, user_id),
        Path(job_id),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    assert_eq!(job_status(&state, job_id).await.as_deref(), Some("queued"));
    assert_eq!(
        job_payload(&state, job_id).await["type"],
        "restart_container",
        "in-place retry must keep the payload intact"
    );
}

/// Cancelling a queued job is terminal: it strips the secret keys and returns
/// 204.
#[tokio::test]
async fn db_cancel_scrubs_payload_secrets() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    let job_id = insert_job_with_payload(
        &state,
        app_id,
        deployment_id,
        "deploy",
        "queued",
        secret_deploy_payload(),
    )
    .await;

    let response = crate::web::cancel_agent_job(
        State(state.clone()),
        owner_headers(&state, user_id),
        Path(job_id),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    assert_eq!(
        job_status(&state, job_id).await.as_deref(),
        Some("cancelled")
    );
    assert_payload_scrubbed(&state, job_id).await;
}
