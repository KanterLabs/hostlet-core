use super::*;
use axum::{body::to_bytes, extract::State, response::IntoResponse};

#[tokio::test]
async fn db_health_targets_emit_one_canonical_split_route_owner() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    super::reset_agent_db(&state).await;
    let user_id = super::insert_user(&state).await;
    let app_id = super::insert_app(&state, user_id).await;
    let deployment_id = super::insert_deployment(&state, app_id).await;
    seed_success_deployment(
        &state,
        app_id,
        deployment_id,
        "hostlet-app-frontend",
        32000,
        serde_json::json!({
            "runtime": "generated_topology",
            "inferenceReceipt": {
                "services": [
                    {"name": "frontend", "role": "frontend", "healthProbe": {"kind": "http", "path": "/health"}},
                    {"name": "backend", "role": "backend", "healthProbe": {"kind": "http", "path": "/ready"}}
                ],
                "routing": {"backendPathPrefixes": ["/api", "/socket.io"]}
            },
            "routing": {"kind": "split", "backendPathPrefixes": ["/api", "/socket.io"]}
        }),
    )
    .await;
    insert_service(
        &state,
        deployment_id,
        app_id,
        "frontend",
        "hostlet-app-frontend",
        3000,
        32990,
    )
    .await;
    // Generated topology reports both routable services as role=web. The
    // inference receipt, not this vocabulary, identifies the backend.
    insert_service(
        &state,
        deployment_id,
        app_id,
        "backend",
        "hostlet-app-backend",
        4000,
        32991,
    )
    .await;

    let targets = health_target_response(&state).await;
    assert_eq!(targets.len(), 1);
    let target = &targets[0];
    assert_eq!(target["serviceName"], "frontend");
    assert_eq!(target["containerName"], "hostlet-app-frontend");
    assert_eq!(target["publishedPort"], 32000);
    assert_eq!(target["containerPort"], 3000);
    assert_eq!(target["healthPath"], "/health");
    assert_eq!(target["probeKind"], "http");
    assert_eq!(target["routeKey"], format!("app-{app_id}"));
    assert_eq!(target["routeGeneration"], 7);
    assert_eq!(
        target["splitRoute"]["backend"]["containerName"],
        "hostlet-app-backend"
    );
    assert_eq!(target["splitRoute"]["backend"]["targetPort"], 4000);
    assert_eq!(target["splitRoute"]["backend"]["publishedPort"], 32991);
    assert_eq!(
        target["splitRoute"]["backendPathPrefixes"],
        serde_json::json!(["/api", "/socket.io"])
    );

    sqlx::query(
        "DELETE FROM deployment_services
         WHERE deployment_id=$1 AND service_name='backend'",
    )
    .bind(deployment_id)
    .execute(&state.db)
    .await
    .unwrap();
    let incomplete = health_target_response(&state).await;
    assert_eq!(incomplete.len(), 1);
    assert!(incomplete[0]
        .get("splitRoute")
        .is_some_and(|value| value.is_null()));

    for malformed_metadata in [
        serde_json::json!({
            "runtime": "generated_topology",
            "routing": {"kind": "split", "backendPathPrefixes": ["/api"]}
        }),
        serde_json::json!({
            "runtime": "generated_topology",
            "inferenceReceipt": {
                "services": [
                    {"name": "frontend", "role": "frontend"},
                    {"name": "backend", "role": "api"}
                ],
                "routing": {"backendPathPrefixes": ["/api"]}
            },
            "routing": {"kind": "split", "backendPathPrefixes": ["/api"]}
        }),
        serde_json::json!({
            "runtime": "generated_topology",
            "inferenceReceipt": {
                "services": [
                    {"name": "frontend", "role": "frontend"},
                    {"name": "backend-a", "role": "backend"},
                    {"name": "backend-b", "role": "backend"}
                ],
                "routing": {"backendPathPrefixes": ["/api"]}
            },
            "routing": {"kind": "split", "backendPathPrefixes": ["/api"]}
        }),
        serde_json::json!({
            "runtime": "generated_topology",
            "inferenceReceipt": {
                "services": [
                    {"name": "frontend", "role": "frontend"},
                    {"name": "backend", "role": "backend"}
                ],
                "routing": {"backendPathPrefixes": ["/api"]}
            },
            "routing": {"kind": "single", "backendPathPrefixes": ["/api"]}
        }),
    ] {
        sqlx::query("UPDATE deployments SET runtime_metadata=$1 WHERE id=$2")
            .bind(malformed_metadata)
            .bind(deployment_id)
            .execute(&state.db)
            .await
            .unwrap();
        let malformed = health_target_response(&state).await;
        assert_eq!(malformed.len(), 1);
        assert!(malformed[0]
            .get("splitRoute")
            .is_some_and(|value| value.is_null()));
    }
}

