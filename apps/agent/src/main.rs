use anyhow::{bail, Context};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::{Output, Stdio},
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
    http: reqwest::Client,
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
        http: http_client()?,
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
    log_docker_tooling().await;
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
    let mut job_claim = tokio::time::interval(Duration::from_secs(3));
    let mut resource_stats = tokio::time::interval(Duration::from_secs(5));
    let mut runtime_health = tokio::time::interval(Duration::from_secs(60));
    let mut health_counts: HashMap<Uuid, HealthCounts> = HashMap::new();
    loop {
        tokio::select! {
            _ = heartbeat.tick() => ws.send(Message::Text(json!({"type":"heartbeat"}).to_string())).await?,
            _ = job_claim.tick() => claim_and_run_job(&cfg).await,
            _ = resource_stats.tick() => publish_resource_stats(&cfg).await,
            _ = runtime_health.tick() => publish_runtime_health(&cfg, &mut health_counts).await,
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => handle_ws_text(&cfg, &text).await,
                    Some(Ok(Message::Ping(payload))) => ws.send(Message::Pong(payload)).await?,
                    Some(Ok(Message::Close(_))) | None => bail!("websocket closed"),
                    Some(Ok(_)) => continue,
                    Some(Err(err)) => bail!("websocket error: {err}"),
                }
            }
        }
    }
}

async fn claim_and_run_job(cfg: &Config) {
    let response = cfg
        .http
        .post(format!("{}/api/agent/jobs/claim", cfg.api_url))
        .header("x-hostlet-server-id", cfg.server_id.to_string())
        .header("x-hostlet-agent-token", &cfg.agent_token)
        .json(&json!({"agent_id": cfg.server_id.to_string()}))
        .send()
        .await;
    let Ok(response) = response else {
        return;
    };
    if !response.status().is_success() {
        return;
    }
    let Ok(value) = response.json::<Value>().await else {
        return;
    };
    let Some(job) = value.get("job").filter(|job| !job.is_null()) else {
        return;
    };
    let Some(payload) = job.get("payload").cloned() else {
        return;
    };
    let Some(signature) = job.get("signature").and_then(|v| v.as_str()) else {
        return;
    };
    let Ok(raw) = serde_json::to_vec(&payload) else {
        return;
    };
    if !verify_signature(&cfg.job_signing_secret, &raw, signature) {
        tracing::warn!("ignored claimed job with invalid signature");
        return;
    }
    let Some(job_id) = payload
        .get("job_id")
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok())
    else {
        return;
    };
    match run_claimed_job_with_lease(cfg.clone(), job_id, payload.clone()).await {
        Ok(()) => complete_claimed_job(cfg, job_id, "success", None).await,
        Err(err) => {
            let message = format!("{err}");
            if let Some(deployment_id) = payload
                .get("deployment_id")
                .and_then(|v| v.as_str())
                .and_then(|v| Uuid::parse_str(v).ok())
            {
                log(cfg, deployment_id, "stderr", &message).await;
                status(
                    cfg,
                    deployment_id,
                    "failed",
                    Some(&format!("{message}. Add a Dockerfile, or add package.json build/start scripts Hostlet can run.")),
                )
                .await;
            }
            complete_claimed_job(cfg, job_id, "failed", Some(&message)).await;
            tracing::warn!("claimed job failed: {message}");
        }
    }
}

async fn run_claimed_job_with_lease(
    cfg: Config,
    job_id: Uuid,
    payload: Value,
) -> anyhow::Result<()> {
    job_status(&cfg, job_id, "running", None).await;
    let renew_cfg = cfg.clone();
    let renew = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            job_status(&renew_cfg, job_id, "running", None).await;
        }
    });
    let result = handle_job(cfg, payload).await;
    renew.abort();
    result
}

async fn complete_claimed_job(cfg: &Config, id: Uuid, status: &str, failure: Option<&str>) {
    let _ = cfg
        .http
        .post(format!("{}/api/agent/jobs/{id}/complete", cfg.api_url))
        .header("x-hostlet-server-id", cfg.server_id.to_string())
        .header("x-hostlet-agent-token", &cfg.agent_token)
        .json(&json!({"status":status,"failure":failure}))
        .send()
        .await;
}

async fn handle_ws_text(cfg: &Config, text: &str) {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        tracing::warn!("ignored invalid websocket JSON from API");
        return;
    };
    if value.get("type").and_then(|v| v.as_str()) != Some("job") {
        return;
    }
    let Some(payload) = value.get("payload").cloned() else {
        tracing::warn!("ignored job without payload");
        return;
    };
    let Some(signature) = value.get("signature").and_then(|v| v.as_str()) else {
        tracing::warn!("ignored job without signature");
        return;
    };
    let Ok(raw) = serde_json::to_vec(&payload) else {
        tracing::warn!("ignored job with unserializable payload");
        return;
    };
    if !verify_signature(&cfg.job_signing_secret, &raw, signature) {
        tracing::warn!("ignored job with invalid signature");
        return;
    }
    let job_id = payload
        .get("job_id")
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok());
    match handle_job(cfg.clone(), payload.clone()).await {
        Ok(()) => {
            if let Some(job_id) = job_id {
                job_status(cfg, job_id, "success", None).await;
            }
        }
        Err(err) => {
            let message = format!("{err}");
            if let Some(deployment_id) = payload
                .get("deployment_id")
                .and_then(|v| v.as_str())
                .and_then(|v| Uuid::parse_str(v).ok())
            {
                log(cfg, deployment_id, "stderr", &message).await;
                status(
                    cfg,
                    deployment_id,
                    "failed",
                    Some(&format!("{message}. Add a Dockerfile, or add package.json build/start scripts Hostlet can run.")),
                )
                .await;
            }
            if let Some(job_id) = job_id {
                job_status(cfg, job_id, "failed", Some(&message)).await;
            }
            tracing::warn!("job failed: {message}");
        }
    }
}

async fn handle_job(cfg: Config, payload: Value) -> anyhow::Result<()> {
    match payload.get("type").and_then(|v| v.as_str()) {
        Some("deploy") => deploy(cfg, payload).await,
        Some("rollback") => rollback(cfg, payload).await,
        Some("delete_app") => delete_app(cfg, payload).await,
        Some("health_check") => {
            health_check_job(&cfg, &payload).await;
            Ok(())
        }
        Some("restart_container") => {
            restart_container_job(&cfg, &payload).await?;
            Ok(())
        }
        Some("docker_cleanup") => docker_cleanup_job(&payload).await,
        _ => Ok(()),
    }
}

