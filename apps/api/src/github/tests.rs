use super::{
    claim_webhook_event, finalize_webhook_event, process_webhook, process_webhook_app,
    process_webhook_faulted, release_webhook_claim, renew_webhook_claim, repo_inspect_failure,
    webhook_app_event_is_terminal, webhook_processing_status, StatusCode, WebhookClaim,
    WebhookFailpoint,
};
use hostlet_contracts::{parse_github_repo, valid_commit_sha};
use sqlx::Row;
use uuid::Uuid;

const TEST_WEBHOOK_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

#[test]
fn rejects_branch_delete_zero_sha() {
    assert!(!valid_commit_sha(
        "0000000000000000000000000000000000000000"
    ));
}

#[test]
fn accepts_normal_commit_sha() {
    assert!(valid_commit_sha("0123456789abcdef0123456789abcdef01234567"));
}

#[test]
fn parses_github_repo_inputs() {
    assert_eq!(
        parse_github_repo("https://github.com/go-gitea/gitea"),
        Some("go-gitea/gitea".into())
    );
    assert_eq!(
        parse_github_repo("git@github.com:owner/repo.git"),
        Some("owner/repo".into())
    );
    assert_eq!(parse_github_repo("owner/repo"), Some("owner/repo".into()));
    assert_eq!(parse_github_repo("https://example.com/owner/repo"), None);
}

#[test]
fn repo_inspect_failure_404_gives_not_found_with_check_name_hint() {
    let (status, body) = repo_inspect_failure(Some(404));
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body.contains("not found"),
        "body should mention 'not found': {body}"
    );
    assert!(
        body.contains("owner/repo") || body.contains("reconnect"),
        "body should hint at fix: {body}"
    );
}

#[test]
fn repo_inspect_failure_401_gives_bad_gateway_with_reconnect_hint() {
    let (status, body) = repo_inspect_failure(Some(401));
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(body.contains("401"), "body should include status: {body}");
    assert!(
        body.contains("Reconnect"),
        "body should suggest reconnect: {body}"
    );
}

#[test]
fn repo_inspect_failure_403_gives_bad_gateway() {
    let (status, body) = repo_inspect_failure(Some(403));
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(body.contains("403"), "body should include status: {body}");
}

#[test]
fn repo_inspect_failure_429_gives_rate_limit_message() {
    let (status, body) = repo_inspect_failure(Some(429));
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(
        body.contains("rate-limited"),
        "body should mention rate limit: {body}"
    );
}

#[test]
fn repo_inspect_failure_other_and_none_give_generic_bad_gateway() {
    for input in [None, Some(500u16), Some(503)] {
        let (status, body) = repo_inspect_failure(input);
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            body, "GitHub repository could not be inspected",
            "unexpected body for {input:?}"
        );
    }
}

#[test]
fn webhook_app_outcomes_only_skip_terminal_results() {
    let deployment_id = Some(Uuid::new_v4());
    assert!(webhook_app_event_is_terminal(
        "deployed",
        deployment_id,
        None
    ));
    assert!(webhook_app_event_is_terminal(
        "ignored",
        None,
        Some("auto redeploy is disabled for this app")
    ));

    // A deployment error is deliberately retryable; old rows written by the
    // previous implementation used `ignored` for these errors, so the reason
    // must distinguish them from the user-disabled terminal outcome.
    assert!(!webhook_app_event_is_terminal(
        "ignored",
        None,
        Some("repository token expired")
    ));
    assert!(!webhook_app_event_is_terminal("deployed", None, None));
}

