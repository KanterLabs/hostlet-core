use super::*;
mod cleanup;
pub(crate) use cleanup::*;

#[derive(Debug, Deserialize)]
pub(crate) struct HostletManifest {
    runtime: String,
    compose: HostletComposeManifest,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HostletComposeManifest {
    file: Option<String>,
    web_service: String,
    port: Option<u16>,
    health_path: Option<String>,
}

/// Resolved Hostlet Compose manifest plus the on-disk compose file it points at.
struct ResolvedCompose<'a> {
    /// Path reported back to the API as `hostletConfigPath` (or `"generated"`).
    manifest_path: &'a str,
    manifest: HostletManifest,
    compose_file_name: String,
    compose_file: PathBuf,
}

/// Builds the shared `docker compose -p <project> -f <compose> -f <override>`
/// argument prefix, returning context-bearing errors for non-UTF-8 paths.
fn compose_invocation<'a>(
    project: &'a str,
    compose_file: &'a Path,
    override_file: &'a Path,
    trailing: &[&'a str],
) -> anyhow::Result<Vec<&'a str>> {
    let mut args = vec![
        "compose",
        "-p",
        project,
        "-f",
        path_str(compose_file)?,
        "-f",
        path_str(override_file)?,
    ];
    args.extend_from_slice(trailing);
    Ok(args)
}

/// Converts a path to `&str`, surfacing a clear error rather than panicking on
/// non-UTF-8 paths.
fn path_str(path: &Path) -> anyhow::Result<&str> {
    path.to_str()
        .with_context(|| format!("path is not valid UTF-8: {}", path.display()))
}