async fn deploy(cfg: Config, p: Value) -> anyhow::Result<()> {
    let deployment_id = Uuid::parse_str(p["deployment_id"].as_str().context("deployment_id")?)?;
    let app_id = Uuid::parse_str(p["app_id"].as_str().context("app_id")?)?;
    let app_name = safe_name(&format!("app-{app_id}"));
    let route_key = p
        .get("route_key")
        .and_then(|v| v.as_str())
        .map(safe_name)
        .unwrap_or_else(|| app_name.clone());
    let repo = p["repo"].as_str().context("repo")?;
    let branch = p["branch"].as_str().context("branch")?;
    let commit_sha = p
        .get("commit_sha")
        .and_then(|v| v.as_str())
        .unwrap_or("HEAD");
    let port = p["container_port"].as_i64().context("container_port")?;
    let domain = p["domain"].as_str().context("domain")?;
    let health_path = p["health_path"].as_str().unwrap_or("/");
    let root_directory = p
        .get("root_directory")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let github_token = p.get("github_token").and_then(|v| v.as_str());
    validate_repo(repo)?;
    validate_branch(branch)?;
    validate_commit_sha(commit_sha)?;
    validate_port(port)?;
    validate_domain(domain)?;
    validate_health_path(health_path)?;
    status(&cfg, deployment_id, "building", None).await;
    let checkout = cfg.workdir.join("repos").join(&app_name);
    let expected_remote = format!("https://github.com/{repo}.git");
    let fetch_remote = git_fetch_remote(repo, github_token);
    if checkout.exists() {
        ensure_checkout_remote(&cfg, deployment_id, &checkout, &expected_remote).await?;
        run_log(
            &cfg,
            deployment_id,
            "git",
            &[
                "-C",
                checkout.to_str().unwrap(),
                "fetch",
                &fetch_remote,
                branch,
            ],
        )
        .await?;
        if commit_sha == "HEAD" {
            run_log(
                &cfg,
                deployment_id,
                "git",
                &[
                    "-C",
                    checkout.to_str().unwrap(),
                    "checkout",
                    "-B",
                    branch,
                    "FETCH_HEAD",
                ],
            )
            .await?;
        } else {
            run_log(
                &cfg,
                deployment_id,
                "git",
                &[
                    "-C",
                    checkout.to_str().unwrap(),
                    "checkout",
                    "--detach",
                    commit_sha,
                ],
            )
            .await?;
        }
    } else {
        tokio::fs::create_dir_all(&checkout).await?;
        run_log(
            &cfg,
            deployment_id,
            "git",
            &["-C", checkout.to_str().unwrap(), "init"],
        )
        .await?;
        run_log(
            &cfg,
            deployment_id,
            "git",
            &[
                "-C",
                checkout.to_str().unwrap(),
                "remote",
                "add",
                "origin",
                &expected_remote,
            ],
        )
        .await?;
        run_log(
            &cfg,
            deployment_id,
            "git",
            &[
                "-C",
                checkout.to_str().unwrap(),
                "fetch",
                &fetch_remote,
                branch,
            ],
        )
        .await?;
        if commit_sha != "HEAD" {
            run_log(
                &cfg,
                deployment_id,
                "git",
                &[
                    "-C",
                    checkout.to_str().unwrap(),
                    "checkout",
                    "--detach",
                    commit_sha,
                ],
            )
            .await?;
        } else {
            run_log(
                &cfg,
                deployment_id,
                "git",
                &[
                    "-C",
                    checkout.to_str().unwrap(),
                    "checkout",
                    "-B",
                    branch,
                    "FETCH_HEAD",
                ],
            )
            .await?;
        }
    }
    if commit_sha != "HEAD" {
        verify_git_head(&cfg, deployment_id, &checkout, commit_sha).await?;
    }
    let image = format!("hostlet/{app_name}:{deployment_id}");
    let project_dir = safe_project_dir(&checkout, root_directory).await?;
    if p.get("runtime_kind").and_then(|v| v.as_str()) == Some("compose") {
        return deploy_compose(
            cfg,
            p.clone(),
            deployment_id,
            app_id,
            &app_name,
            &route_key,
            &project_dir,
            port,
            domain,
            health_path,
        )
        .await;
    }
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
    let port_map = docker_port_map(port as u16);
    let data_volume = app_data_volume(app_id);
    ensure_app_data_volume(&cfg, deployment_id, &data_volume).await?;
    let data_mount = format!("type=volume,source={data_volume},target=/data");
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
        "--mount",
        &data_mount,
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
    let mut env_pairs = env_args(&p);
    if !env_pairs_has_key(&env_pairs, "HOSTLET_DATA_DIR") {
        env_pairs.push("HOSTLET_DATA_DIR=/data".into());
    }
    if !env_pairs_has_key(&env_pairs, "DATA_DIR") {
        env_pairs.push("DATA_DIR=/data".into());
    }
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
            apply_local_caddy_route(
                &cfg,
                deployment_id,
                router,
                &route_key,
                domain,
                internal_port,
            )
            .await?;
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
        apply_caddy_route(&cfg, deployment_id, &route_key, domain, internal_port).await?;
    }
    status_extra(
        &cfg,
        deployment_id,
        "success",
        StatusDetails {
            image: Some(&image),
            container: Some(&container),
            local_url: local_url.as_deref(),
            published_port: Some(internal_port),
            ..StatusDetails::default()
        },
    )
    .await;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct HostletManifest {
    runtime: String,
    compose: HostletComposeManifest,
}

#[derive(Debug, Deserialize)]
struct HostletComposeManifest {
    file: Option<String>,
    web_service: String,
    port: Option<u16>,
    health_path: Option<String>,
}

