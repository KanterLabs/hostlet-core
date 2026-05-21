use anyhow::{bail, Context};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
struct Config {
    api_url: String,
    server_id: Uuid,
    agent_token: String,
    job_signing_secret: String,
    workdir: PathBuf,
    local_mode: bool,
    health_host: String,
    local_router: Option<LocalRouter>,
}

#[derive(Clone)]
struct LocalRouter {
    snippets_dir: PathBuf,
    reload_command: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();
    let cfg = Config {
        api_url: env("HOSTLET_API_URL")?,
        server_id: env("HOSTLET_SERVER_ID")?.parse()?,
        agent_token: env("HOSTLET_AGENT_TOKEN")?,
        job_signing_secret: env("HOSTLET_JOB_SIGNING_SECRET")?,
        workdir: PathBuf::from(
            std::env::var("HOSTLET_WORKDIR").unwrap_or_else(|_| "/var/lib/hostlet".into()),
        ),
        local_mode: std::env::var("HOSTLET_LOCAL_MODE")
            .map(|v| v == "true")
            .unwrap_or(false),
        health_host: std::env::var("HOSTLET_HEALTH_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
        local_router: local_router_config()?,
    };
    tokio::fs::create_dir_all(&cfg.workdir).await?;
    loop {
        if let Err(err) = connect_loop(cfg.clone()).await {
            tracing::warn!("agent disconnected: {err}");
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

async fn connect_loop(cfg: Config) -> anyhow::Result<()> {
    let ws_url = cfg
        .api_url
        .replace("http://", "ws://")
        .replace("https://", "wss://")
        + "/ws/agent";
    let mut req = ws_url.into_client_request()?;
    req.headers_mut()
        .insert("x-hostlet-server-id", cfg.server_id.to_string().parse()?);
    req.headers_mut()
        .insert("x-hostlet-agent-token", cfg.agent_token.parse()?);
    let (mut ws, _) = connect_async(req).await?;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    loop {
        tokio::select! {
            _ = heartbeat.tick() => ws.send(Message::Text(json!({"type":"heartbeat"}).to_string())).await?,
            msg = ws.next() => {
                let Some(Ok(Message::Text(text))) = msg else { bail!("websocket closed"); };
                let value: Value = serde_json::from_str(&text)?;
                if value.get("type").and_then(|v| v.as_str()) == Some("job") {
                    let payload = value.get("payload").context("missing payload")?.clone();
                    let signature = value.get("signature").and_then(|v| v.as_str()).context("missing signature")?;
                    let raw = serde_json::to_vec(&payload)?;
                    if !verify_signature(&cfg.job_signing_secret, &raw, signature) {
                        bail!("job signature verification failed");
                    }
                    if let Err(err) = handle_job(cfg.clone(), payload.clone()).await {
                        let message = format!("{err}");
                        if let Some(deployment_id) = payload.get("deployment_id").and_then(|v| v.as_str()).and_then(|v| Uuid::parse_str(v).ok()) {
                            log(&cfg, deployment_id, "stderr", &message).await;
                            status(&cfg, deployment_id, "failed", Some(&format!("{message}. Add a Dockerfile, or add package.json build/start scripts Hostlet can run."))).await;
                        }
                        tracing::warn!("job failed: {message}");
                    }
                }
            }
        }
    }
}

async fn handle_job(cfg: Config, payload: Value) -> anyhow::Result<()> {
    match payload.get("type").and_then(|v| v.as_str()) {
        Some("deploy") => deploy(cfg, payload).await,
        Some("rollback") => rollback(cfg, payload).await,
        _ => Ok(()),
    }
}

async fn deploy(cfg: Config, p: Value) -> anyhow::Result<()> {
    let deployment_id = Uuid::parse_str(p["deployment_id"].as_str().context("deployment_id")?)?;
    let app_name = safe_name(p["app_name"].as_str().context("app_name")?);
    let repo = p["repo"].as_str().context("repo")?;
    let branch = p["branch"].as_str().context("branch")?;
    let port = p["container_port"].as_i64().context("container_port")?;
    let domain = p["domain"].as_str().context("domain")?;
    let health_path = p["health_path"].as_str().unwrap_or("/");
    let root_directory = p
        .get("root_directory")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    validate_repo(repo)?;
    validate_branch(branch)?;
    validate_port(port)?;
    validate_domain(domain)?;
    validate_health_path(health_path)?;
    status(&cfg, deployment_id, "building", None).await;
    let checkout = cfg.workdir.join("repos").join(&app_name);
    if checkout.exists() {
        run_log(
            &cfg,
            deployment_id,
            "git",
            &["-C", checkout.to_str().unwrap(), "fetch", "origin", branch],
        )
        .await?;
        run_log(
            &cfg,
            deployment_id,
            "git",
            &["-C", checkout.to_str().unwrap(), "checkout", branch],
        )
        .await?;
        run_log(
            &cfg,
            deployment_id,
            "git",
            &[
                "-C",
                checkout.to_str().unwrap(),
                "pull",
                "--ff-only",
                "origin",
                branch,
            ],
        )
        .await?;
    } else {
        tokio::fs::create_dir_all(checkout.parent().unwrap()).await?;
        run_log(
            &cfg,
            deployment_id,
            "git",
            &[
                "clone",
                "--branch",
                branch,
                &format!("https://github.com/{repo}.git"),
                checkout.to_str().unwrap(),
            ],
        )
        .await?;
    }
    let image = format!("hostlet/{app_name}:{deployment_id}");
    let project_dir = safe_project_dir(&checkout, root_directory)?;
    let build = prepare_build(&cfg, deployment_id, &project_dir, port, &p).await?;
    run_log(
        &cfg,
        deployment_id,
        "docker",
        &[
            "build",
            "-f",
            build.dockerfile.to_str().unwrap(),
            "-t",
            &image,
            build.context.to_str().unwrap(),
        ],
    )
    .await?;
    status(&cfg, deployment_id, "starting", None).await;
    let container = format!("hostlet-{app_name}-{deployment_id}");
    let port_map = format!("127.0.0.1::{port}");
    let mut args = vec![
        "run",
        "-d",
        "--name",
        &container,
        "--restart",
        "unless-stopped",
        "--security-opt",
        "no-new-privileges",
        "--cap-drop",
        "ALL",
        "--pids-limit",
        "256",
        "-p",
        &port_map,
    ];
    let memory_limit = p
        .get("memory_limit_mb")
        .and_then(|v| v.as_i64())
        .map(|mb| format!("{mb}m"));
    let cpu_limit = p
        .get("cpu_limit")
        .and_then(|v| v.as_f64())
        .map(|cpus| format!("{cpus:.2}"));
    if let Some(memory) = memory_limit.as_deref() {
        args.push("--memory");
        args.push(memory);
        args.push("--memory-swap");
        args.push(memory);
    }
    if let Some(cpus) = cpu_limit.as_deref() {
        args.push("--cpus");
        args.push(cpus);
    }
    if !build.generated {
        args.push("--read-only");
        args.push("--tmpfs");
        args.push("/tmp");
    }
    let env_pairs = env_args(&p);
    for pair in &env_pairs {
        args.push("-e");
        args.push(pair);
    }
    args.push(&image);
    run_log(&cfg, deployment_id, "docker", &args).await?;
    let internal_port = docker_published_port(&container, port as u16).await?;
    log(
        &cfg,
        deployment_id,
        "stdout",
        &format!("Docker assigned local port {internal_port} for container port {port}."),
    )
    .await;
    status(&cfg, deployment_id, "health_checking", None).await;
    if let Err(err) = wait_health(&cfg, deployment_id, &container, internal_port, health_path).await
    {
        log(&cfg, deployment_id, "stderr", "Recent container logs:").await;
        let _ = run_log(
            &cfg,
            deployment_id,
            "docker",
            &["logs", "--tail", "80", &container],
        )
        .await;
        log(
            &cfg,
            deployment_id,
            "stderr",
            &format!("Preserving failed container for inspection: {container}"),
        )
        .await;
        status(&cfg, deployment_id, "failed", Some(&format!("Health check failed: {err}. The container was left running or exited for inspection. Check the runtime logs above, port setting, and health path."))).await;
        return Ok(());
    }
    status(&cfg, deployment_id, "routing", None).await;
    let mut local_url = None;
    if cfg.local_mode {
        if let Some(router) = &cfg.local_router {
            write_local_caddy_route(router, &app_name, domain, internal_port).await?;
            run_router_reload(&cfg, deployment_id, router).await?;
        }
        let url = if cfg.local_router.is_some() {
            domain.to_string()
        } else {
            format!("localhost:{internal_port}")
        };
        log(
            &cfg,
            deployment_id,
            "stdout",
            &format!("Local app is available at https://{url}"),
        )
        .await;
        local_url = Some(url);
    } else {
        write_caddy_route(&app_name, domain, internal_port).await?;
        run_log(
            &cfg,
            deployment_id,
            "caddy",
            &["reload", "--config", "/etc/caddy/Caddyfile"],
        )
        .await?;
    }
    status_extra(
        &cfg,
        deployment_id,
        "success",
        None,
        Some(&image),
        Some(&container),
        local_url.as_deref(),
    )
    .await;
    Ok(())
}

async fn rollback(cfg: Config, p: Value) -> anyhow::Result<()> {
    let deployment_id = Uuid::parse_str(p["deployment_id"].as_str().context("deployment_id")?)?;
    let container = p["target_container"].as_str().context("target_container")?;
    let domain = p["domain"].as_str().context("domain")?;
    let app_name = safe_name(p["app_id"].as_str().unwrap_or("app"));
    let port_value = p["container_port"].as_i64().unwrap_or(3000);
    validate_port(port_value)?;
    let port = port_value as u16;
    validate_domain(domain)?;
    status(&cfg, deployment_id, "routing", None).await;
    if cfg.local_mode {
        status_extra(
            &cfg,
            deployment_id,
            "rolled_back",
            None,
            None,
            Some(container),
            None,
        )
        .await;
        return Ok(());
    }
    write_caddy_route(&app_name, domain, port).await?;
    let reload = run_log(
        &cfg,
        deployment_id,
        "caddy",
        &["reload", "--config", "/etc/caddy/Caddyfile"],
    )
    .await;
    match reload {
        Ok(_) => {
            status_extra(
                &cfg,
                deployment_id,
                "rolled_back",
                None,
                None,
                Some(container),
                None,
            )
            .await
        }
        Err(err) => {
            status(
                &cfg,
                deployment_id,
                "failed",
                Some(&format!(
                    "Rollback routing failed: {err}. Current working app was preserved."
                )),
            )
            .await
        }
    }
    Ok(())
}

async fn run_log(
    cfg: &Config,
    deployment_id: Uuid,
    bin: &str,
    args: &[&str],
) -> anyhow::Result<()> {
    log(
        cfg,
        deployment_id,
        "stdout",
        &format!("$ {} {}", bin, args.join(" ")),
    )
    .await;
    let mut cmd = Command::new(bin);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to start {bin}"))?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let c1 = cfg.clone();
    let c2 = cfg.clone();
    tokio::spawn(async move {
        stream_lines(c1, deployment_id, "stdout", stdout).await;
    });
    tokio::spawn(async move {
        stream_lines(c2, deployment_id, "stderr", stderr).await;
    });
    let status = child.wait().await?;
    if !status.success() {
        bail!("{bin} exited with {status}");
    }
    Ok(())
}

struct BuildPlan {
    context: PathBuf,
    dockerfile: PathBuf,
    generated: bool,
}

async fn prepare_build(
    cfg: &Config,
    deployment_id: Uuid,
    checkout: &Path,
    port: i64,
    payload: &Value,
) -> anyhow::Result<BuildPlan> {
    let root_dockerfile = checkout.join("Dockerfile");
    if tokio::fs::try_exists(&root_dockerfile).await? {
        log(
            cfg,
            deployment_id,
            "stdout",
            "Detected Dockerfile at repo root. Using Docker build mode.",
        )
        .await;
        return Ok(BuildPlan {
            context: checkout.to_path_buf(),
            dockerfile: root_dockerfile,
            generated: false,
        });
    }

    let package_json = checkout.join("package.json");
    if !tokio::fs::try_exists(&package_json).await? {
        bail!("No Dockerfile or package.json found");
    }

    let contents = tokio::fs::read_to_string(&package_json).await?;
    let package: Value =
        serde_json::from_str(&contents).context("package.json is not valid JSON")?;
    let scripts = package
        .get("scripts")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let deps = collect_deps(&package);
    let framework = detect_framework(&deps);
    let package_manager = detect_package_manager(checkout).await?;
    let install_command = payload
        .get("install_command")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(str::to_string);
    let build_command = payload
        .get("build_command")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            pick_build_command(&scripts, framework)
                .map(|script| package_manager.run_command(&script))
        });
    let start_command = payload
        .get("start_command")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            pick_start_command(&scripts, framework).map(|script| {
                if script == "__hostlet_static" {
                    script
                } else {
                    package_manager.run_command(&script)
                }
            })
        });

