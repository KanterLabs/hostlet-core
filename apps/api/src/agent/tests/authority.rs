use super::*;

async fn insert_job_with_payload(
    state: &AppState,
    app_id: Uuid,
    deployment_id: Uuid,
    job_type: &str,
    status: &str,
    payload: serde_json::Value,
) -> Uuid {
    sqlx::query_scalar(
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

async fn claim_only_queued_job(state: &AppState) {
    let response = claim_job(
        State(state.clone()),
        agent_headers(state, TEST_SERVER_ID),
        Json(ClaimJobRequest {
            agent_id: Some("authority-test-agent".into()),
            protocol_version: 2,
        }),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
}

fn activation_candidate(app_id: Uuid) -> hostlet_contracts::CandidateRuntime {
    hostlet_contracts::CandidateRuntime {
        container_name: format!("hostlet-app-{app_id}"),
        published_port: 32_001,
        image_tag: Some("example:test".into()),
        compose_project: None,
        runtime_metadata: serde_json::json!({}),
        services: Vec::new(),
    }
}

#[tokio::test]
async fn db_long_running_build_heartbeat_renews_lease_without_duplicate_claim() {
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
        "build",
        "running",
        serde_json::json!({"type":"build"}),
    )
    .await;
    let claim_token = Uuid::new_v4();
    let original_lease: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "UPDATE agent_jobs
         SET claimed_by='fixture-agent',
             claimed_at=now()-interval '6 minutes',
             claim_token=$2,
             lease_expires_at=now()+interval '1 minute'
         WHERE id=$1
         RETURNING lease_expires_at",
    )
    .bind(job_id)
    .bind(claim_token)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT claimed_at < clock_timestamp()-interval '5 minutes'
         FROM agent_jobs WHERE id=$1",
    )
    .bind(job_id)
    .fetch_one(&state.db)
    .await
    .unwrap());
    sqlx::query("UPDATE deployments SET status='publishing' WHERE id=$1")
        .bind(deployment_id)
        .execute(&state.db)
        .await
        .unwrap();

    let heartbeat_status = crate::deployment_execution::heartbeat(
        State(state.clone()),
        agent_headers(&state, TEST_SERVER_ID),
        Path(job_id),
        Json(hostlet_contracts::AgentJobHeartbeat {
            claim_token,
            phase: DeploymentStatus::Running,
        }),
    )
    .await
    .into_response()
    .status();
    assert_eq!(heartbeat_status, StatusCode::OK);
    let renewed_lease: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT lease_expires_at FROM agent_jobs WHERE id=$1")
            .bind(job_id)
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert!(
        renewed_lease - original_lease > chrono::Duration::minutes(3),
        "heartbeat must renew a long-running job well beyond its prior lease"
    );
    assert_eq!(
        deployment_status_by_id(&state, deployment_id)
            .await
            .as_deref(),
        Some("publishing")
    );
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT last_heartbeat_at IS NOT NULL FROM deployments WHERE id=$1",
    )
    .bind(deployment_id)
    .fetch_one(&state.db)
    .await
    .unwrap());

    // Polling for new work after the renewal exercises the stale-claim path:
    // the still-running job must remain owned by the original worker rather
    // than being requeued and claimed a second time.
    let stale_claim_response = claim_job(
        State(state.clone()),
        agent_headers(&state, TEST_SERVER_ID),
        Json(ClaimJobRequest {
            agent_id: Some("stale-claimant".into()),
            protocol_version: 2,
        }),
    )
    .await
    .into_response();
    assert_eq!(stale_claim_response.status(), StatusCode::OK);
    assert_eq!(job_status(&state, job_id).await.as_deref(), Some("running"));
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>("SELECT claimed_by FROM agent_jobs WHERE id=$1",)
            .bind(job_id)
            .fetch_one(&state.db)
            .await
            .unwrap()
            .as_deref(),
        Some("fixture-agent")
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM agent_jobs
             WHERE app_id=$1 AND status IN ('claimed','running')",
        )
        .bind(app_id)
        .fetch_one(&state.db)
        .await
        .unwrap(),
        1,
        "a renewed build must not have duplicate active ownership"
    );

    sqlx::query("UPDATE deployments SET status='queued' WHERE id=$1")
        .bind(deployment_id)
        .execute(&state.db)
        .await
        .unwrap();
    let queued_status = crate::deployment_execution::heartbeat(
        State(state.clone()),
        agent_headers(&state, TEST_SERVER_ID),
        Path(job_id),
        Json(hostlet_contracts::AgentJobHeartbeat {
            claim_token,
            phase: DeploymentStatus::Running,
        }),
    )
    .await
    .into_response()
    .status();
    assert_eq!(queued_status, StatusCode::OK);
    assert_eq!(
        deployment_status_by_id(&state, deployment_id)
            .await
            .as_deref(),
        Some("running")
    );
}

