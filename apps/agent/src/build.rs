use super::*;

pub(crate) const NO_NATIVE_BUILD_PLAN: &str = "No repository Dockerfile selected";
pub(crate) const IMAGE_BUDGET_WARN_BYTES: i64 = 500_000_000;
pub(crate) const IMAGE_BUDGET_MAX_BYTES: i64 = 1_000_000_000;
/// Default attempt count.  Do NOT change — e2e test timing depends on this value.
const HEALTH_CHECK_ATTEMPTS_DEFAULT: u16 = 30;
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(2);
const HEALTH_CHECK_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Returns the number of health-check attempts, reading `HOSTLET_HEALTH_CHECK_ATTEMPTS`
/// via the pure [`health_check_attempts_value`] parser.  Defaults to 30.
pub(crate) fn health_check_attempts() -> u16 {
    std::env::var("HOSTLET_HEALTH_CHECK_ATTEMPTS")
        .ok()
        .as_deref()
        .and_then(health_check_attempts_value)
        .unwrap_or(HEALTH_CHECK_ATTEMPTS_DEFAULT)
}

/// Parses a raw `HOSTLET_HEALTH_CHECK_ATTEMPTS` string: trims whitespace,
/// parses as u16, accepts 1..=900.  Returns `None` for empty/invalid input.
pub(crate) fn health_check_attempts_value(value: &str) -> Option<u16> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed
        .parse::<u16>()
        .ok()
        .filter(|&n| (1..=900).contains(&n))
}

/// Classifies four-token Docker inspect output as a fatal container failure.
/// Format: "Running Restarting OOMKilled ExitCode" (output of
/// [`CONTAINER_STATE_INSPECT_FORMAT`]).
///
/// Returns `Some(message)` when the container will not recover on its own and
/// further health probing is pointless.  Returns `None` for running, restarting
/// (crash-loop that may self-recover), or malformed output (keep probing).
pub(crate) fn container_fatal_state(inspect_stdout: &str) -> Option<String> {
    let mut parts = inspect_stdout.split_whitespace();
    let running = parts.next()?;
    let restarting = parts.next()?;
    let oom_killed = parts.next()?;
    let exit_code = parts.next().unwrap_or("unknown");
    if oom_killed == "true" {
        return Some(
            "container was OOM-killed during startup; raise the app memory limit and redeploy"
                .into(),
        );
    }
    if running == "false" && restarting == "false" {
        return Some(format!(
            "container exited with code {exit_code} before passing the health check"
        ));
    }
    None
}

/// Runs `docker inspect` on `container` and checks whether it has entered a
/// fatal non-recoverable state.
///
/// Returns `None` when the daemon is unavailable/slow (transient hiccup — keep
/// probing) or when the container is running/restarting and may still pass.
/// Returns `Some(message)` only for OOM-kill, clean exit, or gone-missing.
pub(crate) async fn fatal_container_failure(container: &str) -> Option<String> {
    let output = command_output(
        "docker",
        &["inspect", "-f", CONTAINER_STATE_INSPECT_FORMAT, container],
        Duration::from_secs(10),
    )
    .await;
    match output {
        Err(_) => None, // daemon hiccup — treat as keep-probing
        Ok(out) if !out.status.success() => {
            // inspect failed: the container is gone (removed externally).
            Some("container no longer exists; it may have been removed externally".into())
        }
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            container_fatal_state(stdout.trim())
        }
    }
}

pub(crate) fn normalize_git_remote(value: &str) -> String {
    value
        .trim()
        .trim_end_matches(".git")
        .trim_start_matches("https://")
        .to_ascii_lowercase()
}

pub(crate) fn git_fetch_remote(repo: &str, github_token: Option<&str>) -> String {
    let Some(token) = github_token.filter(|token| !token.trim().is_empty()) else {
        return format!("https://github.com/{repo}.git");
    };
    let encoded = url::form_urlencoded::byte_serialize(token.as_bytes()).collect::<String>();
    format!("https://x-access-token:{encoded}@github.com/{repo}.git")
}

