use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const DOCKER_METRIC_MAX_BYTES: f64 = 1_125_899_906_842_624.0;
const DOCKER_METRIC_MAX_COUNT: i64 = 1_000_000;
const DOCKER_METRIC_MAX_PERCENT: f64 = 1_000_000.0;

/// Schedules resource telemetry outside the WebSocket select loop.
///
/// Resource stats can issue one HTTP request per running container, and an
/// event endpoint may be slow or unavailable. The scheduler is single-flight:
/// a tick arriving while a previous publish is still running is dropped rather
/// than queued. That gives the agent a bounded task count (one publisher) and
/// avoids replaying stale samples after a reconnect.
#[derive(Clone, Default)]
pub(crate) struct ResourceStatsScheduler {
    slot: Arc<AtomicBool>,
}

impl ResourceStatsScheduler {
    pub(crate) fn schedule(&self, cfg: Config) -> bool {
        self.schedule_work(async move {
            publish_resource_stats(&cfg).await;
        })
    }

    fn schedule_work<F>(&self, work: F) -> bool
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        spawn_single_flight(self.slot.clone(), work)
    }
}

struct ResourceStatsGuard(Arc<AtomicBool>);

impl Drop for ResourceStatsGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn try_acquire_resource_stats_slot(slot: &Arc<AtomicBool>) -> Option<ResourceStatsGuard> {
    slot.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .ok()
        .map(|_| ResourceStatsGuard(slot.clone()))
}

/// Spawn one detached resource publisher when no publisher is already active.
/// Keeping this generic makes the single-flight and stalled-publisher behavior
/// directly testable without invoking Docker or an HTTP server.
fn spawn_single_flight<F>(slot: Arc<AtomicBool>, work: F) -> bool
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let Some(guard) = try_acquire_resource_stats_slot(&slot) else {
        return false;
    };
    tokio::spawn(async move {
        let _guard = guard;
        work.await;
    });
    true
}

pub(crate) async fn publish_resource_stats(cfg: &Config) {
    let Ok(containers) = hostlet_containers().await else {
        return;
    };
    if containers.is_empty() {
        return;
    }
    let mut args = vec!["stats", "--no-stream", "--format", "json"];
    args.extend(containers.iter().map(String::as_str));
    let Ok(output) = command_output("docker", &args, Duration::from_secs(15)).await else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let rows = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect();
    publish_resource_stats_rows(cfg, rows).await;
}

async fn publish_resource_stats_rows(cfg: &Config, rows: Vec<Value>) {
    for raw in rows {
        let Some(container) = raw
            .get("Container")
            .or_else(|| raw.get("Name"))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        if !valid_container_name(container) {
            continue;
        }
        let cpu_percent = raw.get("CPUPerc").and_then(|v| v.as_str()).unwrap_or("0%");
        let memory_usage = raw
            .get("MemUsage")
            .and_then(|v| v.as_str())
            .unwrap_or("0B / 0B");
        let memory_percent = raw.get("MemPerc").and_then(|v| v.as_str()).unwrap_or("0%");
        let network_io = raw
            .get("NetIO")
            .and_then(|v| v.as_str())
            .unwrap_or("0B / 0B");
        let block_io = raw
            .get("BlockIO")
            .and_then(|v| v.as_str())
            .unwrap_or("0B / 0B");
        let pids = raw.get("PIDs").and_then(|v| v.as_str()).unwrap_or("0");
        let (memory_usage_bytes, memory_limit_bytes) = parse_docker_byte_pair(memory_usage);
        let (network_rx_bytes, network_tx_bytes) = parse_docker_byte_pair(network_io);
        let (block_read_bytes, block_write_bytes) = parse_docker_byte_pair(block_io);
        post(
            cfg,
            json!({
                "type": "resource_stats",
                "container": container,
                "cpuPercent": cpu_percent,
                "cpuPercentValue": parse_percent(cpu_percent),
                "memoryUsage": memory_usage,
                "memoryUsageBytes": memory_usage_bytes,
                "memoryLimitBytes": memory_limit_bytes,
                "memoryPercent": memory_percent,
                "memoryPercentValue": parse_percent(memory_percent),
                "networkIo": network_io,
                "networkRxBytes": network_rx_bytes,
                "networkTxBytes": network_tx_bytes,
                "blockIo": block_io,
                "blockReadBytes": block_read_bytes,
                "blockWriteBytes": block_write_bytes,
                "pids": pids,
                "pidsCurrent": parse_metric_count(pids)
            }),
        )
        .await;
    }
}