#[tokio::test]
async fn db_completion_requires_current_claim_and_live_lease() {
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
        serde_json::json!({"type":"deploy"}),
    )
    .await;
    claim_only_queued_job(&state).await;
    let claim_token: Uuid = sqlx::query_scalar("SELECT claim_token FROM agent_jobs WHERE id=$1")
        .bind(job_id)
        .fetch_one(&state.db)
        .await
        .unwrap();
    let headers = agent_headers(&state, TEST_SERVER_ID);

    let missing = complete_job_status(
        &state,
        &headers,
        job_id,
        CompleteJobRequest {
            status: "success".into(),
            failure: None,
            result: None,
            claim_token: None,
        },
    )
    .await;
    assert_eq!(missing, StatusCode::BAD_REQUEST);

    let stale = complete_job_status(
        &state,
        &headers,
        job_id,
        CompleteJobRequest {
            status: "success".into(),
            failure: None,
            result: None,
            claim_token: Some(Uuid::new_v4()),
        },
    )
    .await;
    assert_eq!(stale, StatusCode::CONFLICT);

    sqlx::query("UPDATE agent_jobs SET lease_expires_at=now()-interval '1 second' WHERE id=$1")
        .bind(job_id)
        .execute(&state.db)
        .await
        .unwrap();
    let expired = complete_job_status(
        &state,
        &headers,
        job_id,
        CompleteJobRequest {
            status: "success".into(),
            failure: None,
            result: None,
            claim_token: Some(claim_token),
        },
    )
    .await;
    assert_eq!(expired, StatusCode::CONFLICT);
    assert_eq!(job_status(&state, job_id).await.as_deref(), Some("claimed"));
}

#[tokio::test]
async fn db_pause_during_build_cancels_without_release_or_retry() {
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
        "build",
        "queued",
        serde_json::json!({"type":"build"}),
    )
    .await;
    claim_only_queued_job(&state).await;
    let claim_token: Uuid = sqlx::query_scalar("SELECT claim_token FROM agent_jobs WHERE id=$1")
        .bind(job_id)
        .fetch_one(&state.db)
        .await
        .unwrap();

    crate::suspensions::set_reason(
        &state,
        app_id,
        Some(user_id),
        crate::suspensions::USER_REQUESTED,
        "owner",
    )
    .await
    .unwrap();
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT cancel_requested_at IS NOT NULL FROM agent_jobs WHERE id=$1",
    )
    .bind(job_id)
    .fetch_one(&state.db)
    .await
    .unwrap());

    let completion = complete_job_status(
        &state,
        &agent_headers(&state, TEST_SERVER_ID),
        job_id,
        CompleteJobRequest {
            status: "success".into(),
            failure: None,
            result: None,
            claim_token: Some(claim_token),
        },
    )
    .await;
    assert_eq!(completion, StatusCode::CONFLICT);

    sqlx::query("UPDATE agent_jobs SET lease_expires_at=now()-interval '1 second' WHERE id=$1")
        .bind(job_id)
        .execute(&state.db)
        .await
        .unwrap();
    let recovered = recover_stale_agent_jobs(&state).await.unwrap();
    assert!(recovered >= 1);
    assert_eq!(
        job_status(&state, job_id).await.as_deref(),
        Some("cancelled")
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM deployments WHERE id=$1")
            .bind(deployment_id)
            .fetch_one(&state.db)
            .await
            .unwrap(),
        "canceled"
    );
}

