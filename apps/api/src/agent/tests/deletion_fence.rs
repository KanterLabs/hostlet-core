use super::*;

async fn insert_typed_job(
    state: &AppState,
    app_id: Option<Uuid>,
    deployment_id: Option<Uuid>,
    job_type: &str,
    status: &str,
    payload: serde_json::Value,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agent_jobs
           (server_id,app_id,deployment_id,job_type,status,payload_json,
            claimed_by,claim_token,lease_expires_at)
         VALUES (
           $1,$2,$3,$4,$5,$6,
           CASE WHEN $5 IN ('claimed','running') THEN 'fixture-agent' END,
           CASE WHEN $5 IN ('claimed','running') THEN uuid_generate_v4() END,
           CASE WHEN $5 IN ('claimed','running') THEN now() + interval '5 minutes' END
         )
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

fn teardown_payload(app_id: Uuid, user_id: Uuid) -> serde_json::Value {
    serde_json::json!({
        "type": "delete_app",
        "app_id": app_id,
        "user_id": user_id,
        "domain": "agent.example.test",
        "public_exposure": false,
        "compose_project": format!("hostlet-app-{}", app_id.simple()),
        "containers": [],
        "images": [],
    })
}

async fn claim_for_agent(state: &AppState, agent_id: &str) -> Option<Uuid> {
    let response = claim_job(
        State(state.clone()),
        agent_headers(state, TEST_SERVER_ID),
        Json(ClaimJobRequest {
            agent_id: Some(agent_id.to_string()),
            protocol_version: 3,
        }),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    sqlx::query_scalar(
        "SELECT id FROM agent_jobs
         WHERE claimed_by=$1 AND status='claimed'
         ORDER BY claimed_at DESC LIMIT 1",
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await
    .unwrap()
}

#[tokio::test]
async fn db_claim_rejects_app_bound_job_for_missing_app() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    let orphan = insert_job(&state, app_id, deployment_id).await;
    sqlx::query("DELETE FROM apps WHERE id=$1")
        .bind(app_id)
        .execute(&state.db)
        .await
        .unwrap();
    let cleanup = crate::deploy::enqueue_agent_job(
        &state,
        TEST_SERVER_ID,
        None,
        None,
        "docker_cleanup",
        serde_json::json!({"type": "docker_cleanup"}),
        50,
    )
    .await
    .unwrap();

    assert_eq!(claim_for_agent(&state, "orphan-check").await, Some(cleanup));
    assert_eq!(
        job_status(&state, orphan).await.as_deref(),
        Some("cancelled")
    );
}

#[tokio::test]
async fn db_teardown_fence_cancels_queued_and_rejects_late_enqueue() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    let queued = insert_typed_job(
        &state,
        Some(app_id),
        Some(deployment_id),
        "deploy",
        "queued",
        serde_json::json!({"type":"deploy","env":{"SECRET":"value"}}),
    )
    .await;

    let teardown = crate::job_control::start_app_teardown(
        &state,
        TEST_SERVER_ID,
        app_id,
        teardown_payload(app_id, user_id),
        true,
    )
    .await
    .unwrap();
    assert!(teardown.agent_cleanup_required);
    let row: (String, serde_json::Value) =
        sqlx::query_as("SELECT status,payload_json FROM agent_jobs WHERE id=$1")
            .bind(queued)
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(row.0, "cancelled");
    assert_eq!(row.1, serde_json::json!({}));
    assert!(crate::deploy::enqueue_agent_job(
        &state,
        TEST_SERVER_ID,
        Some(app_id),
        None,
        "health_check",
        serde_json::json!({"type":"health_check"}),
        20,
    )
    .await
    .is_err());
}

#[tokio::test]
async fn db_teardown_fence_flags_active_job_and_blocks_delete_claim() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    let deploy_job = insert_job(&state, app_id, deployment_id).await;
    assert_eq!(
        claim_for_agent(&state, "first-agent").await,
        Some(deploy_job)
    );
    let teardown = crate::job_control::start_app_teardown(
        &state,
        TEST_SERVER_ID,
        app_id,
        teardown_payload(app_id, user_id),
        true,
    )
    .await
    .unwrap();

    assert_eq!(teardown.active_jobs, 1);
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT cancel_requested_at IS NOT NULL FROM agent_jobs WHERE id=$1",
    )
    .bind(deploy_job)
    .fetch_one(&state.db)
    .await
    .unwrap());
    assert_eq!(claim_for_agent(&state, "second-agent").await, None);
    sqlx::query(
        "UPDATE agent_jobs
         SET status='cancelled',lease_expires_at=NULL,finished_at=now()
         WHERE id=$1",
    )
    .bind(deploy_job)
    .execute(&state.db)
    .await
    .unwrap();
    assert_eq!(
        claim_for_agent(&state, "second-agent").await,
        Some(teardown.job_id)
    );
}

