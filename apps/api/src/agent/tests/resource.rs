use super::*;

pub(super) async fn assert_resource_stats_record_numeric_metrics(
    state: &AppState,
    user_id: Uuid,
    app_id: Uuid,
) {
    let container = format!("hostlet-app-{app_id}");
    handle_agent_message(
        state,
        TEST_SERVER_ID,
        serde_json::json!({
            "type": "resource_stats",
            "container": container,
            "cpuPercent": "12.5%",
            "cpuPercentValue": 12.5,
            "memoryUsage": "12.5MiB / 1GiB",
            "memoryUsageBytes": 13_107_200,
            "memoryLimitBytes": 1_073_741_824,
            "memoryPercent": "1.22%",
            "memoryPercentValue": 1.22,
            "networkIo": "1.2kB / 0B",
            "networkRxBytes": 1_200,
            "networkTxBytes": 0,
            "blockIo": "4.0MB / 1.0MB",
            "blockReadBytes": 4_000_000,
            "blockWriteBytes": 1_000_000,
            "pids": "7",
            "pidsCurrent": 7
        }),
    )
    .await;
    let row = sqlx::query(
        "SELECT cpu_percent_value,memory_usage_bytes,memory_limit_bytes,memory_percent_value,
                network_rx_bytes,network_tx_bytes,block_read_bytes,block_write_bytes,pids_current
           FROM app_resource_snapshots WHERE container_name=$1",
    )
    .bind(&container)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(row.get::<Option<f64>, _>("cpu_percent_value"), Some(12.5));
    assert_eq!(
        row.get::<Option<i64>, _>("memory_usage_bytes"),
        Some(13_107_200)
    );
    assert_eq!(
        row.get::<Option<i64>, _>("memory_limit_bytes"),
        Some(1_073_741_824)
    );
    assert_eq!(
        row.get::<Option<f64>, _>("memory_percent_value"),
        Some(1.22)
    );
    assert_eq!(row.get::<Option<i64>, _>("network_rx_bytes"), Some(1_200));
    assert_eq!(row.get::<Option<i64>, _>("network_tx_bytes"), Some(0));
    assert_eq!(
        row.get::<Option<i64>, _>("block_read_bytes"),
        Some(4_000_000)
    );
    assert_eq!(
        row.get::<Option<i64>, _>("block_write_bytes"),
        Some(1_000_000)
    );
    assert_eq!(row.get::<Option<i64>, _>("pids_current"), Some(7));

    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::COOKIE,
        crate::auth::test_session_cookie_header(state, user_id)
            .parse()
            .unwrap(),
    );
    let response = crate::web::app_resources(State(state.clone()), headers, Path(app_id))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["container"], container);
    assert_eq!(payload["cpuPercentValue"], 12.5);
    assert_eq!(payload["memoryUsageBytes"], 13_107_200);
    assert_eq!(payload["memoryLimitBytes"], 1_073_741_824);
    assert_eq!(payload["memoryPercentValue"], 1.22);
    assert_eq!(payload["networkRxBytes"], 1_200);
    assert_eq!(payload["networkTxBytes"], 0);
    assert_eq!(payload["blockReadBytes"], 4_000_000);
    assert_eq!(payload["blockWriteBytes"], 1_000_000);
    assert_eq!(payload["pidsCurrent"], 7);
}

pub(super) async fn assert_resource_stats_reject_invalid_numeric_metrics(
    state: &AppState,
    app_id: Uuid,
) {
    let container = format!("hostlet-app-{app_id}");
    handle_agent_message(
        state,
        TEST_SERVER_ID,
        serde_json::json!({
            "type": "resource_stats",
            "container": container,
            "cpuPercent": "-1%",
            "cpuPercentValue": -1.0,
            "memoryUsage": "-1B / 1PiB",
            "memoryUsageBytes": -1,
            "memoryLimitBytes": 1_125_899_906_842_625i64,
            "memoryPercent": "1000000.1%",
            "memoryPercentValue": 1_000_000.1,
            "networkIo": "-1B / 1PiB",
            "networkRxBytes": -1,
            "networkTxBytes": 1_125_899_906_842_625i64,
            "blockIo": "-1B / 1PiB",
            "blockReadBytes": -1,
            "blockWriteBytes": 1_125_899_906_842_625i64,
            "pids": "-1",
            "pidsCurrent": -1
        }),
    )
    .await;
    let row = sqlx::query(
        "SELECT cpu_percent_value,memory_usage_bytes,memory_limit_bytes,memory_percent_value,
                network_rx_bytes,network_tx_bytes,block_read_bytes,block_write_bytes,pids_current
           FROM app_resource_snapshots WHERE container_name=$1",
    )
    .bind(&container)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(row.get::<Option<f64>, _>("cpu_percent_value"), None);
    assert_eq!(row.get::<Option<i64>, _>("memory_usage_bytes"), None);
    assert_eq!(row.get::<Option<i64>, _>("memory_limit_bytes"), None);
    assert_eq!(row.get::<Option<f64>, _>("memory_percent_value"), None);
    assert_eq!(row.get::<Option<i64>, _>("network_rx_bytes"), None);
    assert_eq!(row.get::<Option<i64>, _>("network_tx_bytes"), None);
    assert_eq!(row.get::<Option<i64>, _>("block_read_bytes"), None);
    assert_eq!(row.get::<Option<i64>, _>("block_write_bytes"), None);
    assert_eq!(row.get::<Option<i64>, _>("pids_current"), None);
}