#[tokio::test]
async fn db_health_targets_keep_legacy_deployment_without_services() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    super::reset_agent_db(&state).await;
    let user_id = super::insert_user(&state).await;
    let app_id = super::insert_app(&state, user_id).await;
    let deployment_id = super::insert_deployment(&state, app_id).await;
    seed_success_deployment(
        &state,
        app_id,
        deployment_id,
        "hostlet-app-legacy",
        32010,
        serde_json::json!({}),
    )
    .await;

    let targets = health_target_response(&state).await;
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0]["containerName"], "hostlet-app-legacy");
    assert_eq!(targets[0]["publishedPort"], 32010);
    assert!(targets[0].get("splitRoute").is_none());
}

#[tokio::test]
async fn db_health_targets_keep_backend_only_generated_topology() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    super::reset_agent_db(&state).await;
    let user_id = super::insert_user(&state).await;
    let app_id = super::insert_app(&state, user_id).await;
    let deployment_id = super::insert_deployment(&state, app_id).await;
    seed_success_deployment(
        &state,
        app_id,
        deployment_id,
        "hostlet-app-backend",
        32015,
        serde_json::json!({
            "runtime": "generated_topology",
            "inferenceReceipt": {
                "services": [
                    {"name": "backend", "role": "backend", "healthProbe": {"kind": "http", "path": "/ready"}}
                ],
                "routing": {"backendPathPrefixes": ["/api"]}
            },
            "routing": {"kind": "split", "backendPathPrefixes": ["/api"]}
        }),
    )
    .await;
    insert_service(
        &state,
        deployment_id,
        app_id,
        "backend",
        "hostlet-app-backend",
        4000,
        32015,
    )
    .await;

    let targets = health_target_response(&state).await;
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0]["serviceName"], "backend");
    assert_eq!(targets[0]["containerName"], "hostlet-app-backend");
    assert_eq!(targets[0]["containerPort"], 4000);
    assert_eq!(targets[0]["publishedPort"], 32015);
    assert_eq!(targets[0]["healthPath"], "/ready");
    assert_eq!(
        targets[0]["splitRoute"]["backend"]["containerName"],
        "hostlet-app-backend"
    );
    assert_eq!(targets[0]["splitRoute"]["backend"]["targetPort"], 4000);
    assert_eq!(targets[0]["splitRoute"]["backend"]["publishedPort"], 32015);

    sqlx::query(
        "UPDATE deployments
         SET runtime_metadata=$1
         WHERE id=$2",
    )
    .bind(serde_json::json!({
        "runtime": "generated_topology",
        "inferenceReceipt": {
            "services": [{"role": "backend"}],
            "routing": {"backendPathPrefixes": ["/api"]}
        },
        "routing": {"kind": "split", "backendPathPrefixes": ["/api"]}
    }))
    .bind(deployment_id)
    .execute(&state.db)
    .await
    .unwrap();
    let malformed = health_target_response(&state).await;
    assert_eq!(malformed.len(), 1);
    assert!(malformed[0]
        .get("splitRoute")
        .is_some_and(|value| value.is_null()));
}