pub(crate) struct BuildPlan {
    pub(crate) context: PathBuf,
    pub(crate) dockerfile: PathBuf,
    pub(crate) generated: bool,
    pub(crate) packaging_strategy: PackagingStrategy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PackagingStrategy {
    Auto,
    Dockerfile,
    Generated,
}

impl PackagingStrategy {
    pub(crate) fn from_payload(payload: &Value) -> anyhow::Result<Self> {
        match payload
            .get("packaging_strategy")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or("auto")
        {
            "auto" => Ok(Self::Auto),
            "dockerfile" => Ok(Self::Dockerfile),
            "generated" => Ok(Self::Generated),
            _ => bail!("packaging strategy must be auto, dockerfile, or generated"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Dockerfile => "dockerfile",
            Self::Generated => "generated",
        }
    }
}

pub(crate) async fn prepare_build(
    cfg: &Config,
    deployment_id: Uuid,
    checkout: &Path,
    _port: i64,
    payload: &Value,
) -> anyhow::Result<BuildPlan> {
    let packaging_strategy = PackagingStrategy::from_payload(payload)?;
    if let Some(plan) =
        dockerfile_packaging_plan(cfg, deployment_id, checkout, packaging_strategy).await?
    {
        return Ok(plan);
    }
    bail!(NO_NATIVE_BUILD_PLAN)
}

/// Returns a repository-Dockerfile [`BuildPlan`] when one applies, or `None`
/// when the build should fall through to auto-generated packaging.
async fn dockerfile_packaging_plan(
    cfg: &Config,
    deployment_id: Uuid,
    checkout: &Path,
    packaging_strategy: PackagingStrategy,
) -> anyhow::Result<Option<BuildPlan>> {
    let root_dockerfile = checkout.join("Dockerfile");
    let has_dockerfile = tokio::fs::try_exists(&root_dockerfile).await?;
    if packaging_strategy == PackagingStrategy::Dockerfile && !has_dockerfile {
        bail!("packaging strategy dockerfile requires a Dockerfile at the app root");
    }
    if !has_dockerfile || packaging_strategy == PackagingStrategy::Generated {
        return Ok(None);
    }
    log(
        cfg,
        deployment_id,
        "stdout",
        "Detected Dockerfile at app root. Using repository Dockerfile packaging.",
    )
    .await;
    Ok(Some(BuildPlan {
        context: checkout.to_path_buf(),
        dockerfile: root_dockerfile,
        generated: false,
        packaging_strategy: PackagingStrategy::Dockerfile,
    }))
}

/// Reads a non-blank string command override from the deploy payload.
pub(crate) fn payload_command(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(str::to_string)
}

pub(crate) async fn safe_project_dir(
    checkout: &Path,
    root_directory: &str,
) -> anyhow::Result<PathBuf> {
    let clean = root_directory.trim().trim_start_matches('/');
    if clean.len() > 256
        || clean.starts_with('\\')
        || clean.split('/').any(|part| part == "..")
        || clean.chars().any(|c| c.is_control() || c == '\\')
    {
        bail!("root directory cannot be absolute or contain ..");
    }
    let checkout = tokio::fs::canonicalize(checkout)
        .await
        .context("failed to canonicalize checkout path")?;
    let project = if clean.is_empty() || clean == "." {
        checkout.to_path_buf()
    } else {
        checkout.join(clean)
    };
    let project = tokio::fs::canonicalize(project)
        .await
        .context("root directory does not exist or is not readable")?;
    if !project.starts_with(&checkout) {
        bail!("root directory cannot resolve outside the repository checkout");
    }
    Ok(project)
}

pub(crate) fn generated_dockerignore() -> &'static str {
    ".git\n\
     .next/cache\n\
     .nuxt\n\
     .output\n\
     dist\n\
     build\n\
     coverage\n\
     node_modules\n\
     npm-debug.log*\n\
     pnpm-debug.log*\n\
     yarn-debug.log*\n\
     .DS_Store\n"
}

pub(crate) fn buildx_args<'a>(
    image: &'a str,
    dockerfile: &'a str,
    context: &'a str,
    cache_from: &'a str,
    cache_to: &'a str,
) -> Vec<&'a str> {
    vec![
        "buildx",
        "build",
        "--load",
        "--cache-from",
        cache_from,
        "--cache-to",
        cache_to,
        "-f",
        dockerfile,
        "-t",
        image,
        context,
    ]
}

