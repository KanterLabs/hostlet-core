use super::*;

pub(crate) const NO_NATIVE_BUILD_PLAN: &str = "No repository Dockerfile selected";

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
) -> anyhow::Result<()> {
    let url = format!("http://{}:{port}{path}", cfg.health_host);
    log(
        cfg,
        deployment_id,
        "stdout",
        &format!("Waiting for health check: {url}"),
    )
    .await;
    for attempt in 1..=30 {
        match reqwest::get(&url).await {
            Ok(resp) if resp.status().is_success() => {
                log(cfg, deployment_id, "stdout", "Health check passed.").await;
                return Ok(());
            }
            Ok(resp) => {
                log(
                    cfg,
                    deployment_id,
                    "stdout",
                    &format!(
                        "Health check attempt {attempt}/30 returned HTTP {}.",
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
                    &format!("Health check attempt {attempt}/30 did not connect: {err}"),
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
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    bail!("no successful response from {url}");
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