#[tokio::test]
async fn db_split_health_ports_persist_only_to_current_backend_service() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    super::reset_agent_db(&state).await;
    let user_id = super::insert_user(&state).await;
    let app_id = super::insert_app(&state, user_id).await;
    let deployment_id = super::insert_deployment(&state, app_id).await;
    seed_success_deployment(
        &state,
        app_id,
        deployment_id,
        "hostlet-app-frontend",
        32020,
        serde_json::json!({
            "runtime": "generated_topology",
            "inferenceReceipt": {
                "services": [
                    {"name": "frontend", "role": "frontend"},
                    {"name": "backend", "role": "backend"}
                ]
            }
        }),
    )
    .await;
    insert_service(
        &state,
        deployment_id,
        app_id,
        "frontend",
        "hostlet-app-frontend",
        3000,
        32020,
    )
    .await;
    insert_service(
        &state,
        deployment_id,
        app_id,
        "backend",
        "hostlet-app-backend",
        4000,
        32021,
    )
    .await;
    insert_service(
        &state,
        deployment_id,
        app_id,
        "worker",
        "hostlet-app-worker",
        5000,
        32022,
    )
    .await;

    super::handle_agent_message(
        &state,
        super::TEST_SERVER_ID,
        serde_json::json!({
            "type": "health_status",
            "app_id": app_id,
            "deployment_id": deployment_id,
            "container_name": "hostlet-app-frontend",
            "published_port": 32100,
            "backend_container_name": "hostlet-app-backend",
            "backend_published_port": 32101,
            "status": "healthy",
            "http_status": 200,
            "latency_ms": 5
        }),
    )
    .await;
    assert_eq!(deployment_port(&state, deployment_id).await, Some(32100));
    assert_eq!(
        service_port(&state, deployment_id, "frontend").await,
        Some(32100)
    );
    assert_eq!(
        service_port(&state, deployment_id, "backend").await,
        Some(32101)
    );
    assert_eq!(
        service_port(&state, deployment_id, "worker").await,
        Some(32022)
    );

    // A valid managed name is not enough to redirect the backend update to a
    // sibling service: it must be the service named by the backend inference
    // receipt for this exact deployment.
    super::handle_agent_message(
        &state,
        super::TEST_SERVER_ID,
        serde_json::json!({
            "type": "health_status",
            "app_id": app_id,
            "deployment_id": deployment_id,
            "container_name": "hostlet-app-frontend",
            "published_port": 32102,
            "backend_container_name": "hostlet-app-worker",
            "backend_published_port": 32999,
            "status": "healthy"
        }),
    )
    .await;
    assert_eq!(deployment_port(&state, deployment_id).await, Some(32102));
    assert_eq!(
        service_port(&state, deployment_id, "frontend").await,
        Some(32102)
    );
    assert_eq!(
        service_port(&state, deployment_id, "backend").await,
        Some(32101)
    );
    assert_eq!(
        service_port(&state, deployment_id, "worker").await,
        Some(32022)
    );

    let stale_deployment_id = super::insert_deployment(&state, app_id).await;
    sqlx::query(
        "UPDATE deployments
         SET status='success',container_name='hostlet-app-stale',published_port=32990
         WHERE id=$1",
    )
    .bind(stale_deployment_id)
    .execute(&state.db)
    .await
    .unwrap();
    let events_before = health_event_count(&state, app_id).await;
    super::handle_agent_message(
        &state,
        super::TEST_SERVER_ID,
        serde_json::json!({
            "type": "health_status",
            "app_id": app_id,
            "deployment_id": stale_deployment_id,
            "container_name": "hostlet-app-stale",
            "published_port": 32991,
            "status": "unhealthy",
            "error": "late stale event"
        }),
    )
    .await;
    assert_eq!(
        health_snapshot(&state, app_id).await,
        (Some(deployment_id), "healthy".to_string())
    );
    assert_eq!(health_event_count(&state, app_id).await, events_before);
    assert_eq!(
        deployment_port(&state, stale_deployment_id).await,
        Some(32990)
    );
}

#[tokio::test]
async fn db_health_history_serializes_before_current_deployment_switch() {
    const HISTORY_LOCK: i64 = 918_273_645;
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    super::reset_agent_db(&state).await;
    let user_id = super::insert_user(&state).await;
    let app_id = super::insert_app(&state, user_id).await;
    let deployment_id = super::insert_deployment(&state, app_id).await;
    seed_success_deployment(
        &state,
        app_id,
        deployment_id,
        "hostlet-app-current",
        32030,
        serde_json::json!({}),
    )
    .await;
    let next_deployment_id = super::insert_deployment(&state, app_id).await;
    sqlx::query(
        "UPDATE deployments
         SET status='success',container_name='hostlet-app-next',published_port=32031
         WHERE id=$1",
    )
    .bind(next_deployment_id)
    .execute(&state.db)
    .await
    .unwrap();

    sqlx::query("DROP TRIGGER IF EXISTS hostlet_test_block_health_history ON app_health_events")
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION IF EXISTS hostlet_test_block_health_history()")
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query(
        "CREATE FUNCTION hostlet_test_block_health_history()
         RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
           PERFORM pg_advisory_xact_lock(918273645);
           RETURN NEW;
         END
         $$",
    )
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER hostlet_test_block_health_history
         BEFORE INSERT ON app_health_events
         FOR EACH ROW EXECUTE FUNCTION hostlet_test_block_health_history()",
    )
    .execute(&state.db)
    .await
    .unwrap();

    let mut blocker = state.db.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(HISTORY_LOCK)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let event_state = state.clone();
    let event = tokio::spawn(async move {
        super::handle_agent_message(
            &event_state,
            super::TEST_SERVER_ID,
            serde_json::json!({
                "type": "health_status",
                "app_id": app_id,
                "deployment_id": deployment_id,
                "container_name": "hostlet-app-current",
                "published_port": 32030,
                "status": "healthy"
            }),
        )
        .await;
    });

    let mut history_waiting = false;
    for _ in 0..100 {
        history_waiting = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
               SELECT 1 FROM pg_locks
               WHERE locktype='advisory' AND NOT granted
             )",
        )
        .fetch_one(&state.db)
        .await
        .unwrap();
        if history_waiting {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let switch_state = state.clone();
    let mut deployment_switch = tokio::spawn(async move {
        sqlx::query("UPDATE apps SET current_deployment_id=$1 WHERE id=$2")
            .bind(next_deployment_id)
            .bind(app_id)
            .execute(&switch_state.db)
            .await
    });
    let switch_was_blocked = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        &mut deployment_switch,
    )
    .await
    .is_err();

    let blocker_released = blocker.commit().await.is_ok();
    event.await.unwrap();
    if switch_was_blocked {
        deployment_switch.await.unwrap().unwrap();
    }
    sqlx::query("DROP TRIGGER hostlet_test_block_health_history ON app_health_events")
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION hostlet_test_block_health_history()")
        .execute(&state.db)
        .await
        .unwrap();

    let current: Option<Uuid> =
        sqlx::query_scalar("SELECT current_deployment_id FROM apps WHERE id=$1")
            .bind(app_id)
            .fetch_one(&state.db)
            .await
            .unwrap();
    let history_deployment: Option<Uuid> = sqlx::query_scalar(
        "SELECT deployment_id FROM app_health_events WHERE app_id=$1 ORDER BY id DESC LIMIT 1",
    )
    .bind(app_id)
    .fetch_optional(&state.db)
    .await
    .unwrap()
    .flatten();
    assert!(
        history_waiting,
        "health history insert never reached the lock"
    );
    assert!(
        switch_was_blocked,
        "deployment switch bypassed the app-row lock"
    );
    assert!(blocker_released);
    assert_eq!(current, Some(next_deployment_id));
    assert_eq!(history_deployment, Some(deployment_id));
}