/// Reads the deploy payload (generated runtime or repo `hostlet.yml`) into a
/// validated [`ResolvedCompose`].
async fn resolve_compose_manifest<'a>(
    p: &'a Value,
    project_dir: &Path,
    build_dir: &Path,
) -> anyhow::Result<ResolvedCompose<'a>> {
    if let Some(generated) = p
        .pointer("/runtime_config/generatedCompose")
        .and_then(|v| v.as_object())
    {
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
        tokio::fs::create_dir_all(build_dir).await?;
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
        return Ok(ResolvedCompose {
            manifest_path: "generated",
            manifest,
            compose_file_name: compose_file_name.to_string(),
            compose_file,
        });
    }

    let manifest_path = p
        .get("hostlet_config_path")
        .and_then(|v| v.as_str())
        .unwrap_or("hostlet.yml");
    validate_relative_file_path(manifest_path)?;
    let manifest_file = project_dir.join(manifest_path);
    let manifest_text = tokio::fs::read_to_string(&manifest_file)
        .await
        .with_context(|| format!("compose runtime requires {manifest_path}"))?;
    let manifest: HostletManifest =
        serde_yaml::from_str(&manifest_text).context("hostlet manifest is not valid YAML")?;
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
    Ok(ResolvedCompose {
        manifest_path,
        manifest,
        compose_file_name,
        compose_file,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn deploy_compose(
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
    git_sync_duration_ms: u128,
    web_image: Option<&str>,
) -> anyhow::Result<()> {
    ensure_docker_compose().await?;
    let build_dir = cfg.workdir.join("builds").join(deployment_id.to_string());
    let resolved = resolve_compose_manifest(&p, project_dir, &build_dir).await?;
    let ResolvedCompose {
        manifest_path,
        manifest,
        compose_file_name,
        compose_file,
    } = &resolved;
    let web_service = &manifest.compose.web_service;

    let compose_text = tokio::fs::read_to_string(compose_file).await?;
    // Auto-map relative host bind mounts (e.g. ./data:/app/data) onto managed
    // named volumes so a repo that persists to a project-relative directory
    // deploys unchanged while the host filesystem stays isolated. Absolute binds
    // and the Docker socket are left intact for validate_compose_subset to
    // reject. The rewritten file is what `docker compose` actually reads.
    let compose_text = remap_host_binds_to_named_volumes(&compose_text)?;
    tokio::fs::write(compose_file, &compose_text).await?;
    validate_compose_subset(&compose_text, web_service)?;
    let backing_spec_hash = compose_backing_spec_hash(&cfg, &compose_text, web_service)?;
    let expected_backing_spec_hash = p.get("expected_backing_spec_hash").and_then(Value::as_str);
    let approved_backing_spec_hash = p.get("approved_backing_spec_hash").and_then(Value::as_str);
    if expected_backing_spec_hash.is_some_and(|expected| expected != backing_spec_hash)
        && approved_backing_spec_hash != Some(backing_spec_hash.as_str())
    {
        let failure = "Compose backing-service configuration changed. Review and approve the maintenance update before deploying.";
        status_extra(
            &cfg,
            deployment_id,
            "failed",
            StatusDetails {
                failure: Some(failure),
                failure_code: Some("compose_backing_change_requires_approval"),
                runtime_metadata: Some(json!({
                    "runtime": "compose",
                    "backingSpecHash": backing_spec_hash,
                })),
                ..StatusDetails::default()
            },
        )
        .await;
        return Err(reported_deployment_failure(failure.to_string()));
    }
    let port = manifest.compose.port.unwrap_or(fallback_port as u16);
    validate_port(port as i64)?;
    let health_path = manifest
        .compose
        .health_path
        .as_deref()
        .unwrap_or(fallback_health_path);
    validate_health_path(health_path)?;
    let stable_project = compose_project_name(app_id);
    let project = compose_release_project_name(deployment_id);
    let override_file = build_dir.join("compose.hostlet.yml");
    let release_override_file = build_dir.join("compose.release.hostlet.yml");
    if let Some(parent) = override_file.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let base_override =
        compose_override_yaml(&compose_text, web_service, port, app_id, deployment_id, &p);
    tokio::fs::write(&override_file, &base_override).await?;
    log(
        &cfg,
        deployment_id,
        "stdout",
        &format!(
            "Detected Hostlet Compose app. Stable backing project {stable_project}, release {project}, web service {web_service}."
        ),
    )
    .await;
    // The compose process environment supplies `${VAR}` interpolation: the
    // app's env (so a generated add-on stack resolves e.g. ${POSTGRES_PASSWORD}
    // from the encrypted store) plus HOSTLET_WEB_IMAGE for managed-add-ons apps
    // whose web service was built from the repo. Secrets travel as the child
    // process env, never as command args, so they are not logged — and `config`
    // runs `--quiet` so the resolved (interpolated) compose is never printed.
    let mut compose_env = compose_interpolation_env(&p);
    if let Some(web_image) = web_image {
        compose_env.push((
            hostlet_contracts::compose::WEB_IMAGE_ENV.to_string(),
            web_image.to_string(),
        ));
    }
    let compose_env_refs: Vec<(&str, &str)> = compose_env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let compose_up_started = Instant::now();
    run_log_in_dir_env(
        &cfg,
        deployment_id,
        project_dir,
        &compose_env_refs,
        "docker",
        &compose_invocation(
            &stable_project,
            compose_file,
            &override_file,
            &["config", "--quiet"],
        )?,
    )
    .await?;
    let backing_services =
        hostlet_contracts::compose::parse_compose_services(&compose_text, web_service)
            .into_iter()
            .filter(|service| service.role == "backing")
            .map(|service| service.name)
            .collect::<Vec<_>>();
    if !backing_services.is_empty() {
        let mut trailing = vec!["up", "-d", "--build"];
        trailing.extend(backing_services.iter().map(String::as_str));
        run_log_in_dir_env(
            &cfg,
            deployment_id,
            project_dir,
            &compose_env_refs,
            "docker",
            &compose_invocation(&stable_project, compose_file, &override_file, &trailing)?,
        )
        .await?;
    } else {
        ensure_compose_network(&format!("{stable_project}_default"), &stable_project).await?;
    }
    let release_override =
        compose_release_override_yaml(&base_override, &compose_text, web_service, &stable_project)?;
    tokio::fs::write(&release_override_file, release_override).await?;
    for volume in compose_named_volume_names(&compose_text, &stable_project)? {
        let logical_name = volume
            .strip_prefix(&format!("{stable_project}_"))
            .context("stable Compose volume has an invalid name")?;
        let project_label = format!("com.docker.compose.project={stable_project}");
        let volume_label = format!("com.docker.compose.volume={logical_name}");
        run_log(
            &cfg,
            deployment_id,
            "docker",
            &[
                "volume",
                "create",
                "--label",
                &project_label,
                "--label",
                &volume_label,
                &volume,
            ],
        )
        .await?;
    }
    run_log_in_dir_env(
        &cfg,
        deployment_id,
        project_dir,
        &compose_env_refs,
        "docker",
        &compose_invocation(
            &project,
            compose_file,
            &release_override_file,
            &["up", "-d", "--build", "--no-deps", web_service],
        )?,
    )
    .await?;
    status(&cfg, deployment_id, "starting", None).await;
    let container = compose_service_container(
        project_dir,
        &project,
        compose_file,
        &release_override_file,
        web_service,
        &compose_env_refs,
    )
    .await?;
    let internal_port = docker_published_port(&container, port).await?;
    let container_start_duration_ms = compose_up_started.elapsed().as_millis();
    status(&cfg, deployment_id, "health_checking", None).await;
    let runtime_metadata = json!({
        "runtime": "compose",
        "composeFile": compose_file_name,
        "hostletConfigPath": manifest_path,
        "webService": web_service,
        "targetPort": port,
        "healthPath": health_path,
        "project": project,
        "stableProject": stable_project,
        "backingSpecHash": backing_spec_hash,
        "appName": app_name,
        "composeUpDurationMs": container_start_duration_ms,
        "gitSyncDurationMs": git_sync_duration_ms,
    });
    let health_check_started = Instant::now();
    let health_check_duration =
        match wait_health(&cfg, deployment_id, &container, internal_port, health_path).await {
            Ok(duration) => duration,
            Err(err) => {
                let _ = run_log_in_dir_env(
                    &cfg,
                    deployment_id,
                    project_dir,
                    &compose_env_refs,
                    "docker",
                    &compose_invocation(
                        &project,
                        compose_file,
                        &override_file,
                        &["logs", "--tail", "120"],
                    )?,
                )
                .await;
                let cleanup_failed = match remove_compose_project_resources(&project).await {
                    Ok(()) => None,
                    Err(cleanup_err) => {
                        let cleanup_log = format!(
                            "Failed to remove unhealthy Compose project {project}: {cleanup_err}"
                        );
                        log(&cfg, deployment_id, "stderr", &cleanup_log).await;
                        Some(cleanup_err)
                    }
                };
                let failure = compose_health_failure_message(&err, cleanup_failed.as_ref());
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
                        container: Some(&container),
                        published_port: Some(internal_port),
                        compose_project: Some(&project),
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
    // Capture service facts before activation so the API can durably prepare
    // the complete candidate in the same transaction as the pending pointer.
    let mut services = compose_all_services(
        project_dir,
        &project,
        compose_file,
        &release_override_file,
        &compose_text,
        web_service,
        &compose_env_refs,
    )
    .await;
    if let Some(web) = services.iter_mut().find(|svc| svc.name == *web_service) {
        web.target_port = Some(port as i32);
        web.published_port = Some(internal_port as i32);
        web.health_status = Some("healthy".to_string());
    }
    let route_generation = prepare_candidate_activation(
        &cfg,
        &p,
        deployment_id,
        web_image,
        &container,
        internal_port,
        Some(&project),
        runtime_metadata.clone(),
        services,
    )
    .await?;
    let mut local_url = None;
    let routing_started = Instant::now();
    let routing_result = if cfg.local_mode {
        if let Some(router) = &cfg.local_router {
            apply_local_caddy_route_versioned(
                &cfg,
                deployment_id,
                router,
                route_key,
                domain,
                internal_port,
                route_generation,
            )
            .await
        } else {
            Ok(())
        }
    } else {
        apply_caddy_route_versioned(
            &cfg,
            deployment_id,
            route_key,
            domain,
            internal_port,
            route_generation,
        )
        .await
    };
    let runtime_metadata =
        add_routing_runtime_metadata(runtime_metadata, routing_started.elapsed().as_millis());
    if let Err(err) = routing_result {
        let failure = format!("Compose routing failed after health check: {err}. The Compose project was left running and the previous working route was preserved when possible.");
        status_extra(
            &cfg,
            deployment_id,
            "failed",
            StatusDetails {
                failure: Some(&failure),
                container: Some(&container),
                published_port: Some(internal_port),
                compose_project: Some(&project),
                runtime_metadata: Some(runtime_metadata.clone()),
                ..StatusDetails::default()
            },
        )
        .await;
        return Err(reported_deployment_failure(failure));
    }
    if cfg.local_mode {
        local_url = Some(if cfg.local_router.is_some() {
            domain.to_string()
        } else {
            format!("localhost:{internal_port}")
        });
    }
    commit_candidate_activation(
        &cfg,
        &p,
        deployment_id,
        route_generation,
        local_url.as_deref(),
        Some(&runtime_metadata),
        false,
    )
    .await?;
    Ok(())
}

fn compose_backing_spec_hash(
    cfg: &Config,
    compose_text: &str,
    web_service: &str,
) -> anyhow::Result<String> {
    let mut value: serde_yaml::Value = serde_yaml::from_str(compose_text)?;
    if let Some(services) = value
        .get_mut("services")
        .and_then(serde_yaml::Value::as_mapping_mut)
    {
        services.remove(serde_yaml::Value::String(web_service.to_string()));
    }
    let canonical = serde_yaml::to_string(&value)?;
    Ok(hostlet_contracts::crypto::sign(
        &cfg.job_signing_secret,
        canonical.as_bytes(),
    ))
}

fn compose_interpolation_env(p: &Value) -> Vec<(String, String)> {
    let mut compose_env = Vec::new();
    if let Some(map) = p.get("env").and_then(|v| v.as_object()) {
        for (key, value) in map {
            if hostlet_contracts::valid_host_process_env_key(key) {
                compose_env.push((key.clone(), value.as_str().unwrap_or_default().to_string()));
            }
        }
    }
    compose_env
}

pub(crate) async fn rollback(cfg: Config, p: Value) -> anyhow::Result<()> {
    if p.pointer("/target_runtime_metadata/inferenceReceipt/schemaVersion")
        .and_then(Value::as_u64)
        == Some(hostlet_contracts::GENERATED_TOPOLOGY_SCHEMA_VERSION as u64)
    {
        return crate::runtime::rollback_generated_topology(&cfg, &p).await;
    }
    let deployment_id = Uuid::parse_str(p["deployment_id"].as_str().context("deployment_id")?)?;
    let container = p["target_container"].as_str().context("target_container")?;
    let domain = p["domain"].as_str().context("domain")?;
    let route_key = p
        .get("route_key")
        .and_then(|v| v.as_str())
        .map(app_slug)
        .unwrap_or_else(|| app_slug(p["app_id"].as_str().unwrap_or("app")));
    let port_value = p["published_port"]
        .as_i64()
        .context("target deployment is missing a published port; redeploy before rolling back")?;
    validate_port(port_value)?;
    let port = port_value as u16;
    validate_domain(domain)?;
    run_quiet("docker", &["start", container]).await?;
    wait_health(
        &cfg,
        deployment_id,
        container,
        port,
        p.get("health_path").and_then(Value::as_str).unwrap_or("/"),
    )
    .await?;
    let mut runtime_metadata = p
        .get("target_runtime_metadata")
        .cloned()
        .unwrap_or_else(|| json!({"rollback": true}));
    if let (Some(metadata), Some(target)) = (
        runtime_metadata.as_object_mut(),
        p.get("target_deployment_id").and_then(Value::as_str),
    ) {
        metadata.insert("rollbackTargetDeploymentId".into(), json!(target));
    }
    let route_generation = prepare_candidate_activation(
        &cfg,
        &p,
        deployment_id,
        p.get("target_image").and_then(Value::as_str),
        container,
        port,
        p.get("target_compose_project").and_then(Value::as_str),
        runtime_metadata,
        Vec::new(),
    )
    .await?;
    if cfg.local_mode {
        if let Some(router) = &cfg.local_router {
            apply_local_caddy_route_versioned(
                &cfg,
                deployment_id,
                router,
                &route_key,
                domain,
                port,
                route_generation,
            )
            .await?;
        }
        let local_url = cfg.local_router.as_ref().map(|_| domain);
        commit_candidate_activation(
            &cfg,
            &p,
            deployment_id,
            route_generation,
            local_url,
            None,
            true,
        )
        .await?;
        return Ok(());
    }
    match apply_caddy_route_versioned(
        &cfg,
        deployment_id,
        &route_key,
        domain,
        port,
        route_generation,
    )
    .await
    {
        Ok(_) => {
            commit_candidate_activation(&cfg, &p, deployment_id, route_generation, None, None, true)
                .await?
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

pub(crate) async fn run_log(
    cfg: &Config,
    deployment_id: Uuid,
    bin: &str,
    args: &[&str],
) -> anyhow::Result<()> {
    run_log_streamed(cfg, deployment_id, None, &[], bin, args, false).await
}

pub(crate) async fn run_log_in_dir_env(
    cfg: &Config,
    deployment_id: Uuid,
    dir: &Path,
    envs: &[(&str, &str)],
    bin: &str,
    args: &[&str],
) -> anyhow::Result<()> {
    run_log_streamed(cfg, deployment_id, Some(dir), envs, bin, args, true).await
}

/// Spawns `bin args`, streaming stdout/stderr back as deployment logs. When
/// `dir` is `Some`, the command runs with that working directory. When
/// `isolate_env` is set the child starts from a wiped environment (see
/// `harden_host_command_env`); this is reserved for commands that re-parse the
/// repo-controlled compose file and interpolate `${VAR}`. Management commands
/// (git, `docker stop`, …) never interpolate repo content, so they keep the
/// agent's functional environment (git transport config, proxy vars) and only
/// receive the baseline linker/PATH hardening.
async fn run_log_streamed(
    cfg: &Config,
    deployment_id: Uuid,
    dir: Option<&Path>,
    envs: &[(&str, &str)],
    bin: &str,
    args: &[&str],
    isolate_env: bool,
) -> anyhow::Result<()> {
    log(
        cfg,
        deployment_id,
        "stdout",
        &format!("$ {} {}", bin, command_args_for_log(args).join(" ")),
    )
    .await;
    let mut cmd = Command::new(bin);
    cmd.kill_on_drop(true);
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    // For compose-file-parsing commands, clear the inherited agent environment
    // (see `harden_host_command_env`) BEFORE layering on the curated per-deploy
    // interpolation env, so a tenant compose file's `${HOSTLET_AGENT_TOKEN}` can
    // never resolve an inherited host secret while the intended `${VAR}` values
    // still apply. Management commands keep the agent's functional environment.
    if isolate_env {
        harden_host_command_env(&mut cmd);
    } else {
        sanitize_management_command_env(&mut cmd);
    }
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            log(
                cfg,
                deployment_id,
                "stderr",
                &command_start_failure_log_line(bin, &err),
            )
            .await;
            return Err(err).with_context(|| format!("failed to start {bin}"));
        }
    };
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let c1 = cfg.clone();
    let c2 = cfg.clone();
    let stdout_task = tokio::spawn(async move {
        stream_lines(c1, deployment_id, "stdout", stdout).await;
    });
    let stderr_task = tokio::spawn(async move {
        stream_lines(c2, deployment_id, "stderr", stderr).await;
    });
    let status = match tokio::time::timeout(Duration::from_secs(30 * 60), child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            bail!("{bin} timed out after 1800 seconds");
        }
    };
    let _ = stdout_task.await;
    let _ = stderr_task.await;
    if !status.success() {
        bail!("{bin} exited with {status}");
    }
    Ok(())
}

/// Hardens a host Docker/Compose command so a repo-controlled compose file can
/// never interpolate the agent's own environment. Docker Compose resolves
/// `${VAR}` in service `environment:` blocks from the *spawned command's*
/// environment, so a tenant that writes `environment: {X: '${HOSTLET_AGENT_TOKEN}'}`
/// would otherwise bake the agent's host-wide secrets (HOSTLET_*, AWS_*,
/// DATABASE_URL, GITHUB_*, …) into their container — on cloud that is the one
/// shared agent token / job-signing secret for every co-located tenant.
///
/// `env_clear` is the primary defense: the child starts from an empty
/// environment and only a tiny, secret-free allowlist is re-added — a fixed
/// safe `PATH` plus the Docker client transport variables (and only when the
/// host actually sets them; rootless Docker needs `XDG_RUNTIME_DIR` to find the
/// user daemon socket). Callers layer the curated per-deploy interpolation env
/// on top *after* calling this.
pub(crate) fn harden_host_command_env(cmd: &mut Command) {
    cmd.env_clear();
    cmd.env(
        "PATH",
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    );
    for key in [
        "DOCKER_HOST",
        "DOCKER_CONTEXT",
        "DOCKER_CONFIG",
        "XDG_RUNTIME_DIR",
    ] {
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }
}

/// Baseline hardening for host management commands (git checkout/submodule,
/// `docker stop`, `docker volume create`, …) that never re-parse a
/// repo-controlled compose file and so carry no `${VAR}` interpolation vector.
/// Unlike `harden_host_command_env`, the environment is NOT cleared: these
/// commands still need the agent's functional environment (git transport config
/// such as `protocol.file.allow`/`insteadOf`, proxy settings, `HOME`). Only
/// dynamic-linker overrides are dropped and `PATH` is pinned to a known-good set
/// so an injected `LD_PRELOAD`/`PATH` cannot redirect the spawned binary.
fn sanitize_management_command_env(cmd: &mut Command) {
    for (key, _) in std::env::vars() {
        if key.starts_with("LD_") || key.starts_with("DYLD_") {
            cmd.env_remove(key);
        }
    }
    cmd.env(
        "PATH",
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    );
}

fn compose_health_failure_message(
    health_err: &anyhow::Error,
    cleanup_failed: Option<&anyhow::Error>,
) -> String {
    match cleanup_failed {
        Some(cleanup_err) => format!(
            "Compose health check failed: {health_err}. Failed to remove the unhealthy Compose project: {cleanup_err}. The previous working route was preserved; inspect Compose service logs for details."
        ),
        None => format!(
            "Compose health check failed: {health_err}. Removed the unhealthy Compose project and preserved the previous working route."
        ),
    }
}

fn command_start_failure_log_line(bin: &str, err: &std::io::Error) -> String {
    redact(&format!("Failed to start {bin}: {err}"))
}

pub(crate) async fn run_quiet(bin: &str, args: &[&str]) -> anyhow::Result<()> {
    let output = command_output(bin, args, Duration::from_secs(120)).await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{bin} exited with {}: {}", output.status, stderr.trim());
    }
    Ok(())
}

pub(crate) async fn run_quiet_absent_ok(
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

pub(crate) async fn run_capture_trim(
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

#[cfg(test)]
#[path = "compose/tests.rs"]
mod tests;
