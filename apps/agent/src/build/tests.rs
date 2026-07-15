use super::*;

fn test_build_plan() -> BuildPlan {
    BuildPlan {
        context: PathBuf::from("/tmp/hostlet-test-app"),
        dockerfile: PathBuf::from("/tmp/hostlet-test-app/Dockerfile"),
        generated: false,
        packaging_strategy: PackagingStrategy::Dockerfile,
    }
}

#[test]
fn build_runtime_metadata_records_build_time_and_image_size() {
    let metadata = build_runtime_metadata(
        &test_build_plan(),
        "hostlet/example:deployment",
        12_345,
        Some(149_422_080),
    );

    assert_eq!(metadata["imageRef"], "hostlet/example:deployment");
    assert_eq!(
        metadata["buildArtifact"]["imageRef"],
        "hostlet/example:deployment"
    );
    assert_eq!(metadata["packagingStrategy"], "dockerfile");
    assert_eq!(metadata["generatedDockerfile"], false);
    assert_eq!(metadata["buildDurationMs"], 12_345);
    assert_eq!(metadata["imageSizeBytes"], 149_422_080);
    assert_eq!(metadata["imageBudgetStatus"], "ok");
    assert_eq!(metadata["imageBudgetWarnBytes"], IMAGE_BUDGET_WARN_BYTES);
    assert_eq!(metadata["imageBudgetMaxBytes"], IMAGE_BUDGET_MAX_BYTES);
}

#[test]
fn build_runtime_metadata_records_unknown_image_size() {
    let metadata =
        build_runtime_metadata(&test_build_plan(), "hostlet/example:unknown", 3_000, None);

    assert_eq!(metadata["imageRef"], "hostlet/example:unknown");
    assert_eq!(
        metadata["buildArtifact"]["imageRef"],
        "hostlet/example:unknown"
    );
    assert_eq!(metadata["packagingStrategy"], "dockerfile");
    assert_eq!(metadata["buildDurationMs"], 3_000);
    assert!(metadata["imageSizeBytes"].is_null());
    assert_eq!(metadata["imageBudgetStatus"], "unknown");
}

#[test]
fn image_budget_status_classifies_thresholds() {
    assert_eq!(image_budget_status(None), "unknown");
    assert_eq!(image_budget_status(Some(IMAGE_BUDGET_WARN_BYTES)), "ok");
    assert_eq!(
        image_budget_status(Some(IMAGE_BUDGET_WARN_BYTES + 1)),
        "warning"
    );
    assert_eq!(image_budget_status(Some(IMAGE_BUDGET_MAX_BYTES)), "warning");
    assert_eq!(
        image_budget_status(Some(IMAGE_BUDGET_MAX_BYTES + 1)),
        "over_budget"
    );
}

#[test]
fn startup_runtime_metadata_preserves_build_metrics_and_records_boot_time() {
    let metadata = build_runtime_metadata(
        &test_build_plan(),
        "hostlet/example:startup",
        2_000,
        Some(42_000),
    );
    let metadata = add_git_sync_runtime_metadata(metadata, 175);
    let metadata = add_build_plan_runtime_metadata(metadata, 45);
    let metadata = add_startup_runtime_metadata(metadata, 350, 1_250);
    let metadata = add_routing_runtime_metadata(metadata, 90);

    assert_eq!(metadata["gitSyncDurationMs"], 175);
    assert_eq!(metadata["buildPlanDurationMs"], 45);
    assert_eq!(metadata["buildDurationMs"], 2_000);
    assert_eq!(metadata["imageSizeBytes"], 42_000);
    assert_eq!(metadata["containerStartDurationMs"], 350);
    assert_eq!(metadata["healthCheckDurationMs"], 1_250);
    assert_eq!(metadata["bootDurationMs"], 1_600);
    assert_eq!(metadata["routingDurationMs"], 90);
}

#[test]
fn build_prepare_failure_runtime_metadata_records_sync_and_planning_time() {
    let metadata = build_prepare_failure_runtime_metadata(35, 175);

    assert_eq!(metadata["buildPlanDurationMs"], 35);
    assert_eq!(metadata["gitSyncDurationMs"], 175);
    assert!(metadata.as_object().is_some_and(|object| object.len() == 2));
}

#[tokio::test]
async fn health_check_request_times_out_per_probe() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((_socket, _peer)) = listener.accept().await {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });

    let started = Instant::now();
    let err = health_check_request(
        &reqwest::Client::new(),
        &format!("http://{addr}/health"),
        Duration::from_millis(25),
    )
    .await
    .unwrap_err();

    assert!(err.is_timeout());
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn health_check_retry_schedule_skips_delay_after_final_attempt() {
    assert!(should_wait_before_next_health_attempt(29, 30));
    assert!(!should_wait_before_next_health_attempt(30, 30));
}

#[test]
fn health_check_attempts_value_accepts_valid_range() {
    assert_eq!(health_check_attempts_value("1"), Some(1));
    assert_eq!(health_check_attempts_value("30"), Some(30));
    assert_eq!(health_check_attempts_value("900"), Some(900));
    assert_eq!(health_check_attempts_value(" 60 "), Some(60));
}

#[test]
fn health_check_attempts_value_rejects_out_of_range_and_invalid() {
    assert_eq!(health_check_attempts_value("0"), None);
    assert_eq!(health_check_attempts_value("901"), None);
    assert_eq!(health_check_attempts_value(""), None);
    assert_eq!(health_check_attempts_value("  "), None);
    assert_eq!(health_check_attempts_value("fast"), None);
}

#[test]
fn container_fatal_state_running_container_is_not_fatal() {
    assert_eq!(container_fatal_state("true false false 0"), None);
}

#[test]
fn container_fatal_state_restarting_container_keeps_probing() {
    assert_eq!(container_fatal_state("false true false 1"), None);
}

#[test]
fn container_fatal_state_stopped_container_reports_exit_code() {
    let msg = container_fatal_state("false false false 2").unwrap();
    assert!(
        msg.contains("2"),
        "exit code should appear in message: {msg}"
    );
    assert!(msg.contains("exited"), "message should mention exit: {msg}");
}

#[test]
fn container_fatal_state_oom_killed_container_reports_memory() {
    let msg = container_fatal_state("false false true 137").unwrap();
    assert!(msg.contains("OOM"), "OOM message should contain OOM: {msg}");
}

#[test]
fn container_fatal_state_malformed_input_is_not_fatal() {
    assert_eq!(container_fatal_state(""), None);
    assert_eq!(container_fatal_state("true"), None);
}
