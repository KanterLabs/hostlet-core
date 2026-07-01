use super::*;

mod capture_url;
mod health;
mod reconcile;
mod resource_stats;
#[cfg(test)]
mod screenshot_tests;
#[cfg(test)]
mod status_tests;
mod storage_stats;

use capture_url::validate_capture_url;
pub(crate) use health::CONTAINER_STATE_INSPECT_FORMAT;
use health::{
    failed_health_probe, health_status_event, health_target_from_payload, health_targets,
    probe_health_target, single_probe_health_event, HealthProbeResult, HealthTarget,
};
use reconcile::{
    container_actual_from_state, decide_reconcile, ContainerActual, ReconcileDecision,
};
pub(crate) use resource_stats::{parse_docker_bytes, publish_resource_stats};
pub(crate) use storage_stats::publish_storage_stats;

/// Build a unique temp path in the *same directory* as the final route file so
/// the write + atomic rename never crosses a filesystem boundary. A per-process
/// PID is not sufficient: a deploy and a runtime health-repair can rewrite the
/// same app's `.caddy` file concurrently within a single agent process, so the
/// random UUID suffix guarantees two writers never share a temp path and cannot
/// clobber each other's in-flight write.
fn route_temp_path(target: &Path) -> PathBuf {
    target.with_extension(format!(
        "caddy.tmp-{}-{}",
        std::process::id(),
        Uuid::new_v4()
    ))
}

pub(crate) async fn write_route_file(target: &Path, contents: &str) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    let tmp = route_temp_path(target);
    // `create_new` refuses to open a path that already exists, so even in the
    // astronomically unlikely event of a UUID collision we never truncate a
    // temp file a concurrent writer is still using.
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .await?;
    file.write_all(contents.as_bytes()).await?;
    file.flush().await?;
    drop(file);
    tokio::fs::rename(&tmp, target).await?;
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
    /// Per-service rows for a multi-service (Compose) deployment, serialized
    /// from `[DeploymentServiceReport]`. Only populated on the success path.
    pub(crate) services: Option<Value>,
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
        "services": details.services,
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

/// Maximum number of reconcile_request events the agent will emit per failure
/// streak before giving up and waiting for the streak to reset.
const MAX_REPAIR_ATTEMPTS: u32 = 3;

#[derive(Default)]
pub(crate) struct HealthCounts {
    failures: u32,
    successes: u32,
    auto_start_attempted: bool,
    /// Set when a `reconcile_request` has been posted for the current failure
    /// streak. Reset to `false` when the app reports healthy again.
    repair_requested: bool,
    /// Total reconcile_request events posted in the current failure streak.
    /// Capped at `MAX_REPAIR_ATTEMPTS` to prevent a hot repair loop.
    repair_attempts: u32,
}

