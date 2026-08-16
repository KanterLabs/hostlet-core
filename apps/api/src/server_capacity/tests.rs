use super::*;
use axum::response::IntoResponse;

#[test]
fn default_runtime_demand_reserves_less_than_the_container_hard_cap() {
    let demand = runtime_demand(&serde_json::json!({"memory_limit_mb": 512}));

    assert_eq!(demand.reserved_memory_mb, 128);
    assert_eq!(demand.services, 1);
}

#[test]
fn runtime_demand_adds_headroom_for_inferred_services() {
    let demand = runtime_demand(&serde_json::json!({
        "compose": {"addOns": [{}, {}]},
        "generatedTopology": {}
    }));

    assert_eq!(demand.reserved_memory_mb, 416);
    assert_eq!(demand.services, 4);
}

#[test]
fn runtime_demand_honors_explicit_capacity_reservations() {
    let demand = runtime_demand(&serde_json::json!({
        "capacity": {
            "reservedMemoryMb": 64,
            "reservedServiceCount": 3
        },
        "compose": {"addOns": [{}, {}]}
    }));

    assert_eq!(demand.reserved_memory_mb, 64);
    assert_eq!(demand.services, 3);
}

#[tokio::test]
async fn db_select_app_runner_rejects_draining_server() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_capacity_db(&state).await;
    sqlx::query("UPDATE servers SET draining=true WHERE id=$1")
        .bind(state.local_server_id)
        .execute(&state.db)
        .await
        .unwrap();

    let err = select_app_runner(&state, Some(state.local_server_id))
        .await
        .unwrap_err();

    assert!(matches!(err, ServerSelectionError::Draining));
}

#[tokio::test]
async fn db_select_app_runner_chooses_least_loaded_runner() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_capacity_db(&state).await;
    let user_id = insert_capacity_user(&state).await;
    let busy_server_id = state.local_server_id;
    let idle_server_id = insert_server(&state, user_id, "idle-runner").await;
    insert_active_app(&state, user_id, busy_server_id, "busy-app").await;

    let selected = select_app_runner(&state, None).await.unwrap();

    assert_eq!(selected, idle_server_id);
}

#[tokio::test]
async fn db_select_app_runner_rejects_remote_or_offline_server() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_capacity_db(&state).await;
    let user_id = insert_capacity_user(&state).await;
    let remote_id =
        insert_server_with_kind_status(&state, user_id, "remote-runner", "remote", "online").await;
    let offline_id =
        insert_server_with_kind_status(&state, user_id, "offline-runner", "local", "offline").await;

    assert!(matches!(
        select_app_runner(&state, Some(remote_id))
            .await
            .unwrap_err(),
        ServerSelectionError::NotFound
    ));
    assert!(matches!(
        select_app_runner(&state, Some(offline_id))
            .await
            .unwrap_err(),
        ServerSelectionError::NotFound
    ));
}

#[tokio::test]
async fn db_select_app_runner_rejects_full_requested_server() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_capacity_db(&state).await;
    let user_id = insert_capacity_user(&state).await;
    sqlx::query("UPDATE servers SET max_concurrent_apps=1 WHERE id=$1")
        .bind(state.local_server_id)
        .execute(&state.db)
        .await
        .unwrap();
    insert_active_app(&state, user_id, state.local_server_id, "full-app").await;

    let err = select_app_runner(&state, Some(state.local_server_id))
        .await
        .unwrap_err();

    assert!(matches!(err, ServerSelectionError::Full));
}

#[tokio::test]
async fn db_select_app_runner_counts_in_flight_deploy_job_as_occupied() {
    // An app with a queued deploy job but no current deployment must still
    // occupy a slot, so a one-slot server reads as full on both the requested
    // and the auto-selected path. This keeps placement in step with the
    // deploy-time re-check in `ensure_server_has_capacity`.
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_capacity_db(&state).await;
    let user_id = insert_capacity_user(&state).await;
    sqlx::query("UPDATE servers SET max_concurrent_apps=1 WHERE id=$1")
        .bind(state.local_server_id)
        .execute(&state.db)
        .await
        .unwrap();
    insert_inflight_app(&state, user_id, state.local_server_id, "inflight-app").await;

    let requested_err = select_app_runner(&state, Some(state.local_server_id))
        .await
        .unwrap_err();
    assert!(matches!(requested_err, ServerSelectionError::Full));

    let auto_err = select_app_runner(&state, None).await.unwrap_err();
    assert!(matches!(auto_err, ServerSelectionError::NoCapacity));
}