#[tokio::test]
async fn db_finalize_delete_defers_until_active_job_terminal_and_scrubs_history() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let active = insert_typed_job(
        &state,
        Some(app_id),
        None,
        "health_check",
        "running",
        serde_json::json!({"type":"health_check","github_token":"secret"}),
    )
    .await;
    let teardown = crate::job_control::start_app_teardown(
        &state,
        TEST_SERVER_ID,
        app_id,
        teardown_payload(app_id, user_id),
        false,
    )
    .await
    .unwrap();

    assert!(
        !crate::web::finalize_delete_app_from_job(&state, teardown.job_id)
            .await
            .unwrap()
    );
    sqlx::query(
        "UPDATE agent_jobs
         SET status='cancelled',lease_expires_at=NULL,finished_at=now()
         WHERE id=$1",
    )
    .bind(active)
    .execute(&state.db)
    .await
    .unwrap();
    assert!(
        crate::web::finalize_delete_app_from_job(&state, teardown.job_id)
            .await
            .unwrap()
    );
    let retained_payloads: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT payload_json FROM agent_jobs WHERE id IN ($1,$2)")
            .bind(active)
            .bind(teardown.job_id)
            .fetch_all(&state.db)
            .await
            .unwrap();
    assert!(retained_payloads
        .iter()
        .all(|payload| payload == &serde_json::json!({})));
}

#[tokio::test]
async fn db_expired_orphaned_job_terminal_cancels_instead_of_requeue() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    let job_id = insert_expired_job(&state, app_id, deployment_id, 1, 3).await;
    sqlx::query("DELETE FROM apps WHERE id=$1")
        .bind(app_id)
        .execute(&state.db)
        .await
        .unwrap();

    recover_stale_agent_jobs(&state).await.unwrap();
    let row: (String, serde_json::Value) =
        sqlx::query_as("SELECT status,payload_json FROM agent_jobs WHERE id=$1")
            .bind(job_id)
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(row.0, "cancelled");
    assert_eq!(row.1, serde_json::json!({}));
}

#[tokio::test]
async fn db_delete_completion_finalizes_without_owner_poll() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let _deployment_id = insert_deployment(&state, app_id).await;
    let teardown = crate::job_control::start_app_teardown(
        &state,
        TEST_SERVER_ID,
        app_id,
        teardown_payload(app_id, user_id),
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        claim_for_agent(&state, "delete-agent").await,
        Some(teardown.job_id)
    );
    let claim_token: Uuid = sqlx::query_scalar("SELECT claim_token FROM agent_jobs WHERE id=$1")
        .bind(teardown.job_id)
        .fetch_one(&state.db)
        .await
        .unwrap();

    let status = complete_job_status(
        &state,
        &agent_headers(&state, TEST_SERVER_ID),
        teardown.job_id,
        CompleteJobRequest {
            status: "success".into(),
            failure: None,
            result: None,
            claim_token: Some(claim_token),
        },
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM apps WHERE id=$1")
            .bind(app_id)
            .fetch_one(&state.db)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn db_two_claimers_never_run_delete_and_stale_compute_concurrently() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    let deploy_job = insert_job(&state, app_id, deployment_id).await;
    assert_eq!(
        claim_for_agent(&state, "running-agent").await,
        Some(deploy_job)
    );
    let teardown = crate::job_control::start_app_teardown(
        &state,
        TEST_SERVER_ID,
        app_id,
        teardown_payload(app_id, user_id),
        true,
    )
    .await
    .unwrap();

    let (left, right) = tokio::join!(
        claim_for_agent(&state, "parallel-agent-a"),
        claim_for_agent(&state, "parallel-agent-b")
    );
    assert_eq!(left, None);
    assert_eq!(right, None);
    assert_eq!(
        job_status(&state, teardown.job_id).await.as_deref(),
        Some("queued")
    );
}

#[tokio::test]
async fn db_no_deployment_delete_waits_for_active_job() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let active = insert_typed_job(
        &state,
        Some(app_id),
        None,
        "health_check",
        "claimed",
        serde_json::json!({"type":"health_check"}),
    )
    .await;
    let teardown = crate::job_control::start_app_teardown(
        &state,
        TEST_SERVER_ID,
        app_id,
        teardown_payload(app_id, user_id),
        false,
    )
    .await
    .unwrap();
    assert_eq!(teardown.active_jobs, 1);
    assert!(
        !crate::web::finalize_delete_app_from_job(&state, teardown.job_id)
            .await
            .unwrap()
    );
    sqlx::query(
        "UPDATE agent_jobs
         SET status='cancelled',lease_expires_at=NULL,finished_at=now()
         WHERE id=$1",
    )
    .bind(active)
    .execute(&state.db)
    .await
    .unwrap();
    assert_eq!(
        crate::web::reconcile_completed_delete_jobs(&state)
            .await
            .unwrap(),
        1
    );
}
