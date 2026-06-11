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

#[derive(Debug)]
struct ReportedDeploymentFailure {
    message: String,
}

impl ReportedDeploymentFailure {
    fn new(message: String) -> Self {
        Self { message }
    }
}

impl std::fmt::Display for ReportedDeploymentFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ReportedDeploymentFailure {}

pub(crate) fn reported_deployment_failure(message: String) -> anyhow::Error {
    ReportedDeploymentFailure::new(message).into()
}

fn deployment_failure_message(err: &anyhow::Error) -> String {
    err.downcast_ref::<ReportedDeploymentFailure>()
        .map(|failure| failure.message.clone())
        .unwrap_or_else(|| format!("{err}"))
}

fn deployment_status_already_reported(err: &anyhow::Error) -> bool {
    err.downcast_ref::<ReportedDeploymentFailure>().is_some()
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
            let message = deployment_failure_message(&err);
            if !deployment_status_already_reported(&err) {
                report_deployment_failure(cfg, &payload, &message).await;
            }
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
            let message = deployment_failure_message(&err);
            if !deployment_status_already_reported(&err) {
                report_deployment_failure(cfg, &payload, &message).await;
            }
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
    status(cfg, deployment_id, "failed", Some(message)).await;
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
        Some("capture_screenshot") => capture_screenshot_job(&cfg, &payload).await,
        Some("restart_container") => {
            restart_container_job(&cfg, &payload).await?;
            Ok(())
        }
        Some("docker_cleanup") => docker_cleanup_job(&payload).await,
        _ => Ok(()),
    }
}

pub(crate) async fn deploy(cfg: Config, p: Value) -> anyhow::Result<()> {
    let deployment_id = Uuid::parse_str(p["deployment_id"].as_str().context("deployment_id")?)?;
    let app_id = Uuid::parse_str(p["app_id"].as_str().context("app_id")?)?;
    let app_name = app_slug(&format!("app-{app_id}"));
    let route_key = p
        .get("route_key")
        .and_then(|v| v.as_str())
        .map(app_slug)
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
    let git_sync_started = Instant::now();
    let mut git_sync_error = None;
    let checkout_result = sync_checkout(
        &cfg,
        deployment_id,
        &checkout,
        &expected_remote,
        &fetch_remote,
        branch,
        commit_sha,
    )
    .await;
    if let Err(err) = checkout_result {
        git_sync_error = Some(err);
    }
    if git_sync_error.is_none() && commit_sha != "HEAD" {
        let verify_result = verify_git_head(&cfg, deployment_id, &checkout, commit_sha).await;
        if let Err(err) = verify_result {
            git_sync_error = Some(err);
        }
    }
    let git_sync_duration_ms = git_sync_started.elapsed().as_millis();
    if let Some(err) = git_sync_error {
        let failure = format!("Repository sync failed: {err}");
        status_extra(
            &cfg,
            deployment_id,
            "failed",
            StatusDetails {
                failure: Some(&failure),
                runtime_metadata: Some(json!({
                    "gitSyncDurationMs": git_sync_duration_ms,
                })),
                ..StatusDetails::default()
            },
        )
        .await;
        return Err(reported_deployment_failure(failure));
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
            git_sync_duration_ms,
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
        git_sync_duration_ms,
    )
    .await?;
    status(&cfg, deployment_id, "starting", None).await;
    let container = format!("hostlet-{app_name}-{deployment_id}");
    let container_start_started = Instant::now();
    let internal_port = run_app_container(
        &cfg,
        deployment_id,
        app_id,
        &image,
        &container,
        port,
        built.hardening,
        &p,
    )
    .await?;
    let container_start_duration_ms = container_start_started.elapsed().as_millis();
    status(&cfg, deployment_id, "health_checking", None).await;
    let runtime_metadata = built.runtime_metadata;
    let health_check_started = Instant::now();
    let health_check_duration =
        match wait_health(&cfg, deployment_id, &container, internal_port, health_path).await {
            Ok(duration) => duration,
            Err(err) => {
                log(&cfg, deployment_id, "stderr", "Recent container logs:").await;
                let _ = run_log(
                    &cfg,
                    deployment_id,
                    "docker",
                    &["logs", "--tail", "80", &container],
                )
                .await;
                stop_failed_container_after_health_check(&cfg, deployment_id, &container).await;
                log(
                    &cfg,
                    deployment_id,
                    "stderr",
                    &format!("Stopped failed container after health check failure: {container}"),
                )
                .await;
                let failure = health_check_failure_message(&err);
                let runtime_metadata = add_startup_runtime_metadata(
                    runtime_metadata,
                    container_start_duration_ms,
                    health_check_started.elapsed().as_millis(),
                );
                status_extra(
                    &cfg,
                    deployment_id,
                    "failed",
                    StatusDetails {
                        failure: Some(&failure),
                        image: Some(&image),
                        container: Some(&container),
                        published_port: Some(internal_port),
                        runtime_metadata: Some(runtime_metadata),
                        ..StatusDetails::default()
                    },
                )
                .await;
                return Ok(());
            }
        };
    let runtime_metadata = add_startup_runtime_metadata(
        runtime_metadata,
        container_start_duration_ms,
        health_check_duration.as_millis(),
    );
    status(&cfg, deployment_id, "routing", None).await;
    let mut local_url = None;
    let routing_started = Instant::now();
    let routing_result = if cfg.local_mode {
        if let Some(router) = &cfg.local_router {
            apply_local_caddy_route(
                &cfg,
                deployment_id,
                router,
                &route_key,
                domain,
                internal_port,
            )
            .await
        } else {
            Ok(())
        }
    } else {
        apply_caddy_route(&cfg, deployment_id, &route_key, domain, internal_port).await
    };
    let runtime_metadata =
        add_routing_runtime_metadata(runtime_metadata, routing_started.elapsed().as_millis());
    if let Err(err) = routing_result {
        let failure = format!("Routing failed after health check: {err}. The container was left running and the previous working route was preserved when possible.");
        status_extra(
            &cfg,
            deployment_id,
            "failed",
            StatusDetails {
                failure: Some(&failure),
                image: Some(&image),
                container: Some(&container),
                published_port: Some(internal_port),
                runtime_metadata: Some(runtime_metadata.clone()),
                ..StatusDetails::default()
            },
        )
        .await;
        return Err(reported_deployment_failure(failure));
    }
    if cfg.local_mode {
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
            runtime_metadata: Some(runtime_metadata),
            ..StatusDetails::default()
        },
    )
    .await;
    Ok(())
}

