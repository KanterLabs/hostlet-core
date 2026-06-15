use super::*;

// Pure-function tests for env parsers ----------------------------------------

#[test]
fn cleanup_keep_previous_value_accepts_valid_range() {
    assert_eq!(cleanup_keep_previous_value("1"), Some(1));
    assert_eq!(cleanup_keep_previous_value(" 3 "), Some(3));
    assert_eq!(cleanup_keep_previous_value("100"), Some(100));
}

#[test]
fn cleanup_keep_previous_value_rejects_zero_empty_nonnumeric_over_100() {
    assert_eq!(cleanup_keep_previous_value("0"), None);
    assert_eq!(cleanup_keep_previous_value(""), None);
    assert_eq!(cleanup_keep_previous_value("soon"), None);
    assert_eq!(cleanup_keep_previous_value("101"), None);
    assert_eq!(cleanup_keep_previous_value("-1"), None);
}

#[test]
fn auto_cleanup_enabled_value_false_for_falsy() {
    assert!(!auto_cleanup_enabled_value("0"));
    assert!(!auto_cleanup_enabled_value("false"));
    assert!(!auto_cleanup_enabled_value("no"));
    assert!(!auto_cleanup_enabled_value("FALSE"));
    assert!(!auto_cleanup_enabled_value(" no "));
}

#[test]
fn auto_cleanup_enabled_value_true_for_truthy_and_unknown() {
    assert!(auto_cleanup_enabled_value("1"));
    assert!(auto_cleanup_enabled_value("true"));
    assert!(auto_cleanup_enabled_value("yes"));
    assert!(auto_cleanup_enabled_value("anything"));
}

#[test]
fn cleanup_keep_failed_hours_value_accepts_zero_through_720() {
    assert_eq!(cleanup_keep_failed_hours_value("0"), Some(0));
    assert_eq!(cleanup_keep_failed_hours_value(" 24 "), Some(24));
    assert_eq!(cleanup_keep_failed_hours_value("720"), Some(720));
}

#[test]
fn cleanup_keep_failed_hours_value_rejects_negative_empty_nonnumeric_over_720() {
    assert_eq!(cleanup_keep_failed_hours_value("-1"), None);
    assert_eq!(cleanup_keep_failed_hours_value("721"), None);
    assert_eq!(cleanup_keep_failed_hours_value(""), None);
    assert_eq!(cleanup_keep_failed_hours_value("soon"), None);
}

// DB-gated helpers -----------------------------------------------------------

async fn reset_cleanup_db(state: &AppState) {
    sqlx::query(
        "TRUNCATE deployment_logs, app_health_events, app_health_snapshots, \
             app_resource_snapshots, agent_jobs, deployments, app_env_vars, apps, users CASCADE",
    )
    .execute(&state.db)
    .await
    .unwrap();
}

pub(super) async fn insert_cleanup_user(state: &AppState) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (github_id, login) VALUES ($1,'cleanup-user') RETURNING id",
    )
    .bind(9_090_001_i64)
    .fetch_one(&state.db)
    .await
    .unwrap()
}

async fn insert_cleanup_app(state: &AppState, user_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO apps
               (user_id,server_id,name,repo_full_name,branch,container_port,health_path,\
                domain,runtime_kind,root_directory,public_exposure,auto_deploy)
             VALUES ($1,$2,'cleanup-app','hostlet-ci/node-hello','main',3000,'/health',\
               'cleanup.example.test','single','.',true,false)
             RETURNING id",
    )
    .bind(user_id)
    .bind(state.local_server_id)
    .fetch_one(&state.db)
    .await
    .unwrap()
}

async fn insert_cleanup_deployment(
    state: &AppState,
    app_id: Uuid,
    status: &str,
    image_tag: &str,
    container_name: &str,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO deployments
               (app_id,server_id,status,commit_sha,image_tag,container_name,\
                started_at,finished_at,runtime_kind)
             VALUES ($1,$2,$3,'HEAD',$4,$5,now(),now(),'single')
             RETURNING id",
    )
    .bind(app_id)
    .bind(state.local_server_id)
    .bind(status)
    .bind(image_tag)
    .bind(container_name)
    .fetch_one(&state.db)
    .await
    .unwrap()
}

async fn insert_cleanup_deployment_with_meta(
    state: &AppState,
    app_id: Uuid,
    status: &str,
    image_tag: &str,
    container_name: &str,
    metadata: serde_json::Value,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO deployments
               (app_id,server_id,status,commit_sha,image_tag,container_name,\
                runtime_metadata,started_at,finished_at,runtime_kind)
             VALUES ($1,$2,$3,'HEAD',$4,$5,$6,now(),now(),'single')
             RETURNING id",
    )
    .bind(app_id)
    .bind(state.local_server_id)
    .bind(status)
    .bind(image_tag)
    .bind(container_name)
    .bind(metadata)
    .fetch_one(&state.db)
    .await
    .unwrap()
}