#[allow(clippy::too_many_arguments)]
async fn deploy_compose(
    cfg: Config,
    p: Value,
    deployment_id: Uuid,
    app_id: Uuid,
    app_name: &str,
    route_key: &str,
    project_dir: &Path,
    fallback_port: i64,
    domain: &str,
    fallback_health_path: &str,
) -> anyhow::Result<()> {
    ensure_docker_compose().await?;
    let generated_compose = p
        .pointer("/runtime_config/generatedCompose")
        .and_then(|v| v.as_object());
    let build_dir = cfg.workdir.join("builds").join(deployment_id.to_string());
    let (manifest_path, manifest, compose_file_name, compose_file) =
        if let Some(generated) = generated_compose {
            let compose_file_name = generated
                .get("composeFile")
                .and_then(|v| v.as_str())
                .unwrap_or("compose.generated.hostlet.yml");
            validate_relative_file_path(compose_file_name)?;
            let web_service = generated
                .get("webService")
                .and_then(|v| v.as_str())
                .unwrap_or("web")
                .to_string();
            validate_service_name(&web_service)?;
            let compose_text = generated
                .get("compose")
                .and_then(|v| v.as_str())
                .context("generated Compose runtime is missing compose YAML")?;
            tokio::fs::create_dir_all(&build_dir).await?;
            let compose_file = build_dir.join(compose_file_name);
            tokio::fs::write(&compose_file, compose_text).await?;
            let manifest = HostletManifest {
                runtime: "compose".into(),
                compose: HostletComposeManifest {
                    file: Some(compose_file_name.to_string()),
                    web_service,
                    port: generated
                        .get("port")
                        .and_then(|v| v.as_u64())
                        .and_then(|v| u16::try_from(v).ok()),
                    health_path: generated
                        .get("healthPath")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                },
            };
            (
                "generated",
                manifest,
                compose_file_name.to_string(),
                compose_file,
            )
        } else {
            let manifest_path = p
                .get("hostlet_config_path")
                .and_then(|v| v.as_str())
                .unwrap_or("hostlet.yml");
            validate_relative_file_path(manifest_path)?;
            let manifest_file = project_dir.join(manifest_path);
            let manifest_text = tokio::fs::read_to_string(&manifest_file)
                .await
                .with_context(|| format!("compose runtime requires {manifest_path}"))?;
            let manifest: HostletManifest = serde_yaml::from_str(&manifest_text)
                .context("hostlet manifest is not valid YAML")?;
            if manifest.runtime != "compose" {
                bail!("hostlet manifest runtime must be compose");
            }
            validate_service_name(&manifest.compose.web_service)?;
            let compose_file_name = manifest
                .compose
                .file
                .clone()
                .unwrap_or_else(|| "compose.yaml".into());
            validate_relative_file_path(&compose_file_name)?;
            let compose_file = project_dir.join(&compose_file_name);
            if !tokio::fs::try_exists(&compose_file).await? {
                bail!("compose file {compose_file_name} does not exist");
            }
            (manifest_path, manifest, compose_file_name, compose_file)
        };
    let compose_text = tokio::fs::read_to_string(&compose_file).await?;
    validate_compose_subset(&compose_text, &manifest.compose.web_service)?;
    let port = manifest.compose.port.unwrap_or(fallback_port as u16);
    validate_port(port as i64)?;
    let health_path = manifest
        .compose
        .health_path
        .as_deref()
        .unwrap_or(fallback_health_path);
    validate_health_path(health_path)?;
    let project = compose_project_name(app_id);
    let override_file = build_dir.join("compose.hostlet.yml");
    if let Some(parent) = override_file.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(
        &override_file,
        compose_override_yaml(
            &manifest.compose.web_service,
            port,
            app_id,
            deployment_id,
            &p,
        ),
    )
    .await?;
    log(
        &cfg,
        deployment_id,
        "stdout",
        &format!(
            "Detected Hostlet Compose app. Project {project}, web service {}.",
            manifest.compose.web_service
        ),
    )
    .await;
    run_log_in_dir(
        &cfg,
        deployment_id,
        project_dir,
        "docker",
        &[
            "compose",
            "-p",
            &project,
            "-f",
            compose_file.to_str().unwrap(),
            "-f",
            override_file.to_str().unwrap(),
            "config",
        ],
    )
    .await?;
    run_log_in_dir(
        &cfg,
        deployment_id,
        project_dir,
        "docker",
        &[
            "compose",
            "-p",
            &project,
            "-f",
            compose_file.to_str().unwrap(),
            "-f",
            override_file.to_str().unwrap(),
            "up",
            "-d",
            "--build",
            "--remove-orphans",
        ],
    )
    .await?;
    status(&cfg, deployment_id, "starting", None).await;
    let container = compose_service_container(
        project_dir,
        &project,
        &compose_file,
        &override_file,
        &manifest.compose.web_service,
    )
    .await?;
    let internal_port = docker_published_port(&container, port).await?;
    status(&cfg, deployment_id, "health_checking", None).await;
    if let Err(err) = wait_health(&cfg, deployment_id, &container, internal_port, health_path).await
    {
        let _ = run_log_in_dir(
            &cfg,
            deployment_id,
            project_dir,
            "docker",
            &[
                "compose",
                "-p",
                &project,
                "-f",
                compose_file.to_str().unwrap(),
                "-f",
                override_file.to_str().unwrap(),
                "logs",
                "--tail",
                "120",
            ],
        )
        .await;
        status(&cfg, deployment_id, "failed", Some(&format!("Compose health check failed: {err}. The previous working route was preserved; inspect Compose service logs for details."))).await;
        return Ok(());
    }
    status(&cfg, deployment_id, "routing", None).await;
    let mut local_url = None;
    if cfg.local_mode {
        if let Some(router) = &cfg.local_router {
            apply_local_caddy_route(
                &cfg,
                deployment_id,
                router,
                route_key,
                domain,
                internal_port,
            )
            .await?;
        }
        local_url = Some(if cfg.local_router.is_some() {
            domain.to_string()
        } else {
            format!("localhost:{internal_port}")
        });
    } else {
        apply_caddy_route(&cfg, deployment_id, route_key, domain, internal_port).await?;
    }
    status_extra(
        &cfg,
        deployment_id,
        "success",
        StatusDetails {
            container: Some(&container),
            local_url: local_url.as_deref(),
            published_port: Some(internal_port),
            compose_project: Some(&project),
            runtime_metadata: Some(json!({
                "runtime": "compose",
                "composeFile": compose_file_name,
                "hostletConfigPath": manifest_path,
                "webService": manifest.compose.web_service,
                "targetPort": port,
                "healthPath": health_path,
                "project": project,
                "appName": app_name,
            })),
            ..StatusDetails::default()
        },
    )
    .await;
    Ok(())
}