#[tokio::test]
async fn db_ensure_server_has_capacity_blocks_extra_app_when_slot_reserved() {
    // N apps created back-to-back on a one-slot server, none with a current
    // deployment, must not all be able to deploy: once one reserves the slot
    // with a queued deploy job the others are refused at enqueue time.
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_capacity_db(&state).await;
    let user_id = insert_capacity_user(&state).await;
    let server_id = state.local_server_id;
    sqlx::query("UPDATE servers SET max_concurrent_apps=1 WHERE id=$1")
        .bind(server_id)
        .execute(&state.db)
        .await
        .unwrap();
    let app_b = insert_idle_app(&state, user_id, server_id, "cap-b").await;

    // Nothing in flight yet: the single free slot accepts a deploy.
    ensure_server_has_capacity(&state, server_id, app_b)
        .await
        .expect("a free one-slot server should accept a deploy");

    // Another app reserves the slot with a queued deploy job (still no current
    // deployment) — exactly the state that used to slip past capacity checks.
    let app_a = insert_inflight_app(&state, user_id, server_id, "cap-a").await;

    // The extra app is now refused: the reserved slot fills the server even
    // though no app has a current deployment.
    let err = ensure_server_has_capacity(&state, server_id, app_b)
        .await
        .expect_err("second app must be blocked when the slot is reserved");
    assert!(
        err.to_string().contains("at capacity"),
        "unexpected error: {err}"
    );

    // The reserving app is excluded from its own count, so it can still deploy
    // into the slot it holds (a redeploy must not be self-blocked).
    ensure_server_has_capacity(&state, server_id, app_a)
        .await
        .expect("an app must not be blocked by its own in-flight deploy job");
}