/// Same as `insert_cleanup_deployment` but with an explicit `finished_at`
/// offset (in minutes ago) so tests can establish a strict ordering without
/// relying on wall-clock timing.
async fn insert_cleanup_deployment_ago(
    state: &AppState,
    app_id: Uuid,
    status: &str,
    image_tag: &str,
    container_name: &str,
    minutes_ago: i32,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO deployments
               (app_id,server_id,status,commit_sha,image_tag,container_name,\
                started_at,finished_at,runtime_kind)
             VALUES ($1,$2,$3,'HEAD',$4,$5,now(),\
               now() - ($6 * interval '1 minute'),'single')
             RETURNING id",
    )
    .bind(app_id)
    .bind(state.local_server_id)
    .bind(status)
    .bind(image_tag)
    .bind(container_name)
    .bind(minutes_ago)
    .fetch_one(&state.db)
    .await
    .unwrap()
}

// DB-gated tests -------------------------------------------------------------

#[tokio::test]
async fn db_cleanup_plan_marks_failed_deployment_containers_stale() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_cleanup_db(&state).await;
    let user_id = insert_cleanup_user(&state).await;
    let app_id = insert_cleanup_app(&state, user_id).await;
    let current_deployment = insert_cleanup_deployment(
        &state,
        app_id,
        "success",
        "hostlet/app-current:latest",
        "hostlet-app-current",
    )
    .await;
    insert_cleanup_deployment(
        &state,
        app_id,
        "health_checking",
        "hostlet/app-active:latest",
        "hostlet-app-active",
    )
    .await;
    insert_cleanup_deployment(
        &state,
        app_id,
        "failed",
        "hostlet/app-failed:latest",
        "hostlet-app-failed",
    )
    .await;
    sqlx::query("UPDATE apps SET current_deployment_id=$1 WHERE id=$2")
        .bind(current_deployment)
        .bind(app_id)
        .execute(&state.db)
        .await
        .unwrap();

    let plan = cleanup_plan(&state).await.unwrap();

    assert_eq!(plan.docker.stale_deployment_containers, 1);
    assert_eq!(
        plan.keep_containers,
        vec!["hostlet-app-active", "hostlet-app-current"]
    );
    assert_eq!(
        plan.keep_images,
        vec!["hostlet/app-active:latest", "hostlet/app-current:latest"]
    );
    assert!(!plan.keep_containers.contains(&"hostlet-app-failed".into()));
    assert!(!plan
        .keep_images
        .contains(&"hostlet/app-failed:latest".into()));
}

#[tokio::test]
async fn db_cleanup_plan_keeps_runtime_metadata_image_refs() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_cleanup_db(&state).await;
    let user_id = insert_cleanup_user(&state).await;
    let app_id = insert_cleanup_app(&state, user_id).await;
    let current_deployment = insert_cleanup_deployment_with_meta(
        &state,
        app_id,
        "success",
        "hostlet/app-current:latest",
        "hostlet-app-current",
        serde_json::json!({
            "imageRef": "registry.example.test/images/app:sha",
            "buildArtifact": {
                "imageDigest": "registry.example.test/images/app@sha256:abc123"
            },
            "image_ref": "  ",
            "image_digest": "bad ref with whitespace"
        }),
    )
    .await;
    sqlx::query("UPDATE apps SET current_deployment_id=$1 WHERE id=$2")
        .bind(current_deployment)
        .bind(app_id)
        .execute(&state.db)
        .await
        .unwrap();

    let plan = cleanup_plan(&state).await.unwrap();

    assert!(plan
        .keep_images
        .contains(&"hostlet/app-current:latest".into()));
    assert!(plan
        .keep_images
        .contains(&"registry.example.test/images/app:sha".into()));
    assert!(plan
        .keep_images
        .contains(&"registry.example.test/images/app@sha256:abc123".into()));
    assert!(!plan.keep_images.contains(&"bad ref with whitespace".into()));
}