pub(crate) fn docker_build_args<'a>(
    image: &'a str,
    dockerfile: &'a str,
    context: &'a str,
) -> Vec<&'a str> {
    vec!["build", "-f", dockerfile, "-t", image, context]
}

pub(crate) async fn docker_buildx_available() -> bool {
    command_output("docker", &["buildx", "version"], Duration::from_secs(30))
        .await
        .map(|output| output.status.success())
        .unwrap_or(false)
}

pub(crate) async fn image_size_bytes(image: &str) -> anyhow::Result<i64> {
    let output = command_output(
        "docker",
        &["image", "inspect", "-f", "{{.Size}}", image],
        Duration::from_secs(120),
    )
    .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "docker image inspect exited with {}: {}",
            output.status,
            stderr.trim()
        );
    }
    let value = String::from_utf8(output.stdout)
        .context("docker image inspect output was not valid UTF-8")?;
    value
        .trim()
        .parse::<i64>()
        .context("docker image inspect size was not an integer")
}

pub(crate) fn build_runtime_metadata(
    build: &BuildPlan,
    image_ref: &str,
    build_duration_ms: u128,
    image_size_bytes: Option<i64>,
) -> Value {
    build_artifact_runtime_metadata(image_budget_runtime_metadata(
        json!({
            "imageRef": image_ref,
            "packagingStrategy": build.packaging_strategy.label(),
            "generatedDockerfile": build.generated,
            "detectedLanguage": null,
            "detectedFramework": null,
            "runtimeKind": null,
            "packageManager": null,
            "buildDurationMs": build_duration_ms,
            "imageSizeBytes": image_size_bytes,
        }),
        image_size_bytes,
    ))
}

pub(crate) fn build_artifact_runtime_metadata(mut metadata: Value) -> Value {
    let Some(image_ref) = metadata
        .get("imageRef")
        .and_then(Value::as_str)
        .filter(|image_ref| !image_ref.trim().is_empty())
        .map(str::to_owned)
    else {
        return metadata;
    };
    let Some(object) = metadata.as_object_mut() else {
        return metadata;
    };
    object
        .entry("buildArtifact")
        .or_insert_with(|| json!({ "imageRef": image_ref }));
    metadata
}

pub(crate) fn image_budget_runtime_metadata(
    mut metadata: Value,
    image_size_bytes: Option<i64>,
) -> Value {
    let status = image_budget_status(image_size_bytes);
    if let Some(object) = metadata.as_object_mut() {
        object.insert("imageBudgetStatus".into(), json!(status));
        object.insert(
            "imageBudgetWarnBytes".into(),
            json!(IMAGE_BUDGET_WARN_BYTES),
        );
        object.insert("imageBudgetMaxBytes".into(), json!(IMAGE_BUDGET_MAX_BYTES));
        metadata
    } else {
        json!({
            "imageBudgetStatus": status,
            "imageBudgetWarnBytes": IMAGE_BUDGET_WARN_BYTES,
            "imageBudgetMaxBytes": IMAGE_BUDGET_MAX_BYTES,
        })
    }
}

pub(crate) fn image_budget_status(image_size_bytes: Option<i64>) -> &'static str {
    match image_size_bytes {
        Some(size) if size > IMAGE_BUDGET_MAX_BYTES => "over_budget",
        Some(size) if size > IMAGE_BUDGET_WARN_BYTES => "warning",
        Some(_) => "ok",
        None => "unknown",
    }
}