#[tokio::test]
async fn db_build_completion_admits_one_release_and_waits_for_the_next() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_capacity_db(&state).await;
    let user_id = insert_capacity_user(&state).await;
    let server_id = state.local_server_id;
    sqlx::query(
        "UPDATE servers
         SET max_concurrent_apps=1,max_concurrent_builds=8
         WHERE id=$1",
    )
    .bind(server_id)
    .execute(&state.db)
    .await
    .unwrap();
    let app_a = insert_idle_app(&state, user_id, server_id, "release-a").await;
    let app_b = insert_idle_app(&state, user_id, server_id, "release-b").await;
    let deployment_a = insert_building_deployment(&state, app_a, server_id).await;
    let deployment_b = insert_building_deployment(&state, app_b, server_id).await;

    // `scripts/ci-db-tests.sh` runs the destructive DB inventory with one test
    // thread, which makes this process-local registry fixture deterministic.
    let old_user = std::env::var("HOSTLET_ARTIFACT_REGISTRY_PULL_USERNAME").ok();
    let old_password = std::env::var("HOSTLET_ARTIFACT_REGISTRY_PULL_PASSWORD").ok();
    std::env::set_var(
        "HOSTLET_ARTIFACT_REGISTRY_PULL_USERNAME",
        "capacity-test-user",
    );
    std::env::set_var(
        "HOSTLET_ARTIFACT_REGISTRY_PULL_PASSWORD",
        "capacity-test-password",
    );
    let mut completion_inputs = Vec::new();
    for (app_id, deployment_id) in [(app_a, deployment_a), (app_b, deployment_b)] {
        let build_id: Uuid = sqlx::query_scalar(
            "INSERT INTO deployment_builds
               (deployment_id,build_pool_id,status,required_platform,build_spec_json)
             VALUES ($1,'00000000-0000-0000-0000-000000000010','building','linux/amd64','{}')
             RETURNING id",
        )
        .bind(deployment_id)
        .fetch_one(&state.db)
        .await
        .unwrap();
        let payload = serde_json::json!({
            "type": "build",
            "build_id": build_id,
            "deployment_id": deployment_id,
            "app_id": app_id,
            "commit_sha": "0123456789012345678901234567890123456789",
            "required_platform": "linux/amd64",
            "artifact_registry": {
                "host": "registry.test",
                "repository": format!("hostlet/apps/{app_id}/artifacts")
            }
        });
        let manifest = serde_json::json!({
            "schemaVersion": 2,
            "buildId": build_id,
            "deploymentId": deployment_id,
            "appId": app_id,
            "commitSha": "0123456789012345678901234567890123456789",
            "platform": "linux/amd64",
            "buildPlanDigest": format!("sha256:{}", "b".repeat(64)),
            "index": {
                "digestRef": format!("registry.test/hostlet/apps/{app_id}/artifacts@sha256:{}", "c".repeat(64)),
                "mediaType": "application/vnd.oci.image.index.v1+json",
                "sizeBytes": 10
            },
            "bundle": {
                "digestRef": format!("registry.test/hostlet/apps/{app_id}/artifacts@sha256:{}", "d".repeat(64)),
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "sizeBytes": 10
            },
            "images": [{
                "service": "web",
                "role": "web",
                "source": "built",
                "artifact": {
                    "digestRef": format!("registry.test/hostlet/apps/{app_id}/artifacts@sha256:{}", "e".repeat(64)),
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "sizeBytes": 10
                }
            }]
        });
        completion_inputs.push((app_id, deployment_id, payload, manifest));
    }
    let completion_name_a = format!("core04-build-a-{}", Uuid::new_v4().simple());
    let completion_name_b = format!("core04-build-b-{}", Uuid::new_v4().simple());
    let completion_state_a = state_with_application_name(&completion_name_a).await;
    let completion_state_b = state_with_application_name(&completion_name_b).await;
    let mut server_blocker = state.db.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='2s'")
        .execute(&mut *server_blocker)
        .await
        .unwrap();
    sqlx::query("SET LOCAL statement_timeout='5s'")
        .execute(&mut *server_blocker)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM servers WHERE id=$1 FOR UPDATE")
        .bind(server_id)
        .fetch_one(&mut *server_blocker)
        .await
        .unwrap();

    let mut completion_inputs = completion_inputs.into_iter();
    let (app_a, deployment_a, payload_a, manifest_a) = completion_inputs.next().unwrap();
    let (app_b, deployment_b, payload_b, manifest_b) = completion_inputs.next().unwrap();
    let completion_a = tokio::spawn(async move {
        let mut tx = completion_state_a.db.begin().await?;
        sqlx::query("SET LOCAL lock_timeout='2s'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SET LOCAL statement_timeout='5s'")
            .execute(&mut *tx)
            .await?;
        crate::agent::build_execution::complete_successful_build(
            &completion_state_a,
            &mut tx,
            deployment_a,
            app_a,
            &payload_a,
            &manifest_a,
        )
        .await?;
        tx.commit().await?;
        Ok::<_, anyhow::Error>(())
    });
    let completion_b = tokio::spawn(async move {
        let mut tx = completion_state_b.db.begin().await?;
        sqlx::query("SET LOCAL lock_timeout='2s'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SET LOCAL statement_timeout='5s'")
            .execute(&mut *tx)
            .await?;
        crate::agent::build_execution::complete_successful_build(
            &completion_state_b,
            &mut tx,
            deployment_b,
            app_b,
            &payload_b,
            &manifest_b,
        )
        .await?;
        tx.commit().await?;
        Ok::<_, anyhow::Error>(())
    });
    let waiters = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*)
                 FROM pg_stat_activity
                 WHERE application_name IN ($1,$2)
                   AND wait_event_type='Lock'
                   AND query LIKE '%SELECT id FROM servers WHERE id=$1 FOR UPDATE%'",
            )
            .bind(&completion_name_a)
            .bind(&completion_name_b)
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
    .expect("both build completions must contend on the server lock");
    assert!(waiters >= 2);
    server_blocker.commit().await.unwrap();
    let (completion_a, completion_b) = tokio::try_join!(completion_a, completion_b)
        .expect("concurrent build completion tasks must not deadlock");
    completion_a.unwrap();
    completion_b.unwrap();
    if let Some(value) = old_user {
        std::env::set_var("HOSTLET_ARTIFACT_REGISTRY_PULL_USERNAME", value);
    } else {
        std::env::remove_var("HOSTLET_ARTIFACT_REGISTRY_PULL_USERNAME");
    }
    if let Some(value) = old_password {
        std::env::set_var("HOSTLET_ARTIFACT_REGISTRY_PULL_PASSWORD", value);
    } else {
        std::env::remove_var("HOSTLET_ARTIFACT_REGISTRY_PULL_PASSWORD");
    }

    let first_payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload_json FROM agent_jobs WHERE deployment_id=$1 AND job_type='release'",
    )
    .bind(deployment_a)
    .fetch_one(&state.db)
    .await
    .unwrap();
    let second_payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload_json FROM agent_jobs WHERE deployment_id=$1 AND job_type='release'",
    )
    .bind(deployment_b)
    .fetch_one(&state.db)
    .await
    .unwrap();
    let first_reserved = first_payload["capacity_reserved"] == true;
    let second_reserved = second_payload["capacity_reserved"] == true;
    assert_ne!(first_reserved, second_reserved);
    let (admitted_app, admitted_deployment, waiting_deployment, waiting_payload) = if first_reserved
    {
        (app_a, deployment_a, deployment_b, second_payload)
    } else {
        (app_b, deployment_b, deployment_a, first_payload)
    };
    assert_eq!(waiting_payload["capacity_wait"], true);
    assert_eq!(waiting_payload["capacity_wait_reason"], "app_slots");
    let queue = crate::deploy::deployment_queue_status(
        &state,
        waiting_deployment,
        server_id,
        "queued_for_release",
    )
    .await;
    assert_eq!(queue.status, "waiting_capacity");

    // The concurrent build completions created exactly one claimable release.
    // Drive that release through the real prepare/commit authority and prove
    // the second release remains capacity-waiting instead of merely checking
    // reservation JSON.
    let first_claim = Uuid::new_v4();
    let first_job: Uuid = sqlx::query_scalar(
        "SELECT id FROM agent_jobs
         WHERE deployment_id=$1 AND job_type='release'",
    )
    .bind(admitted_deployment)
    .fetch_one(&state.db)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE agent_jobs
         SET status='running',claim_token=$2,
             lease_expires_at=clock_timestamp()+interval '5 minutes'
         WHERE id=$1",
    )
    .bind(first_job)
    .bind(first_claim)
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query("UPDATE apps SET pending_deployment_id=$1,route_generation=1 WHERE id=$2")
        .bind(admitted_deployment)
        .bind(admitted_app)
        .execute(&state.db)
        .await
        .unwrap();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "x-hostlet-server-id",
        server_id.to_string().parse().unwrap(),
    );
    headers.insert(
        "x-hostlet-agent-token",
        std::env::var("LOCAL_AGENT_TOKEN")
            .unwrap_or_else(|_| "ci-test-local-agent-token-value-000001".into())
            .parse()
            .unwrap(),
    );
    let candidate = hostlet_contracts::CandidateRuntime {
        container_name: format!("hostlet-app-{admitted_app}"),
        published_port: 32_001,
        image_tag: Some("example:test".into()),
        compose_project: None,
        runtime_metadata: serde_json::json!({}),
        services: Vec::new(),
    };
    let prepared = crate::deployment_execution::prepare_activation(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::extract::Path(admitted_deployment),
        axum::Json(hostlet_contracts::PrepareActivationRequest {
            job_id: first_job,
            claim_token: first_claim,
            expected_current_deployment_id: None,
            candidate,
        }),
    )
    .await
    .into_response();
    assert_eq!(prepared.status(), axum::http::StatusCode::OK);
    let committed = crate::deployment_execution::commit_activation(
        axum::extract::State(state.clone()),
        headers,
        axum::extract::Path(admitted_deployment),
        axum::Json(hostlet_contracts::CommitActivationRequest {
            job_id: first_job,
            claim_token: first_claim,
            route_generation: 1,
            local_url: None,
            runtime_metadata: None,
            rolled_back: false,
        }),
    )
    .await
    .into_response();
    assert_eq!(committed.status(), axum::http::StatusCode::NO_CONTENT);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM agent_jobs WHERE deployment_id=$1")
            .bind(waiting_deployment)
            .fetch_one(&state.db)
            .await
            .unwrap(),
        "queued"
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

