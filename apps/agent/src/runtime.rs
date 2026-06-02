use super::*;

#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) api_url: String,
    pub(crate) http: reqwest::Client,
    pub(crate) server_id: Uuid,
    pub(crate) agent_token: String,
    pub(crate) job_signing_secret: String,
    pub(crate) workdir: PathBuf,
    pub(crate) local_mode: bool,
    pub(crate) health_host: String,
    pub(crate) local_router: Option<LocalRouter>,
}

#[derive(Clone)]
pub(crate) struct LocalRouter {
    pub(crate) snippets_dir: PathBuf,
    pub(crate) reload_command: Vec<String>,
}

pub(crate) async fn run() -> anyhow::Result<()> {
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

pub(crate) async fn connect_loop(cfg: Config) -> anyhow::Result<()> {
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

pub(crate) async fn claim_and_run_job(cfg: &Config) {
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
            report_deployment_failure(cfg, &payload, &message).await;
            complete_claimed_job(cfg, job_id, "failed", Some(&message)).await;
            tracing::warn!("claimed job failed: {message}");
        }
    }
}

pub(crate) async fn run_claimed_job_with_lease(
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

pub(crate) async fn complete_claimed_job(
    cfg: &Config,
    id: Uuid,
    status: &str,
    failure: Option<&str>,
) {
    let _ = cfg
        .http
        .post(format!("{}/api/agent/jobs/{id}/complete", cfg.api_url))
        .header("x-hostlet-server-id", cfg.server_id.to_string())
        .header("x-hostlet-agent-token", &cfg.agent_token)
        .json(&json!({"status":status,"failure":failure}))
        .send()
        .await;
}

pub(crate) async fn handle_ws_text(cfg: &Config, text: &str) {
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
            report_deployment_failure(cfg, &payload, &message).await;
            if let Some(job_id) = job_id {
                job_status(cfg, job_id, "failed", Some(&message)).await;
            }
            tracing::warn!("job failed: {message}");
        }
    }
}

/// Reports a failed job back to the API as a `failed` deployment status (when the
/// payload carries a deployment id), mirroring the stderr log and status update
/// shared by the claim and websocket job paths.
async fn report_deployment_failure(cfg: &Config, payload: &Value, message: &str) {
    let Some(deployment_id) = payload
        .get("deployment_id")
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok())
    else {
        return;
    };
    log(cfg, deployment_id, "stderr", message).await;
    status(
        cfg,
        deployment_id,
        "failed",
        Some(&format!(
            "{message}. Add a Dockerfile, or add package.json build/start scripts Hostlet can run."
        )),
    )
    .await;
}

pub(crate) async fn handle_job(cfg: Config, payload: Value) -> anyhow::Result<()> {
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

/// Runs `git` against the checkout directory, streaming output as deployment logs.
async fn run_git(
    cfg: &Config,
    deployment_id: Uuid,
    checkout: &Path,
    args: &[&str],
) -> anyhow::Result<()> {
    let mut full = vec!["-C", checkout.to_str().unwrap()];
    full.extend_from_slice(args);
    run_log(cfg, deployment_id, "git", &full).await
}

/// Checks out the requested ref: the branch tip (`commit_sha == "HEAD"`) is reset
/// to `FETCH_HEAD`, otherwise the exact commit is checked out detached. Shared by
/// the existing-checkout and fresh-clone paths so the ref logic lives in one place.
async fn checkout_fetched_ref(
    cfg: &Config,
    deployment_id: Uuid,
    checkout: &Path,
    branch: &str,
    commit_sha: &str,
) -> anyhow::Result<()> {
    if commit_sha == "HEAD" {
        run_git(
            cfg,
            deployment_id,
            checkout,
            &["checkout", "-B", branch, "FETCH_HEAD"],
        )
        .await
    } else {
        run_git(
            cfg,
            deployment_id,
            checkout,
            &["checkout", "--detach", commit_sha],
        )
        .await
    }
}

/// Ensures `checkout` contains the requested branch/commit, reusing an existing
/// clone when present or initializing a fresh one otherwise.
async fn sync_checkout(
    cfg: &Config,
    deployment_id: Uuid,
    checkout: &Path,
    expected_remote: &str,
    fetch_remote: &str,
    branch: &str,
    commit_sha: &str,
) -> anyhow::Result<()> {
    if checkout.exists() {
        ensure_checkout_remote(cfg, deployment_id, checkout, expected_remote).await?;
    } else {
        tokio::fs::create_dir_all(checkout).await?;
        run_git(cfg, deployment_id, checkout, &["init"]).await?;
        run_git(
            cfg,
            deployment_id,
            checkout,
            &["remote", "add", "origin", expected_remote],
        )
        .await?;
    }
    run_git(
        cfg,
        deployment_id,
        checkout,
        &["fetch", fetch_remote, branch],
    )
    .await?;
    checkout_fetched_ref(cfg, deployment_id, checkout, branch, commit_sha).await
}

pub(crate) async fn deploy(cfg: Config, p: Value) -> anyhow::Result<()> {
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
    sync_checkout(
        &cfg,
        deployment_id,
        &checkout,
        &expected_remote,
        &fetch_remote,
        branch,
        commit_sha,
    )
    .await?;
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
    let built = build_image(
        &cfg,
        deployment_id,
        &app_name,
        &image,
        &project_dir,
        port,
        &p,
    )
    .await?;
    status(&cfg, deployment_id, "starting", None).await;
    let container = format!("hostlet-{app_name}-{deployment_id}");
    let internal_port = run_app_container(
        &cfg,
        deployment_id,
        app_id,
        &image,
        &container,
        port,
        built.generated,
        &p,
    )
    .await?;
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
            runtime_metadata: Some(built.runtime_metadata),
            ..StatusDetails::default()
        },
    )
    .await;
    Ok(())
}

