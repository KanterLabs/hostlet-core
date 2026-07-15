use super::*;

#[test]
fn deploy_payload_keeps_existing_wire_shape() {
    let deployment_id = Uuid::nil();
    let app_id = Uuid::from_u128(1);
    let payload = AgentJobPayload::Deploy(Box::new(DeployJob {
        deployment_id,
        app_id,
        route_key: "app-1".into(),
        app_name: "demo".into(),
        repo: "owner/repo".into(),
        branch: "main".into(),
        commit_sha: "HEAD".into(),
        container_port: 3000,
        health_path: "/".into(),
        domain: "demo.example.test".into(),
        env: BTreeMap::new(),
        runtime_kind: "single".into(),
        hostlet_config_path: "hostlet.yml".into(),
        runtime_config: serde_json::json!({}),
        packaging_strategy: "auto".into(),
        root_directory: ".".into(),
        install_command: None,
        build_command: None,
        start_command: None,
        memory_limit_mb: Some(512),
        cpu_limit: Some(0.5),
        github_token: None,
    }));
    let value = serde_json::to_value(&payload).unwrap();
    assert_eq!(value["type"], "deploy");
    assert_eq!(value["deployment_id"], deployment_id.to_string());
    assert_eq!(value["app_id"], app_id.to_string());
    assert_eq!(
        serde_json::from_value::<AgentJobPayload>(value).unwrap(),
        payload
    );
}

#[test]
fn agent_events_keep_existing_type_tags() {
    let event = AgentEvent::Heartbeat;
    let value = serde_json::to_value(event).unwrap();
    assert_eq!(value["type"], "heartbeat");
}

#[test]
fn storage_stats_event_uses_camelcase_and_round_trips() {
    let event = AgentEvent::StorageStats(StorageStatsEvent {
        app_id: Uuid::from_u128(1),
        used_bytes: 1_572_864,
        volumes: vec![VolumeUsage {
            name: "pgdata".into(),
            used_bytes: 1_048_576,
        }],
    });
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["type"], "storage_stats");
    assert_eq!(value["appId"], Uuid::from_u128(1).to_string());
    assert_eq!(value["usedBytes"], 1_572_864);
    assert_eq!(value["volumes"][0]["name"], "pgdata");
    assert_eq!(value["volumes"][0]["usedBytes"], 1_048_576);
    assert_eq!(serde_json::from_value::<AgentEvent>(value).unwrap(), event);
}

#[test]
fn resource_stats_event_uses_camelcase_numeric_metrics() {
    let event = ResourceStatsEvent {
        container: "hostlet-demo".into(),
        cpu_percent: "12.5%".into(),
        cpu_percent_value: Some(12.5),
        memory_usage: "12.5MiB / 1GiB".into(),
        memory_usage_bytes: Some(13_107_200),
        memory_limit_bytes: Some(1_073_741_824),
        memory_percent: "1.22%".into(),
        memory_percent_value: Some(1.22),
        network_io: "1.2kB / 0B".into(),
        network_rx_bytes: Some(1_200),
        network_tx_bytes: Some(0),
        block_io: "4.0MB / 1.0MB".into(),
        block_read_bytes: Some(4_000_000),
        block_write_bytes: Some(1_000_000),
        pids: "7".into(),
        pids_current: Some(7),
    };

    let value = serde_json::to_value(&event).unwrap();

    assert_eq!(value["cpuPercent"], "12.5%");
    assert_eq!(value["cpuPercentValue"], 12.5);
    assert_eq!(value["memoryUsageBytes"], 13_107_200);
    assert_eq!(value["networkRxBytes"], 1_200);
    assert_eq!(value["blockReadBytes"], 4_000_000);
    assert_eq!(
        serde_json::from_value::<ResourceStatsEvent>(value).unwrap(),
        event
    );
}

#[test]
fn capture_screenshot_payload_keeps_wire_shape() {
    let app_id = Uuid::from_u128(1);
    let deployment_id = Uuid::from_u128(2);
    let payload = AgentJobPayload::CaptureScreenshot(Box::new(CaptureScreenshotJob {
        app_id,
        deployment_id,
        capture_url: "https://demo.example.test/".into(),
        width: 1280,
        height: 720,
        format: "jpeg".into(),
        screenshotter_image: "local/hostlet-screenshotter:test".into(),
    }));

    let value = serde_json::to_value(&payload).unwrap();

    assert_eq!(value["type"], "capture_screenshot");
    assert_eq!(value["app_id"], app_id.to_string());
    assert_eq!(value["deployment_id"], deployment_id.to_string());
    assert_eq!(value["capture_url"], "https://demo.example.test/");
    assert_eq!(
        value["screenshotter_image"],
        "local/hostlet-screenshotter:test"
    );
    assert_eq!(
        serde_json::from_value::<AgentJobPayload>(value).unwrap(),
        payload
    );
}

#[test]
fn deployment_status_strings_match_database_values() {
    assert_eq!(DeploymentStatus::HealthChecking.as_str(), "health_checking");
    assert_eq!(DeploymentStatus::RolledBack.as_str(), "rolled_back");
    assert_eq!(
        "health_checking".parse::<DeploymentStatus>().unwrap(),
        DeploymentStatus::HealthChecking
    );
}