#[tokio::test]
async fn db_webhook_redelivery_reuses_the_recorded_event() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    let delivery = format!("core-07-{}", Uuid::new_v4());
    let payload = serde_json::json!({
        "repository": {"full_name": "core-07/replay"},
        "ref": "refs/heads/main",
        "after": "not-a-commit-sha"
    });

    process_webhook(&state, &delivery, "push", "core-07/replay", &payload)
        .await
        .expect("first delivery should finalize");
    process_webhook(&state, &delivery, "push", "core-07/replay", &payload)
        .await
        .expect("redelivery should be an idempotent no-op");

    let row = sqlx::query(
        "SELECT processed,ignored_reason FROM webhook_events
         WHERE github_delivery_id=$1",
    )
    .bind(&delivery)
    .fetch_one(&state.db)
    .await
    .expect("recorded event");
    assert!(row.get::<bool, _>("processed"));
    assert_eq!(
        row.get::<Option<String>, _>("ignored_reason").as_deref(),
        Some("push did not include a valid commit SHA")
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM webhook_events WHERE github_delivery_id=$1")
            .bind(&delivery)
            .fetch_one(&state.db)
            .await
            .expect("event count");
    assert_eq!(count, 1);
    sqlx::query("DELETE FROM webhook_events WHERE github_delivery_id=$1")
        .bind(&delivery)
        .execute(&state.db)
        .await
        .expect("test cleanup");
    state.db.close().await;
}

#[tokio::test]
async fn db_webhook_failpoints_keep_each_boundary_retryable() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    let user_id = insert_webhook_test_user(&state).await;
    let repo = "core-07/fault-fixture";
    let app_id = insert_webhook_test_app(&state, user_id, repo, false).await;
    let invalid_payload = serde_json::json!({
        "repository": {"full_name": "core-07/no-app"},
        "ref": "refs/heads/main",
        "after": "not-a-commit-sha"
    });
    let valid_payload = serde_json::json!({
        "repository": {"full_name": repo},
        "ref": "refs/heads/main",
        "after": TEST_WEBHOOK_SHA
    });
    let boundaries = [
        (WebhookFailpoint::EventInsert, &invalid_payload),
        (WebhookFailpoint::EventRead, &invalid_payload),
        (WebhookFailpoint::EventLock, &invalid_payload),
        (WebhookFailpoint::AppLookup, &valid_payload),
        (WebhookFailpoint::AppOutcomeWrite, &valid_payload),
        (WebhookFailpoint::AppOutcomeUpdate, &valid_payload),
        (WebhookFailpoint::Finalization, &invalid_payload),
        (WebhookFailpoint::Commit, &invalid_payload),
    ];

    for (failpoint, payload) in boundaries {
        let delivery = format!("core-07-fault-{failpoint:?}-{}", Uuid::new_v4());
        let seeded_event_id = if failpoint == WebhookFailpoint::EventInsert {
            None
        } else {
            Some(seed_webhook_event(&state, &delivery, payload).await)
        };
        let event_id = seeded_event_id;
        if failpoint == WebhookFailpoint::AppOutcomeUpdate {
            let event_id = event_id.expect("update fault must have a seeded event");
            sqlx::query(
                "INSERT INTO webhook_app_events
                 (webhook_event_id,app_id,repo_full_name,branch,commit_sha,status,ignored_reason)
                 VALUES ($1,$2,$3,'main',$4,'ignored','transient test failure')",
            )
            .bind(event_id)
            .bind(app_id)
            .bind(repo)
            .bind(TEST_WEBHOOK_SHA)
            .execute(&state.db)
            .await
            .expect("seed retryable app outcome");
        }

        let result = process_webhook_faulted(
            &state,
            &delivery,
            "push",
            payload
                .pointer("/repository/full_name")
                .and_then(|value| value.as_str())
                .unwrap_or_default(),
            payload,
            failpoint,
        )
        .await;
        assert!(
            result.is_err(),
            "{failpoint:?} should fail deterministically"
        );
        assert_eq!(
            webhook_processing_status(&result),
            StatusCode::INTERNAL_SERVER_ERROR,
            "{failpoint:?} must map to a retryable response"
        );
        if failpoint == WebhookFailpoint::EventInsert {
            assert_eq!(
                webhook_processed_optional(&state, &delivery).await,
                None,
                "{failpoint:?} must leave no event row after insertion failure"
            );
        } else {
            assert!(
                !webhook_processed(&state, &delivery).await,
                "{failpoint:?} must leave the delivery unprocessed"
            );
        }

        process_webhook(
            &state,
            &delivery,
            "push",
            payload
                .pointer("/repository/full_name")
                .and_then(|value| value.as_str())
                .unwrap_or_default(),
            payload,
        )
        .await
        .expect("redelivery should succeed after injected failure");
        assert!(webhook_processed(&state, &delivery).await);
        let event_id = match event_id {
            Some(event_id) => event_id,
            None => sqlx::query_scalar("SELECT id FROM webhook_events WHERE github_delivery_id=$1")
                .bind(&delivery)
                .fetch_one(&state.db)
                .await
                .expect("event inserted on redelivery"),
        };
        if matches!(
            failpoint,
            WebhookFailpoint::AppOutcomeWrite | WebhookFailpoint::AppOutcomeUpdate
        ) {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM webhook_app_events WHERE webhook_event_id=$1 AND app_id=$2",
            )
            .bind(event_id)
            .bind(app_id)
            .fetch_one(&state.db)
            .await
            .expect("app outcome count");
            assert_eq!(count, 1, "{failpoint:?} must remain idempotent");
        }
        sqlx::query("DELETE FROM webhook_events WHERE id=$1")
            .bind(event_id)
            .execute(&state.db)
            .await
            .expect("event cleanup");
    }

    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user_id)
        .execute(&state.db)
        .await
        .expect("fixture cleanup");
    state.db.close().await;
}