/// Outcome of building the deployment image: the metadata reported to the API
/// plus whether the Dockerfile was Hostlet-generated (which relaxes the
/// read-only container hardening at run time).
struct BuiltImage {
    runtime_metadata: Value,
    generated: bool,
}

/// Prepares the build plan, writes the `.dockerignore` for generated builds, and
/// builds the image via buildx (with a local cache) or a plain `docker build`.
async fn build_image(
    cfg: &Config,
    deployment_id: Uuid,
    app_name: &str,
    image: &str,
    project_dir: &Path,
    port: i64,
    p: &Value,
) -> anyhow::Result<BuiltImage> {
    let build = prepare_build(cfg, deployment_id, project_dir, port, p).await?;
    if build.generated {
        tokio::fs::write(project_dir.join(".dockerignore"), generated_dockerignore()).await?;
    }
    let build_started = Instant::now();
    if docker_buildx_available().await {
        let cache_root = cfg.workdir.join("build-cache").join(app_name);
        let cache_next = cfg
            .workdir
            .join("build-cache")
            .join(format!("{app_name}-{deployment_id}"));
        tokio::fs::create_dir_all(&cache_root).await?;
        tokio::fs::create_dir_all(&cache_next).await?;
        let cache_from = format!("type=local,src={}", cache_root.to_string_lossy());
        let cache_to = format!("type=local,dest={},mode=max", cache_next.to_string_lossy());
        let args = buildx_args(
            image,
            build.dockerfile.to_str().unwrap(),
            build.context.to_str().unwrap(),
            &cache_from,
            &cache_to,
        );
        run_log(cfg, deployment_id, "docker", &args).await?;
        let _ = tokio::fs::remove_dir_all(&cache_root).await;
        let _ = tokio::fs::rename(&cache_next, &cache_root).await;
    } else {
        log(
            cfg,
            deployment_id,
            "stdout",
            "Docker BuildKit buildx is unavailable; falling back to docker build without local cache.",
        )
        .await;
        let args = docker_build_args(
            image,
            build.dockerfile.to_str().unwrap(),
            build.context.to_str().unwrap(),
        );
        run_log(cfg, deployment_id, "docker", &args).await?;
    }
    let build_duration_ms = build_started.elapsed().as_millis();
    let image_size = image_size_bytes(image).await.ok();
    if let Some(size) = image_size {
        log(
            cfg,
            deployment_id,
            "stdout",
            &format!("Built image size: {size} bytes."),
        )
        .await;
    }
    Ok(BuiltImage {
        runtime_metadata: build_runtime_metadata(&build, build_duration_ms, image_size),
        generated: build.generated,
    })
}

/// Starts the application container with Hostlet's hardening, resource limits, and
/// env wiring, then returns the loopback port Docker published for it.
#[allow(clippy::too_many_arguments)]
async fn run_app_container(
    cfg: &Config,
    deployment_id: Uuid,
    app_id: Uuid,
    image: &str,
    container: &str,
    port: i64,
    generated: bool,
    p: &Value,
) -> anyhow::Result<u16> {
    let port_map = docker_port_map(port as u16);
    let data_volume = app_data_volume(app_id);
    ensure_app_data_volume(cfg, deployment_id, &data_volume).await?;
    let data_mount = format!("type=volume,source={data_volume},target=/data");
    let mut args = vec![
        "run",
        "-d",
        "--name",
        container,
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
    if !generated {
        args.push("--read-only");
        args.push("--tmpfs");
        args.push("/tmp");
    }
    let mut env_pairs = env_args(p);
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
    args.push(image);
    run_log(cfg, deployment_id, "docker", &args).await?;
    let internal_port = docker_published_port(container, port as u16).await?;
    log(
        cfg,
        deployment_id,
        "stdout",
        &format!("Docker assigned local port {internal_port} for container port {port}."),
    )
    .await;
    Ok(internal_port)
}
