use super::*;

pub(crate) async fn write_route_file(target: &Path, contents: &str) -> anyhow::Result<()> {
    let tmp = target.with_extension(format!("caddy.tmp-{}", std::process::id()));
    tokio::fs::write(&tmp, contents).await?;
    tokio::fs::rename(tmp, target).await?;
    Ok(())
}

pub(crate) async fn restore_route_file(
    target: &Path,
    previous: Option<Vec<u8>>,
) -> anyhow::Result<()> {
    if let Some(contents) = previous {
        tokio::fs::write(target, contents).await?;
    } else {
        match tokio::fs::remove_file(target).await {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

pub(crate) async fn remove_local_caddy_route(
    router: &LocalRouter,
    app: &str,
) -> anyhow::Result<()> {
    let target = router.snippets_dir.join(format!("{app}.caddy"));
    match tokio::fs::remove_file(target).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

pub(crate) async fn ensure_no_conflicting_route(
    dir: &Path,
    target: &Path,
    domain: &str,
) -> anyhow::Result<()> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path == target || path.extension().and_then(|value| value.to_str()) != Some("caddy") {
            continue;
        }
        let Ok(contents) = tokio::fs::read_to_string(&path).await else {
            continue;
        };
        if route_domain(&contents).is_some_and(|existing| existing == domain) {
            bail!("another Hostlet route already uses domain {domain}");
        }
    }
    Ok(())
}

pub(crate) fn route_domain(contents: &str) -> Option<&str> {
    for line in contents.lines().map(str::trim) {
        if let Some(domain) = line.strip_prefix("# hostlet-domain:") {
            return Some(domain.trim());
        }
        if let Some((_, domain)) = line.split_once(" host ") {
            return Some(domain.trim());
        }
        if let Some(domain) = line.strip_suffix(" {") {
            return Some(domain.trim());
        }
    }
    None
}

pub(crate) async fn run_router_reload(
    cfg: &Config,
    deployment_id: Uuid,
    router: &LocalRouter,
) -> anyhow::Result<()> {
    let Some((bin, args)) = router.reload_command.split_first() else {
        return Ok(());
    };
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    run_log(cfg, deployment_id, bin, &args).await
}

pub(crate) async fn run_router_reload_quiet(router: &LocalRouter) -> anyhow::Result<()> {
    let Some((bin, args)) = router.reload_command.split_first() else {
        return Ok(());
    };
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    run_quiet(bin, &args).await
}

pub(crate) async fn status(cfg: &Config, id: Uuid, status: &str, failure: Option<&str>) {
    status_extra(
        cfg,
        id,
        status,
        StatusDetails {
            failure,
            ..StatusDetails::default()
        },
    )
    .await;
}

#[derive(Default)]
pub(crate) struct StatusDetails<'a> {
    pub(crate) failure: Option<&'a str>,
    pub(crate) image: Option<&'a str>,
    pub(crate) container: Option<&'a str>,
    pub(crate) local_url: Option<&'a str>,
    pub(crate) published_port: Option<u16>,
    pub(crate) compose_project: Option<&'a str>,
    pub(crate) runtime_metadata: Option<Value>,
}

pub(crate) async fn status_extra(cfg: &Config, id: Uuid, status: &str, details: StatusDetails<'_>) {
    post_reliable(
        cfg,
        json!({
            "type": "deployment_status",
            "deployment_id": id,
            "status": status,
            "failure": details.failure,
            "image_tag": details.image,
            "container_name": details.container,
            "local_url": details.local_url,
            "published_port": details.published_port,
            "compose_project": details.compose_project,
            "runtime_metadata": details.runtime_metadata,
        }),
    )
    .await;
}

pub(crate) async fn log(cfg: &Config, id: Uuid, stream: &str, line: &str) {
    post(
        cfg,
        json!({"type":"log","deployment_id":id,"stream":stream,"line":line}),
    )
    .await;
}

pub(crate) async fn job_status(cfg: &Config, id: Uuid, status: &str, failure: Option<&str>) {
    post_reliable(
        cfg,
        json!({"type":"job_status","job_id":id,"status":status,"failure":failure}),
    )
    .await;
}

pub(crate) async fn post(cfg: &Config, msg: Value) {
    let _ = send_event(cfg, &msg).await;
}

pub(crate) async fn post_reliable(cfg: &Config, msg: Value) {
    let attempts = event_retry_delays();
    for attempt in 0..attempts.len() {
        match send_event(cfg, &msg).await {
            Ok(()) => return,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    attempt = attempt + 1,
                    event_type = msg.get("type").and_then(|value| value.as_str()).unwrap_or("unknown"),
                    "failed to post agent event"
                );
                if let Some(delay) = attempts.get(attempt + 1) {
                    tokio::time::sleep(*delay).await;
                }
            }
        }
    }
}

