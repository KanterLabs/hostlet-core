use super::*;

#[test]
fn failed_deployment_status_event_keeps_runtime_metrics() {
    let deployment_id = Uuid::from_u128(42);
    let event = deployment_status_event(
        deployment_id,
        "failed",
        StatusDetails {
            failure: Some("Health check failed"),
            image: Some("hostlet/demo:latest"),
            container: Some("hostlet-demo"),
            published_port: Some(32001),
            compose_project: Some("hostlet_app_0000000000000000000000000000002a"),
            runtime_metadata: Some(json!({
                "containerStartDurationMs": 125,
                "healthCheckDurationMs": 4_000,
                "bootDurationMs": 4_125,
            })),
            ..StatusDetails::default()
        },
    );

    assert_eq!(event["type"], "deployment_status");
    assert_eq!(event["deployment_id"], deployment_id.to_string());
    assert_eq!(event["status"], "failed");
    assert_eq!(event["failure"], "Health check failed");
    assert_eq!(event["image_tag"], "hostlet/demo:latest");
    assert_eq!(event["container_name"], "hostlet-demo");
    assert_eq!(event["published_port"], 32001);
    assert_eq!(
        event["compose_project"],
        "hostlet_app_0000000000000000000000000000002a"
    );
    assert_eq!(event["runtime_metadata"]["containerStartDurationMs"], 125);
    assert_eq!(event["runtime_metadata"]["healthCheckDurationMs"], 4_000);
    assert_eq!(event["runtime_metadata"]["bootDurationMs"], 4_125);
}

#[test]
fn auto_start_waits_for_threshold_and_stopped_container() {
    let mut counts = HealthCounts::default();
    let stopped = health_probe_for_state(ContainerState::Stopped("0".into()));

    record_health_probe(&mut counts, &stopped);
    record_health_probe(&mut counts, &stopped);
    assert!(!should_auto_start_container(&counts, &stopped));

    record_health_probe(&mut counts, &stopped);
    assert!(should_auto_start_container(&counts, &stopped));
}

#[test]
fn auto_start_is_once_per_unhealthy_streak() {
    let mut counts = HealthCounts {
        failures: 3,
        successes: 0,
        auto_start_attempted: true,
    };
    let stopped = health_probe_for_state(ContainerState::Stopped("0".into()));

    assert!(!should_auto_start_container(&counts, &stopped));

    let healthy = HealthProbeResult {
        healthy: true,
        url: "http://127.0.0.1:3000/health".into(),
        http_status: Some(200),
        latency_ms: 1,
        error: None,
        container_state: Some(ContainerState::Running),
    };
    record_health_probe(&mut counts, &healthy);

    assert_eq!(counts.failures, 0);
    assert!(!counts.auto_start_attempted);
}

#[test]
fn auto_start_ignores_restarting_oom_and_missing_containers() {
    let counts = HealthCounts {
        failures: 3,
        successes: 0,
        auto_start_attempted: false,
    };

    for state in [
        ContainerState::Restarting("1".into()),
        ContainerState::OomKilled,
        ContainerState::Missing,
    ] {
        let result = health_probe_for_state(state);
        assert!(!should_auto_start_container(&counts, &result));
    }
}

fn health_probe_for_state(state: ContainerState) -> HealthProbeResult {
    HealthProbeResult {
        healthy: false,
        url: "http://127.0.0.1:3000/health".into(),
        http_status: None,
        latency_ms: 1,
        error: Some(state.error_message()),
        container_state: Some(state),
    }
}
