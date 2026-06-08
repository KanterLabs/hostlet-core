use super::*;

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
    validate_compose_subset(&compose_text, web_service)?;
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
        compose_override_yaml(web_service, port, app_id, deployment_id, &p),
    )
    .await?;
    log(
        &cfg,
        deployment_id,
        "stdout",
        &format!("Detected Hostlet Compose app. Project {project}, web service {web_service}."),
    )
    .await;
    let compose_up_started = Instant::now();
    run_log_in_dir(
        &cfg,
        deployment_id,
        project_dir,
        "docker",
        &compose_invocation(&project, compose_file, &override_file, &["config"])?,
    )
    .await?;
    run_log_in_dir(
        &cfg,
        deployment_id,
        project_dir,
        "docker",
        &compose_invocation(
            &project,
            compose_file,
            &override_file,
            &["up", "-d", "--build", "--remove-orphans"],
        )?,
    )
    .await?;
    status(&cfg, deployment_id, "starting", None).await;
    let container = compose_service_container(
        project_dir,
        &project,
        compose_file,
        &override_file,
        web_service,
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
        "appName": app_name,
        "composeUpDurationMs": container_start_duration_ms,
        "gitSyncDurationMs": git_sync_duration_ms,
    });
    let health_check_started = Instant::now();
    let health_check_duration = match wait_health(
        &cfg,
        deployment_id,
        &container,
        internal_port,
        health_path,
    )
    .await
    {
        Ok(duration) => duration,
        Err(err) => {
            let _ = run_log_in_dir(
                &cfg,
                deployment_id,
                project_dir,
                "docker",
                &compose_invocation(
                    &project,
                    compose_file,
                    &override_file,
                    &["logs", "--tail", "120"],
                )?,
            )
            .await;
            let failure = format!("Compose health check failed: {err}. The previous working route was preserved; inspect Compose service logs for details.");
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
    status(&cfg, deployment_id, "routing", None).await;
    let mut local_url = None;
    let routing_started = Instant::now();
    let routing_result = if cfg.local_mode {
        if let Some(router) = &cfg.local_router {
            apply_local_caddy_route(
                &cfg,
                deployment_id,
                router,
                route_key,
                domain,
                internal_port,
            )
            .await
        } else {
            Ok(())
        }
    } else {
        apply_caddy_route(&cfg, deployment_id, route_key, domain, internal_port).await
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
    status_extra(
        &cfg,
        deployment_id,
        "success",
        StatusDetails {
            container: Some(&container),
            local_url: local_url.as_deref(),
            published_port: Some(internal_port),
            compose_project: Some(&project),
            runtime_metadata: Some(runtime_metadata),
            ..StatusDetails::default()
        },
    )
    .await;
    Ok(())
}

pub(crate) async fn rollback(cfg: Config, p: Value) -> anyhow::Result<()> {
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

pub(crate) async fn delete_app(cfg: Config, p: Value) -> anyhow::Result<()> {
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

pub(crate) async fn docker_cleanup_job(p: &Value) -> anyhow::Result<()> {
    let dry_run = p.get("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);
    let keep_containers = string_set_from_array(p.get("keep_containers"));
    let keep_images = string_set_from_array(p.get("keep_images"));

    let containers = hostlet_containers_all().await?;
    for container in containers {
        let compose_managed = docker_compose_managed_container(&container).await?;
        if cleanup_should_remove_container(&container, &keep_containers, compose_managed)?
            && !dry_run
        {
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

fn cleanup_should_remove_container(
    container: &str,
    keep_containers: &HashSet<String>,
    compose_managed: bool,
) -> anyhow::Result<bool> {
    if keep_containers.contains(container) || compose_managed {
        return Ok(false);
    }
    if !valid_container_name(container) {
        bail!("refusing to clean invalid managed container name");
    }
    Ok(true)
}

pub(crate) async fn run_log(
    cfg: &Config,
    deployment_id: Uuid,
    bin: &str,
    args: &[&str],
) -> anyhow::Result<()> {
    run_log_streamed(cfg, deployment_id, None, &[], bin, args).await
}

pub(crate) async fn run_log_in_dir(
    cfg: &Config,
    deployment_id: Uuid,
    dir: &Path,
    bin: &str,
    args: &[&str],
) -> anyhow::Result<()> {
    run_log_streamed(cfg, deployment_id, Some(dir), &[], bin, args).await
}

pub(crate) async fn run_log_in_dir_env(
    cfg: &Config,
    deployment_id: Uuid,
    dir: &Path,
    envs: &[(&str, &str)],
    bin: &str,
    args: &[&str],
) -> anyhow::Result<()> {
    run_log_streamed(cfg, deployment_id, Some(dir), envs, bin, args).await
}

/// Spawns `bin args`, streaming stdout/stderr back as deployment logs. When
/// `dir` is `Some`, the command runs with that working directory.
async fn run_log_streamed(
    cfg: &Config,
    deployment_id: Uuid,
    dir: Option<&Path>,
    envs: &[(&str, &str)],
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
    if let Some(dir) = dir {
        cmd.current_dir(dir);
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

pub(crate) async fn ensure_checkout_remote(
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

pub(crate) async fn verify_git_head(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_start_failure_log_line_includes_command_and_error() {
        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "No such file or directory");

        let line = command_start_failure_log_line("railpack", &err);

        assert!(line.contains("Failed to start railpack:"));
        assert!(line.contains("No such file or directory"));
    }

    #[test]
    fn command_start_failure_log_line_redacts_sensitive_error_text() {
        let err = std::io::Error::other("token=abc123");

        let line = command_start_failure_log_line("railpack", &err);

        assert_eq!(line, "[redacted]");
    }

    #[test]
    fn docker_cleanup_removes_unkept_hostlet_app_containers() {
        let keep_containers = HashSet::from(["hostlet-app-current".to_string()]);

        let should_remove = cleanup_should_remove_container(
            "hostlet-app-stale-restarting",
            &keep_containers,
            false,
        )
        .unwrap();

        assert!(should_remove);
    }

    #[test]
    fn docker_cleanup_keeps_protected_and_compose_containers() {
        let keep_containers = HashSet::from(["hostlet-app-current".to_string()]);

        assert!(
            !cleanup_should_remove_container("hostlet-app-current", &keep_containers, false)
                .unwrap()
        );
        assert!(
            !cleanup_should_remove_container("hostlet-app-compose", &keep_containers, true)
                .unwrap()
        );
    }

    #[test]
    fn docker_cleanup_rejects_invalid_unkept_container_names() {
        let err =
            cleanup_should_remove_container("not-hostlet-app", &HashSet::new(), false).unwrap_err();

        assert!(err
            .to_string()
            .contains("refusing to clean invalid managed container name"));
    }
}