pub(crate) async fn send_event(cfg: &Config, msg: &Value) -> anyhow::Result<()> {
    cfg.http
        .post(format!("{}/api/agent/events", cfg.api_url))
        .header("x-hostlet-server-id", cfg.server_id.to_string())
        .header("x-hostlet-agent-token", &cfg.agent_token)
        .json(msg)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

pub(crate) fn event_retry_delays() -> [Duration; 4] {
    [
        Duration::from_millis(0),
        Duration::from_millis(250),
        Duration::from_secs(1),
        Duration::from_secs(3),
    ]
}

#[derive(Default)]
pub(crate) struct HealthCounts {
    failures: u32,
    successes: u32,
}

pub(crate) struct HealthTarget {
    app_id: Uuid,
    deployment_id: Uuid,
    container_name: String,
    pub(crate) published_port: u16,
    health_path: String,
}

/// Builds a `health_status` event payload for a probed target. Centralizes the
/// wire shape shared by scheduled, on-demand, and restart health reporting.
fn health_status_event(
    target: &HealthTarget,
    result: &HealthProbeResult,
    status: &str,
    failure_count: u32,
    success_count: u32,
) -> Value {
    json!({
        "type": "health_status",
        "app_id": target.app_id,
        "deployment_id": target.deployment_id,
        "container_name": target.container_name,
        "status": status,
        "checked_url": result.url,
        "http_status": result.http_status,
        "latency_ms": result.latency_ms,
        "failure_count": failure_count,
        "success_count": success_count,
        "error": result.error,
    })
}

/// `health_status` event for a one-shot probe, where the status is simply
/// healthy or degraded and counts reflect this single observation.
fn single_probe_health_event(target: &HealthTarget, result: &HealthProbeResult) -> Value {
    let status = if result.healthy {
        "healthy"
    } else {
        "degraded"
    };
    let (failure_count, success_count) = if result.healthy { (0, 1) } else { (1, 0) };
    health_status_event(target, result, status, failure_count, success_count)
}

pub(crate) async fn publish_runtime_health(cfg: &Config, counts: &mut HashMap<Uuid, HealthCounts>) {
    let Ok(targets) = health_targets(cfg).await else {
        return;
    };
    for target in targets {
        let result = probe_health_target(cfg, &target).await;
        let entry = counts.entry(target.app_id).or_default();
        if result.healthy {
            entry.successes = entry.successes.saturating_add(1);
            entry.failures = 0;
        } else {
            entry.failures = entry.failures.saturating_add(1);
        }
        let status = if result.healthy {
            "healthy"
        } else if entry.failures >= 3 {
            "unhealthy"
        } else {
            "degraded"
        };
        post(
            cfg,
            health_status_event(&target, &result, status, entry.failures, entry.successes),
        )
        .await;
    }
}

pub(crate) async fn health_check_job(cfg: &Config, payload: &Value) {
    let Some(target) = health_target_from_payload(payload) else {
        return;
    };
    let result = probe_health_target(cfg, &target).await;
    post(cfg, single_probe_health_event(&target, &result)).await;
}

pub(crate) async fn restart_container_job(cfg: &Config, payload: &Value) -> anyhow::Result<()> {
    let Some(target) = health_target_from_payload(payload) else {
        bail!("restart job missing valid health target");
    };
    run_quiet("docker", &["restart", &target.container_name]).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let result = probe_health_target(cfg, &target).await;
    post(cfg, single_probe_health_event(&target, &result)).await;
    Ok(())
}

pub(crate) async fn capture_screenshot_job(cfg: &Config, payload: &Value) -> anyhow::Result<()> {
    let app_id = payload_uuid(payload, "app_id").context("screenshot job missing app_id")?;
    let deployment_id =
        payload_uuid(payload, "deployment_id").context("screenshot job missing deployment_id")?;
    let job_id = payload_uuid(payload, "job_id").context("screenshot job missing job_id")?;
    let capture_url = payload
        .get("capture_url")
        .and_then(|value| value.as_str())
        .context("screenshot job missing capture_url")?;
    validate_capture_url(capture_url)?;
    let width = payload
        .get("width")
        .and_then(|value| value.as_i64())
        .filter(|value| (1..=4096).contains(value))
        .unwrap_or(1280);
    let height = payload
        .get("height")
        .and_then(|value| value.as_i64())
        .filter(|value| (1..=4096).contains(value))
        .unwrap_or(720);
    let output_dir = cfg.workdir.join("screenshots").join(job_id.to_string());
    tokio::fs::create_dir_all(&output_dir).await?;
    let output_file = output_dir.join("screenshot.jpg");
    let output_dir_string = output_dir.to_string_lossy().to_string();
    let size_env = format!("HOSTLET_SCREENSHOT_SIZE={width}x{height}");
    let volume_arg = format!("{output_dir_string}:/out");
    let args = [
        "run",
        "--rm",
        "--network",
        "host",
        "--security-opt",
        "no-new-privileges:true",
        "--cap-drop",
        "ALL",
        "--memory",
        "512m",
        "--cpus",
        "1",
        "--tmpfs",
        "/tmp:rw,nosuid,size=256m",
        "-e",
        &size_env,
        "-v",
        &volume_arg,
        screenshotter_image(payload),
        capture_url,
        "/out/screenshot.jpg",
    ];
    log(
        cfg,
        deployment_id,
        "stdout",
        "Capturing deployment screenshot.",
    )
    .await;
    let output = command_output("docker", &args, Duration::from_secs(45)).await?;
    if !output.status.success() {
        let combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        bail!(
            "screenshotter exited with {}: {}",
            output.status,
            combined.trim()
        );
    }
    let bytes = tokio::fs::read(&output_file)
        .await
        .context("screenshotter did not produce an image")?;
    upload_screenshot(
        cfg,
        app_id,
        deployment_id,
        job_id,
        width,
        height,
        capture_url,
        bytes,
    )
    .await?;
    let _ = tokio::fs::remove_dir_all(output_dir).await;
    log(
        cfg,
        deployment_id,
        "stdout",
        "Deployment screenshot captured.",
    )
    .await;
    Ok(())
}

async fn upload_screenshot(
    cfg: &Config,
    app_id: Uuid,
    deployment_id: Uuid,
    job_id: Uuid,
    width: i64,
    height: i64,
    capture_url: &str,
    bytes: Vec<u8>,
) -> anyhow::Result<()> {
    cfg.http
        .post(format!("{}/api/agent/screenshots", cfg.api_url))
        .headers(agent_auth_headers(cfg)?)
        .header(reqwest::header::CONTENT_TYPE, "image/jpeg")
        .query(&[
            ("app_id", app_id.to_string()),
            ("deployment_id", deployment_id.to_string()),
            ("job_id", job_id.to_string()),
            ("width", width.to_string()),
            ("height", height.to_string()),
            ("capture_url", capture_url.to_string()),
        ])
        .body(bytes)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

fn agent_auth_headers(cfg: &Config) -> anyhow::Result<reqwest::header::HeaderMap> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-hostlet-server-id", cfg.server_id.to_string().parse()?);
    headers.insert("x-hostlet-agent-token", cfg.agent_token.parse()?);
    Ok(headers)
}

fn screenshotter_image(payload: &Value) -> &str {
    payload
        .get("screenshotter_image")
        .and_then(|value| value.as_str())
        .unwrap_or("hostlet-screenshotter:latest")
}

fn payload_uuid(payload: &Value, key: &str) -> Option<Uuid> {
    payload
        .get(key)
        .and_then(|value| value.as_str())
        .and_then(|value| Uuid::parse_str(value).ok())
}

fn validate_capture_url(value: &str) -> anyhow::Result<()> {
    let url = url::Url::parse(value).context("capture_url must be an absolute URL")?;
    match url.scheme() {
        "http" | "https" => Ok(()),
        _ => bail!("capture_url must use http or https"),
    }
}

#[cfg(test)]
mod screenshot_tests {
    use super::*;

    #[test]
    fn validate_capture_url_accepts_http_and_https() {
        assert!(validate_capture_url("http://localhost:3000/").is_ok());
        assert!(validate_capture_url("https://demo.example.com/").is_ok());
    }

    #[test]
    fn validate_capture_url_rejects_non_http_schemes() {
        assert!(validate_capture_url("file:///etc/passwd").is_err());
    }
}

pub(crate) async fn health_targets(cfg: &Config) -> anyhow::Result<Vec<HealthTarget>> {
    let raw = cfg
        .http
        .get(format!("{}/api/agent/health-targets", cfg.api_url))
        .header("x-hostlet-server-id", cfg.server_id.to_string())
        .header("x-hostlet-agent-token", &cfg.agent_token)
        .send()
        .await?
        .error_for_status()?
        .json::<Vec<Value>>()
        .await?;
    Ok(raw
        .iter()
        .filter_map(health_target_from_payload)
        .collect::<Vec<_>>())
}

pub(crate) fn health_target_from_payload(value: &Value) -> Option<HealthTarget> {
    let app_id = value
        .get("appId")
        .or_else(|| value.get("app_id"))
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok())?;
    let deployment_id = value
        .get("deploymentId")
        .or_else(|| value.get("deployment_id"))
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok())?;
    let container_name = value
        .get("containerName")
        .or_else(|| value.get("container_name"))
        .and_then(|v| v.as_str())?
        .to_string();
    if !valid_container_name(&container_name) {
        return None;
    }
    let published_port = value
        .get("publishedPort")
        .or_else(|| value.get("published_port"))
        .and_then(|v| v.as_i64())
        .and_then(|v| (1..=65_535).contains(&v).then_some(v as u16))?;
    let health_path = value
        .get("healthPath")
        .or_else(|| value.get("health_path"))
        .and_then(|v| v.as_str())
        .unwrap_or("/");
    if validate_health_path(health_path).is_err() {
        return None;
    }
    Some(HealthTarget {
        app_id,
        deployment_id,
        container_name,
        published_port,
        health_path: health_path.to_string(),
    })
}