#[tokio::test]
async fn db_paused_app_rejects_rollback_creation() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let current_id: Uuid = sqlx::query_scalar(
        "INSERT INTO deployments
           (app_id,server_id,status,commit_sha,started_at,finished_at,runtime_kind,container_name,published_port)
         VALUES ($1,$2,'success','current',now(),now(),'single',$3,32001)
         RETURNING id",
    )
    .bind(app_id)
    .bind(TEST_SERVER_ID)
    .bind(format!("hostlet-app-{app_id}"))
    .fetch_one(&state.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO deployments
           (app_id,server_id,status,commit_sha,started_at,finished_at,runtime_kind,container_name,published_port)
         VALUES ($1,$2,'success','previous',now(),now(),'single',$3,32002)",
    )
    .bind(app_id)
    .bind(TEST_SERVER_ID)
    .bind(format!("hostlet-app-{app_id}-previous"))
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query("UPDATE apps SET current_deployment_id=$1 WHERE id=$2")
        .bind(current_id)
        .bind(app_id)
        .execute(&state.db)
        .await
        .unwrap();
    crate::suspensions::set_reason(
        &state,
        app_id,
        Some(user_id),
        crate::suspensions::USER_REQUESTED,
        "owner",
    )
    .await
    .unwrap();

    let error = crate::deploy::create_and_send_rollback(&state, user_id, app_id)
        .await
        .expect_err("a paused app must not create a rollback");
    assert!(error.to_string().contains("app is paused"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM deployments WHERE app_id=$1 AND status IN ('queued','queued_for_build','running','building','publishing','queued_for_release','pulling','starting','health_checking','routing')",
        )
        .bind(app_id)
        .fetch_one(&state.db)
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn db_capacity_managed_enqueue_rechecks_pause_under_app_lock() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    crate::suspensions::set_reason(
        &state,
        app_id,
        Some(user_id),
        crate::suspensions::USER_REQUESTED,
        "owner",
    )
    .await
    .unwrap();

    for job_type in ["rollback", "build", "release"] {
        let error = crate::deploy::enqueue_agent_job(
            &state,
            TEST_SERVER_ID,
            Some(app_id),
            None,
            job_type,
            serde_json::json!({"type":job_type,"app_id":app_id}),
            10,
        )
        .await
        .expect_err("paused app must be rejected while the enqueue transaction owns its row");
        assert!(error.to_string().contains("app is paused"));
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM agent_jobs WHERE app_id=$1")
            .bind(app_id)
            .fetch_one(&state.db)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn db_expired_claim_cannot_commit_activation() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    let job_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_jobs
           (server_id,app_id,deployment_id,job_type,status,payload_json,claim_token,attempt,lease_expires_at)
         VALUES ($1,$2,$3,'release','running','{}',$4,1,now()-interval '1 second')
         RETURNING id",
    )
    .bind(TEST_SERVER_ID)
    .bind(app_id)
    .bind(deployment_id)
    .bind(Uuid::new_v4())
    .fetch_one(&state.db)
    .await
    .unwrap();
    let claim_token: Uuid = sqlx::query_scalar("SELECT claim_token FROM agent_jobs WHERE id=$1")
        .bind(job_id)
        .fetch_one(&state.db)
        .await
        .unwrap();
    sqlx::query("UPDATE apps SET pending_deployment_id=$1,route_generation=1 WHERE id=$2")
        .bind(deployment_id)
        .bind(app_id)
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query("UPDATE deployments SET status='routing' WHERE id=$1")
        .bind(deployment_id)
        .execute(&state.db)
        .await
        .unwrap();

    let response = crate::deployment_execution::commit_activation(
        State(state.clone()),
        agent_headers(&state, TEST_SERVER_ID),
        Path(deployment_id),
        Json(hostlet_contracts::CommitActivationRequest {
            job_id,
            claim_token,
            route_generation: 1,
            local_url: None,
            runtime_metadata: None,
            rolled_back: false,
        }),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT current_deployment_id FROM apps WHERE id=$1",
        )
        .bind(app_id)
        .fetch_one(&state.db)
        .await
        .unwrap(),
        None
    );
}

#[tokio::test]
async fn db_cancel_between_prepare_and_commit_cannot_activate() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    let claim_token = Uuid::new_v4();
    let job_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_jobs
           (server_id,app_id,deployment_id,job_type,status,payload_json,claim_token,attempt,lease_expires_at)
         VALUES ($1,$2,$3,'release','running','{}',$4,1,now()+interval '5 minutes')
         RETURNING id",
    )
    .bind(TEST_SERVER_ID)
    .bind(app_id)
    .bind(deployment_id)
    .bind(claim_token)
    .fetch_one(&state.db)
    .await
    .unwrap();

    let prepared = crate::deployment_execution::prepare_activation(
        State(state.clone()),
        agent_headers(&state, TEST_SERVER_ID),
        Path(deployment_id),
        Json(hostlet_contracts::PrepareActivationRequest {
            job_id,
            claim_token,
            expected_current_deployment_id: None,
            candidate: activation_candidate(app_id),
        }),
    )
    .await
    .into_response();
    assert_eq!(prepared.status(), StatusCode::OK);
    assert_eq!(
        sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT pending_deployment_id FROM apps WHERE id=$1",
        )
        .bind(app_id)
        .fetch_one(&state.db)
        .await
        .unwrap(),
        Some(deployment_id)
    );

    let outcome = crate::job_control::cancel_agent_job_for_actor(
        &state,
        crate::job_control::AgentJobVisibility {
            user_id,
            cloud_mode: false,
        },
        job_id,
        crate::job_control::JobAuditActor::owner(),
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        crate::job_control::AgentJobCancelOutcome::Cancelled
    );

    let committed = crate::deployment_execution::commit_activation(
        State(state.clone()),
        agent_headers(&state, TEST_SERVER_ID),
        Path(deployment_id),
        Json(hostlet_contracts::CommitActivationRequest {
            job_id,
            claim_token,
            route_generation: 1,
            local_url: None,
            runtime_metadata: None,
            rolled_back: false,
        }),
    )
    .await
    .into_response();
    assert_eq!(committed.status(), StatusCode::CONFLICT);
    assert_eq!(
        sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT current_deployment_id FROM apps WHERE id=$1",
        )
        .bind(app_id)
        .fetch_one(&state.db)
        .await
        .unwrap(),
        None
    );
}