async fn rollback(cfg: Config, p: Value) -> anyhow::Result<()> {
    let deployment_id = Uuid::parse_str(p["deployment_id"].as_str().context("deployment_id")?)?;
    let container = p["target_container"].as_str().context("target_container")?;
    let domain = p["domain"].as_str().context("domain")?;
    let route_key = p
        .get("route_key")
        .and_then(|v| v.as_str())
        .map(safe_name)
        .unwrap_or_else(|| safe_name(p["app_id"].as_str().unwrap_or("app")));
    let port_value = p["published_port"]
        .as_i64()
        .context("target deployment is missing a published port; redeploy before rolling back")?;
    validate_port(port_value)?;
    let port = port_value as u16;
    validate_domain(domain)?;
    status(&cfg, deployment_id, "routing", None).await;
    if cfg.local_mode {
        if let Some(router) = &cfg.local_router {
            apply_local_caddy_route(&cfg, deployment_id, router, &route_key, domain, port).await?;
        }
        let local_url = cfg.local_router.as_ref().map(|_| domain);
        status_extra(
            &cfg,
            deployment_id,
            "rolled_back",
            StatusDetails {
                container: Some(container),
                local_url,
                published_port: Some(port),
                ..StatusDetails::default()
            },
        )
        .await;
        return Ok(());
    }
    match apply_caddy_route(&cfg, deployment_id, &route_key, domain, port).await {
        Ok(_) => {
            status_extra(
                &cfg,
                deployment_id,
                "rolled_back",
                StatusDetails {
                    container: Some(container),
                    published_port: Some(port),
                    ..StatusDetails::default()
                },
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

async fn delete_app(cfg: Config, p: Value) -> anyhow::Result<()> {
    let app_id = p
        .get("app_id")
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok());
    let route_key = p
        .get("route_key")
        .and_then(|v| v.as_str())
        .map(safe_name)
        .unwrap_or_else(|| safe_name(p["app_id"].as_str().unwrap_or("app")));
    if let Some(project) = p.get("compose_project").and_then(|v| v.as_str()) {
        remove_compose_project_resources(project).await?;
    }
    let containers = p
        .get("containers")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for container in containers.iter().filter_map(|v| v.as_str()) {
        if !valid_container_name(container) {
            bail!("refusing to remove invalid managed container name during teardown");
        }
        run_quiet_absent_ok("docker", &["rm", "-f", container], &["No such container"]).await?;
    }
    let images = p
        .get("images")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for image in images.iter().filter_map(|v| v.as_str()) {
        if !valid_hostlet_image(image) {
            bail!("refusing to remove invalid managed image name during teardown");
        }
        run_quiet_absent_ok("docker", &["image", "rm", "-f", image], &["No such image"]).await?;
    }
    if cfg.local_mode {
        if let Some(router) = &cfg.local_router {
            remove_local_caddy_route(router, &route_key).await?;
            run_router_reload_quiet(router).await?;
        }
        if let Some(app_id) = app_id {
            remove_app_data_volume(app_id).await?;
        }
        return Ok(());
    }
    remove_caddy_route(&route_key).await?;
    run_quiet("caddy", &["reload", "--config", "/etc/caddy/Caddyfile"]).await?;
    if let Some(app_id) = app_id {
        remove_app_data_volume(app_id).await?;
    }
    Ok(())
}

async fn docker_cleanup_job(p: &Value) -> anyhow::Result<()> {
    let dry_run = p.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);
    let keep_containers = string_set_from_array(p.get("keep_containers"));
    let keep_images = string_set_from_array(p.get("keep_images"));

    let containers = hostlet_containers_all().await?;
    for container in containers {
        if keep_containers.contains(&container) {
            continue;
        }
        if docker_compose_managed_container(&container).await? {
            continue;
        }
        if !valid_container_name(&container) {
            bail!("refusing to clean invalid managed container name");
        }
        if !dry_run {
            run_quiet_absent_ok("docker", &["rm", "-f", &container], &["No such container"])
                .await?;
        }
    }

    let images = hostlet_images().await?;
    for image in images {
        if keep_images.contains(&image) {
            continue;
        }
        if !valid_hostlet_image(&image) {
            bail!("refusing to clean invalid managed image name");
        }
        if !dry_run {
            run_quiet_absent_ok("docker", &["image", "rm", "-f", &image], &["No such image"])
                .await?;
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
        &format!("$ {} {}", bin, command_args_for_log(args).join(" ")),
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
    let status = match tokio::time::timeout(Duration::from_secs(30 * 60), child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            let _ = child.kill().await;
            bail!("{bin} timed out after 1800 seconds");
        }
    };
    if !status.success() {
        bail!("{bin} exited with {status}");
    }
    Ok(())
}

async fn run_log_in_dir(
    cfg: &Config,
    deployment_id: Uuid,
    dir: &Path,
    bin: &str,
    args: &[&str],
) -> anyhow::Result<()> {
    log(
        cfg,
        deployment_id,
        "stdout",
        &format!("$ {} {}", bin, command_args_for_log(args).join(" ")),
    )
    .await;
    let mut cmd = Command::new(bin);
    cmd.current_dir(dir)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
    let status = match tokio::time::timeout(Duration::from_secs(30 * 60), child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            let _ = child.kill().await;
            bail!("{bin} timed out after 1800 seconds");
        }
    };
    if !status.success() {
        bail!("{bin} exited with {status}");
    }
    Ok(())
}

async fn run_quiet(bin: &str, args: &[&str]) -> anyhow::Result<()> {
    let output = command_output(bin, args, Duration::from_secs(120)).await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{bin} exited with {}: {}", output.status, stderr.trim());
    }
    Ok(())
}

async fn run_quiet_absent_ok(
    bin: &str,
    args: &[&str],
    absent_needles: &[&str],
) -> anyhow::Result<()> {
    let output = command_output(bin, args, Duration::from_secs(120)).await?;
    if output.status.success() {
        return Ok(());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    if absent_needles
        .iter()
        .any(|needle| combined.contains(needle))
    {
        return Ok(());
    }
    bail!("{bin} exited with {}: {}", output.status, combined.trim());
}

async fn run_capture_trim(
    cfg: &Config,
    deployment_id: Uuid,
    bin: &str,
    args: &[&str],
) -> anyhow::Result<String> {
    log(
        cfg,
        deployment_id,
        "stdout",
        &format!("$ {} {}", bin, command_args_for_log(args).join(" ")),
    )
    .await;
    let output = command_output(bin, args, Duration::from_secs(120)).await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "{bin} exited with {}: {}",
            output.status,
            redact(stderr.trim())
        );
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .context("command output was not valid UTF-8")
}

async fn ensure_checkout_remote(
    cfg: &Config,
    deployment_id: Uuid,
    checkout: &Path,
    expected_remote: &str,
) -> anyhow::Result<()> {
    let remote = run_capture_trim(
        cfg,
        deployment_id,
        "git",
        &[
            "-C",
            checkout.to_str().unwrap(),
            "config",
            "--get",
            "remote.origin.url",
        ],
    )
    .await?;
    if normalize_git_remote(&remote) != normalize_git_remote(expected_remote) {
        bail!("existing checkout remote does not match the requested repository");
    }
    Ok(())
}

async fn verify_git_head(
    cfg: &Config,
    deployment_id: Uuid,
    checkout: &Path,
    expected_commit: &str,
) -> anyhow::Result<()> {
    let head = run_capture_trim(
        cfg,
        deployment_id,
        "git",
        &["-C", checkout.to_str().unwrap(), "rev-parse", "HEAD"],
    )
    .await?;
    if !head.eq_ignore_ascii_case(expected_commit) {
        bail!("checked-out commit did not match the signed deployment commit");
    }
    Ok(())
}

fn normalize_git_remote(value: &str) -> String {
    value
        .trim()
        .trim_end_matches(".git")
        .trim_start_matches("https://")
        .to_ascii_lowercase()
}

fn git_fetch_remote(repo: &str, github_token: Option<&str>) -> String {
    let Some(token) = github_token.filter(|token| !token.trim().is_empty()) else {
        return format!("https://github.com/{repo}.git");
    };
    let encoded = url::form_urlencoded::byte_serialize(token.as_bytes()).collect::<String>();
    format!("https://x-access-token:{encoded}@github.com/{repo}.git")
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

    let hostlet_dir = cfg.workdir.join("builds").join(deployment_id.to_string());
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

async fn safe_project_dir(checkout: &Path, root_directory: &str) -> anyhow::Result<PathBuf> {
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

fn docker_port_map(port: u16) -> String {
    format!("127.0.0.1::{port}")
}

async fn apply_caddy_route(
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

fn render_caddy_route(app: &str, domain: &str, port: u16) -> String {
    format!(
        "# hostlet-route-key: {app}\n# hostlet-domain: {domain}\n{domain} {{\n  reverse_proxy 127.0.0.1:{port}\n}}\n"
    )
}

async fn remove_caddy_route(app: &str) -> anyhow::Result<()> {
    let target = PathBuf::from("/etc/caddy/hostlet").join(format!("{app}.caddy"));
    match tokio::fs::remove_file(target).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

async fn apply_local_caddy_route(
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

fn render_local_caddy_route(app: &str, domain: &str, port: u16) -> String {
    format!(
        "# hostlet-route-key: {app}\n# hostlet-domain: {domain}\n@{app} host {domain}\nreverse_proxy @{app} 127.0.0.1:{port}\n"
    )
}

async fn write_route_file(target: &Path, contents: &str) -> anyhow::Result<()> {
    let tmp = target.with_extension(format!("caddy.tmp-{}", std::process::id()));
    tokio::fs::write(&tmp, contents).await?;
    tokio::fs::rename(tmp, target).await?;
    Ok(())
}

async fn restore_route_file(target: &Path, previous: Option<Vec<u8>>) -> anyhow::Result<()> {
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

async fn remove_local_caddy_route(router: &LocalRouter, app: &str) -> anyhow::Result<()> {
    let target = router.snippets_dir.join(format!("{app}.caddy"));
    match tokio::fs::remove_file(target).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

async fn ensure_no_conflicting_route(
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

fn route_domain(contents: &str) -> Option<&str> {
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

async fn run_router_reload_quiet(router: &LocalRouter) -> anyhow::Result<()> {
    let Some((bin, args)) = router.reload_command.split_first() else {
        return Ok(());
    };
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    run_quiet(bin, &args).await
}

async fn status(cfg: &Config, id: Uuid, status: &str, failure: Option<&str>) {
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
struct StatusDetails<'a> {
    failure: Option<&'a str>,
    image: Option<&'a str>,
    container: Option<&'a str>,
    local_url: Option<&'a str>,
    published_port: Option<u16>,
    compose_project: Option<&'a str>,
    runtime_metadata: Option<Value>,
}

async fn status_extra(cfg: &Config, id: Uuid, status: &str, details: StatusDetails<'_>) {
    post_reliable(cfg, json!({"type":"deployment_status","deployment_id":id,"status":status,"failure":details.failure,"image_tag":details.image,"container_name":details.container,"local_url":details.local_url,"published_port":details.published_port,"compose_project":details.compose_project,"runtime_metadata":details.runtime_metadata})).await;
}

async fn log(cfg: &Config, id: Uuid, stream: &str, line: &str) {
    post(
        cfg,
        json!({"type":"log","deployment_id":id,"stream":stream,"line":line}),
    )
    .await;
}

async fn job_status(cfg: &Config, id: Uuid, status: &str, failure: Option<&str>) {
    post_reliable(
        cfg,
        json!({"type":"job_status","job_id":id,"status":status,"failure":failure}),
    )
    .await;
}

async fn post(cfg: &Config, msg: Value) {
    let _ = send_event(cfg, &msg).await;
}

async fn post_reliable(cfg: &Config, msg: Value) {
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

async fn send_event(cfg: &Config, msg: &Value) -> anyhow::Result<()> {
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

fn event_retry_delays() -> [Duration; 4] {
    [
        Duration::from_millis(0),
        Duration::from_millis(250),
        Duration::from_secs(1),
        Duration::from_secs(3),
    ]
}

#[derive(Default)]
struct HealthCounts {
    failures: u32,
    successes: u32,
}

struct HealthTarget {
    app_id: Uuid,
    deployment_id: Uuid,
    container_name: String,
    published_port: u16,
    health_path: String,
}

async fn publish_runtime_health(cfg: &Config, counts: &mut HashMap<Uuid, HealthCounts>) {
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
            json!({
                "type": "health_status",
                "app_id": target.app_id,
                "deployment_id": target.deployment_id,
                "container_name": target.container_name,
                "status": status,
                "checked_url": result.url,
                "http_status": result.http_status,
                "latency_ms": result.latency_ms,
                "failure_count": entry.failures,
                "success_count": entry.successes,
                "error": result.error,
            }),
        )
        .await;
    }
}

async fn health_check_job(cfg: &Config, payload: &Value) {
    let Some(target) = health_target_from_payload(payload) else {
        return;
    };
    let result = probe_health_target(cfg, &target).await;
    post(
        cfg,
        json!({
            "type": "health_status",
            "app_id": target.app_id,
            "deployment_id": target.deployment_id,
            "container_name": target.container_name,
            "status": if result.healthy { "healthy" } else { "degraded" },
            "checked_url": result.url,
            "http_status": result.http_status,
            "latency_ms": result.latency_ms,
            "failure_count": if result.healthy { 0 } else { 1 },
            "success_count": if result.healthy { 1 } else { 0 },
            "error": result.error,
        }),
    )
    .await;
}

async fn restart_container_job(cfg: &Config, payload: &Value) -> anyhow::Result<()> {
    let Some(target) = health_target_from_payload(payload) else {
        bail!("restart job missing valid health target");
    };
    run_quiet("docker", &["restart", &target.container_name]).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let result = probe_health_target(cfg, &target).await;
    post(
        cfg,
        json!({
            "type": "health_status",
            "app_id": target.app_id,
            "deployment_id": target.deployment_id,
            "container_name": target.container_name,
            "status": if result.healthy { "healthy" } else { "degraded" },
            "checked_url": result.url,
            "http_status": result.http_status,
            "latency_ms": result.latency_ms,
            "failure_count": if result.healthy { 0 } else { 1 },
            "success_count": if result.healthy { 1 } else { 0 },
            "error": result.error,
        }),
    )
    .await;
    Ok(())
}

async fn health_targets(cfg: &Config) -> anyhow::Result<Vec<HealthTarget>> {
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

fn health_target_from_payload(value: &Value) -> Option<HealthTarget> {
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

struct HealthProbeResult {
    healthy: bool,
    url: String,
    http_status: Option<u16>,
    latency_ms: u128,
    error: Option<String>,
}

async fn probe_health_target(cfg: &Config, target: &HealthTarget) -> HealthProbeResult {
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

fn health_error_for_status(status: StatusCode) -> Option<String> {
    if status.is_success() || status.is_redirection() {
        None
    } else {
        Some(format!("HTTP {status}"))
    }
}

async fn container_running(container: &str) -> anyhow::Result<()> {
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

async fn publish_resource_stats(cfg: &Config) {
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

async fn hostlet_containers() -> anyhow::Result<Vec<String>> {
    let output = command_output(
        "docker",
        &[
            "ps",
            "--filter",
            "name=^/hostlet-",
            "--format",
            "{{.Names}}",
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
        .filter(|name| valid_container_name(name))
        .map(str::to_string)
        .collect())
}

async fn hostlet_containers_all() -> anyhow::Result<Vec<String>> {
    let output = command_output(
        "docker",
        &[
            "ps",
            "-a",
            "--filter",
            "name=^/hostlet-",
            "--format",
            "{{.Names}}",
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
        .filter(|name| valid_container_name(name))
        .map(str::to_string)
        .collect())
}

async fn hostlet_images() -> anyhow::Result<Vec<String>> {
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

async fn docker_compose_managed_container(container: &str) -> anyhow::Result<bool> {
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

fn string_set_from_array(value: Option<&Value>) -> HashSet<String> {
    value
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .map(str::to_string)
        .collect()
}

async fn command_output(bin: &str, args: &[&str], timeout: Duration) -> anyhow::Result<Output> {
    let mut cmd = Command::new(bin);
    cmd.args(args).kill_on_drop(true);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(output) => output.with_context(|| format!("failed to start {bin}")),
        Err(_) => bail!("{bin} timed out after {} seconds", timeout.as_secs()),
    }
}

async fn log_docker_tooling() {
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

async fn ensure_docker_compose() -> anyhow::Result<()> {
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

fn http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .user_agent("Hostlet-Agent")
        .build()
        .context("failed to build HTTP client")
}

fn valid_container_name(value: &str) -> bool {
    value.starts_with("hostlet-")
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn valid_hostlet_image(value: &str) -> bool {
    value.starts_with("hostlet/")
        && value.len() <= 300
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | ':' | '.' | '_' | '-'))
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
    let output = command_output(
        "docker",
        &["port", container, &target],
        Duration::from_secs(15),
    )
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

async fn compose_service_container(
    dir: &Path,
    project: &str,
    compose_file: &Path,
    override_file: &Path,
    service: &str,
) -> anyhow::Result<String> {
    let output = command_output_in_dir(
        dir,
        "docker",
        &[
            "compose",
            "-p",
            project,
            "-f",
            compose_file.to_str().unwrap(),
            "-f",
            override_file.to_str().unwrap(),
            "ps",
            "-q",
            service,
        ],
        Duration::from_secs(30),
    )
    .await?;
    if !output.status.success() {
        bail!("docker compose ps failed");
    }
    let id = String::from_utf8(output.stdout)?.trim().to_string();
    if id.is_empty() {
        bail!("compose web service did not create a container");
    }
    let name_output = command_output(
        "docker",
        &["inspect", "-f", "{{.Name}}", &id],
        Duration::from_secs(15),
    )
    .await?;
    if !name_output.status.success() {
        bail!("failed to inspect compose web container");
    }
    let name = String::from_utf8(name_output.stdout)?
        .trim()
        .trim_start_matches('/')
        .to_string();
    if !valid_container_name(&name) {
        bail!("compose web container name is not Hostlet-managed");
    }
    Ok(name)
}

async fn command_output_in_dir(
    dir: &Path,
    bin: &str,
    args: &[&str],
    timeout: Duration,
) -> anyhow::Result<Output> {
    let mut cmd = Command::new(bin);
    cmd.current_dir(dir).args(args).kill_on_drop(true);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(output) => output.with_context(|| format!("failed to start {bin}")),
        Err(_) => bail!("{bin} timed out after {} seconds", timeout.as_secs()),
    }
}

fn compose_project_name(app_id: Uuid) -> String {
    format!("hostlet-app-{}", app_id.simple())
}

fn compose_override_yaml(
    web_service: &str,
    port: u16,
    app_id: Uuid,
    deployment_id: Uuid,
    payload: &Value,
) -> String {
    let mut env = vec![
        format!("HOSTLET_APP_ID={app_id}"),
        format!("HOSTLET_DEPLOYMENT_ID={deployment_id}"),
        "HOSTLET_DATA_DIR=/data".to_string(),
        "DATA_DIR=/data".to_string(),
    ];
    if let Some(map) = payload.get("env").and_then(|v| v.as_object()) {
        for (key, value) in map {
            if valid_env_key(key) {
                let value = value.as_str().unwrap_or_default();
                env.push(format!("{}={}", key, value.replace('\n', "\\n")));
            }
        }
    }
    let env_yaml = env
        .iter()
        .map(|item| format!("      - {}", serde_yaml::to_string(item).unwrap().trim()))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "services:\n  {web_service}:\n    labels:\n      hostlet.app_id: \"{app_id}\"\n      hostlet.deployment_id: \"{deployment_id}\"\n      hostlet.role: \"web\"\n    environment:\n{env_yaml}\n    ports:\n      - target: {port}\n        host_ip: 127.0.0.1\n        protocol: tcp\n"
    )
}

fn validate_compose_subset(contents: &str, web_service: &str) -> anyhow::Result<()> {
    let value: serde_yaml::Value =
        serde_yaml::from_str(contents).context("compose file is not valid YAML")?;
    let services = value
        .get("services")
        .and_then(|v| v.as_mapping())
        .context("compose file must define services")?;
    if !services.contains_key(serde_yaml::Value::String(web_service.to_string())) {
        bail!("compose file does not contain declared web service {web_service}");
    }
    for (name, raw_service) in services {
        let Some(service_name) = name.as_str() else {
            bail!("compose service names must be strings");
        };
        validate_service_name(service_name)?;
        let service = raw_service
            .as_mapping()
            .context("compose services must be objects")?;
        for key in [
            "container_name",
            "network_mode",
            "privileged",
            "pid",
            "ipc",
            "devices",
            "networks",
            "ports",
        ] {
            if service.contains_key(serde_yaml::Value::String(key.to_string())) {
                bail!("compose service {service_name} uses unsupported field {key}");
            }
        }
        if let Some(volumes) = service
            .get(serde_yaml::Value::String("volumes".into()))
            .and_then(|v| v.as_sequence())
        {
            for volume in volumes {
                if let Some(value) = volume.as_str() {
                    if value.starts_with('/') || value.contains("../") {
                        bail!("compose service {service_name} uses an unsupported host bind mount");
                    }
                    continue;
                }
                if let Some(mapping) = volume.as_mapping() {
                    let volume_type = mapping
                        .get(serde_yaml::Value::String("type".into()))
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    let source = mapping
                        .get(serde_yaml::Value::String("source".into()))
                        .or_else(|| mapping.get(serde_yaml::Value::String("src".into())))
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    if volume_type == "bind" || source.starts_with('/') || source.contains("../") {
                        bail!("compose service {service_name} uses an unsupported host bind mount");
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_relative_file_path(value: &str) -> anyhow::Result<()> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 256
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.split('/').any(|part| part.is_empty() || part == "..")
        || value.chars().any(|c| c.is_control() || c == '\\')
    {
        bail!("path must be a relative file path inside the repository");
    }
    Ok(())
}

fn validate_service_name(value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > 48
        || !value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || value.starts_with('-')
        || value.ends_with('-')
    {
        bail!("compose service names must use lowercase letters, numbers, and hyphens");
    }
    Ok(())
}

fn valid_env_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
}
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn redact(line: &str) -> String {
    if let Some(redacted) = redact_url_credentials(line) {
        return redacted;
    }
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

fn redact_url_credentials(value: &str) -> Option<String> {
    let scheme = "https://";
    let start = value.find(scheme)?;
    let credentials_start = start + scheme.len();
    let at = value[credentials_start..].find('@')? + credentials_start;
    let mut redacted = String::with_capacity(value.len());
    redacted.push_str(&value[..start]);
    redacted.push_str("https://[redacted]@");
    redacted.push_str(&value[at + 1..]);
    Some(redacted)
}

fn command_args_for_log(args: &[&str]) -> Vec<String> {
    let mut output = Vec::with_capacity(args.len());
    let mut redact_next = false;
    for arg in args {
        if redact_next {
            output.push(redact_env_arg(arg));
            redact_next = false;
            continue;
        }
        if *arg == "-e" || *arg == "--env" {
            output.push((*arg).to_string());
            redact_next = true;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--env=") {
            output.push(format!("--env={}", redact_env_arg(value)));
            continue;
        }
        output.push(redact(arg));
    }
    output
}

fn redact_env_arg(arg: &str) -> String {
    match arg.split_once('=') {
        Some((key, _)) if !key.is_empty() => format!("{key}=[redacted]"),
        _ => "[redacted]".into(),
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

fn env_pairs_has_key(pairs: &[String], key: &str) -> bool {
    pairs
        .iter()
        .filter_map(|pair| pair.split_once('='))
        .any(|(existing, _)| existing == key)
}

fn app_data_volume(app_id: Uuid) -> String {
    format!("hostlet-app-data-{app_id}")
}

async fn ensure_app_data_volume(
    cfg: &Config,
    deployment_id: Uuid,
    volume: &str,
) -> anyhow::Result<()> {
    run_log(cfg, deployment_id, "docker", &["volume", "create", volume]).await?;
    let volume_mount = format!("{volume}:/data");
    run_log(
        cfg,
        deployment_id,
        "docker",
        &[
            "run",
            "--rm",
            "-v",
            &volume_mount,
            "alpine:3.20",
            "sh",
            "-lc",
            "chmod 0777 /data",
        ],
    )
    .await
}

async fn remove_app_data_volume(app_id: Uuid) -> anyhow::Result<()> {
    let volume = app_data_volume(app_id);
    run_quiet_absent_ok(
        "docker",
        &["volume", "rm", "-f", &volume],
        &["No such volume"],
    )
    .await
}

async fn remove_compose_project_resources(project: &str) -> anyhow::Result<()> {
    if !valid_compose_project_name(project) {
        bail!("refusing to remove invalid compose project");
    }
    let containers = docker_names_by_label(
        "ps",
        &[
            "-a",
            "--filter",
            &format!("label=com.docker.compose.project={project}"),
        ],
        "{{.Names}}",
    )
    .await?;
    for container in containers {
        if valid_container_name(&container) {
            run_quiet_absent_ok("docker", &["rm", "-f", &container], &["No such container"])
                .await?;
        }
    }
    let volumes = docker_names_by_label(
        "volume",
        &[
            "ls",
            "--filter",
            &format!("label=com.docker.compose.project={project}"),
        ],
        "{{.Name}}",
    )
    .await?;
    for volume in volumes {
        if valid_compose_volume_name(&volume) {
            run_quiet_absent_ok(
                "docker",
                &["volume", "rm", "-f", &volume],
                &["No such volume"],
            )
            .await?;
        }
    }
    Ok(())
}

async fn docker_names_by_label(
    cmd: &str,
    args: &[&str],
    format: &str,
) -> anyhow::Result<Vec<String>> {
    let mut full = vec![cmd];
    full.extend(args);
    full.push("--format");
    full.push(format);
    let output = command_output("docker", &full, Duration::from_secs(30)).await?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

fn valid_compose_project_name(value: &str) -> bool {
    value.starts_with("hostlet-app-")
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn valid_compose_volume_name(value: &str) -> bool {
    value.starts_with("hostlet-app-")
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
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

fn validate_commit_sha(value: &str) -> anyhow::Result<()> {
    if value == "HEAD" {
        return Ok(());
    }
    if value.len() == 40 && value.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(());
    }
    bail!("commit sha must be HEAD or a 40-character hex SHA");
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
    fn redacts_docker_env_values_in_logged_commands() {
        assert_eq!(
            command_args_for_log(&["run", "-e", "DATABASE_URL=postgres://secret", "image"]),
            vec![
                "run".to_string(),
                "-e".to_string(),
                "DATABASE_URL=[redacted]".to_string(),
                "image".to_string()
            ]
        );
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

    #[test]
    fn app_ports_bind_to_loopback_only() {
        assert_eq!(docker_port_map(3000), "127.0.0.1::3000");
        let override_yaml = compose_override_yaml(
            "web",
            3000,
            Uuid::nil(),
            Uuid::nil(),
            &serde_json::json!({}),
        );
        assert!(override_yaml.contains("host_ip: 127.0.0.1"));
        assert!(!override_yaml.contains("host_ip: 0.0.0.0"));
    }

    #[test]
    fn caddy_routes_render_loopback_upstreams() {
        assert!(render_caddy_route("app", "app.example.com", 12345)
            .contains("reverse_proxy 127.0.0.1:12345"));
        assert!(render_local_caddy_route("app", "app.example.com", 12345)
            .contains("reverse_proxy @app 127.0.0.1:12345"));
    }

    #[test]
    fn reliable_status_events_have_retry_backoff() {
        let delays = event_retry_delays();
        assert_eq!(delays.len(), 4);
        assert_eq!(delays[0], Duration::from_millis(0));
        assert!(delays[1] < delays[2]);
        assert!(delays[2] < delays[3]);
    }

    #[test]
    fn route_domain_parsing_is_exact_not_substring_based() {
        let route = "# hostlet-route-key: app-a\n# hostlet-domain: myapp.example.com\n@a host myapp.example.com\n";
        assert_eq!(route_domain(route), Some("myapp.example.com"));
        assert_ne!(route_domain(route), Some("app.example.com"));
    }

    #[tokio::test]
    async fn caddy_route_reload_failure_restores_previous_file_state() {
        let dir = std::env::temp_dir().join(format!("hostlet-agent-test-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let target = dir.join("app.caddy");

        tokio::fs::write(&target, b"old route").await.unwrap();
        restore_route_file(&target, Some(b"old route".to_vec()))
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(&target).await.unwrap(),
            "old route"
        );

        restore_route_file(&target, None).await.unwrap();
        assert!(!target.exists());
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[test]
    fn git_remote_with_token_redacts_credentials() {
        let remote = git_fetch_remote("owner/repo", Some("secret-token"));
        assert!(remote.contains("x-access-token"));
        assert_eq!(
            redact(&remote),
            "https://[redacted]@github.com/owner/repo.git"
        );
        assert_eq!(
            redact(&format!("fatal: unable to access '{remote}'")),
            "fatal: unable to access 'https://[redacted]@github.com/owner/repo.git'"
        );
    }

    #[test]
    fn compose_validation_accepts_private_services() {
        let compose = r#"
services:
  web:
    build: .
    depends_on:
      - redis
  worker:
    build: .
    command: npm run worker
  redis:
    image: redis:7-alpine
    volumes:
      - redis-data:/data
volumes:
  redis-data:
"#;
        validate_compose_subset(compose, "web").unwrap();
    }

    #[test]
    fn compose_validation_rejects_host_ports_and_bind_mounts() {
        let ports = r#"
services:
  web:
    build: .
    ports:
      - "3000:3000"
"#;
        assert!(validate_compose_subset(ports, "web").is_err());
        let bind_mount = r#"
services:
  web:
    build: .
    volumes:
      - /etc:/host-etc
"#;
        assert!(validate_compose_subset(bind_mount, "web").is_err());
        let long_bind_mount = r#"
services:
  web:
    build: .
    volumes:
      - type: bind
        source: /etc
        target: /host-etc
"#;
        assert!(validate_compose_subset(long_bind_mount, "web").is_err());
        let service_network = r#"
services:
  web:
    build: .
    networks:
      - hostlet
"#;
        assert!(validate_compose_subset(service_network, "web").is_err());
    }
}