pub(crate) struct HealthProbeResult {
    healthy: bool,
    url: String,
    http_status: Option<u16>,
    latency_ms: u128,
    error: Option<String>,
}

pub(crate) async fn probe_health_target(cfg: &Config, target: &HealthTarget) -> HealthProbeResult {
    let url = format!(
        "http://{}:{}{}",
        cfg.health_host, target.published_port, target.health_path
    );
    let started = std::time::Instant::now();
    let running = container_running(&target.container_name).await;
    if let Err(err) = running {
        return HealthProbeResult {
            healthy: false,
            url,
            http_status: None,
            latency_ms: started.elapsed().as_millis(),
            error: Some(err.to_string()),
        };
    }
    match cfg
        .http
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            HealthProbeResult {
                healthy: status.is_success() || status.is_redirection(),
                url,
                http_status: Some(status.as_u16()),
                latency_ms: started.elapsed().as_millis(),
                error: health_error_for_status(status),
            }
        }
        Err(err) => HealthProbeResult {
            healthy: false,
            url,
            http_status: None,
            latency_ms: started.elapsed().as_millis(),
            error: Some(err.to_string()),
        },
    }
}

pub(crate) fn health_error_for_status(status: StatusCode) -> Option<String> {
    if status.is_success() || status.is_redirection() {
        None
    } else {
        Some(format!("HTTP {status}"))
    }
}

