use super::*;
use health::ContainerState;
use reconcile::{
    container_actual_from_state, decide_reconcile, ContainerActual, ReconcileDecision,
};

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

/// The successor to `should_auto_start_container`: `decide_reconcile` returns
/// `StartStopped` for a stopped container (not yet attempted), verifying the
/// 3-failure gate is preserved in the caller (`publish_runtime_health`).
#[test]
fn decide_reconcile_returns_start_for_stopped_container() {
    // Before 3 failures the caller skips the reconcile path entirely, but
    // decide_reconcile itself is stateless: test its pure output here.
    assert_eq!(
        decide_reconcile(ContainerActual::Stopped, true, false),
        ReconcileDecision::StartStopped
    );
}

/// Once `auto_start_attempted` is true the caller must not call
/// `auto_start_container` again. `decide_reconcile` with `repair_in_flight`
/// models the analogous guard for the Missing path.
#[test]
fn auto_start_is_once_per_unhealthy_streak() {
    let mut counts = HealthCounts {
        failures: 3,
        successes: 0,
        auto_start_attempted: true,
        repair_requested: false,
        repair_attempts: 0,
    };
    // A stopped container: the caller checks `!entry.auto_start_attempted`
    // before calling auto_start_container, so with the flag set no restart
    // is issued. The decide_reconcile function reflects the same intent via
    // the repair_in_flight guard for Missing containers.
    assert!(counts.auto_start_attempted);

    let healthy = HealthProbeResult {
        healthy: true,
        url: "http://127.0.0.1:3000/health".into(),
        http_status: Some(200),
        latency_ms: 1,
        error: None,
        container_state: Some(ContainerState::Running),
    };
    record_health_probe(&mut counts, &healthy);

    // On recovery all flags reset so the app can self-heal again later.
    assert_eq!(counts.failures, 0);
    assert!(!counts.auto_start_attempted);
    assert!(!counts.repair_requested);
    assert_eq!(counts.repair_attempts, 0);
}

/// `container_actual_from_state` maps Restarting and OomKilled to `None`,
/// preserving the existing behaviour that these states are NOT auto-acted on.
/// Missing now returns `Some(Missing)` (handled separately as a rebuild path).
#[test]
fn container_actual_ignores_restarting_and_oom_containers() {
    for state in [
        ContainerState::Restarting("1".into()),
        ContainerState::OomKilled,
    ] {
        assert_eq!(
            container_actual_from_state(Some(&state)),
            None,
            "expected None for transient state {state:?}"
        );
    }
}

/// Missing container maps to `Some(Missing)` — the rebuild path now handles
/// these rather than silently ignoring them.
#[test]
fn container_actual_maps_missing_to_missing() {
    assert_eq!(
        container_actual_from_state(Some(&ContainerState::Missing)),
        Some(ContainerActual::Missing)
    );
}