fn parse_percent(value: &str) -> Option<f64> {
    let percent = value.trim().trim_end_matches('%').parse::<f64>().ok()?;
    (percent.is_finite() && (0.0..=DOCKER_METRIC_MAX_PERCENT).contains(&percent)).then_some(percent)
}

fn parse_docker_byte_pair(value: &str) -> (Option<i64>, Option<i64>) {
    let mut parts = value.split('/').map(str::trim);
    let first = parts.next().and_then(parse_docker_bytes);
    let second = parts.next().and_then(parse_docker_bytes);
    (first, second)
}

fn parse_metric_count(value: &str) -> Option<i64> {
    let count = value.trim().parse::<i64>().ok()?;
    (0..=DOCKER_METRIC_MAX_COUNT)
        .contains(&count)
        .then_some(count)
}

pub(crate) fn parse_docker_bytes(value: &str) -> Option<i64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let number_len = value
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_digit() || *c == '.')
        .last()
        .map(|(idx, c)| idx + c.len_utf8())
        .unwrap_or(0);
    if number_len == 0 {
        return None;
    }
    let number = value[..number_len].parse::<f64>().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let unit = value[number_len..].trim().to_ascii_lowercase();
    let multiplier = match unit.as_str() {
        "" | "b" => 1.0,
        "kb" => 1_000.0,
        "kib" => 1024.0,
        "mb" => 1_000_000.0,
        "mib" => 1024.0 * 1024.0,
        "gb" => 1_000_000_000.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        "tb" => 1_000_000_000_000.0,
        "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let bytes = (number * multiplier).round();
    (bytes.is_finite() && (0.0..=DOCKER_METRIC_MAX_BYTES).contains(&bytes)).then_some(bytes as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::{mpsc, oneshot};

    #[test]
    fn docker_resource_stats_are_parseable_for_budget_checks() {
        assert_eq!(parse_percent("12.5%"), Some(12.5));
        assert_eq!(parse_docker_bytes("1.5MiB"), Some(1_572_864));
        assert_eq!(parse_docker_bytes("2kB"), Some(2_000));
        assert_eq!(parse_docker_bytes("1TiB"), Some(1_099_511_627_776));
        assert_eq!(parse_metric_count("7"), Some(7));
        assert_eq!(
            parse_docker_byte_pair("12.5MiB / 1GiB"),
            (Some(13_107_200), Some(1_073_741_824))
        );
        assert_eq!(parse_docker_byte_pair("1.2kB / 0B"), (Some(1_200), Some(0)));
    }

    #[test]
    fn docker_resource_stats_reject_invalid_numeric_values() {
        assert_eq!(parse_percent("-1%"), None);
        assert_eq!(parse_percent("NaN%"), None);
        assert_eq!(parse_percent("inf%"), None);
        assert_eq!(parse_percent("1000001%"), None);

        assert_eq!(parse_docker_bytes("-1B"), None);
        assert_eq!(parse_docker_bytes("NaNB"), None);
        assert_eq!(parse_docker_bytes("2PiB"), None);
        assert_eq!(parse_docker_bytes("1125899906842625B"), None);
        assert_eq!(parse_metric_count("-1"), None);
        assert_eq!(parse_metric_count("1000001"), None);
        assert_eq!(parse_docker_byte_pair("NaNB / 2PiB"), (None, None));
    }

    fn resource_stats_test_config(api_url: String) -> Config {
        Config {
            api_url,
            // Keep the stalled request alive while the mocked socket duties
            // below continue to run.
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(3600))
                .build()
                .unwrap(),
            server_id: Uuid::nil(),
            agent_token: "test-agent-token".to_string(),
            job_signing_secret: "test-signing-secret".to_string(),
            workdir: PathBuf::new(),
            local_mode: true,
            app_public_scheme: crate::runtime::AppPublicScheme::Http,
            health_host: "127.0.0.1".to_string(),
            local_router: None,
            screenshot_router: None,
            max_concurrent_jobs: 1,
        }
    }

    fn resource_stat_rows() -> Vec<Value> {
        ["hostlet-app-a", "hostlet-app-b", "hostlet-app-c"]
            .into_iter()
            .map(|container| {
                json!({
                    "Container": container,
                    "CPUPerc": "12.5%",
                    "MemUsage": "12MiB / 1GiB",
                    "MemPerc": "1.2%",
                    "NetIO": "1KiB / 2KiB",
                    "BlockIO": "3KiB / 4KiB",
                    "PIDs": "7"
                })
            })
            .collect()
    }

    async fn read_http_body(stream: &mut TcpStream) -> Vec<u8> {
        let mut bytes = Vec::new();
        let (header_end, content_length) = loop {
            let mut chunk = [0_u8; 1024];
            let read = stream.read(&mut chunk).await.unwrap();
            assert!(read > 0, "resource event request ended before its headers");
            bytes.extend_from_slice(&chunk[..read]);
            let Some(header_start) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let header_end = header_start + 4;
            let headers = String::from_utf8_lossy(&bytes[..header_start]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            break (header_end, content_length);
        };
        while bytes.len() < header_end + content_length {
            let mut chunk = [0_u8; 1024];
            let read = stream.read(&mut chunk).await.unwrap();
            assert!(read > 0, "resource event request ended before its body");
            bytes.extend_from_slice(&chunk[..read]);
        }
        bytes[header_end..header_end + content_length].to_vec()
    }

    async fn stalled_resource_event_server(
        listener: TcpListener,
        requests: mpsc::Sender<Vec<u8>>,
        mut releases: mpsc::Receiver<()>,
    ) {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let body = read_http_body(&mut stream).await;
            requests.send(body).await.unwrap();
            releases
                .recv()
                .await
                .expect("test must release every stalled resource event");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        }
    }

    fn resource_event_container(body: &[u8]) -> String {
        let event: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(
            event.get("type").and_then(Value::as_str),
            Some("resource_stats")
        );
        event
            .get("container")
            .and_then(Value::as_str)
            .unwrap()
            .to_string()
    }

    enum MockLoopEvent {
        Heartbeat,
        JobClaim,
        Health,
        Incoming(String),
    }

    fn cadence_complete(
        heartbeat_count: u8,
        claim_count: u8,
        health_count: u8,
        incoming_count: u8,
    ) -> bool {
        heartbeat_count >= 2 && claim_count >= 5 && health_count >= 2 && incoming_count >= 1
    }

    #[tokio::test]
    async fn stalled_resource_http_does_not_block_all_socket_duties() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api_url = format!("http://{}", listener.local_addr().unwrap());
        let (request_tx, mut request_rx) = mpsc::channel(3);
        let (release_tx, release_rx) = mpsc::channel(3);
        let server = tokio::spawn(stalled_resource_event_server(
            listener, request_tx, release_rx,
        ));

        let cfg = resource_stats_test_config(api_url);
        let rows = resource_stat_rows();
        let scheduler = ResourceStatsScheduler::default();
        let (publisher_done_tx, publisher_done_rx) = oneshot::channel();
        let publisher_cfg = cfg.clone();
        let publisher_rows = rows.clone();
        assert!(scheduler.schedule_work(async move {
            publish_resource_stats_rows(&publisher_cfg, publisher_rows).await;
            let _ = publisher_done_tx.send(());
        }));

        let first_body = request_rx
            .recv()
            .await
            .expect("first resource sample should reach the HTTP endpoint");
        assert_eq!(resource_event_container(&first_body), "hostlet-app-a");

        // The endpoint has not released the first sample, so the publisher is
        // still in flight. A second resource tick is dropped by policy.
        let retry_cfg = cfg.clone();
        assert!(!scheduler.schedule_work(async move {
            publish_resource_stats_rows(&retry_cfg, rows).await;
        }));

        // This is the WebSocket loop's mocked cadence harness: its timer arms
        // and incoming-message arm all share one select, just like connect_loop.
        // Mocked ticks make the assertions deterministic without wall-clock
        // timing or sleeps.
        let (incoming_tx, mut incoming_rx) = mpsc::channel::<Message>(1);
        let (incoming_seen_tx, incoming_seen_rx) = oneshot::channel();
        let (loop_event_tx, mut loop_event_rx) = mpsc::channel::<MockLoopEvent>(10);
        let (cadence_ready_tx, cadence_ready_rx) = oneshot::channel();
        let cadence = tokio::spawn(async move {
            let _ = cadence_ready_tx.send(());
            let mut heartbeat_count = 0;
            let mut claim_count = 0;
            let mut health_count = 0;
            let mut incoming_count = 0;
            let mut incoming_seen_tx = Some(incoming_seen_tx);
            while !cadence_complete(heartbeat_count, claim_count, health_count, incoming_count) {
                tokio::select! {
                    Some(event) = loop_event_rx.recv() => match event {
                        MockLoopEvent::Heartbeat => heartbeat_count += 1,
                        MockLoopEvent::JobClaim => claim_count += 1,
                        MockLoopEvent::Health => health_count += 1,
                        MockLoopEvent::Incoming(message) => {
                            incoming_tx
                                .send(Message::Text(message))
                                .await
                                .unwrap();
                        }
                    },
                    Some(Message::Text(message)) = incoming_rx.recv() => {
                        assert_eq!(message, "incoming-websocket-message");
                        incoming_count += 1;
                        if let Some(sender) = incoming_seen_tx.take() {
                            let _ = sender.send(message);
                        }
                    }
                }
            }
            (heartbeat_count, claim_count, health_count, incoming_count)
        });
        cadence_ready_rx.await.unwrap();
        loop_event_tx.send(MockLoopEvent::Heartbeat).await.unwrap();
        loop_event_tx.send(MockLoopEvent::Heartbeat).await.unwrap();
        for _ in 0..5 {
            loop_event_tx.send(MockLoopEvent::JobClaim).await.unwrap();
        }
        for _ in 0..2 {
            loop_event_tx.send(MockLoopEvent::Health).await.unwrap();
        }
        loop_event_tx
            .send(MockLoopEvent::Incoming(
                "incoming-websocket-message".to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(
            incoming_seen_rx.await.unwrap(),
            "incoming-websocket-message"
        );

        assert_eq!(cadence.await.unwrap(), (2, 5, 2, 1));

        // Release each sample one at a time. The same stalled endpoint sees
        // all three resource events, while the socket duties above completed
        // before any HTTP response was released.
        release_tx.send(()).await.unwrap();
        let second_body = request_rx
            .recv()
            .await
            .expect("second resource sample should reach the HTTP endpoint");
        assert_eq!(resource_event_container(&second_body), "hostlet-app-b");

        release_tx.send(()).await.unwrap();
        let third_body = request_rx
            .recv()
            .await
            .expect("third resource sample should reach the HTTP endpoint");
        assert_eq!(resource_event_container(&third_body), "hostlet-app-c");

        release_tx.send(()).await.unwrap();
        publisher_done_rx.await.unwrap();
        assert!(!scheduler.slot.load(Ordering::SeqCst));
        server.await.unwrap();
    }
}
