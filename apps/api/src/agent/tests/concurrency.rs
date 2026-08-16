use super::*;

#[tokio::test]
async fn db_concurrent_cancellation_and_activation_follow_lock_order() {
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
           (server_id,app_id,deployment_id,job_type,status,payload_json,claim_token,lease_expires_at)
         VALUES ($1,$2,$3,'release','running','{}',$4,now()+interval '5 minutes')
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
            candidate: hostlet_contracts::CandidateRuntime {
                container_name: format!("hostlet-app-{app_id}"),
                published_port: 32_001,
                image_tag: Some("example:test".into()),
                compose_project: None,
                runtime_metadata: serde_json::json!({}),
                services: Vec::new(),
            },
        }),
    )
    .await
    .into_response();
    assert_eq!(prepared.status(), StatusCode::OK);

    // Hold the parent app row while both production transactions reach their
    // canonical app -> deployment -> job lock. Polling pg_stat_activity makes
    // the overlap deterministic instead of relying on scheduler timing.
    let mut blocker = state.db.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='2s'")
        .execute(&mut *blocker)
        .await
        .unwrap();
    sqlx::query("SET LOCAL statement_timeout='5s'")
        .execute(&mut *blocker)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM apps WHERE id=$1 FOR UPDATE")
        .bind(app_id)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();

    let cancel_state = state.clone();
    let cancel = tokio::spawn(async move {
        crate::job_control::cancel_agent_job_for_actor(
            &cancel_state,
            crate::job_control::AgentJobVisibility {
                user_id,
                cloud_mode: false,
            },
            job_id,
            crate::job_control::JobAuditActor::owner(),
        )
        .await
    });
    let activate_state = state.clone();
    let activate_headers = agent_headers(&state, TEST_SERVER_ID);
    let activate = tokio::spawn(async move {
        Ok::<_, anyhow::Error>(
            crate::deployment_execution::commit_activation(
                State(activate_state),
                activate_headers,
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
            .into_response()
            .status(),
        )
    });

    let waiters = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*)
                 FROM pg_stat_activity
                 WHERE datname=current_database()
                   AND pid <> pg_backend_pid()
                   AND wait_event_type='Lock'
                   AND query LIKE '%SELECT id FROM apps WHERE id=$1 FOR UPDATE%'",
            )
            .fetch_one(&state.db)
            .await
            .unwrap();
            if count >= 2 {
                break count;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("both production transactions must reach the app lock");
    assert!(waiters >= 2);
    blocker.commit().await.unwrap();

    let (cancelled, activated) =
        tokio::time::timeout(std::time::Duration::from_secs(5), async move {
            tokio::try_join!(cancel, activate)
        })
        .await
        .expect("canonical row-lock order must not deadlock")
        .expect("concurrent authority transactions must succeed");
    let cancelled = cancelled.expect("cancellation transaction must commit");
    let activated = activated.expect("activation transaction must commit");

    if cancelled == crate::job_control::AgentJobCancelOutcome::Cancelled {
        assert_eq!(activated, StatusCode::CONFLICT);
        let acknowledged = complete_job_status(
            &state,
            &agent_headers(&state, TEST_SERVER_ID),
            job_id,
            CompleteJobRequest {
                status: "cancelled".into(),
                failure: None,
                result: None,
                claim_token: Some(claim_token),
            },
        )
        .await;
        assert_eq!(acknowledged, StatusCode::NO_CONTENT);
    } else {
        assert_eq!(
            cancelled,
            crate::job_control::AgentJobCancelOutcome::NotFound
        );
        assert_eq!(activated, StatusCode::NO_CONTENT);
    }

    let final_state: (String, bool, String, Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT j.status,j.cancel_requested_at IS NOT NULL,d.status,
                a.current_deployment_id,a.pending_deployment_id
         FROM agent_jobs j JOIN deployments d ON d.id=j.deployment_id
         JOIN apps a ON a.id=j.app_id WHERE j.id=$1",
    )
    .bind(job_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert!(
        (final_state.0 == "cancelled"
            && final_state.1
            && final_state.2 == "canceled"
            && final_state.3.is_none()
            && final_state.4.is_none())
            || (final_state.0 == "success"
                && !final_state.1
                && final_state.2 == "success"
                && final_state.3 == Some(deployment_id)
                && final_state.4.is_none())
    );
}

#[tokio::test]
async fn db_capacity_and_pause_boundary_has_no_deadlock_or_queued_work() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    sqlx::query("UPDATE deployments SET status='queued_for_release' WHERE id=$1")
        .bind(deployment_id)
        .execute(&state.db)
        .await
        .unwrap();
    let job_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_jobs
           (server_id,app_id,deployment_id,job_type,status,payload_json)
         VALUES ($1,$2,$3,'release','queued','{\"capacity_wait\":true}')
         RETURNING id",
    )
    .bind(TEST_SERVER_ID)
    .bind(app_id)
    .bind(deployment_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    sqlx::query("UPDATE servers SET max_concurrent_apps=1,max_concurrent_builds=4 WHERE id=$1")
        .bind(TEST_SERVER_ID)
        .execute(&state.db)
        .await
        .unwrap();
    let capacity_name = format!("core05-capacity-{}", Uuid::new_v4().simple());
    let pause_name = format!("core05-pause-{}", Uuid::new_v4().simple());
    let capacity_state = state_with_application_name(&capacity_name).await;
    let pause_state = state_with_application_name(&pause_name).await;
    let mut app_blocker = state.db.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='2s'")
        .execute(&mut *app_blocker)
        .await
        .unwrap();
    sqlx::query("SET LOCAL statement_timeout='5s'")
        .execute(&mut *app_blocker)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM apps WHERE id=$1 FOR UPDATE")
        .bind(app_id)
        .fetch_one(&mut *app_blocker)
        .await
        .unwrap();
    // Both tasks call production transactions. Their named pools let the
    // observation below prove each one is waiting on the held app boundary.
    let capacity = tokio::spawn(async move {
        crate::server_capacity::admit_waiting_deployments(&capacity_state).await
    });
    let pause = tokio::spawn(async move {
        crate::suspensions::set_reason(
            &pause_state,
            app_id,
            Some(user_id),
            crate::suspensions::USER_REQUESTED,
            "owner",
        )
        .await
        .map(|_| ())
        .map_err(|err| anyhow::anyhow!(err.to_string()))
    });
    let waiters = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let sessions: Vec<(String, String)> = sqlx::query_as(
                "SELECT application_name,query
                 FROM pg_stat_activity
                 WHERE application_name IN ($1,$2)
                   AND wait_event_type='Lock'
                   AND query LIKE '%FOR UPDATE%'",
            )
            .bind(&capacity_name)
            .bind(&pause_name)
            .fetch_all(&state.db)
            .await
            .unwrap();
            if sessions.len() == 2 && sessions.iter().all(|(_, query)| query.contains("apps")) {
                break sessions;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("capacity and pause must reach the held app lock");
    assert_eq!(waiters.len(), 2);
    app_blocker.commit().await.unwrap();
    let (capacity, pause) = tokio::time::timeout(std::time::Duration::from_secs(5), async move {
        tokio::try_join!(capacity, pause)
    })
    .await
    .expect("capacity admission and pause must not deadlock")
    .expect("capacity/pause transactions must succeed");
    let admitted = capacity.expect("capacity scheduler transaction must commit");
    pause.expect("pause transaction must commit");
    assert!(admitted <= 1);
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT suspended_at IS NOT NULL FROM apps WHERE id=$1",)
            .bind(app_id)
            .fetch_one(&state.db)
            .await
            .unwrap()
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
}

async fn state_with_application_name(application_name: &str) -> AppState {
    let base_url =
        std::env::var("HOSTLET_DB_TEST_URL").expect("DB-backed tests require HOSTLET_DB_TEST_URL");
    let separator = if base_url.contains('?') { '&' } else { '?' };
    let database_url = format!("{base_url}{separator}application_name={application_name}");
    let previous_database_url = std::env::var("DATABASE_URL").ok();
    std::env::set_var("DATABASE_URL", database_url);
    let state = crate::state::AppState::from_env()
        .await
        .expect("named DB test state must initialize");
    if let Some(previous_database_url) = previous_database_url {
        std::env::set_var("DATABASE_URL", previous_database_url);
    } else {
        std::env::remove_var("DATABASE_URL");
    }
    state
}
