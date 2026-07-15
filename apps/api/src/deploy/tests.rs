use super::{
    deployment_queue_status, is_active_deployment_status, required_protocol_version,
    rollback_supported_for_runtime, route_key, storage_over_quota_error, StorageScope,
};
use crate::state::AppState;
use uuid::Uuid;

const TEST_SERVER_ID: Uuid = Uuid::from_u128(1);

#[test]
fn route_key_is_app_prefixed_id() {
    let app_id = Uuid::parse_str("00000000-0000-0000-0000-0000000000ab").unwrap();
    assert_eq!(
        route_key(app_id),
        "app-00000000-0000-0000-0000-0000000000ab"
    );
}

#[test]
fn active_statuses_match_deploy_lifecycle() {
    for status in [
        "queued",
        "running",
        "building",
        "starting",
        "health_checking",
        "routing",
    ] {
        assert!(is_active_deployment_status(status));
    }
    for status in ["success", "failed", "rolled_back", "canceled"] {
        assert!(!is_active_deployment_status(status));
    }
}

#[test]
fn blue_green_runtimes_support_rollback() {
    assert!(rollback_supported_for_runtime("single"));
    assert!(rollback_supported_for_runtime("compose"));
    assert!(!rollback_supported_for_runtime("unknown"));
}

#[test]
fn inferred_topology_jobs_require_protocol_v3() {
    assert_eq!(
        required_protocol_version(
            "deploy",
            &serde_json::json!({
                "type": "deploy",
                "runtime_config": {"generatedTopology": {"schemaVersion": 1, "mode": "auto"}}
            })
        ),
        3
    );
    assert_eq!(
        required_protocol_version(
            "rollback",
            &serde_json::json!({
                "type": "rollback",
                "target_runtime_metadata": {"runtime": "generated_topology", "inferenceReceipt": {"schemaVersion": 1}}
            })
        ),
        3
    );
    assert_eq!(
        required_protocol_version("deploy", &serde_json::json!({"type": "deploy"})),
        2
    );
}

#[test]
fn storage_quota_returns_none_when_under_limit() {
    // Under-limit returns None for both scopes; used == limit - 1 is still ok.
    let limit = 512_i64 * 1024 * 1024;
    assert_eq!(
        storage_over_quota_error(limit - 1, limit, StorageScope::PerApp),
        None
    );
    assert_eq!(
        storage_over_quota_error(limit - 1, limit, StorageScope::Account),
        None
    );
}

#[test]
fn storage_quota_per_app_error_at_or_over_limit() {
    // used == limit triggers the gate; message mentions limit in MB and "This app".
    let limit = 512_i64 * 1024 * 1024;
    let msg = storage_over_quota_error(limit, limit, StorageScope::PerApp)
        .expect("used == limit should produce an error");
    assert!(msg.contains("512 MB"), "limit in message: {msg}");
    assert!(msg.contains("This app"), "per-app scope in message: {msg}");
}

#[test]
fn storage_quota_account_error_over_limit() {
    // Account-scope message mentions limit in MB and prompts plan upgrade.
    let limit = 4096_i64 * 1024 * 1024;
    let msg = storage_over_quota_error(limit + 1, limit, StorageScope::Account)
        .expect("used > limit should produce an error");
    assert!(msg.contains("4096 MB"), "limit in message: {msg}");
    assert!(
        msg.contains("Your projects"),
        "account scope in message: {msg}"
    );
    assert!(
        msg.contains("upgrade your plan"),
        "upgrade hint in message: {msg}"
    );
}

#[tokio::test]
async fn db_deployment_queue_reports_deploys_ahead() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_deploy_db(&state).await;
    let user_id = insert_user(&state).await;
    let first_app_id = insert_app(&state, user_id, "queue-first").await;
    let target_app_id = insert_app(&state, user_id, "queue-target").await;
    let first_deployment_id = insert_deployment(&state, first_app_id, "running").await;
    let target_deployment_id = insert_deployment(&state, target_app_id, "running").await;
    insert_deploy_job(
        &state,
        first_app_id,
        first_deployment_id,
        "queued",
        "2 minutes",
    )
    .await;
    insert_deploy_job(
        &state,
        target_app_id,
        target_deployment_id,
        "queued",
        "1 minute",
    )
    .await;

    let queue =
        deployment_queue_status(&state, target_deployment_id, TEST_SERVER_ID, "running").await;

    assert_eq!(queue.status, "queued");
    assert_eq!(queue.deploys_ahead, 1);
    assert_eq!(queue.position, Some(2));
    assert!(queue.updated_at.is_some());
}

#[tokio::test]
async fn db_deployment_queue_without_job_falls_back_to_status() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_deploy_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id, "queue-no-job").await;
    let deployment_id = insert_deployment(&state, app_id, "building").await;

    let queue = deployment_queue_status(&state, deployment_id, TEST_SERVER_ID, "building").await;

    assert_eq!(queue.status, "building");
    assert_eq!(queue.deploys_ahead, 0);
    assert_eq!(queue.position, None);
    assert_eq!(queue.updated_at, None);
}

async fn reset_deploy_db(state: &AppState) {
    sqlx::query(
            "TRUNCATE app_screenshots, app_resource_snapshots, agent_jobs, deployments, app_env_vars, apps CASCADE",
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
        "INSERT INTO users (github_id, login) VALUES (9601,'deploy-queue-user') RETURNING id",
    )
    .fetch_one(&state.db)
    .await
    .unwrap()
}

async fn insert_app(state: &AppState, user_id: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO apps
               (user_id,server_id,name,repo_full_name,branch,container_port,health_path,domain,runtime_kind,root_directory,public_exposure,auto_deploy)
             VALUES ($1,$2,$3,'hostlet-ci/node-hello','main',3000,'/health',$4,'single','.',true,false)
             RETURNING id",
        )
        .bind(user_id)
        .bind(TEST_SERVER_ID)
        .bind(name)
        .bind(format!("{name}.example.test"))
        .fetch_one(&state.db)
        .await
        .unwrap()
}

async fn insert_deployment(state: &AppState, app_id: Uuid, status: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO deployments (app_id,server_id,status,commit_sha,started_at,runtime_kind)
             VALUES ($1,$2,$3,'HEAD',now(),'single')
             RETURNING id",
    )
    .bind(app_id)
    .bind(TEST_SERVER_ID)
    .bind(status)
    .fetch_one(&state.db)
    .await
    .unwrap()
}

async fn insert_deploy_job(
    state: &AppState,
    app_id: Uuid,
    deployment_id: Uuid,
    status: &str,
    age: &str,
) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO agent_jobs
               (server_id,app_id,deployment_id,job_type,status,payload_json,created_at,updated_at)
             VALUES ($1,$2,$3,'deploy',$4,'{\"type\":\"deploy\"}'::jsonb,now() - $5::interval,now() - $5::interval)
             RETURNING id",
        )
        .bind(TEST_SERVER_ID)
        .bind(app_id)
        .bind(deployment_id)
        .bind(status)
        .bind(age)
        .fetch_one(&state.db)
        .await
        .unwrap()
}