#[tokio::test]
async fn db_activation_commit_accepts_terminal_completion_ack() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    let claim_token = Uuid::new_v4();
    let job_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_jobs
           (server_id,app_id,deployment_id,job_type,status,payload_json,claim_token,attempt,lease_expires_at)
         VALUES ($1,$2,$3,'release','running','{}',$4,1,now()+interval '5 minutes')
         RETURNING id",
    )
    .bind(TEST_SERVER_ID)
    .bind(app_id)
    .bind(deployment_id)
    .bind(claim_token)
    .fetch_one(&state.db)
    .await
    .unwrap();

    let prepared = crate::deployment_execution::prepare_activation(
        State(state.clone()),
        agent_headers(&state, TEST_SERVER_ID),
        Path(deployment_id),
        Json(hostlet_contracts::PrepareActivationRequest {
            job_id,
            claim_token,
            expected_current_deployment_id: None,
            candidate: activation_candidate(app_id),
        }),
    )
    .await
    .into_response();
    assert_eq!(prepared.status(), StatusCode::OK);

    let committed = crate::deployment_execution::commit_activation(
        State(state.clone()),
        agent_headers(&state, TEST_SERVER_ID),
        Path(deployment_id),
        Json(hostlet_contracts::CommitActivationRequest {
            job_id,
            claim_token,
            route_generation: 1,
            local_url: None,
            runtime_metadata: None,
            rolled_back: false,
        }),
    )
    .await
    .into_response();
    assert_eq!(committed.status(), StatusCode::NO_CONTENT);

    let stale_retry = crate::deployment_execution::commit_activation(
        State(state.clone()),
        agent_headers(&state, TEST_SERVER_ID),
        Path(deployment_id),
        Json(hostlet_contracts::CommitActivationRequest {
            job_id,
            claim_token: Uuid::new_v4(),
            route_generation: 1,
            local_url: None,
            runtime_metadata: None,
            rolled_back: false,
        }),
    )
    .await
    .into_response();
    assert_eq!(stale_retry.status(), StatusCode::CONFLICT);

    let wrong_generation_retry = crate::deployment_execution::commit_activation(
        State(state.clone()),
        agent_headers(&state, TEST_SERVER_ID),
        Path(deployment_id),
        Json(hostlet_contracts::CommitActivationRequest {
            job_id,
            claim_token,
            route_generation: 2,
            local_url: None,
            runtime_metadata: None,
            rolled_back: false,
        }),
    )
    .await
    .into_response();
    assert_eq!(wrong_generation_retry.status(), StatusCode::CONFLICT);

    let acknowledgement = complete_job_status(
        &state,
        &agent_headers(&state, TEST_SERVER_ID),
        job_id,
        CompleteJobRequest {
            status: "success".into(),
            failure: None,
            result: None,
            claim_token: Some(claim_token),
        },
    )
    .await;
    assert_eq!(acknowledgement, StatusCode::NO_CONTENT);
    assert_eq!(job_status(&state, job_id).await.as_deref(), Some("success"));
    assert_eq!(
        current_deployment(&state, app_id).await,
        Some(deployment_id)
    );
}

