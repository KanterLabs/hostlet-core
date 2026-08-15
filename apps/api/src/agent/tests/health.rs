use super::*;
use crate::health_alerts::{HealthEventHooks, HealthTransitionEvent};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct CapturingHealthHooks {
    events: Mutex<Vec<HealthTransitionEvent>>,
}

impl CapturingHealthHooks {
    fn events(&self) -> Vec<HealthTransitionEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl HealthEventHooks for CapturingHealthHooks {
    fn handle_health_transition(&self, _state: AppState, event: HealthTransitionEvent) {
        self.events.lock().unwrap().push(event);
    }
}

#[tokio::test]
async fn db_health_down_hook_fires_once_per_unhealthy_transition() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    let hooks = Arc::new(CapturingHealthHooks::default());
    let state = state.with_health_event_hooks(hooks.clone());
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;
    sqlx::query(
        "UPDATE deployments
         SET status='success',container_name=$1,published_port=32054
         WHERE id=$2",
    )
    .bind(format!("hostlet-app-{app_id}"))
    .bind(deployment_id)
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query("UPDATE apps SET current_deployment_id=$1 WHERE id=$2")
        .bind(deployment_id)
        .bind(app_id)
        .execute(&state.db)
        .await
        .unwrap();

    send_health_status(&state, app_id, deployment_id, "degraded").await;
    send_health_status(&state, app_id, deployment_id, "unhealthy").await;
    send_health_status(&state, app_id, deployment_id, "unhealthy").await;
    send_health_status(&state, app_id, deployment_id, "healthy").await;
    send_health_status(&state, app_id, deployment_id, "unhealthy").await;

    let events = hooks.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].app_id, app_id);
    assert_eq!(events[0].deployment_id, Some(deployment_id));
    assert_eq!(events[0].status, "unhealthy");
    assert_eq!(events[0].previous_status.as_deref(), Some("degraded"));
    assert_eq!(events[1].previous_status.as_deref(), Some("healthy"));
}

#[tokio::test]
async fn db_new_activation_starts_health_state_from_unknown() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    let hooks = Arc::new(CapturingHealthHooks::default());
    let state = state.with_health_event_hooks(hooks.clone());
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let first_deployment_id = insert_deployment(&state, app_id).await;

    // Activate A and populate every app-keyed health field that a later
    // deployment must not inherit.
    super::handle_agent_message(
        &state,
        super::TEST_SERVER_ID,
        serde_json::json!({
            "type": "deployment_status",
            "deployment_id": first_deployment_id,
            "status": "success",
            "container_name": "hostlet-app-health-a",
            "published_port": 32056,
            "local_url": "http://health-a.example.test"
        }),
    )
    .await;
    send_health_status(&state, app_id, first_deployment_id, "healthy").await;
    send_health_status(&state, app_id, first_deployment_id, "unhealthy").await;
    sqlx::query(
        "INSERT INTO app_browser_health
           (app_id,deployment_id,status,failure,checked_at)
         VALUES ($1,$2,'failed','old browser result',now())
         ON CONFLICT (app_id) DO UPDATE SET
           deployment_id=EXCLUDED.deployment_id,status=EXCLUDED.status,
           failure=EXCLUDED.failure,checked_at=EXCLUDED.checked_at",
    )
    .bind(app_id)
    .bind(first_deployment_id)
    .execute(&state.db)
    .await
    .unwrap();

    let second_deployment_id = insert_deployment(&state, app_id).await;
    super::handle_agent_message(
        &state,
        super::TEST_SERVER_ID,
        serde_json::json!({
            "type": "deployment_status",
            "deployment_id": second_deployment_id,
            "status": "success",
            "container_name": "hostlet-app-health-b",
            "published_port": 32057,
            "local_url": "http://health-b.example.test"
        }),
    )
    .await;

    let snapshot = sqlx::query(
        "SELECT deployment_id,status,failure_count,success_count,checked_url,last_error
         FROM app_health_snapshots WHERE app_id=$1",
    )
    .bind(app_id)
    .fetch_optional(&state.db)
    .await
    .unwrap();
    assert!(
        snapshot.is_none(),
        "deployment B inherited deployment A health"
    );
    let browser_status = sqlx::query_scalar::<_, Option<String>>(
        "SELECT status FROM app_browser_health WHERE app_id=$1",
    )
    .bind(app_id)
    .fetch_optional(&state.db)
    .await
    .unwrap()
    .flatten();
    assert!(
        browser_status.is_none() || browser_status.as_deref() == Some("pending"),
        "deployment B inherited browser health: {browser_status:?}"
    );

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::COOKIE,
        crate::auth::test_session_cookie_header(&state, user_id)
            .parse()
            .unwrap(),
    );
    let response = crate::web::app_health(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::extract::Path(app_id),
    )
    .await
    .into_response();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["status"], "unknown");
    assert_eq!(payload["deploymentId"], serde_json::Value::Null);
    assert_eq!(payload["failureCount"], 0);
    assert_eq!(payload["successCount"], 0);
    assert_eq!(payload["checkedUrl"], serde_json::Value::Null);
    assert!(payload["browser"].is_null() || payload["browser"]["status"] == "pending");

    // The first B failure must be a fresh transition, not suppressed by A's
    // unhealthy snapshot.
    send_health_status(&state, app_id, second_deployment_id, "unhealthy").await;
    let events = hooks.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].deployment_id, Some(second_deployment_id));
    assert_eq!(events[1].previous_status, None);
}