#[tokio::test]
async fn db_webhook_stale_claim_cannot_release_or_finalize_new_owner() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    let delivery = format!("core-07-stale-{}", Uuid::new_v4());
    let payload = serde_json::json!({
        "repository": {"full_name": "core-07/stale-owner"},
        "ref": "refs/heads/main",
        "after": "not-a-commit-sha"
    });
    let user_id = insert_webhook_test_user(&state).await;
    let app_id = insert_webhook_test_app(&state, user_id, "core-07/stale-owner", false).await;
    let event_id = seed_webhook_event(&state, &delivery, &payload).await;
    let first = match claim_webhook_event(
        &state,
        &delivery,
        "push",
        "core-07/stale-owner",
        &payload,
        WebhookFailpoint::None,
    )
    .await
    .expect("first claim")
    {
        WebhookClaim::Claimed(event) => event,
        WebhookClaim::AlreadyProcessed => panic!("fixture must be claimable"),
    };
    let first_token = first.processing_token.expect("first claim token");
    assert!(renew_webhook_claim(&state, event_id, first_token)
        .await
        .expect("renew first claim"));
    let lease_is_fresh: bool = sqlx::query_scalar(
        "SELECT processing_started_at > clock_timestamp() - interval '1 minute'
         FROM webhook_events WHERE id=$1",
    )
    .bind(event_id)
    .fetch_one(&state.db)
    .await
    .expect("renewed lease timestamp");
    assert!(lease_is_fresh);
    let premature_takeover = claim_webhook_event(
        &state,
        &delivery,
        "push",
        "core-07/stale-owner",
        &payload,
        WebhookFailpoint::None,
    )
    .await;
    assert!(
        premature_takeover.is_err(),
        "renewal must keep a current claim from being taken over"
    );
    sqlx::query(
        "UPDATE webhook_events
         SET processing_started_at=now()-interval '11 minutes'
         WHERE id=$1",
    )
    .bind(event_id)
    .execute(&state.db)
    .await
    .expect("expire first claim");

    let second = match claim_webhook_event(
        &state,
        &delivery,
        "push",
        "core-07/stale-owner",
        &payload,
        WebhookFailpoint::None,
    )
    .await
    .expect("second claim")
    {
        WebhookClaim::Claimed(event) => event,
        WebhookClaim::AlreadyProcessed => panic!("expired fixture must be reclaimable"),
    };
    let second_token = second.processing_token.expect("second claim token");
    assert_ne!(first_token, second_token);
    assert!(!renew_webhook_claim(&state, event_id, first_token)
        .await
        .expect("stale claim renewal"));

    sqlx::query(
        "INSERT INTO webhook_app_events
         (webhook_event_id,app_id,repo_full_name,branch,commit_sha,status,ignored_reason)
         VALUES ($1,$2,$3,'main',$4,'ignored','stale outcome fixture')",
    )
    .bind(event_id)
    .bind(app_id)
    .bind("core-07/stale-owner")
    .bind(TEST_WEBHOOK_SHA)
    .execute(&state.db)
    .await
    .expect("seed stale outcome fixture");
    let stale_fanout = process_webhook_app(
        &state,
        event_id,
        app_id,
        user_id,
        false,
        "core-07/stale-owner",
        "main",
        TEST_WEBHOOK_SHA,
        first_token,
        WebhookFailpoint::None,
    )
    .await;
    assert!(
        stale_fanout.is_err(),
        "stale owner must not process app fan-out"
    );
    let outcome = sqlx::query(
        "SELECT status,ignored_reason FROM webhook_app_events
         WHERE webhook_event_id=$1 AND app_id=$2",
    )
    .bind(event_id)
    .bind(app_id)
    .fetch_one(&state.db)
    .await
    .expect("stale outcome remains");
    assert_eq!(outcome.get::<String, _>("status"), "ignored");
    assert_eq!(
        outcome.get::<String, _>("ignored_reason"),
        "stale outcome fixture"
    );

    release_webhook_claim(&state, event_id, first_token)
        .await
        .expect("stale release query");
    let token_after_release: Uuid =
        sqlx::query_scalar("SELECT processing_token FROM webhook_events WHERE id=$1")
            .bind(event_id)
            .fetch_one(&state.db)
            .await
            .expect("second claim remains owned");
    assert_eq!(token_after_release, second_token);

    let stale_finalize = finalize_webhook_event(
        &state,
        event_id,
        &delivery,
        Some("main"),
        Some("not-a-commit-sha"),
        None,
        first_token,
        WebhookFailpoint::None,
    )
    .await;
    assert!(stale_finalize.is_err(), "stale owner must not finalize");
    let state_after_finalize = sqlx::query(
        "SELECT processed,processing,processing_token
         FROM webhook_events WHERE id=$1",
    )
    .bind(event_id)
    .fetch_one(&state.db)
    .await
    .expect("claim state");
    assert!(!state_after_finalize.get::<bool, _>("processed"));
    assert!(state_after_finalize.get::<bool, _>("processing"));
    assert_eq!(
        state_after_finalize.get::<Uuid, _>("processing_token"),
        second_token
    );

    release_webhook_claim(&state, event_id, second_token)
        .await
        .expect("release second claim");
    sqlx::query("DELETE FROM webhook_events WHERE id=$1")
        .bind(event_id)
        .execute(&state.db)
        .await
        .expect("event cleanup");
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user_id)
        .execute(&state.db)
        .await
        .expect("fixture cleanup");
    state.db.close().await;
}