#[tokio::test]
async fn db_terminal_completion_retries_are_idempotent_and_fenced() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let failed_deployment = insert_deployment(&state, app_id).await;
    let failed_token = Uuid::new_v4();
    let failed_job: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_jobs
           (server_id,app_id,deployment_id,job_type,status,payload_json,claim_token,attempt,lease_expires_at)
         VALUES ($1,$2,$3,'deploy','running','{}',$4,1,now()+interval '5 minutes')
         RETURNING id",
    )
    .bind(TEST_SERVER_ID)
    .bind(app_id)
    .bind(failed_deployment)
    .bind(failed_token)
    .fetch_one(&state.db)
    .await
    .unwrap();
    let headers = agent_headers(&state, TEST_SERVER_ID);
    let failed_request = CompleteJobRequest {
        status: "failed".into(),
        failure: Some("agent failed".into()),
        result: None,
        claim_token: Some(failed_token),
    };
    assert_eq!(
        complete_job_status(&state, &headers, failed_job, failed_request).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        complete_job_status(
            &state,
            &headers,
            failed_job,
            CompleteJobRequest {
                status: "failed".into(),
                failure: Some("agent failed".into()),
                result: None,
                claim_token: Some(failed_token),
            },
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        complete_job_status(
            &state,
            &headers,
            failed_job,
            CompleteJobRequest {
                status: "success".into(),
                failure: None,
                result: None,
                claim_token: Some(failed_token),
            },
        )
        .await,
        StatusCode::CONFLICT
    );

    let canceled_deployment = insert_deployment(&state, app_id).await;
    let canceled_token = Uuid::new_v4();
    let canceled_job: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_jobs
           (server_id,app_id,deployment_id,job_type,status,payload_json,claim_token,attempt,lease_expires_at,cancel_requested_at)
         VALUES ($1,$2,$3,'deploy','running','{}',$4,1,now()+interval '5 minutes',now())
         RETURNING id",
    )
    .bind(TEST_SERVER_ID)
    .bind(app_id)
    .bind(canceled_deployment)
    .bind(canceled_token)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(
        complete_job_status(
            &state,
            &headers,
            canceled_job,
            CompleteJobRequest {
                status: "cancelled".into(),
                failure: Some("owner canceled".into()),
                result: None,
                claim_token: Some(canceled_token),
            },
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        complete_job_status(
            &state,
            &headers,
            canceled_job,
            CompleteJobRequest {
                status: "cancelled".into(),
                failure: Some("owner canceled".into()),
                result: None,
                claim_token: Some(canceled_token),
            },
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        complete_job_status(
            &state,
            &headers,
            canceled_job,
            CompleteJobRequest {
                status: "failed".into(),
                failure: Some("wrong terminal status".into()),
                result: None,
                claim_token: Some(canceled_token),
            },
        )
        .await,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn db_direct_cancel_of_queued_build_clears_pending_and_build_state() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    sqlx::query("UPDATE deployments SET status='queued_for_build' WHERE id=$1")
        .bind(deployment_id)
        .execute(&state.db)
        .await
        .unwrap();
    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO deployment_builds
           (deployment_id,build_pool_id,status,required_platform,build_spec_json)
         VALUES ($1,'00000000-0000-0000-0000-000000000010','queued','linux/amd64','{}')
         RETURNING id",
    )
    .bind(deployment_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    sqlx::query("UPDATE deployments SET build_id=$1 WHERE id=$2")
        .bind(build_id)
        .bind(deployment_id)
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query("UPDATE apps SET pending_deployment_id=$1 WHERE id=$2")
        .bind(deployment_id)
        .bind(app_id)
        .execute(&state.db)
        .await
        .unwrap();
    let job_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_jobs
           (server_id,build_pool_id,app_id,deployment_id,job_type,status,payload_json,protocol_version)
         VALUES (NULL,'00000000-0000-0000-0000-000000000010',$1,$2,'build','queued',
                 '{\"type\":\"build\"}',6)
         RETURNING id",
    )
    .bind(app_id)
    .bind(deployment_id)
    .fetch_one(&state.db)
    .await
    .unwrap();

    let outcome = crate::job_control::cancel_agent_job_for_actor(
        &state,
        crate::job_control::AgentJobVisibility {
            user_id,
            cloud_mode: false,
        },
        job_id,
        crate::job_control::JobAuditActor::owner(),
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        crate::job_control::AgentJobCancelOutcome::Cancelled
    );
    assert_eq!(
        job_status(&state, job_id).await.as_deref(),
        Some("cancelled")
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM deployments WHERE id=$1")
            .bind(deployment_id)
            .fetch_one(&state.db)
            .await
            .unwrap(),
        "canceled"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM deployment_builds WHERE id=$1")
            .bind(build_id)
            .fetch_one(&state.db)
            .await
            .unwrap(),
        "canceled"
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT pending_deployment_id FROM apps WHERE id=$1",
        )
        .bind(app_id)
        .fetch_one(&state.db)
        .await
        .unwrap(),
        None
    );
}