#[tokio::test]
async fn db_protocol_v2_commit_activation_resets_health_state() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    let hooks = Arc::new(CapturingHealthHooks::default());
    let state = state.with_health_event_hooks(hooks.clone());
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let first_deployment_id = insert_deployment(&state, app_id).await;
    sqlx::query(
        "UPDATE deployments
         SET status='success',container_name='hostlet-app-health-a',published_port=32056
         WHERE id=$1",
    )
    .bind(first_deployment_id)
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query("UPDATE apps SET current_deployment_id=$1 WHERE id=$2")
        .bind(first_deployment_id)
        .bind(app_id)
        .execute(&state.db)
        .await
        .unwrap();

    send_health_status(&state, app_id, first_deployment_id, "healthy").await;
    send_health_status(&state, app_id, first_deployment_id, "unhealthy").await;
    sqlx::query(
        "INSERT INTO app_browser_health
           (app_id,deployment_id,status,failure,checked_at)
         VALUES ($1,$2,'failed','old browser result',now())
         ON CONFLICT (app_id) DO UPDATE SET
           deployment_id=EXCLUDED.deployment_id,status=EXCLUDED.status,
           failure=EXCLUDED.failure,checked_at=EXCLUDED.checked_at",
    )
    .bind(app_id)
    .bind(first_deployment_id)
    .execute(&state.db)
    .await
    .unwrap();

    let second_deployment_id = insert_deployment(&state, app_id).await;
    let job_id = insert_job(&state, app_id, second_deployment_id).await;
    let claim_token = Uuid::new_v4();
    sqlx::query(
        "UPDATE agent_jobs
         SET status='claimed',claimed_by='fixture-agent',claim_token=$1,
             lease_expires_at=now()+interval '5 minutes',protocol_version=2
         WHERE id=$2",
    )
    .bind(claim_token)
    .bind(job_id)
    .execute(&state.db)
    .await
    .unwrap();
    let headers = agent_headers(&state, TEST_SERVER_ID);
    let candidate = hostlet_contracts::CandidateRuntime {
        container_name: "hostlet-app-health-b".into(),
        published_port: 32057,
        image_tag: Some("example:health-b".into()),
        compose_project: None,
        runtime_metadata: serde_json::json!({}),
        services: vec![],
    };
    let prepared = crate::deployment_execution::prepare_activation(
        State(state.clone()),
        headers.clone(),
        Path(second_deployment_id),
        Json(hostlet_contracts::PrepareActivationRequest {
            job_id,
            claim_token,
            expected_current_deployment_id: Some(first_deployment_id),
            candidate,
        }),
    )
    .await
    .into_response();
    assert_eq!(prepared.status(), StatusCode::OK);
    let body = axum::body::to_bytes(prepared.into_body(), 16 * 1024)
        .await
        .unwrap();
    let receipt: hostlet_contracts::PrepareActivationReceipt =
        serde_json::from_slice(&body).unwrap();

    let committed = crate::deployment_execution::commit_activation(
        State(state.clone()),
        headers,
        Path(second_deployment_id),
        Json(hostlet_contracts::CommitActivationRequest {
            job_id,
            claim_token,
            route_generation: receipt.route_generation,
            local_url: Some("http://health-b.example.test".into()),
            runtime_metadata: None,
            rolled_back: false,
        }),
    )
    .await
    .into_response();
    assert_eq!(committed.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        current_deployment(&state, app_id).await,
        Some(second_deployment_id)
    );
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT NOT EXISTS(
               SELECT 1 FROM app_health_snapshots WHERE app_id=$1
             )",
    )
    .bind(app_id)
    .fetch_one(&state.db)
    .await
    .unwrap());
    let browser = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT deployment_id,status FROM app_browser_health WHERE app_id=$1",
    )
    .bind(app_id)
    .fetch_optional(&state.db)
    .await
    .unwrap();
    if let Some((deployment_id, status)) = browser {
        assert_eq!(deployment_id, second_deployment_id);
        assert_eq!(status, "pending");
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM app_health_events WHERE app_id=$1",)
            .bind(app_id)
            .fetch_one(&state.db)
            .await
            .unwrap(),
        2
    );

    // A first B failure must not inherit A's alert suppression state.
    send_health_status(&state, app_id, second_deployment_id, "unhealthy").await;
    let events = hooks.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].deployment_id, Some(second_deployment_id));
    assert_eq!(events[1].previous_status, None);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM app_health_events WHERE app_id=$1",)
            .bind(app_id)
            .fetch_one(&state.db)
            .await
            .unwrap(),
        3
    );
}