async fn stop_failed_container_after_health_check(
    cfg: &Config,
    deployment_id: Uuid,
    container: &str,
) {
    if let Err(err) = run_log(cfg, deployment_id, "docker", &["stop", container]).await {
        log(
            cfg,
            deployment_id,
            "stderr",
            &format!("Failed to stop unhealthy container {container}: {err}"),
        )
        .await;
    }
}

fn health_check_failure_message(err: &anyhow::Error) -> String {
    format!("Health check failed: {err}. Runtime logs were captured, then the failed container was stopped to prevent restart loops. Check the logs above, port setting, and health path.")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContainerHardening {
    ReadOnlyRootFs,
    WritableRootFs,
}

impl ContainerHardening {
    fn for_generated_runtime(generated: bool) -> Self {
        if generated {
            Self::WritableRootFs
        } else {
            Self::ReadOnlyRootFs
        }
    }

    fn read_only_root_filesystem(self) -> bool {
        matches!(self, Self::ReadOnlyRootFs)
    }

    fn add_runtime_metadata(self, mut metadata: Value) -> Value {
        if let Some(object) = metadata.as_object_mut() {
            object.insert(
                "readOnlyRootFilesystem".into(),
                json!(self.read_only_root_filesystem()),
            );
            metadata
        } else {
            json!({
                "readOnlyRootFilesystem": self.read_only_root_filesystem(),
            })
        }
    }
}

/// Outcome of building the deployment image: the metadata reported to the API
/// plus the container hardening profile needed to run that image.
struct BuiltImage {
    runtime_metadata: Value,
    hardening: ContainerHardening,
}

/// Prepares the build plan, writes the `.dockerignore` for generated builds, and
/// builds the image via buildx (with a local cache) or a plain `docker build`.
#[allow(clippy::too_many_arguments)]
async fn build_image(
    cfg: &Config,
    deployment_id: Uuid,
    app_name: &str,
    image: &str,
    project_dir: &Path,
    port: i64,
    p: &Value,
    git_sync_duration_ms: u128,
) -> anyhow::Result<BuiltImage> {
    let build_plan_started = Instant::now();
    let build = match prepare_build(cfg, deployment_id, project_dir, port, p).await {
        Ok(build) => build,
        Err(err) if should_try_railpack(&err) => {
            let build_plan_duration_ms = build_plan_started.elapsed().as_millis();
            let build_started = Instant::now();
            let built =
                match railpack_build_app(cfg, deployment_id, app_name, image, project_dir, port, p)
                    .await
                {
                    Ok(built) => built,
                    Err(err) => {
                        let failure = format!("Generated image build failed: {err}");
                        status_extra(
                            cfg,
                            deployment_id,
                            "failed",
                            StatusDetails {
                                failure: Some(&failure),
                                image: Some(image),
                                runtime_metadata: Some(add_git_sync_runtime_metadata(
                                    add_build_plan_runtime_metadata(
                                        railpack_runtime_metadata(
                                            build_started.elapsed().as_millis(),
                                            None,
                                        ),
                                        build_plan_duration_ms,
                                    ),
                                    git_sync_duration_ms,
                                )),
                                ..StatusDetails::default()
                            },
                        )
                        .await;
                        return Err(reported_deployment_failure(failure));
                    }
                };
            return Ok(BuiltImage {
                runtime_metadata: ContainerHardening::WritableRootFs.add_runtime_metadata(
                    add_git_sync_runtime_metadata(
                        add_build_plan_runtime_metadata(
                            built.runtime_metadata,
                            build_plan_duration_ms,
                        ),
                        git_sync_duration_ms,
                    ),
                ),
                hardening: ContainerHardening::WritableRootFs,
            });
        }
        Err(err) => {
            let build_plan_duration_ms = build_plan_started.elapsed().as_millis();
            let failure = format!("Image build preparation failed: {err}");
            status_extra(
                cfg,
                deployment_id,
                "failed",
                StatusDetails {
                    failure: Some(&failure),
                    image: Some(image),
                    runtime_metadata: Some(build_prepare_failure_runtime_metadata(
                        build_plan_duration_ms,
                        git_sync_duration_ms,
                    )),
                    ..StatusDetails::default()
                },
            )
            .await;
            return Err(reported_deployment_failure(failure));
        }
    };
    let build_plan_duration_ms = build_plan_started.elapsed().as_millis();
    if build.generated {
        tokio::fs::write(project_dir.join(".dockerignore"), generated_dockerignore()).await?;
    }
    let build_started = Instant::now();
    let build_result: anyhow::Result<()> = async {
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
        Ok(())
    }
    .await;
    if let Err(err) = build_result {
        let failure = format!("Image build failed: {err}");
        status_extra(
            cfg,
            deployment_id,
            "failed",
            StatusDetails {
                failure: Some(&failure),
                image: Some(image),
                runtime_metadata: Some(add_git_sync_runtime_metadata(
                    add_build_plan_runtime_metadata(
                        build_runtime_metadata(&build, build_started.elapsed().as_millis(), None),
                        build_plan_duration_ms,
                    ),
                    git_sync_duration_ms,
                )),
                ..StatusDetails::default()
            },
        )
        .await;
        return Err(reported_deployment_failure(failure));
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
    let hardening = ContainerHardening::for_generated_runtime(build.generated);
    Ok(BuiltImage {
        runtime_metadata: hardening.add_runtime_metadata(add_git_sync_runtime_metadata(
            add_build_plan_runtime_metadata(
                build_runtime_metadata(&build, build_duration_ms, image_size),
                build_plan_duration_ms,
            ),
            git_sync_duration_ms,
        )),
        hardening,
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
    hardening: ContainerHardening,
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
    apply_container_hardening_args(&mut args, hardening);
    let env_pairs = runtime_env_args(p, port);
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

fn apply_container_hardening_args(args: &mut Vec<&str>, hardening: ContainerHardening) {
    if hardening.read_only_root_filesystem() {
        args.push("--read-only");
        args.push("--tmpfs");
        args.push("/tmp");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_hardening_adds_rootfs_and_tmpfs_flags() {
        let mut args = Vec::new();

        apply_container_hardening_args(&mut args, ContainerHardening::ReadOnlyRootFs);

        assert_eq!(args, vec!["--read-only", "--tmpfs", "/tmp"]);
    }

    #[test]
    fn writable_hardening_keeps_generated_runtime_rootfs_mutable() {
        let mut args = Vec::new();

        apply_container_hardening_args(&mut args, ContainerHardening::WritableRootFs);

        assert!(args.is_empty());
    }

    #[test]
    fn hardening_metadata_records_read_only_rootfs_decision() {
        let read_only = ContainerHardening::ReadOnlyRootFs.add_runtime_metadata(json!({}));
        let writable = ContainerHardening::WritableRootFs.add_runtime_metadata(json!({
            "buildBackend": "railpack",
        }));

        assert_eq!(read_only["readOnlyRootFilesystem"], true);
        assert_eq!(writable["buildBackend"], "railpack");
        assert_eq!(writable["readOnlyRootFilesystem"], false);
    }

    #[test]
    fn health_check_failure_message_says_container_is_stopped() {
        let err = anyhow::anyhow!("no successful response from http://127.0.0.1:3000/health");

        let message = health_check_failure_message(&err);

        assert!(message.contains("Health check failed"));
        assert!(message.contains("failed container was stopped"));
        assert!(!message.contains("left running"));
    }
}