pub(crate) async fn container_running(container: &str) -> anyhow::Result<()> {
    let output = command_output(
        "docker",
        &["inspect", "-f", "{{.State.Running}}", container],
        Duration::from_secs(10),
    )
    .await?;
    if !output.status.success() {
        bail!("container does not exist");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim() != "true" {
        bail!("container is not running");
    }
    Ok(())
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
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(raw) = serde_json::from_str::<Value>(line) else {
            continue;
        };
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
        post(
            cfg,
            json!({
                "type": "resource_stats",
                "container": container,
                "cpuPercent": raw.get("CPUPerc").and_then(|v| v.as_str()).unwrap_or("0%"),
                "memoryUsage": raw.get("MemUsage").and_then(|v| v.as_str()).unwrap_or("0B / 0B"),
                "memoryPercent": raw.get("MemPerc").and_then(|v| v.as_str()).unwrap_or("0%"),
                "networkIo": raw.get("NetIO").and_then(|v| v.as_str()).unwrap_or("0B / 0B"),
                "blockIo": raw.get("BlockIO").and_then(|v| v.as_str()).unwrap_or("0B / 0B"),
                "pids": raw.get("PIDs").and_then(|v| v.as_str()).unwrap_or("0")
            }),
        )
        .await;
    }
}

pub(crate) async fn hostlet_containers() -> anyhow::Result<Vec<String>> {
    list_hostlet_containers(false).await
}

