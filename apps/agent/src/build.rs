use super::*;

pub(crate) const NO_NATIVE_BUILD_PLAN: &str = "No repository Dockerfile selected";
const HEALTH_CHECK_ATTEMPTS: u8 = 30;
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(2);
const HEALTH_CHECK_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

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
    build_duration_ms: u128,
    image_size_bytes: Option<i64>,
) -> Value {
    json!({
        "packagingStrategy": build.packaging_strategy.label(),
        "generatedDockerfile": build.generated,
        "detectedLanguage": null,
        "detectedFramework": null,
        "runtimeKind": null,
        "packageManager": null,
        "buildDurationMs": build_duration_ms,
        "imageSizeBytes": image_size_bytes,
    })
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
    log(
        cfg,
        deployment_id,
        "stdout",
        &format!("Waiting for health check: {url}"),
    )
    .await;
    for attempt in 1..=HEALTH_CHECK_ATTEMPTS {
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
                        "Health check attempt {attempt}/{HEALTH_CHECK_ATTEMPTS} returned HTTP {}.",
                        resp.status()
                    ),
                )
                .await;
            }
            Err(err) => {
                log(
                    cfg,
                    deployment_id,
                    "stdout",
                    &format!("Health check attempt {attempt}/{HEALTH_CHECK_ATTEMPTS} did not connect: {err}"),
                )
                .await;
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
        if should_wait_before_next_health_attempt(attempt) {
            tokio::time::sleep(HEALTH_CHECK_INTERVAL).await;
        }
    }
    bail!("no successful response from {url}");
}

fn should_wait_before_next_health_attempt(attempt: u8) -> bool {
    attempt < HEALTH_CHECK_ATTEMPTS
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
        let metadata = build_runtime_metadata(&test_build_plan(), 12_345, Some(149_422_080));

        assert_eq!(metadata["packagingStrategy"], "dockerfile");
        assert_eq!(metadata["generatedDockerfile"], false);
        assert_eq!(metadata["buildDurationMs"], 12_345);
        assert_eq!(metadata["imageSizeBytes"], 149_422_080);
    }

    #[test]
    fn build_runtime_metadata_records_unknown_image_size() {
        let metadata = build_runtime_metadata(&test_build_plan(), 3_000, None);

        assert_eq!(metadata["packagingStrategy"], "dockerfile");
        assert_eq!(metadata["buildDurationMs"], 3_000);
        assert!(metadata["imageSizeBytes"].is_null());
    }

    #[test]
    fn startup_runtime_metadata_preserves_build_metrics_and_records_boot_time() {
        let metadata = build_runtime_metadata(&test_build_plan(), 2_000, Some(42_000));
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
        assert!(should_wait_before_next_health_attempt(
            HEALTH_CHECK_ATTEMPTS - 1
        ));
        assert!(!should_wait_before_next_health_attempt(
            HEALTH_CHECK_ATTEMPTS
        ));
    }
}
