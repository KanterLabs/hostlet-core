use super::*;

mod resource_stats;

pub(crate) use resource_stats::publish_resource_stats;

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
    post_reliable(cfg, deployment_status_event(id, status, details)).await;
}

fn deployment_status_event(id: Uuid, status: &str, details: StatusDetails<'_>) -> Value {
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
    })
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
    let size_env = format!("HOSTLET_SCREENSHOT_SIZE={width}x{height}");
    log(
        cfg,
        deployment_id,
        "stdout",
        "Capturing deployment screenshot.",
    )
    .await;
    run_screenshotter_container(
        job_id,
        screenshotter_image(payload),
        capture_url,
        &size_env,
        &output_file,
    )
    .await?;
    let bytes = tokio::fs::read(&output_file)
        .await
        .context("screenshotter did not produce an image")?;
    upload_screenshot(
        cfg,
        ScreenshotUpload {
            app_id,
            deployment_id,
            job_id,
            width,
            height,
            capture_url: capture_url.to_string(),
            bytes,
        },
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

const SCREENSHOT_CONTAINER_OUTPUT_PATH: &str = "/tmp/hostlet-screenshot.jpg";

async fn run_screenshotter_container(
    job_id: Uuid,
    image: &str,
    capture_url: &str,
    size_env: &str,
    output_file: &Path,
) -> anyhow::Result<()> {
    let container_name = screenshot_container_name(job_id);
    remove_screenshot_container(&container_name).await;
    let create_args = screenshot_create_args(&container_name, size_env, image, capture_url);
    let create_refs = create_args.iter().map(String::as_str).collect::<Vec<_>>();
    let create_output = command_output("docker", &create_refs, Duration::from_secs(30)).await?;
    if !create_output.status.success() {
        bail!(
            "screenshotter container create failed with {}: {}",
            create_output.status,
            command_combined_output(&create_output).trim()
        );
    }

    let result = async {
        let start_output = command_output(
            "docker",
            &["start", "--attach", &container_name],
            Duration::from_secs(45),
        )
        .await?;
        if !start_output.status.success() {
            bail!(
                "screenshotter exited with {}: {}",
                start_output.status,
                command_combined_output(&start_output).trim()
            );
        }

        let copy_source = format!("{container_name}:{SCREENSHOT_CONTAINER_OUTPUT_PATH}");
        let output_path = output_file.to_string_lossy().to_string();
        let copy_output = command_output(
            "docker",
            &["cp", &copy_source, &output_path],
            Duration::from_secs(15),
        )
        .await?;
        if !copy_output.status.success() {
            bail!(
                "screenshotter copy failed with {}: {}",
                copy_output.status,
                command_combined_output(&copy_output).trim()
            );
        }
        Ok(())
    }
    .await;

    remove_screenshot_container(&container_name).await;
    result
}

fn screenshot_create_args(
    container_name: &str,
    size_env: &str,
    image: &str,
    capture_url: &str,
) -> Vec<String> {
    [
        "create",
        "--name",
        container_name,
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
        size_env,
        image,
        capture_url,
        SCREENSHOT_CONTAINER_OUTPUT_PATH,
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn screenshot_container_name(job_id: Uuid) -> String {
    format!("hostlet-screenshot-{job_id}")
}

async fn remove_screenshot_container(container_name: &str) {
    let _ = command_output(
        "docker",
        &["rm", "-f", container_name],
        Duration::from_secs(15),
    )
    .await;
}

fn command_combined_output(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

struct ScreenshotUpload {
    app_id: Uuid,
    deployment_id: Uuid,
    job_id: Uuid,
    width: i64,
    height: i64,
    capture_url: String,
    bytes: Vec<u8>,
}

async fn upload_screenshot(cfg: &Config, upload: ScreenshotUpload) -> anyhow::Result<()> {
    cfg.http
        .post(format!("{}/api/agent/screenshots", cfg.api_url))
        .headers(agent_auth_headers(cfg)?)
        .header(reqwest::header::CONTENT_TYPE, "image/jpeg")
        .query(&[
            ("app_id", upload.app_id.to_string()),
            ("deployment_id", upload.deployment_id.to_string()),
            ("job_id", upload.job_id.to_string()),
            ("width", upload.width.to_string()),
            ("height", upload.height.to_string()),
            ("capture_url", upload.capture_url),
        ])
        .body(upload.bytes)
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
        &[
            "inspect",
            "-f",
            "{{.State.Running}} {{.State.Restarting}} {{.State.OOMKilled}} {{.State.ExitCode}}",
            container,
        ],
        Duration::from_secs(10),
    )
    .await?;
    if !output.status.success() {
        bail!("container does not exist");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    inspect_container_state(stdout.trim())
}

fn inspect_container_state(value: &str) -> anyhow::Result<()> {
    let mut parts = value.split_whitespace();
    let running = parts.next();
    let restarting = parts.next();
    let oom_killed = parts.next();
    let exit_code = parts.next();
    if restarting == Some("true") {
        let exit_code = exit_code.unwrap_or("unknown");
        bail!("container is restarting after exit code {exit_code}");
    }
    if oom_killed == Some("true") {
        bail!("container was OOM-killed");
    }
    if running != Some("true") {
        let exit_code = exit_code.unwrap_or("unknown");
        bail!("container is not running; last exit code {exit_code}");
    }
    Ok(())
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
        let combined = command_combined_output(&output);
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
    let combined = command_combined_output(&output);
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

#[cfg(test)]
mod status_tests {
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
    fn inspect_container_state_accepts_running_container() {
        inspect_container_state("true false false 0").unwrap();
    }

    #[test]
    fn inspect_container_state_reports_restart_loop() {
        let err = inspect_container_state("true true false 1").unwrap_err();

        assert_eq!(err.to_string(), "container is restarting after exit code 1");
    }

    #[test]
    fn inspect_container_state_reports_oom_kill() {
        let err = inspect_container_state("false false true 137").unwrap_err();

        assert_eq!(err.to_string(), "container was OOM-killed");
    }

    #[test]
    fn inspect_container_state_reports_stopped_exit_code() {
        let err = inspect_container_state("false false false 2").unwrap_err();

        assert_eq!(
            err.to_string(),
            "container is not running; last exit code 2"
        );
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

    #[test]
    fn screenshot_create_args_use_container_copy_path_without_host_bind() {
        let args = screenshot_create_args(
            "hostlet-screenshot-job",
            "HOSTLET_SCREENSHOT_SIZE=1280x720",
            "local/hostlet-screenshotter:test",
            "https://demo.example.com/",
        );

        assert_eq!(args.first().map(String::as_str), Some("create"));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--name", "hostlet-screenshot-job"]));
        assert!(!args.iter().any(|arg| arg == "-v"));
        assert!(args
            .iter()
            .any(|arg| arg == SCREENSHOT_CONTAINER_OUTPUT_PATH));
    }

    #[test]
    fn screenshot_container_name_is_job_scoped() {
        let job_id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();

        assert_eq!(
            screenshot_container_name(job_id),
            "hostlet-screenshot-11111111-2222-3333-4444-555555555555"
        );
    }
}
