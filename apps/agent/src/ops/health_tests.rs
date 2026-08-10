use super::*;

#[test]
fn inspect_container_state_accepts_running_container() {
    assert_eq!(
        inspect_container_state("true false false 0"),
        Some(ContainerState::Running)
    );
}

#[test]
fn inspect_container_state_reports_restart_loop() {
    let state = inspect_container_state("true true false 1").unwrap();
    assert_eq!(
        state.error_message(),
        "container is restarting after exit code 1"
    );
}

#[test]
fn inspect_container_state_reports_oom_kill() {
    let state = inspect_container_state("false false true 137").unwrap();
    assert_eq!(state.error_message(), "container was OOM-killed");
}

#[test]
fn inspect_container_state_reports_stopped_exit_code() {
    let state = inspect_container_state("false false false 2").unwrap();
    assert_eq!(
        state.error_message(),
        "container is not running; last exit code 2"
    );
}

#[test]
fn inspect_container_state_rejects_malformed_output() {
    assert_eq!(inspect_container_state(""), None);
    assert_eq!(inspect_container_state("true false"), None);
}

#[test]
fn health_target_payload_accepts_route_metadata() {
    let app_id = Uuid::from_u128(1);
    let deployment_id = Uuid::from_u128(2);
    let target = health_target_from_payload(&json!({
        "appId": app_id,
        "deploymentId": deployment_id,
        "containerName": "hostlet-app-demo",
        "containerPort": 3000,
        "publishedPort": 32000,
        "healthPath": "/health",
        "domain": "demo.example.com",
        "routeKey": "app-00000000-0000-0000-0000-000000000001"
    }))
    .unwrap();

    assert_eq!(target.domain.as_deref(), Some("demo.example.com"));
    assert_eq!(
        target.route_key.as_deref(),
        Some("app-00000000-0000-0000-0000-000000000001")
    );
    assert!(!target.tcp_probe);
}

#[test]
fn health_target_payload_preserves_tcp_probe_kind() {
    let target = health_target_from_payload(&json!({
        "appId": Uuid::from_u128(1),
        "deploymentId": Uuid::from_u128(2),
        "containerName": "hostlet-app-websocket",
        "containerPort": 3000,
        "publishedPort": 32001,
        "healthPath": "/",
        "probeKind": "tcp"
    }))
    .unwrap();
    assert!(target.tcp_probe);
}

#[test]
fn health_target_payload_accepts_valid_split_route() {
    let target = health_target_from_payload(&json!({
        "appId": Uuid::from_u128(1),
        "deploymentId": Uuid::from_u128(2),
        "containerName": "hostlet-app-frontend",
        "containerPort": 3000,
        "publishedPort": 32000,
        "healthPath": "/health",
        "domain": "demo.example.com",
        "routeKey": "app-00000000-0000-0000-0000-000000000001",
        "routeGeneration": 7,
        "splitRoute": {
            "backend": {
                "serviceName": "api",
                "containerName": "hostlet-app-backend",
                "targetPort": 4000,
                "publishedPort": 32001,
                "healthPath": "/ready",
                "probeKind": "http"
            },
            "backendPathPrefixes": ["/api", "/socket.io"]
        }
    }))
    .unwrap();

    let route = target.split_route.as_ref().unwrap();
    assert_eq!(route.backend_container_name, "hostlet-app-backend");
    assert_eq!(route.backend_container_port, 4000);
    assert_eq!(route.backend_published_port, 32001);
    assert_eq!(
        route.backend_path_prefixes,
        vec!["/api".to_string(), "/socket.io".to_string()]
    );
}

#[test]
fn health_target_payload_accepts_backend_only_split_route() {
    let target = health_target_from_payload(&json!({
        "appId": Uuid::from_u128(1),
        "deploymentId": Uuid::from_u128(2),
        "containerName": "hostlet-app-backend",
        "containerPort": 4000,
        "publishedPort": 32001,
        "domain": "demo.example.com",
        "routeKey": "app-00000000-0000-0000-0000-000000000001",
        "routeGeneration": 7,
        "splitRoute": {
            "backend": {
                "containerName": "hostlet-app-backend",
                "targetPort": 4000,
                "publishedPort": 32001
            },
            "backendPathPrefixes": ["/api"]
        }
    }))
    .unwrap();

    assert_eq!(
        target
            .split_route
            .as_ref()
            .map(|route| route.backend_container_name.as_str()),
        Some("hostlet-app-backend")
    );
}

