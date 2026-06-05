use super::*;

const RAILPACK_BUILDKIT_CONTAINER: &str = "hostlet-railpack-buildkit";
const DEFAULT_RAILPACK_BUILDKIT_IMAGE: &str = "moby/buildkit:buildx-stable-1";

pub(crate) struct RailpackBuildResult {
    pub(crate) runtime_metadata: Value,
}

pub(crate) fn should_try_railpack(err: &anyhow::Error) -> bool {
    err.to_string() == NO_NATIVE_BUILD_PLAN
}

pub(crate) async fn railpack_build_app(
    cfg: &Config,
    deployment_id: Uuid,
    app_name: &str,
    image: &str,
    project_dir: &Path,
    port: i64,
    p: &Value,
) -> anyhow::Result<RailpackBuildResult> {
    let railpack = std::env::var("HOSTLET_RAILPACK_BIN").unwrap_or_else(|_| "railpack".into());
    let buildkit_host = ensure_railpack_buildkit(cfg, deployment_id).await?;
    log(
        cfg,
        deployment_id,
        "stdout",
        "Building generated runtime with Railpack.",
    )
    .await;
    let build_started = Instant::now();
    let args = railpack_build_args(image, app_name, port, project_dir, p)?;
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    run_log_in_dir_env(
        cfg,
        deployment_id,
        project_dir,
        &[("BUILDKIT_HOST", buildkit_host.as_str())],
        &railpack,
        &arg_refs,
    )
    .await
    .with_context(|| format!("Railpack failed to build {}", project_dir.display()))?;
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
    Ok(RailpackBuildResult {
        runtime_metadata: json!({
            "packagingStrategy": "generated",
            "generatedDockerfile": false,
            "buildBackend": "railpack",
            "detectedLanguage": null,
            "detectedFramework": null,
            "runtimeKind": null,
            "packageManager": null,
            "buildDurationMs": build_duration_ms,
            "imageSizeBytes": image_size,
        }),
    })
}

fn railpack_build_args(
    image: &str,
    cache_key: &str,
    port: i64,
    project_dir: &Path,
    p: &Value,
) -> anyhow::Result<Vec<String>> {
    let mut args = vec![
        "build".to_string(),
        "--name".to_string(),
        image.to_string(),
        "--progress".to_string(),
        "plain".to_string(),
        "--cache-key".to_string(),
        cache_key.to_string(),
        "--env".to_string(),
        format!("PORT={port}"),
        "--error-missing-start".to_string(),
    ];
    if let Some(command) = payload_command(p, "build_command") {
        validate_dockerfile_command(&command)?;
        args.push("--build-cmd".to_string());
        args.push(command);
    }
    if let Some(command) = payload_command(p, "start_command") {
        validate_dockerfile_command(&command)?;
        args.push("--start-cmd".to_string());
        args.push(command);
    }
    args.push(
        project_dir
            .to_str()
            .with_context(|| format!("path is not valid UTF-8: {}", project_dir.display()))?
            .to_string(),
    );
    Ok(args)
}

async fn ensure_railpack_buildkit(cfg: &Config, deployment_id: Uuid) -> anyhow::Result<String> {
    if let Ok(host) = std::env::var("BUILDKIT_HOST") {
        if !host.trim().is_empty() {
            return Ok(host);
        }
    }
    let container = railpack_buildkit_container();
    let output = command_output(
        "docker",
        &["inspect", "-f", "{{.State.Running}}", container.as_str()],
        Duration::from_secs(30),
    )
    .await;
    match output {
        Ok(output) if output.status.success() => {
            if String::from_utf8_lossy(&output.stdout).trim() != "true" {
                run_log(cfg, deployment_id, "docker", &["start", container.as_str()]).await?;
            }
        }
        _ => {
            let image = std::env::var("HOSTLET_RAILPACK_BUILDKIT_IMAGE")
                .unwrap_or_else(|_| DEFAULT_RAILPACK_BUILDKIT_IMAGE.into());
            run_log(
                cfg,
                deployment_id,
                "docker",
                &[
                    "run",
                    "-d",
                    "--name",
                    container.as_str(),
                    "--privileged",
                    &image,
                ],
            )
            .await?;
        }
    }
    Ok(format!("docker-container://{container}"))
}

fn railpack_buildkit_container() -> String {
    std::env::var("HOSTLET_RAILPACK_BUILDKIT_CONTAINER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| RAILPACK_BUILDKIT_CONTAINER.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_packaging_miss_falls_through_to_railpack() {
        assert!(should_try_railpack(&anyhow::anyhow!(NO_NATIVE_BUILD_PLAN)));
        assert!(!should_try_railpack(&anyhow::anyhow!(
            "packaging strategy dockerfile requires a Dockerfile at the app root"
        )));
    }

    #[test]
    fn railpack_args_include_cache_port_and_overrides() {
        let args = railpack_build_args(
            "hostlet/test:latest",
            "demo-app",
            4173,
            Path::new("/tmp/demo-app"),
            &json!({
                "build_command": "python -m compileall .",
                "start_command": "python main.py"
            }),
        )
        .unwrap();
        assert_eq!(args[0], "build");
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--name", "hostlet/test:latest"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--cache-key", "demo-app"]));
        assert!(args.windows(2).any(|pair| pair == ["--env", "PORT=4173"]));
        assert!(args.contains(&"--error-missing-start".to_string()));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--build-cmd", "python -m compileall ."]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--start-cmd", "python main.py"]));
        assert_eq!(args.last().unwrap(), "/tmp/demo-app");
    }

    #[test]
    fn railpack_buildkit_container_can_be_overridden_for_ci() {
        std::env::set_var(
            "HOSTLET_RAILPACK_BUILDKIT_CONTAINER",
            "hostlet-buildkit-ci-123",
        );
        assert_eq!(railpack_buildkit_container(), "hostlet-buildkit-ci-123");
        std::env::remove_var("HOSTLET_RAILPACK_BUILDKIT_CONTAINER");
    }
}