#[tokio::test]
async fn db_browser_health_result_waits_for_activation_fence() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let first_deployment_id = insert_deployment(&state, app_id).await;
    sqlx::query("UPDATE deployments SET status='success' WHERE id=$1")
        .bind(first_deployment_id)
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query("UPDATE apps SET current_deployment_id=$1 WHERE id=$2")
        .bind(first_deployment_id)
        .bind(app_id)
        .execute(&state.db)
        .await
        .unwrap();
    let second_deployment_id = insert_deployment(&state, app_id).await;
    sqlx::query(
        "INSERT INTO app_browser_health
           (app_id,deployment_id,status,checked_at)
         VALUES ($1,$2,'ready',now())",
    )
    .bind(app_id)
    .bind(first_deployment_id)
    .execute(&state.db)
    .await
    .unwrap();

    // Take the same app-row lock activation holds while switching and resetting.
    // The browser writer must wait, then fence its old deployment after commit.
    let mut activation_tx = state.db.begin().await.unwrap();
    sqlx::query("SELECT id FROM apps WHERE id=$1 FOR UPDATE")
        .bind(app_id)
        .fetch_one(&mut *activation_tx)
        .await
        .unwrap();
    let browser_db = state.db.clone();
    let writer_application_name = format!("hostlet-c15-race-{}", Uuid::new_v4().simple());
    let (writer_ready_tx, writer_ready_rx) = tokio::sync::oneshot::channel();
    let writer_application_name_for_task = writer_application_name.clone();
    let browser_writer = tokio::spawn(async move {
        let mut tx = browser_db.begin().await?;
        sqlx::query("SELECT set_config('application_name',$1,true)")
            .bind(&writer_application_name_for_task)
            .execute(&mut *tx)
            .await?;
        writer_ready_tx
            .send(())
            .map_err(|_| anyhow::anyhow!("race writer readiness receiver dropped"))?;
        crate::browser_health::record_job_result(
            &mut tx,
            crate::browser_health::BROWSER_SMOKE_JOB,
            Some(app_id),
            Some(first_deployment_id),
            "success",
            None,
        )
        .await?;
        tx.commit().await?;
        Ok::<(), anyhow::Error>(())
    });
    writer_ready_rx.await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting_on_app_lock = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(
                   SELECT 1 FROM pg_stat_activity
                   WHERE application_name=$1
                     AND state='active'
                     AND wait_event_type='Lock'
                 )",
            )
            .bind(&writer_application_name)
            .fetch_one(&state.db)
            .await
            .unwrap();
            if waiting_on_app_lock {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("browser writer did not reach the app-row lock wait");
    sqlx::query("UPDATE apps SET current_deployment_id=$1 WHERE id=$2")
        .bind(second_deployment_id)
        .bind(app_id)
        .execute(&mut *activation_tx)
        .await
        .unwrap();
    crate::browser_health::reset_for_activation(&mut activation_tx, app_id)
        .await
        .unwrap();
    activation_tx.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), browser_writer)
        .await
        .expect("browser writer remained blocked after activation commit")
        .expect("browser writer task panicked")
        .expect("browser writer failed");

    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT NOT EXISTS(
               SELECT 1 FROM app_browser_health WHERE app_id=$1
             )",
    )
    .bind(app_id)
    .fetch_one(&state.db)
    .await
    .unwrap());
}