#[tokio::test]
async fn db_webhook_deployment_create_failure_redelivers_once() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    set_webhook_deploy_test_env();
    let user_id = insert_webhook_test_user(&state).await;
    let repo = "core-07/deployment-create-fixture";
    let app_id = insert_webhook_test_app(&state, user_id, repo, true).await;
    let delivery = format!("core-07-create-failure-{}", Uuid::new_v4());
    let payload = serde_json::json!({
        "repository": {"full_name": repo},
        "ref": "refs/heads/main",
        "after": TEST_WEBHOOK_SHA
    });
    let event_id = seed_webhook_event(&state, &delivery, &payload).await;

    let failed = process_webhook_faulted(
        &state,
        &delivery,
        "push",
        repo,
        &payload,
        WebhookFailpoint::DeploymentCreate,
    )
    .await;
    assert!(failed.is_err(), "deployment creation failpoint must fail");
    assert_eq!(
        webhook_processing_status(&failed),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert!(!webhook_processed(&state, &delivery).await);
    let before_redelivery: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM deployments WHERE app_id=$1 AND commit_sha=$2")
            .bind(app_id)
            .bind(TEST_WEBHOOK_SHA)
            .fetch_one(&state.db)
            .await
            .expect("deployment count before redelivery");
    assert_eq!(before_redelivery, 0);

    process_webhook(&state, &delivery, "push", repo, &payload)
        .await
        .expect("redelivery should create deployment");
    process_webhook(&state, &delivery, "push", repo, &payload)
        .await
        .expect("second redelivery should be idempotent");
    assert!(webhook_processed(&state, &delivery).await);
    let deployment_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM deployments WHERE app_id=$1 AND commit_sha=$2")
            .bind(app_id)
            .bind(TEST_WEBHOOK_SHA)
            .fetch_one(&state.db)
            .await
            .expect("deployment count");
    assert_eq!(deployment_count, 1);
    let outcome_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM webhook_app_events WHERE webhook_event_id=$1 AND app_id=$2",
    )
    .bind(event_id)
    .bind(app_id)
    .fetch_one(&state.db)
    .await
    .expect("outcome count");
    assert_eq!(outcome_count, 1);

    sqlx::query("DELETE FROM webhook_events WHERE id=$1")
        .bind(event_id)
        .execute(&state.db)
        .await
        .expect("event cleanup");
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user_id)
        .execute(&state.db)
        .await
        .expect("fixture cleanup");
    state.db.close().await;
}

