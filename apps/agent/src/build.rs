use super::*;

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
    pub(crate) detected_framework: Option<Framework>,
    pub(crate) package_manager: Option<PackageManager>,
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
    port: i64,
    payload: &Value,
) -> anyhow::Result<BuildPlan> {
    let packaging_strategy = PackagingStrategy::from_payload(payload)?;
    if let Some(plan) =
        dockerfile_packaging_plan(cfg, deployment_id, checkout, packaging_strategy).await?
    {
        return Ok(plan);
    }
    generated_packaging_plan(cfg, deployment_id, checkout, port, payload).await
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
        detected_framework: None,
        package_manager: None,
    }))
}

/// Inspects `package.json`, infers build commands, and writes an auto-generated
/// Hostlet Dockerfile, returning the resulting [`BuildPlan`].
async fn generated_packaging_plan(
    cfg: &Config,
    deployment_id: Uuid,
    checkout: &Path,
    port: i64,
    payload: &Value,
) -> anyhow::Result<BuildPlan> {
    let package_json = checkout.join("package.json");
    if !tokio::fs::try_exists(&package_json).await? {
        bail!("No usable Dockerfile or package.json found");
    }
    let contents = tokio::fs::read_to_string(&package_json).await?;
    let package: Value =
        serde_json::from_str(&contents).context("package.json is not valid JSON")?;
    let scripts = package
        .get("scripts")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let framework = detect_framework(&collect_deps(&package));
    let package_manager = detect_package_manager(checkout).await?;

    let commands = infer_node_commands(payload, &scripts, framework, package_manager)?;

    log(
        cfg,
        deployment_id,
        "stdout",
        &format!(
            "Detected {} app. Generating optimized Hostlet Dockerfile with {}.",
            framework.label(),
            package_manager.label()
        ),
    )
    .await;

    let hostlet_dir = cfg.workdir.join("builds").join(deployment_id.to_string());
    tokio::fs::create_dir_all(&hostlet_dir).await?;
    let dockerfile = hostlet_dir.join("Dockerfile");
    tokio::fs::write(
        &dockerfile,
        generated_node_dockerfile(
            package_manager,
            commands.install.as_deref(),
            commands.build.as_deref(),
            &commands.start,
            port,
            framework,
        ),
    )
    .await?;
    Ok(BuildPlan {
        context: checkout.to_path_buf(),
        dockerfile,
        generated: true,
        packaging_strategy: PackagingStrategy::Generated,
        detected_framework: Some(framework),
        package_manager: Some(package_manager),
    })
}

/// Build commands inferred for an auto-packaged Node app, after validation.
struct NodeCommands {
    install: Option<String>,
    build: Option<String>,
    start: String,
}

/// Resolves install/build/start commands from the payload overrides and
/// framework heuristics, then validates each one.
fn infer_node_commands(
    payload: &Value,
    scripts: &serde_json::Map<String, Value>,
    framework: Framework,
    package_manager: PackageManager,
) -> anyhow::Result<NodeCommands> {
    let install = payload_command(payload, "install_command");
    let build = payload_command(payload, "build_command").or_else(|| {
        pick_build_command(scripts, framework).map(|script| package_manager.run_command(&script))
    });
    let start = payload_command(payload, "start_command").or_else(|| {
        pick_start_command(scripts, framework).map(|script| {
            if script == STATIC_START_SENTINEL {
                script
            } else {
                package_manager.run_command(&script)
            }
        })
    });

    let Some(start) = start else {
        bail!("Node app detected, but no start command could be inferred");
    };
    if let Some(command) = install.as_deref() {
        validate_dockerfile_command(command)?;
    }
    if let Some(command) = build.as_deref() {
        validate_dockerfile_command(command)?;
    }
    validate_dockerfile_command(&start)?;
    Ok(NodeCommands {
        install,
        build,
        start,
    })
}

/// Reads a non-blank string command override from the deploy payload.
fn payload_command(payload: &Value, key: &str) -> Option<String> {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PackageManager {
    Npm,
    Pnpm,
    Yarn,
}

impl PackageManager {
    fn label(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
        }
    }
    fn run_command(self, script: &str) -> String {
        match self {
            Self::Npm => format!("npm run {script}"),
            Self::Pnpm => format!("pnpm run {script}"),
            Self::Yarn => format!("yarn {script}"),
        }
    }
}

mod dockerfile;

pub(crate) use dockerfile::generated_node_dockerfile;
use dockerfile::STATIC_START_SENTINEL;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Framework {
    Next,
    Vite,
    Astro,
    Nuxt,
    Remix,
    SvelteKit,
    Node,
}

impl Framework {
    fn label(self) -> &'static str {
        match self {
            Self::Next => "Next.js",
            Self::Vite => "Vite",
            Self::Astro => "Astro",
            Self::Nuxt => "Nuxt",
            Self::Remix => "Remix",
            Self::SvelteKit => "SvelteKit",
            Self::Node => "Node",
        }
    }

    fn runtime_kind(self) -> &'static str {
        match self {
            Self::Vite | Self::Astro | Self::SvelteKit => "static",
            Self::Next | Self::Nuxt | Self::Remix | Self::Node => "node",
        }
    }
}

pub(crate) async fn detect_package_manager(checkout: &Path) -> anyhow::Result<PackageManager> {
    if tokio::fs::try_exists(checkout.join("pnpm-lock.yaml")).await? {
        return Ok(PackageManager::Pnpm);
    }
    if tokio::fs::try_exists(checkout.join("yarn.lock")).await? {
        return Ok(PackageManager::Yarn);
    }
    Ok(PackageManager::Npm)
}

pub(crate) fn collect_deps(package: &Value) -> HashMap<String, String> {
    let mut deps = HashMap::new();
    for key in ["dependencies", "devDependencies"] {
        if let Some(map) = package.get(key).and_then(|v| v.as_object()) {
            for (name, version) in map {
                deps.insert(name.to_string(), version.as_str().unwrap_or("").to_string());
            }
        }
    }
    deps
}

pub(crate) fn detect_framework(deps: &HashMap<String, String>) -> Framework {
    if deps.contains_key("next") {
        Framework::Next
    } else if deps.contains_key("astro") {
        Framework::Astro
    } else if deps.contains_key("nuxt") {
        Framework::Nuxt
    } else if deps.contains_key("@remix-run/node") || deps.contains_key("@remix-run/react") {
        Framework::Remix
    } else if deps.contains_key("@sveltejs/kit") {
        Framework::SvelteKit
    } else if deps.contains_key("vite") {
        Framework::Vite
    } else {
        Framework::Node
    }
}

pub(crate) fn pick_build_command(
    scripts: &serde_json::Map<String, Value>,
    framework: Framework,
) -> Option<String> {
    if scripts.contains_key("build") {
        return Some("build".into());
    }
    match framework {
        Framework::Node => None,
        _ => Some("build".into()),
    }
}

pub(crate) fn pick_start_command(
    scripts: &serde_json::Map<String, Value>,
    framework: Framework,
) -> Option<String> {
    if scripts.contains_key("start") {
        return Some("start".into());
    }
    match framework {
        Framework::Vite | Framework::Astro | Framework::SvelteKit => {
            Some(STATIC_START_SENTINEL.into())
        }
        Framework::Next | Framework::Nuxt | Framework::Remix | Framework::Node => None,
    }
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
        "detectedFramework": build.detected_framework.map(|framework| framework.label()),
        "runtimeKind": build.detected_framework.map(|framework| framework.runtime_kind()),
        "packageManager": build.package_manager.map(|pm| pm.label()),
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