/// Verifies the rollback-target fix.
///
/// Scenario: A(success, oldest) < B(success) < R(rolled_back) < C(success,
/// current).  With `keep_previous=1`, the keep list must include C, R, and
/// B — B is the actual rollback target (most-recent success ≠ current).
///
/// Under the old `rn<=2` rule, R occupied rn=2 and pushed B to rn=3, where
/// it would have been reaped.  The `success_rn` column fixes this by
/// protecting B regardless of its overall rank.
#[tokio::test]
async fn db_cleanup_plan_protects_rollback_target_behind_rolled_back_row() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_cleanup_db(&state).await;
    let user_id = insert_cleanup_user(&state).await;
    let app_id = insert_cleanup_app(&state, user_id).await;

    // A: oldest success (stale with keep_previous=1)
    insert_cleanup_deployment_ago(
        &state,
        app_id,
        "success",
        "hostlet/app-a:v1",
        "hostlet-app-a",
        40,
    )
    .await;
    // B: second success — the rollback target deploy.rs would pick
    insert_cleanup_deployment_ago(
        &state,
        app_id,
        "success",
        "hostlet/app-b:v2",
        "hostlet-app-b",
        30,
    )
    .await;
    // R: rolled_back — more recent than B, occupies rn=2
    insert_cleanup_deployment_ago(
        &state,
        app_id,
        "rolled_back",
        "hostlet/app-r:v3",
        "hostlet-app-r",
        20,
    )
    .await;
    // C: newest success, set as current deployment
    let c = insert_cleanup_deployment_ago(
        &state,
        app_id,
        "success",
        "hostlet/app-c:v4",
        "hostlet-app-c",
        10,
    )
    .await;
    sqlx::query("UPDATE apps SET current_deployment_id=$1 WHERE id=$2")
        .bind(c)
        .bind(app_id)
        .execute(&state.db)
        .await
        .unwrap();

    let plan = cleanup_plan(&state).await.unwrap();

    assert!(
        plan.keep_containers.contains(&"hostlet-app-c".into()),
        "current container (C) must be kept"
    );
    assert!(
        plan.keep_containers.contains(&"hostlet-app-r".into()),
        "rolled_back container (R) must be kept"
    );
    assert!(
        plan.keep_containers.contains(&"hostlet-app-b".into()),
        "rollback target (B) must be kept via success_rn"
    );
    assert!(
        !plan.keep_containers.contains(&"hostlet-app-a".into()),
        "oldest success (A) must be stale"
    );
    assert_eq!(
        plan.docker.stale_deployment_containers, 1,
        "exactly one container should be stale (A)"
    );
}

/// Verifies the keep-failed-hours knob and non-displacement guarantee.
///
/// Scenario: C(success/current,5m) > F(failed,60m) > R(rolled_back,120m) >
/// O(failed,1500m=25h).  F newer than R so R's rn=2 slot is the stress case.
/// knob=24: C/R/F kept, O not, stale=1.  knob=0: only C/R kept, stale=2.
#[tokio::test]
async fn db_cleanup_plan_protects_recent_failed_deployments_only() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    reset_cleanup_db(&state).await;
    let user_id = insert_cleanup_user(&state).await;
    let app_id = insert_cleanup_app(&state, user_id).await;

    // C: current success (5 min ago)
    let c = insert_cleanup_deployment_ago(
        &state,
        app_id,
        "success",
        "hostlet/app-c:v1",
        "hostlet-app-c",
        5,
    )
    .await;
    sqlx::query("UPDATE apps SET current_deployment_id=$1 WHERE id=$2")
        .bind(c)
        .bind(app_id)
        .execute(&state.db)
        .await
        .unwrap();
    // F: recent failed (60 min ago — newer than R, exercises non-displacement)
    insert_cleanup_deployment_ago(
        &state,
        app_id,
        "failed",
        "hostlet/app-f:bad",
        "hostlet-app-f",
        60,
    )
    .await;
    // R: rolled_back (120 min ago)
    insert_cleanup_deployment_ago(
        &state,
        app_id,
        "rolled_back",
        "hostlet/app-r:v0",
        "hostlet-app-r",
        120,
    )
    .await;
    // O: old failed (1500 min = 25 h ago)
    insert_cleanup_deployment_ago(
        &state,
        app_id,
        "failed",
        "hostlet/app-o:bad",
        "hostlet-app-o",
        1500,
    )
    .await;

    // --- knob = 24 h ---
    let plan = cleanup_plan_with_keep_failed_hours(&state, 24)
        .await
        .unwrap();
    let kc = &plan.keep_containers;
    assert!(kc.contains(&"hostlet-app-c".into()), "C must be kept");
    assert!(
        kc.contains(&"hostlet-app-r".into()),
        "rolled_back row (R) must not be displaced by the recent failed row (F)"
    );
    assert!(kc.contains(&"hostlet-app-f".into()), "F(60m) kept");
    assert!(!kc.contains(&"hostlet-app-o".into()), "O(25h) not kept");
    assert!(plan.keep_images.contains(&"hostlet/app-f:bad".into()));
    assert!(!plan.keep_images.contains(&"hostlet/app-o:bad".into()));
    assert_eq!(plan.docker.stale_deployment_containers, 1);

    // --- knob = 0 (disabled) ---
    let plan = cleanup_plan_with_keep_failed_hours(&state, 0)
        .await
        .unwrap();
    let kc = &plan.keep_containers;
    assert!(kc.contains(&"hostlet-app-c".into()));
    assert!(kc.contains(&"hostlet-app-r".into()));
    assert!(!kc.contains(&"hostlet-app-f".into()), "F not kept(0)");
    assert!(!kc.contains(&"hostlet-app-o".into()), "O not kept(0)");
    assert_eq!(plan.docker.stale_deployment_containers, 2);
}