async fn reset_capacity_db(state: &AppState) {
    sqlx::query(
        "TRUNCATE app_screenshots, app_resource_snapshots, agent_jobs, deployments, app_env_vars, apps CASCADE",
    )
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query("DELETE FROM users WHERE github_id BETWEEN 9700 AND 9799")
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query(
        "DELETE FROM servers
         WHERE id <> $1
           AND name LIKE 'capacity-%'",
    )
    .bind(state.local_server_id)
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE servers
         SET kind='local',
             status='online',
             capabilities=ARRAY['builder','app_runner']::TEXT[],
             draining=false,
             max_concurrent_apps=8,
             max_concurrent_builds=1
         WHERE id=$1",
    )
    .bind(state.local_server_id)
    .execute(&state.db)
    .await
    .unwrap();
}

async fn insert_capacity_user(state: &AppState) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (github_id, login)
         VALUES (9701, 'capacity-user')
         ON CONFLICT (github_id) DO UPDATE SET login=EXCLUDED.login
         RETURNING id",
    )
    .fetch_one(&state.db)
    .await
    .unwrap()
}

async fn insert_server(state: &AppState, user_id: Uuid, name: &str) -> Uuid {
    insert_server_with_kind_status(state, user_id, name, "local", "online").await
}

async fn insert_server_with_kind_status(
    state: &AppState,
    user_id: Uuid,
    name: &str,
    kind: &str,
    status: &str,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO servers
           (user_id,name,kind,status,capabilities,draining,max_concurrent_apps,max_concurrent_builds)
         VALUES ($1,$2,$3,$4,ARRAY['app_runner']::TEXT[],false,8,1)
         RETURNING id",
    )
    .bind(user_id)
    .bind(format!("capacity-{name}"))
    .bind(kind)
    .bind(status)
    .fetch_one(&state.db)
    .await
    .unwrap()
}