#[test]
fn health_target_payload_rejects_invalid_present_split_route() {
    let payload = json!({
        "appId": Uuid::from_u128(1),
        "deploymentId": Uuid::from_u128(2),
        "containerName": "hostlet-app-frontend",
        "containerPort": 3000,
        "publishedPort": 32000,
        "splitRoute": {
            "backend": {
                "containerName": "hostlet-app-backend",
                "targetPort": 4000,
                "publishedPort": 32001
            },
            "backendPathPrefixes": ["/api/", "/bad..prefix"]
        }
    });
    assert!(health_target_from_payload(&payload).is_none());
}

#[test]
fn split_route_port_drift_requires_route_refresh_when_either_port_changes() {
    let target = health_target_from_payload(&json!({
        "appId": Uuid::from_u128(1),
        "deploymentId": Uuid::from_u128(2),
        "containerName": "hostlet-app-frontend",
        "containerPort": 3000,
        "publishedPort": 32000,
        "domain": "demo.example.com",
        "routeKey": "app-00000000-0000-0000-0000-000000000001",
        "routeGeneration": 7,
        "splitRoute": {
            "backend": {
                "containerName": "hostlet-app-backend",
                "targetPort": 4000,
                "publishedPort": 32001
            },
            "backendPathPrefixes": ["/api"]
        }
    }))
    .unwrap();
    assert!(!split_route_ports_changed(&target, 32000, Some(32001)));
    assert!(split_route_ports_changed(&target, 32002, Some(32001)));
    assert!(split_route_ports_changed(&target, 32000, Some(32003)));
}

#[test]
fn health_target_payload_rejects_split_route_without_versioned_route_owner() {
    let payload = json!({
        "appId": Uuid::from_u128(1),
        "deploymentId": Uuid::from_u128(2),
        "containerName": "hostlet-app-frontend",
        "containerPort": 3000,
        "publishedPort": 32000,
        "splitRoute": {
            "backend": {
                "containerName": "hostlet-app-backend",
                "targetPort": 4000,
                "publishedPort": 32001
            },
            "backendPathPrefixes": ["/api"]
        }
    });
    assert!(health_target_from_payload(&payload).is_none());
}

#[test]
fn interactive_target_selection_requires_exact_current_deployment() {
    let app_id = Uuid::from_u128(1);
    let deployment_id = Uuid::from_u128(2);
    let target = health_target_from_payload(&json!({
        "appId": app_id,
        "deploymentId": deployment_id,
        "containerName": "hostlet-app-current",
        "containerPort": 3000,
        "publishedPort": 32000
    }))
    .unwrap();
    assert!(select_current_health_target(
        &json!({"app_id": app_id, "deployment_id": deployment_id}),
        vec![target.clone()]
    )
    .is_some());
    assert!(select_current_health_target(
        &json!({"app_id": app_id, "deployment_id": Uuid::from_u128(3)}),
        vec![target]
    )
    .is_none());
}

#[test]
fn route_owner_comparison_ignores_observed_ports_but_not_split_shape() {
    let payload = json!({
        "appId": Uuid::from_u128(1),
        "deploymentId": Uuid::from_u128(2),
        "containerName": "hostlet-app-frontend",
        "containerPort": 3000,
        "publishedPort": 32000,
        "domain": "demo.example.com",
        "routeKey": "app-00000000-0000-0000-0000-000000000001",
        "routeGeneration": 7,
        "splitRoute": {
            "backend": {
                "containerName": "hostlet-app-backend",
                "targetPort": 4000,
                "publishedPort": 32001
            },
            "backendPathPrefixes": ["/api"]
        }
    });
    let stale = health_target_from_payload(&payload).unwrap();
    let mut current_payload = payload;
    current_payload["publishedPort"] = json!(32100);
    current_payload["splitRoute"]["backend"]["publishedPort"] = json!(32101);
    let current = health_target_from_payload(&current_payload).unwrap();
    assert!(same_route_owner(&stale, &current));

    current_payload
        .as_object_mut()
        .unwrap()
        .remove("splitRoute");
    let single = health_target_from_payload(&current_payload).unwrap();
    assert!(!same_route_owner(&stale, &single));
}

#[test]
fn health_target_payload_rejects_invalid_route_metadata_without_rejecting_target() {
    let app_id = Uuid::from_u128(1);
    let deployment_id = Uuid::from_u128(2);
    let target = health_target_from_payload(&json!({
        "app_id": app_id,
        "deployment_id": deployment_id,
        "container_name": "hostlet-app-demo",
        "container_port": 3000,
        "published_port": 32000,
        "health_path": "/health",
        "domain": "not a domain",
        "route_key": "../../bad"
    }))
    .unwrap();

    assert_eq!(target.domain, None);
    assert_eq!(target.route_key, None);
}

#[test]
fn published_port_changed_detects_drift_only() {
    assert!(!published_port_changed(32000, 32000));
    assert!(published_port_changed(32000, 32001));
}