    let Some(start_command) = start_command else {
        bail!("Node app detected, but no start command could be inferred");
    };
    if let Some(command) = install_command.as_deref() {
        validate_dockerfile_command(command)?;
    }
    if let Some(command) = build_command.as_deref() {
        validate_dockerfile_command(command)?;
    }
    validate_dockerfile_command(&start_command)?;

    log(
        cfg,
        deployment_id,
        "stdout",
        &format!(
            "No Dockerfile found. Detected {} app. Generating Hostlet Dockerfile with {}.",
            framework.label(),
            package_manager.label()
        ),
    )
    .await;

    let hostlet_dir = checkout.join(".hostlet");
    tokio::fs::create_dir_all(&hostlet_dir).await?;
    let dockerfile = hostlet_dir.join("Dockerfile");
    tokio::fs::write(
        &dockerfile,
        generated_node_dockerfile(
            package_manager,
            install_command.as_deref(),
            build_command.as_deref(),
            &start_command,
            port,
        ),
    )
    .await?;
    Ok(BuildPlan {
        context: checkout.to_path_buf(),
        dockerfile,
        generated: true,
    })
}

fn safe_project_dir(checkout: &Path, root_directory: &str) -> anyhow::Result<PathBuf> {
    let clean = root_directory.trim().trim_start_matches('/');
    if clean.len() > 256
        || clean.starts_with('\\')
        || clean.split('/').any(|part| part == "..")
        || clean.chars().any(|c| c.is_control() || c == '\\')
    {
        bail!("root directory cannot be absolute or contain ..");
    }
    Ok(if clean.is_empty() || clean == "." {
        checkout.to_path_buf()
    } else {
        checkout.join(clean)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PackageManager {
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
    fn install_command(self) -> &'static str {
        match self {
            Self::Npm => "npm ci",
            Self::Pnpm => "corepack enable && corepack prepare pnpm@10.33.2 --activate && pnpm install --frozen-lockfile --config.dangerouslyAllowAllBuilds=true",
            Self::Yarn => "corepack enable && yarn install --frozen-lockfile",
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

#[derive(Clone, Copy)]
enum Framework {
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
}

async fn detect_package_manager(checkout: &Path) -> anyhow::Result<PackageManager> {
    if tokio::fs::try_exists(checkout.join("pnpm-lock.yaml")).await? {
        return Ok(PackageManager::Pnpm);
    }
    if tokio::fs::try_exists(checkout.join("yarn.lock")).await? {
        return Ok(PackageManager::Yarn);
    }
    Ok(PackageManager::Npm)
}

fn collect_deps(package: &Value) -> HashMap<String, String> {
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

fn detect_framework(deps: &HashMap<String, String>) -> Framework {
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

fn pick_build_command(
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

fn pick_start_command(
    scripts: &serde_json::Map<String, Value>,
    framework: Framework,
) -> Option<String> {
    if scripts.contains_key("start") {
        return Some("start".into());
    }
    match framework {
        Framework::Vite | Framework::Astro | Framework::SvelteKit => {
            Some("__hostlet_static".into())
        }
        Framework::Next | Framework::Nuxt | Framework::Remix | Framework::Node => None,
    }
}

fn generated_node_dockerfile(
    pm: PackageManager,
    install_command: Option<&str>,
    build_command: Option<&str>,
    start_command: &str,
    port: i64,
) -> String {
    let build_line = build_command
        .map(|command| format!("RUN {command}\n"))
        .unwrap_or_default();
    let install = install_command.unwrap_or_else(|| pm.install_command());
    let start_line = if start_command == "__hostlet_static" {
        "CMD [\"npx\", \"serve\", \"-s\", \"dist\", \"-l\", \"tcp://0.0.0.0:${PORT}\"]".to_string()
    } else {
        format!(
            "CMD [\"sh\", \"-lc\", {}]",
            serde_json::to_string(start_command).expect("string serialization cannot fail")
        )
    };
    format!(
        "FROM node:22-alpine\n\
         WORKDIR /app\n\
         COPY package.json package-lock.json* pnpm-lock.yaml* yarn.lock* ./\n\
         RUN {install}\n\
         COPY . .\n\
         RUN addgroup -S hostlet && adduser -S hostlet -G hostlet && chown -R hostlet:hostlet /app\n\
         USER hostlet\n\
         ENV NODE_ENV=production\n\
         ENV NPM_CONFIG_CACHE=/tmp/.npm\n\
         ENV PORT={port}\n\
         {build_line}\
         EXPOSE {port}\n\
         {start_line}\n",
        install = install,
        port = port,
        build_line = build_line,
        start_line = start_line
    )
}

async fn stream_lines<R: tokio::io::AsyncRead + Unpin>(
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

async fn wait_health(
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

async fn write_caddy_route(app: &str, domain: &str, port: u16) -> anyhow::Result<()> {
    let dir = PathBuf::from("/etc/caddy/hostlet");
    tokio::fs::create_dir_all(&dir).await?;
    let block = format!("{domain} {{\n  reverse_proxy 127.0.0.1:{port}\n}}\n");
    tokio::fs::write(dir.join(format!("{app}.caddy")), block).await?;
    Ok(())
}

async fn write_local_caddy_route(
    router: &LocalRouter,
    app: &str,
    domain: &str,
    port: u16,
) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(&router.snippets_dir).await?;
    let block = format!("@{app} host {domain}\nreverse_proxy @{app} 127.0.0.1:{port}\n");
    tokio::fs::write(router.snippets_dir.join(format!("{app}.caddy")), block).await?;
    Ok(())
}

async fn run_router_reload(
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

async fn status(cfg: &Config, id: Uuid, status: &str, failure: Option<&str>) {
    status_extra(cfg, id, status, failure, None, None, None).await;
}

async fn status_extra(
    cfg: &Config,
    id: Uuid,
    status: &str,
    failure: Option<&str>,
    image: Option<&str>,
    container: Option<&str>,
    local_url: Option<&str>,
) {
    post(cfg, json!({"type":"deployment_status","deployment_id":id,"status":status,"failure":failure,"image_tag":image,"container_name":container,"local_url":local_url})).await;
}

async fn log(cfg: &Config, id: Uuid, stream: &str, line: &str) {
    post(
        cfg,
        json!({"type":"log","deployment_id":id,"stream":stream,"line":line}),
    )
    .await;
}

async fn post(cfg: &Config, msg: Value) {
    let _ = reqwest::Client::new()
        .post(format!("{}/api/agent/events", cfg.api_url))
        .header("x-hostlet-server-id", cfg.server_id.to_string())
        .header("x-hostlet-agent-token", &cfg.agent_token)
        .json(&msg)
        .send()
        .await;
}

fn verify_signature(secret: &str, payload: &[u8], signature: &str) -> bool {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(payload);
    let expected = format!(
        "sha256={}",
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    constant_time_eq(expected.as_bytes(), signature.as_bytes())
}

fn env(key: &str) -> anyhow::Result<String> {
    std::env::var(key).with_context(|| format!("{key} is required"))
}

fn local_router_config() -> anyhow::Result<Option<LocalRouter>> {
    if std::env::var("HOSTLET_LOCAL_ROUTER").ok().as_deref() != Some("caddy") {
        return Ok(None);
    }
    let snippets_dir = PathBuf::from(
        std::env::var("HOSTLET_LOCAL_ROUTER_SNIPPETS_DIR")
            .unwrap_or_else(|_| "/var/lib/hostlet/caddy".into()),
    );
    let reload_command = std::env::var("HOSTLET_LOCAL_ROUTER_RELOAD")
        .unwrap_or_else(|_| {
            "docker exec hostlet-caddy caddy reload --config /etc/caddy/Caddyfile".into()
        })
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if reload_command.is_empty() {
        bail!("HOSTLET_LOCAL_ROUTER_RELOAD cannot be empty");
    }
    Ok(Some(LocalRouter {
        snippets_dir,
        reload_command,
    }))
}

fn safe_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}
async fn docker_published_port(container: &str, container_port: u16) -> anyhow::Result<u16> {
    let target = format!("{container_port}/tcp");
    let output = Command::new("docker")
        .args(["port", container, &target])
        .output()
        .await
        .context("failed to inspect Docker published port")?;
    if !output.status.success() {
        bail!("could not inspect Docker published port");
    }
    let stdout =
        String::from_utf8(output.stdout).context("Docker port output was not valid UTF-8")?;
    stdout
        .lines()
        .filter_map(|line| line.rsplit(':').next())
        .filter_map(|port| port.trim().parse::<u16>().ok())
        .next()
        .context("Docker did not report a published port")
}
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn redact(line: &str) -> String {
    let lowered = line.to_lowercase();
    let sensitive = [
        "token",
        "secret",
        "password",
        "passwd",
        "api_key",
        "apikey",
        "access_key",
        "private key",
        "authorization:",
        "bearer ",
        "credential",
    ];
    if sensitive.iter().any(|needle| lowered.contains(needle)) {
        "[redacted]".into()
    } else {
        line.into()
    }
}
fn env_args(p: &Value) -> Vec<String> {
    p.get("env")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("")))
                .collect()
        })
        .unwrap_or_default()
}

fn validate_repo(value: &str) -> anyhow::Result<()> {
    let mut parts = value.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || [owner, repo].into_iter().any(|part| {
            part.is_empty()
                || part.len() > 100
                || part.starts_with('.')
                || part.ends_with('.')
                || !part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        })
    {
        bail!("repo must be a GitHub owner/repo name");
    }
    Ok(())
}

fn validate_branch(value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > 255
        || value.starts_with('-')
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains("..")
        || value.contains("@{")
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
    {
        bail!("branch name contains unsupported characters");
    }
    Ok(())
}

fn validate_port(value: i64) -> anyhow::Result<()> {
    if !(1..=65_535).contains(&value) {
        bail!("container port must be between 1 and 65535");
    }
    Ok(())
}

fn validate_domain(value: &str) -> anyhow::Result<()> {
    let valid = if let Some((host, port)) = value.rsplit_once(':') {
        valid_hostname(host) && !port.is_empty() && port.parse::<u16>().is_ok()
    } else {
        valid_hostname(value)
    };
    if !valid {
        bail!("domain must be a hostname with optional port");
    }
    Ok(())
}

fn valid_hostname(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && !value.starts_with('.')
        && !value.ends_with('.')
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

fn validate_health_path(value: &str) -> anyhow::Result<()> {
    if !value.starts_with('/')
        || value.len() > 256
        || value.chars().any(|c| c.is_control() || c == '\\')
    {
        bail!("health path must start with / and cannot contain control characters");
    }
    Ok(())
}

fn validate_dockerfile_command(value: &str) -> anyhow::Result<()> {
    if value.len() > 500 || value.chars().any(|c| matches!(c, '\n' | '\r' | '\0')) {
        bail!("commands cannot contain newlines, NUL bytes, or more than 500 characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redacts_secret_lines() {
        assert_eq!(redact("TOKEN=abc"), "[redacted]");
        assert_eq!(redact("build ok"), "build ok");
    }
    #[test]
    fn rejects_bad_job_signature() {
        assert!(!verify_signature("secret", b"{}", "sha256=bad"));
    }

    #[test]
    fn detects_next_framework() {
        let package = serde_json::json!({"dependencies":{"next":"16.0.0"}});
        assert!(matches!(
            detect_framework(&collect_deps(&package)),
            Framework::Next
        ));
    }

    #[test]
    fn generated_node_dockerfile_uses_selected_port() {
        let dockerfile = generated_node_dockerfile(
            PackageManager::Npm,
            None,
            Some("npm run build"),
            "npm run start",
            3000,
        );
        assert!(dockerfile.contains("ENV PORT=3000"));
        assert!(dockerfile.contains("npm run build"));
        assert!(dockerfile.contains("npm run start"));
    }
}