pub(crate) async fn hostlet_containers_all() -> anyhow::Result<Vec<String>> {
    list_hostlet_containers(true).await
}

/// Lists Hostlet-managed container names via `docker ps`. When `include_all` is
/// set, stopped containers are included (`docker ps -a`).
async fn list_hostlet_containers(include_all: bool) -> anyhow::Result<Vec<String>> {
    let mut args = vec!["ps"];
    if include_all {
        args.push("-a");
    }
    args.extend_from_slice(&["--filter", "name=^/hostlet-", "--format", "{{.Names}}"]);
    let output = command_output("docker", &args, Duration::from_secs(15)).await?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    let stdout = String::from_utf8(output.stdout)?;
    Ok(stdout
        .lines()
        .map(str::trim)
        .filter(|name| valid_container_name(name))
        .map(str::to_string)
        .collect())
}

pub(crate) async fn hostlet_images() -> anyhow::Result<Vec<String>> {
    let output = command_output(
        "docker",
        &[
            "images",
            "hostlet/*",
            "--format",
            "{{.Repository}}:{{.Tag}}",
        ],
        Duration::from_secs(15),
    )
    .await?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    let stdout = String::from_utf8(output.stdout)?;
    Ok(stdout
        .lines()
        .map(str::trim)
        .filter(|name| valid_hostlet_image(name))
        .map(str::to_string)
        .collect())
}

pub(crate) async fn docker_compose_managed_container(container: &str) -> anyhow::Result<bool> {
    if !valid_container_name(container) {
        bail!("refusing to inspect invalid managed container name");
    }
    let output = command_output(
        "docker",
        &[
            "inspect",
            "--format",
            "{{ index .Config.Labels \"com.docker.compose.project\" }}",
            container,
        ],
        Duration::from_secs(15),
    )
    .await?;
    if !output.status.success() {
        let combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if combined.contains("No such object") {
            return Ok(true);
        }
        bail!("docker exited with {}: {}", output.status, combined.trim());
    }
    Ok(!String::from_utf8(output.stdout)?.trim().is_empty())
}

pub(crate) fn string_set_from_array(value: Option<&Value>) -> HashSet<String> {
    value
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .map(str::to_string)
        .collect()
}

pub(crate) async fn command_output(
    bin: &str,
    args: &[&str],
    timeout: Duration,
) -> anyhow::Result<Output> {
    let mut cmd = Command::new(bin);
    cmd.args(args).kill_on_drop(true);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(output) => output.with_context(|| format!("failed to start {bin}")),
        Err(_) => bail!("{bin} timed out after {} seconds", timeout.as_secs()),
    }
}

pub(crate) async fn log_docker_tooling() {
    match command_output("docker", &["version"], Duration::from_secs(10)).await {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            tracing::info!(docker = %version.lines().next().unwrap_or("available"), "Docker CLI available");
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(error = %stderr.trim(), "Docker CLI check failed");
        }
        Err(err) => tracing::warn!(error = %err, "Docker CLI is not available"),
    }
    match command_output("docker", &["compose", "version"], Duration::from_secs(10)).await {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            tracing::info!(compose = %version.trim(), "Docker Compose v2 available");
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(
                error = %stderr.trim(),
                "Docker Compose v2 is missing; Compose apps require the docker-compose CLI plugin"
            );
        }
        Err(err) => tracing::warn!(
            error = %err,
            "Docker Compose v2 is missing; Compose apps require the docker-compose CLI plugin"
        ),
    }
}

pub(crate) async fn ensure_docker_compose() -> anyhow::Result<()> {
    let output = command_output("docker", &["compose", "version"], Duration::from_secs(10)).await?;
    if output.status.success() {
        return Ok(());
    }
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    bail!(
        "Docker Compose v2 is required for Compose apps; install or mount the docker-compose CLI plugin. {}",
        combined.trim()
    );
}

pub(crate) fn http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .user_agent("Hostlet-Agent")
        .build()
        .context("failed to build HTTP client")
}

pub(crate) fn valid_container_name(value: &str) -> bool {
    value.starts_with("hostlet-")
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}