/// Inserts an app with no current deployment and no deploy job — the state an
/// app sits in between creation and its first deploy.
async fn insert_idle_app(state: &AppState, user_id: Uuid, server_id: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO apps
           (user_id,server_id,name,repo_full_name,branch,container_port,health_path,domain,runtime_kind,root_directory,public_exposure,auto_deploy)
         VALUES ($1,$2,$3,'owner/repo','main',3000,'/',$4,'container','.',false,false)
         RETURNING id",
    )
    .bind(user_id)
    .bind(server_id)
    .bind(name)
    .bind(format!("{name}.example.test"))
    .fetch_one(&state.db)
    .await
    .unwrap()
}

async fn insert_building_deployment(state: &AppState, app_id: Uuid, server_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO deployments
           (app_id,server_id,status,commit_sha,started_at,runtime_kind)
         VALUES ($1,$2,'building','0123456789012345678901234567890123456789',now(),'container')
         RETURNING id",
    )
    .bind(app_id)
    .bind(server_id)
    .fetch_one(&state.db)
    .await
    .unwrap()
}

/// Inserts an app with an in-flight (`queued`) deploy job but *no* current
/// deployment — the state that used to slip past the capacity count.
async fn insert_inflight_app(state: &AppState, user_id: Uuid, server_id: Uuid, name: &str) -> Uuid {
    let app_id = insert_idle_app(state, user_id, server_id, name).await;
    insert_inflight_job_for_app(state, app_id, server_id, "deploy").await;
    app_id
}

async fn insert_inflight_job_for_app(
    state: &AppState,
    app_id: Uuid,
    server_id: Uuid,
    job_type: &str,
) -> Uuid {
    sqlx::query(
        "INSERT INTO agent_jobs (server_id,app_id,job_type,status,payload_json)
         VALUES ($1,$2,$3,'queued','{\"capacity_reserved\":true}'::jsonb)",
    )
    .bind(server_id)
    .bind(app_id)
    .bind(job_type)
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query_scalar(
        "SELECT id FROM agent_jobs WHERE app_id=$1 AND job_type=$2 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(app_id)
    .bind(job_type)
    .fetch_one(&state.db)
    .await
    .unwrap()
}

async fn insert_active_app(state: &AppState, user_id: Uuid, server_id: Uuid, name: &str) {
    let app_id: Uuid = sqlx::query_scalar(
        "INSERT INTO apps
           (user_id,server_id,name,repo_full_name,branch,container_port,health_path,domain,runtime_kind,root_directory,public_exposure,auto_deploy,current_deployment_id)
         VALUES ($1,$2,$3,'owner/repo','main',3000,'/',$4,'container','.',false,false,'00000000-0000-0000-0000-000000000001')
         RETURNING id",
    )
    .bind(user_id)
    .bind(server_id)
    .bind(name)
    .bind(format!("{name}.example.test"))
    .fetch_one(&state.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO deployments
           (id,app_id,server_id,status,commit_sha,started_at,finished_at,runtime_kind)
         VALUES ('00000000-0000-0000-0000-000000000001',$1,$2,'success','abc',now(),now(),'container')",
    )
    .bind(app_id)
    .bind(server_id)
    .execute(&state.db)
    .await
    .unwrap();
}