pub(crate) fn add_git_sync_runtime_metadata(
    mut metadata: Value,
    git_sync_duration_ms: u128,
) -> Value {
    if let Some(object) = metadata.as_object_mut() {
        object.insert("gitSyncDurationMs".into(), json!(git_sync_duration_ms));
        metadata
    } else {
        json!({
            "gitSyncDurationMs": git_sync_duration_ms,
        })
    }
}

pub(crate) fn add_build_plan_runtime_metadata(
    mut metadata: Value,
    build_plan_duration_ms: u128,
) -> Value {
    if let Some(object) = metadata.as_object_mut() {
        object.insert("buildPlanDurationMs".into(), json!(build_plan_duration_ms));
        metadata
    } else {
        json!({
            "buildPlanDurationMs": build_plan_duration_ms,
        })
    }
}

pub(crate) fn build_prepare_failure_runtime_metadata(
    build_plan_duration_ms: u128,
    git_sync_duration_ms: u128,
) -> Value {
    add_git_sync_runtime_metadata(
        add_build_plan_runtime_metadata(json!({}), build_plan_duration_ms),
        git_sync_duration_ms,
    )
}

pub(crate) fn add_startup_runtime_metadata(
    mut metadata: Value,
    container_start_duration_ms: u128,
    health_check_duration_ms: u128,
) -> Value {
    let boot_duration_ms = container_start_duration_ms + health_check_duration_ms;
    if let Some(object) = metadata.as_object_mut() {
        object.insert(
            "containerStartDurationMs".into(),
            json!(container_start_duration_ms),
        );
        object.insert(
            "healthCheckDurationMs".into(),
            json!(health_check_duration_ms),
        );
        object.insert("bootDurationMs".into(), json!(boot_duration_ms));
        metadata
    } else {
        json!({
            "containerStartDurationMs": container_start_duration_ms,
            "healthCheckDurationMs": health_check_duration_ms,
            "bootDurationMs": boot_duration_ms,
        })
    }
}

pub(crate) fn add_routing_runtime_metadata(
    mut metadata: Value,
    routing_duration_ms: u128,
) -> Value {
    if let Some(object) = metadata.as_object_mut() {
        object.insert("routingDurationMs".into(), json!(routing_duration_ms));
        metadata
    } else {
        json!({
            "routingDurationMs": routing_duration_ms,
        })
    }
}

pub(crate) async fn stream_lines<R: tokio::io::AsyncRead + Unpin>(
    cfg: Config,
    deployment_id: Uuid,
    stream: &str,
    reader: R,
) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        log(&cfg, deployment_id, stream, &redact(&line)).await;
    }
}

pub(crate) async fn wait_health(
    cfg: &Config,
    deployment_id: Uuid,
    container: &str,
    port: u16,
    path: &str,
) -> anyhow::Result<Duration> {
    let url = format!("http://{}:{port}{path}", cfg.health_host);
    let started = Instant::now();
    let health_client = &cfg.http;
    let max_attempts = health_check_attempts();
    log(
        cfg,
        deployment_id,
        "stdout",
        &format!("Waiting for health check: {url}"),
    )
    .await;
    for attempt in 1..=max_attempts {
        match health_check_request(health_client, &url, HEALTH_CHECK_REQUEST_TIMEOUT).await {
            Ok(resp) if resp.status().is_success() => {
                log(cfg, deployment_id, "stdout", "Health check passed.").await;
                return Ok(started.elapsed());
            }
            Ok(resp) => {
                log(
                    cfg,
                    deployment_id,
                    "stdout",
                    &format!(
                        "Health check attempt {attempt}/{max_attempts} returned HTTP {}.",
                        resp.status()
                    ),
                )
                .await;
                if let Some(msg) = fatal_container_failure(container).await {
                    bail!("{msg}");
                }
            }
            Err(err) => {
                log(
                    cfg,
                    deployment_id,
                    "stdout",
                    &format!(
                        "Health check attempt {attempt}/{max_attempts} did not connect: {err}"
                    ),
                )
                .await;
                if let Some(msg) = fatal_container_failure(container).await {
                    bail!("{msg}");
                }
            }
        }
        if attempt == 5 || attempt == 15 {
            let _ = run_log(
                cfg,
                deployment_id,
                "docker",
                &["logs", "--tail", "30", container],
            )
            .await;
        }
        if should_wait_before_next_health_attempt(attempt, max_attempts) {
            tokio::time::sleep(HEALTH_CHECK_INTERVAL).await;
        }
    }
    bail!("no successful response from {url}");
}