pub(crate) async fn publish_runtime_health(cfg: &Config, counts: &mut HashMap<Uuid, HealthCounts>) {
    let Ok(targets) = health_targets(cfg).await else {
        return;
    };
    for mut target in targets {
        let mut result = probe_health_target(cfg, &mut target).await;
        let entry = counts.entry(target.app_id).or_default();
        record_health_probe(entry, &result);
        // Only act on unhealthy containers once the 3-failure threshold is met.
        if entry.failures >= 3 {
            match container_actual_from_state(result.container_state.as_ref()) {
                Some(ContainerActual::Stopped) => {
                    // Existing path: inline docker start for stopped containers.
                    if !entry.auto_start_attempted {
                        result = auto_start_container(cfg, &mut target, entry).await;
                    }
                }
                // New path: request a redeploy for completely-removed containers.
                Some(ContainerActual::Missing)
                    if !entry.repair_requested && entry.repair_attempts < MAX_REPAIR_ATTEMPTS =>
                {
                    let image_tag = format!(
                        "hostlet/{}:{}",
                        app_slug(&format!("app-{}", target.app_id)),
                        target.deployment_id
                    );
                    let image_present =
                        docker_image_present(&image_tag, target.deployment_id).await;
                    // In-flight is keyed on `repair_requested` only — NOT on
                    // `auto_start_attempted`. A container that was stopped (and
                    // auto-started) earlier in the same failure streak and then
                    // removed must still be rebuilt; the stopped-path flag must
                    // not suppress the missing-path repair.
                    let decision = decide_reconcile(
                        ContainerActual::Missing,
                        image_present,
                        entry.repair_requested,
                    );
                    if matches!(
                        decision,
                        ReconcileDecision::Rebuild | ReconcileDecision::RebuildImageGone
                    ) {
                        request_app_rebuild(
                            cfg,
                            target.app_id,
                            target.deployment_id,
                            image_present,
                        )
                        .await;
                        entry.repair_requested = true;
                        entry.repair_attempts += 1;
                    }
                }
                _ => {}
            }
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

fn record_health_probe(entry: &mut HealthCounts, result: &HealthProbeResult) {
    if result.healthy {
        entry.successes = entry.successes.saturating_add(1);
        entry.failures = 0;
        entry.auto_start_attempted = false;
        entry.repair_requested = false;
        entry.repair_attempts = 0;
    } else {
        entry.failures = entry.failures.saturating_add(1);
    }
}

async fn auto_start_container(
    cfg: &Config,
    target: &mut HealthTarget,
    entry: &mut HealthCounts,
) -> HealthProbeResult {
    entry.auto_start_attempted = true;
    log(
        cfg,
        target.deployment_id,
        "stdout",
        &format!(
            "Health checks failed and container is stopped; starting {}.",
            target.container_name
        ),
    )
    .await;
    match run_quiet("docker", &["start", &target.container_name]).await {
        Ok(()) => {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let result = probe_health_target(cfg, target).await;
            if result.healthy {
                entry.successes = entry.successes.saturating_add(1);
                entry.failures = 0;
                entry.auto_start_attempted = false;
                log(
                    cfg,
                    target.deployment_id,
                    "stdout",
                    &format!("Auto-started container {}.", target.container_name),
                )
                .await;
            }
            result
        }
        Err(err) => {
            let result = failed_health_probe(cfg, target, format!("auto-start failed: {err}"));
            log(
                cfg,
                target.deployment_id,
                "stderr",
                result.error.as_deref().unwrap_or("auto-start failed"),
            )
            .await;
            result
        }
    }
}

/// Check whether a Docker image tag is present on the local daemon.
/// On any error (timeout, docker not available, inspect error) treats the image
/// as absent so we still request a redeploy — never let an inspect failure
/// suppress the repair.
async fn docker_image_present(image_tag: &str, deployment_id: Uuid) -> bool {
    match command_output(
        "docker",
        &["image", "inspect", "--format", "{{.Id}}", image_tag],
        Duration::from_secs(5),
    )
    .await
    {
        Ok(output) if output.status.success() => true,
        Ok(_) => false,
        Err(err) => {
            tracing::debug!(
                %deployment_id,
                error = %err,
                image = image_tag,
                "docker image inspect failed; treating image as absent for reconcile decision"
            );
            false
        }
    }
}

/// Post a `reconcile_request` agent event asking the API to enqueue a fresh
/// deploy job for the app whose container has been removed.
async fn request_app_rebuild(cfg: &Config, app_id: Uuid, deployment_id: Uuid, image_present: bool) {
    tracing::warn!(
        %app_id,
        %deployment_id,
        image_present,
        "Container for current deployment is missing; requesting redeploy from source."
    );
    post(
        cfg,
        json!({
            "type": "reconcile_request",
            "app_id": app_id,
            "deployment_id": deployment_id,
            "reason": "rebuild",
            "image_present": image_present,
        }),
    )
    .await;
}

pub(crate) async fn health_check_job(cfg: &Config, payload: &Value) {
    let Some(mut target) = health_target_from_payload(payload) else {
        return;
    };
    let result = probe_health_target(cfg, &mut target).await;
    post(cfg, single_probe_health_event(&target, &result)).await;
}

pub(crate) async fn restart_container_job(cfg: &Config, payload: &Value) -> anyhow::Result<()> {
    let Some(target) = health_target_from_payload(payload) else {
        bail!("restart job missing valid health target");
    };
    run_quiet("docker", &["restart", &target.container_name]).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut target = target;
    let result = probe_health_target(cfg, &mut target).await;
    post(cfg, single_probe_health_event(&target, &result)).await;
    Ok(())
}

/// Stop (without removing) the container for a suspended app — the reversible
/// counterpart to app deletion, used when Hostlet Cloud billing goes inactive.
/// `docker stop` on an already-stopped or already-removed container is treated
/// as success (idempotent), since the caller (the billing reaper) may retry
/// after a partial failure. Reactivation reuses [`restart_container_job`]
/// (`docker restart` also starts a stopped container), so no "start" job type
/// is needed.
pub(crate) async fn stop_container_job(payload: &Value) -> anyhow::Result<()> {
    let Some(target) = health_target_from_payload(payload) else {
        bail!("stop job missing valid health target");
    };
    run_quiet_absent_ok(
        "docker",
        &["stop", &target.container_name],
        &["No such container"],
    )
    .await
}

/// Concise, user-facing reasons for a failed screenshot capture. They flow
/// verbatim to the dashboard via the agent job's `failure_summary` field.
const SCREENSHOT_ERR_TIMEOUT: &str = "Timed out loading the page";
const SCREENSHOT_ERR_BLOCKED: &str = "Blocked a private or loopback address";
const SCREENSHOT_ERR_SITE: &str = "The site returned an error";
const SCREENSHOT_ERR_SERVICE: &str = "Screenshot service crashed";
const SCREENSHOT_ERR_UPLOAD: &str = "Failed to upload the screenshot";

/// Classifies a screenshot-pipeline error into one of the reasons above by
/// matching stable Docker/Playwright/SSRF-guard phrases in the whole error
/// chain; unrecognized output falls back to a generic service crash.
fn screenshot_failure_reason(err: &anyhow::Error) -> &'static str {
    let detail = format!("{err:#}").to_ascii_lowercase();
    if detail.contains("private or local address")
        || detail.contains("public hostname")
        || detail.contains("blocked request")
        || detail.contains("blocked redirect")
        || detail.contains("err_blocked_by_client")
    {
        SCREENSHOT_ERR_BLOCKED
    } else if detail.contains("timeout") || detail.contains("timed out") {
        SCREENSHOT_ERR_TIMEOUT
    } else if detail.contains("net::err") || detail.contains("too many redirects") {
        SCREENSHOT_ERR_SITE
    } else {
        SCREENSHOT_ERR_SERVICE
    }
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
    if let Err(err) = validate_capture_url(capture_url) {
        return Err(reported_deployment_failure(
            screenshot_failure_reason(&err).to_string(),
        ));
    }
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
    // All work after create_dir_all runs inside an async block so that
    // output_dir is unconditionally removed on every exit path, mirroring
    // run_screenshotter_container's own cleanup pattern.
    let result: anyhow::Result<()> = async {
        let output_file = output_dir.join("screenshot.jpg");
        let size_env = format!("HOSTLET_SCREENSHOT_SIZE={width}x{height}");
        log(
            cfg,
            deployment_id,
            "stdout",
            "Capturing deployment screenshot.",
        )
        .await;
        // Run + read under one categorized boundary so failures map to a reason.
        let capture: anyhow::Result<Vec<u8>> = async {
            run_screenshotter_container(
                job_id,
                screenshotter_image(payload),
                capture_url,
                &size_env,
                &output_file,
            )
            .await?;
            tokio::fs::read(&output_file)
                .await
                .context("screenshotter did not produce an image")
        }
        .await;
        let bytes = match capture {
            Ok(bytes) => bytes,
            Err(err) => {
                let reason = screenshot_failure_reason(&err);
                tracing::warn!(error = %format!("{err:#}"), reason, "screenshot capture failed");
                return Err(reported_deployment_failure(reason.to_string()));
            }
        };
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
        .await
        .map_err(|err| {
            tracing::warn!(error = %format!("{err:#}"), "screenshot upload failed");
            reported_deployment_failure(SCREENSHOT_ERR_UPLOAD.to_string())
        })?;
        Ok(())
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&output_dir).await;
    result?;
    log(
        cfg,
        deployment_id,
        "stdout",
        "Deployment screenshot captured.",
    )
    .await;
    Ok(())
}

const SCREENSHOT_CONTAINER_OUTPUT_PATH: &str = "/app/hostlet-screenshot.jpg";

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

pub(crate) use hostlet_contracts::valid_container_name;

#[cfg(test)]
mod route_temp_tests {
    use super::route_temp_path;
    use std::path::Path;

    #[test]
    fn temp_paths_for_same_target_are_distinct() {
        let target = Path::new("/etc/caddy/snippets/app.caddy");
        let first = route_temp_path(target);
        let second = route_temp_path(target);
        // Two concurrent writers of the same route file must never collide.
        assert_ne!(first, second);
        // Both temp files must live alongside the final route file so the
        // subsequent rename stays within one directory (atomic on the FS).
        assert_eq!(first.parent(), target.parent());
        assert_eq!(second.parent(), target.parent());
    }
}