async fn health_target_response(state: &AppState) -> Vec<serde_json::Value> {
    let response = crate::agent::health_targets(
        State(state.clone()),
        super::agent_headers(state, super::TEST_SERVER_ID),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn seed_success_deployment(
    state: &AppState,
    app_id: Uuid,
    deployment_id: Uuid,
    container_name: &str,
    published_port: i32,
    runtime_metadata: serde_json::Value,
) {
    sqlx::query(
        "UPDATE deployments
         SET status='success', container_name=$1, published_port=$2, runtime_metadata=$3
         WHERE id=$4 AND app_id=$5 AND server_id=$6",
    )
    .bind(container_name)
    .bind(published_port)
    .bind(runtime_metadata)
    .bind(deployment_id)
    .bind(app_id)
    .bind(super::TEST_SERVER_ID)
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query("UPDATE apps SET current_deployment_id=$1, route_generation=7 WHERE id=$2")
        .bind(deployment_id)
        .bind(app_id)
        .execute(&state.db)
        .await
        .unwrap();
}

async fn insert_service(
    state: &AppState,
    deployment_id: Uuid,
    app_id: Uuid,
    name: &str,
    container_name: &str,
    target_port: i32,
    published_port: i32,
) {
    sqlx::query(
        "INSERT INTO deployment_services
           (deployment_id,app_id,service_name,role,container_name,target_port,published_port)
         VALUES ($1,$2,$3,'web',$4,$5,$6)",
    )
    .bind(deployment_id)
    .bind(app_id)
    .bind(name)
    .bind(container_name)
    .bind(target_port)
    .bind(published_port)
    .execute(&state.db)
    .await
    .unwrap();
}

async fn deployment_port(state: &AppState, deployment_id: Uuid) -> Option<i32> {
    sqlx::query_scalar("SELECT published_port FROM deployments WHERE id=$1")
        .bind(deployment_id)
        .fetch_optional(&state.db)
        .await
        .unwrap()
        .flatten()
}

async fn service_port(state: &AppState, deployment_id: Uuid, name: &str) -> Option<i32> {
    sqlx::query_scalar(
        "SELECT published_port FROM deployment_services
         WHERE deployment_id=$1 AND service_name=$2",
    )
    .bind(deployment_id)
    .bind(name)
    .fetch_optional(&state.db)
    .await
    .unwrap()
}

async fn health_snapshot(state: &AppState, app_id: Uuid) -> (Option<Uuid>, String) {
    sqlx::query_as("SELECT deployment_id,status FROM app_health_snapshots WHERE app_id=$1")
        .bind(app_id)
        .fetch_one(&state.db)
        .await
        .unwrap()
}

async fn health_event_count(state: &AppState, app_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*)::bigint FROM app_health_events WHERE app_id=$1")
        .bind(app_id)
        .fetch_one(&state.db)
        .await
        .unwrap()
}