#[tokio::test]
async fn db_webhook_valid_push_redelivery_repairs_legacy_missing_outcome_once() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    set_webhook_deploy_test_env();
    let user_id = insert_webhook_test_user(&state).await;
    let repo = "core-07/fanout-fixture";
    let app_id = insert_webhook_test_app(&state, user_id, repo, true).await;
    let delivery = format!("core-07-fanout-{}", Uuid::new_v4());
    let payload = serde_json::json!({
        "repository": {"full_name": repo},
        "ref": "refs/heads/main",
        "after": TEST_WEBHOOK_SHA
    });
    let event_id = seed_webhook_event(&state, &delivery, &payload).await;
    // Simulate the legacy handler: it marked the delivery processed before a
    // fan-out outcome was durably recorded. A redelivery must repair it.
    sqlx::query("UPDATE webhook_events SET processed=true,processed_at=now() WHERE id=$1")
        .bind(event_id)
        .execute(&state.db)
        .await
        .expect("seed legacy processed event");

    process_webhook(&state, &delivery, "push", repo, &payload)
        .await
        .expect("legacy processed push should be repaired");
    process_webhook(&state, &delivery, "push", repo, &payload)
        .await
        .expect("redelivery should be idempotent");

    let deployment_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM deployments WHERE app_id=$1 AND commit_sha=$2")
            .bind(app_id)
            .bind(TEST_WEBHOOK_SHA)
            .fetch_one(&state.db)
            .await
            .expect("deployment count");
    assert_eq!(deployment_count, 1);
    let outcome_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM webhook_app_events WHERE webhook_event_id=$1 AND app_id=$2",
    )
    .bind(event_id)
    .bind(app_id)
    .fetch_one(&state.db)
    .await
    .expect("outcome count");
    assert_eq!(outcome_count, 1);
    assert!(webhook_processed(&state, &delivery).await);

    sqlx::query("DELETE FROM webhook_events WHERE id=$1")
        .bind(event_id)
        .execute(&state.db)
        .await
        .expect("event cleanup");
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user_id)
        .execute(&state.db)
        .await
        .expect("fixture cleanup");
    state.db.close().await;
}

#[tokio::test]
async fn db_webhook_does_not_reuse_active_deployment_without_viable_job() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    let user_id = insert_webhook_test_user(&state).await;
    let repo = "core-07/missing-job-fixture";
    let app_id = insert_webhook_test_app(&state, user_id, repo, true).await;
    let orphaned_deployment: Uuid = sqlx::query_scalar(
        "INSERT INTO deployments
         (app_id,server_id,status,commit_sha,started_at,runtime_kind)
         VALUES ($1,$2,'queued',$3,now(),'single') RETURNING id",
    )
    .bind(app_id)
    .bind(state.local_server_id)
    .bind(TEST_WEBHOOK_SHA)
    .fetch_one(&state.db)
    .await
    .expect("seed orphaned deployment");
    let delivery = format!("core-07-missing-job-{}", Uuid::new_v4());
    let payload = serde_json::json!({
        "repository": {"full_name": repo},
        "ref": "refs/heads/main",
        "after": TEST_WEBHOOK_SHA
    });
    let event_id = seed_webhook_event(&state, &delivery, &payload).await;

    let result = process_webhook(&state, &delivery, "push", repo, &payload).await;
    assert!(
        result.is_err(),
        "an active deployment without a job is retryable"
    );
    assert_eq!(
        webhook_processing_status(&result),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert!(!webhook_processed(&state, &delivery).await);
    let deployment_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM deployments WHERE app_id=$1 AND commit_sha=$2")
            .bind(app_id)
            .bind(TEST_WEBHOOK_SHA)
            .fetch_one(&state.db)
            .await
            .expect("deployment count");
    assert_eq!(
        deployment_count, 1,
        "retry must not duplicate orphaned work"
    );

    // Once recovery marks the orphan terminal, the redelivery can safely
    // create fresh durable work instead of reusing the missing-job row.
    sqlx::query("UPDATE deployments SET status='failed',finished_at=now() WHERE id=$1")
        .bind(orphaned_deployment)
        .execute(&state.db)
        .await
        .expect("mark orphaned deployment failed");
    set_webhook_deploy_test_env();
    process_webhook(&state, &delivery, "push", repo, &payload)
        .await
        .expect("redelivery should create recoverable work");
    assert!(webhook_processed(&state, &delivery).await);

    sqlx::query("DELETE FROM webhook_events WHERE id=$1")
        .bind(event_id)
        .execute(&state.db)
        .await
        .expect("event cleanup");
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user_id)
        .execute(&state.db)
        .await
        .expect("fixture cleanup");
    state.db.close().await;
}