fn should_wait_before_next_health_attempt(attempt: u16, max_attempts: u16) -> bool {
    attempt < max_attempts
}

async fn health_check_request(
    client: &reqwest::Client,
    url: &str,
    timeout: Duration,
) -> Result<reqwest::Response, reqwest::Error> {
    client.get(url).timeout(timeout).send().await
}

pub(crate) fn docker_port_map(port: u16) -> String {
    format!("127.0.0.1::{port}")
}

pub(crate) async fn apply_caddy_route(
    cfg: &Config,
    deployment_id: Uuid,
    app: &str,
    domain: &str,
    port: u16,
) -> anyhow::Result<()> {
    let dir = PathBuf::from("/etc/caddy/hostlet");
    tokio::fs::create_dir_all(&dir).await?;
    let target = dir.join(format!("{app}.caddy"));
    ensure_no_conflicting_route(&dir, &target, domain).await?;
    let previous = tokio::fs::read(&target).await.ok();
    write_route_file(&target, &render_caddy_route(app, domain, port)).await?;
    let reload = run_log(
        cfg,
        deployment_id,
        "caddy",
        &["reload", "--config", "/etc/caddy/Caddyfile"],
    )
    .await;
    if let Err(err) = reload {
        restore_route_file(&target, previous).await?;
        let _ = run_quiet("caddy", &["reload", "--config", "/etc/caddy/Caddyfile"]).await;
        bail!("Caddy reload failed and the previous route was restored: {err}");
    }
    Ok(())
}

pub(crate) fn render_caddy_route(app: &str, domain: &str, port: u16) -> String {
    format!(
        "# hostlet-route-key: {app}\n# hostlet-domain: {domain}\n{domain} {{\n  reverse_proxy 127.0.0.1:{port}\n}}\n"
    )
}

pub(crate) async fn remove_caddy_route(app: &str) -> anyhow::Result<()> {
    let target = PathBuf::from("/etc/caddy/hostlet").join(format!("{app}.caddy"));
    match tokio::fs::remove_file(target).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

pub(crate) async fn apply_local_caddy_route(
    cfg: &Config,
    deployment_id: Uuid,
    router: &LocalRouter,
    app: &str,
    domain: &str,
    port: u16,
) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(&router.snippets_dir).await?;
    let target = router.snippets_dir.join(format!("{app}.caddy"));
    ensure_no_conflicting_route(&router.snippets_dir, &target, domain).await?;
    let previous = tokio::fs::read(&target).await.ok();
    write_route_file(&target, &render_local_caddy_route(app, domain, port)).await?;
    if let Err(err) = run_router_reload(cfg, deployment_id, router).await {
        restore_route_file(&target, previous).await?;
        let _ = run_router_reload_quiet(router).await;
        bail!("Caddy reload failed and the previous route was restored: {err}");
    }
    Ok(())
}

pub(crate) fn render_local_caddy_route(app: &str, domain: &str, port: u16) -> String {
    format!(
        "# hostlet-route-key: {app}\n# hostlet-domain: {domain}\n@{app} host {domain}\nreverse_proxy @{app} 127.0.0.1:{port}\n"
    )
}

#[cfg(test)]
mod tests {
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
}