async fn seed_webhook_event(
    state: &crate::state::AppState,
    delivery: &str,
    payload: &serde_json::Value,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO webhook_events (github_delivery_id,repo_full_name,event_type,payload)
         VALUES ($1,$2,'push',$3) RETURNING id",
    )
    .bind(delivery)
    .bind(
        payload
            .pointer("/repository/full_name")
            .and_then(|value| value.as_str())
            .unwrap_or_default(),
    )
    .bind(payload)
    .fetch_one(&state.db)
    .await
    .expect("seed webhook event")
}

async fn webhook_processed(state: &crate::state::AppState, delivery: &str) -> bool {
    webhook_processed_optional(state, delivery)
        .await
        .expect("webhook event row")
}

async fn webhook_processed_optional(
    state: &crate::state::AppState,
    delivery: &str,
) -> Option<bool> {
    sqlx::query_scalar("SELECT processed FROM webhook_events WHERE github_delivery_id=$1")
        .bind(delivery)
        .fetch_optional(&state.db)
        .await
        .expect("webhook processed query")
}

async fn insert_webhook_test_user(state: &crate::state::AppState) -> Uuid {
    let github_id = (Uuid::new_v4().as_u128() & (i64::MAX as u128)) as i64;
    sqlx::query_scalar("INSERT INTO users (github_id,login) VALUES ($1,$2) RETURNING id")
        .bind(github_id)
        .bind(format!("core-07-{}", Uuid::new_v4()))
        .fetch_one(&state.db)
        .await
        .expect("fixture user")
}

async fn insert_webhook_test_app(
    state: &crate::state::AppState,
    user_id: Uuid,
    repo: &str,
    auto_deploy: bool,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO apps
         (user_id,server_id,name,repo_full_name,branch,container_port,health_path,
          domain,runtime_kind,root_directory,public_exposure,auto_deploy)
         VALUES ($1,$2,$3,$4,'main',3000,'/', $5,'single','.',true,$6)
         RETURNING id",
    )
    .bind(user_id)
    .bind(state.local_server_id)
    .bind(format!("core-07-{}", Uuid::new_v4()))
    .bind(repo)
    .bind(format!("{}.example.test", Uuid::new_v4()))
    .bind(auto_deploy)
    .fetch_one(&state.db)
    .await
    .expect("fixture app")
}

fn set_webhook_deploy_test_env() {
    for (key, value) in [
        (
            "HOSTLET_ARTIFACT_REGISTRY_LOCAL_URL",
            "http://127.0.0.1:5000",
        ),
        ("HOSTLET_ARTIFACT_REGISTRY_PUSH_USERNAME", "ci-push"),
        (
            "HOSTLET_ARTIFACT_REGISTRY_PUSH_PASSWORD",
            "ci-push-password",
        ),
        ("HOSTLET_ARTIFACT_REGISTRY_PULL_USERNAME", "ci-pull"),
        (
            "HOSTLET_ARTIFACT_REGISTRY_PULL_PASSWORD",
            "ci-pull-password",
        ),
    ] {
        if std::env::var(key).is_err() {
            std::env::set_var(key, value);
        }
    }
}
