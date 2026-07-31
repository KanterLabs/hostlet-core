use super::*;
use base64::Engine;
use hostlet_contracts::{
    ArtifactImageSource, ArtifactServiceImage, BuildArtifactManifestV2, ContainerFilesystemMode,
    GeneratedTopologyConfig, HealthProbe, HealthProbeKind, OciArtifactDescriptor, ReleaseBundleV1,
    ReleaseRuntime, ReleaseService, RuntimeEnvPolicy, ServiceRole, BUILD_ARTIFACT_SCHEMA_VERSION,
    RELEASE_BUNDLE_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};

include!("remote_build/registry.rs");

async fn sync_build_checkout(
    cfg: &Config,
    payload: &Value,
    deployment_id: Uuid,
) -> anyhow::Result<(PathBuf, u128)> {
    let repo = payload["repo"].as_str().context("repo")?;
    let branch = payload["branch"].as_str().context("branch")?;
    let commit_sha = payload["commit_sha"].as_str().context("commit_sha")?;
    let github_token = payload.get("github_token").and_then(Value::as_str);
    validate_repo(repo)?;
    validate_branch(branch)?;
    validate_commit_sha(commit_sha)?;
    let checkout = cfg
        .workdir
        .join("build-repos")
        .join(deployment_id.to_string());
    let expected_remote = format!("https://github.com/{repo}.git");
    let fetch_remote = git_fetch_remote(repo, github_token);
    let started = Instant::now();
    sync_checkout(
        cfg,
        deployment_id,
        &checkout,
        &expected_remote,
        &fetch_remote,
        branch,
        commit_sha,
        github_token,
    )
    .await?;
    verify_git_head(cfg, deployment_id, &checkout, commit_sha).await?;
    Ok((checkout, started.elapsed().as_millis()))
}

fn build_plan_digest(payload: &Value) -> anyhow::Result<String> {
    let mut value = payload.clone();
    if let Some(object) = value.as_object_mut() {
        for key in [
            "artifact_registry",
            "github_token",
            "job_id",
            "claim_token",
            "runner_server_id",
        ] {
            object.remove(key);
        }
    }
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&value)?)
    ))
}

fn filesystem_mode(hardening: super::pipeline::ContainerHardening) -> ContainerFilesystemMode {
    if hardening.read_only_root_filesystem() {
        ContainerFilesystemMode::ReadOnly
    } else {
        ContainerFilesystemMode::Writable
    }
}

fn staging_tag(deployment_id: Uuid, service: &str) -> String {
    format!("staging-{}-{}", deployment_id.simple(), app_slug(service))
}

async fn write_bundle_image(
    cfg: &Config,
    deployment_id: Uuid,
    bundle: &ReleaseBundleV1,
    compose: Option<&str>,
) -> anyhow::Result<String> {
    let directory = cfg
        .workdir
        .join("artifact-bundles")
        .join(deployment_id.to_string());
    let _ = tokio::fs::remove_dir_all(&directory).await;
    tokio::fs::create_dir_all(&directory).await?;
    tokio::fs::write(directory.join("release.json"), serde_json::to_vec(bundle)?).await?;
    tokio::fs::write(
        directory.join("compose.release.yml"),
        compose.unwrap_or_default(),
    )
    .await?;
    tokio::fs::write(
        directory.join("Dockerfile"),
        "FROM scratch\nCOPY release.json /bundle/release.json\nCOPY compose.release.yml* /bundle/\nCMD [\"/bundle/release.json\"]\n",
    )
    .await?;
    let image = format!("hostlet/release-bundle:{deployment_id}");
    let directory_value = directory.to_string_lossy().to_string();
    run_log(
        cfg,
        deployment_id,
        "docker",
        &[
            "build",
            "-f",
            &directory.join("Dockerfile").to_string_lossy(),
            "-t",
            &image,
            &directory_value,
        ],
    )
    .await?;
    Ok(image)
}

async fn single_artifact(
    cfg: &Config,
    payload: &Value,
    deployment_id: Uuid,
    app_id: Uuid,
    project_dir: &Path,
    git_sync_duration_ms: u128,
    target: &RegistryTarget<'_>,
) -> anyhow::Result<(Vec<ArtifactServiceImage>, ReleaseRuntime, Option<String>)> {
    let app_name = app_slug(&format!("app-{app_id}"));
    let local_image = format!("hostlet/{app_name}:{deployment_id}");
    let port = payload["container_port"]
        .as_i64()
        .context("container_port")?;
    let built = super::pipeline::build_image(
        cfg,
        deployment_id,
        &app_name,
        &local_image,
        project_dir,
        port,
        payload,
        git_sync_duration_ms,
    )
    .await?;
    let artifact = push_image(
        cfg,
        deployment_id,
        &local_image,
        target,
        &staging_tag(deployment_id, "web"),
    )
    .await?;
    let service = ReleaseService {
        name: "web".into(),
        role: "web".into(),
        image_digest_ref: artifact.digest_ref.clone(),
        container_port: u16::try_from(port).context("container port")?,
        health_probe: HealthProbe {
            kind: HealthProbeKind::Http,
            path: Some(
                payload
                    .get("health_path")
                    .and_then(Value::as_str)
                    .unwrap_or("/")
                    .to_string(),
            ),
        },
        filesystem: filesystem_mode(built.hardening),
        env_policy: RuntimeEnvPolicy::All,
        public_env: Vec::new(),
        runtime_metadata: built.runtime_metadata,
    };
    Ok((
        vec![ArtifactServiceImage {
            service: "web".into(),
            role: "web".into(),
            source: ArtifactImageSource::Built,
            artifact,
        }],
        ReleaseRuntime::Single { service },
        None,
    ))
}

async fn compose_config_json(
    cfg: &Config,
    deployment_id: Uuid,
    project_dir: &Path,
    compose_file: &Path,
    override_file: Option<&Path>,
    env: &[(String, String)],
) -> anyhow::Result<Value> {
    let mut args = vec![
        "compose".to_string(),
        "-p".to_string(),
        format!("hostlet-build-{}", deployment_id.simple()),
        "-f".to_string(),
        compose_file.to_string_lossy().to_string(),
    ];
    if let Some(override_file) = override_file {
        args.push("-f".into());
        args.push(override_file.to_string_lossy().to_string());
    }
    args.extend(["config".into(), "--format".into(), "json".into()]);
    let mut command = Command::new("docker");
    command
        .args(&args)
        .current_dir(project_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    harden_host_command_env(&mut command);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = tokio::time::timeout(Duration::from_secs(120), command.output())
        .await
        .context("docker compose config timed out")??;
    if !output.status.success() {
        let message = redact(&String::from_utf8_lossy(&output.stderr));
        log(cfg, deployment_id, "stderr", &message).await;
        bail!("docker compose config failed");
    }
    serde_json::from_slice(&output.stdout).context("docker compose config returned invalid JSON")
}

fn compose_release_yaml(
    compose_text: &str,
    images: &HashMap<String, String>,
) -> anyhow::Result<String> {
    let mut value: serde_yaml::Value = serde_yaml::from_str(compose_text)?;
    let services = value
        .get_mut("services")
        .and_then(serde_yaml::Value::as_mapping_mut)
        .context("compose file must define services")?;
    for (name, service) in services.iter_mut() {
        let name = name.as_str().context("compose service name")?;
        let image = images
            .get(name)
            .with_context(|| format!("compose service {name} has no artifact image"))?;
        let mapping = service
            .as_mapping_mut()
            .context("compose service must be an object")?;
        mapping.remove(serde_yaml::Value::String("build".into()));
        mapping.insert("image".into(), image.clone().into());
        mapping.remove(serde_yaml::Value::String("pull_policy".into()));
    }
    serde_yaml::to_string(&value).context("failed to serialize release compose")
}

fn validate_release_compose(
    compose: &str,
    manifest: Option<&BuildArtifactManifestV2>,
) -> anyhow::Result<()> {
    let value: serde_yaml::Value = serde_yaml::from_str(compose)?;
    let services = value
        .get("services")
        .and_then(serde_yaml::Value::as_mapping)
        .context("release compose must define services")?;
    anyhow::ensure!(!services.is_empty(), "release compose defines no services");
    for (name, service) in services {
        let name = name.as_str().context("release compose service name")?;
        let service = service
            .as_mapping()
            .context("release compose service must be an object")?;
        anyhow::ensure!(
            !service.contains_key(serde_yaml::Value::String("build".into())),
            "release compose service {name} contains a build directive"
        );
        let image = service
            .get(serde_yaml::Value::String("image".into()))
            .and_then(serde_yaml::Value::as_str)
            .with_context(|| format!("release compose service {name} has no image"))?;
        if let Some(manifest) = manifest {
            anyhow::ensure!(
                manifest.images.iter().any(|artifact| {
                    artifact.service == name && artifact.artifact.digest_ref == image
                }),
                "release compose image does not match artifact manifest"
            );
        }
    }
    Ok(())
}

async fn compose_artifact(
    cfg: &Config,
    payload: &Value,
    deployment_id: Uuid,
    app_id: Uuid,
    project_dir: &Path,
    git_sync_duration_ms: u128,
    target: &RegistryTarget<'_>,
) -> anyhow::Result<(Vec<ArtifactServiceImage>, ReleaseRuntime, Option<String>)> {
    ensure_docker_compose().await?;
    let build_dir = cfg.workdir.join("builds").join(deployment_id.to_string());
    tokio::fs::create_dir_all(&build_dir)
        .await
        .context("create Compose artifact build directory")?;
    let resolved = resolve_compose_manifest(payload, project_dir, &build_dir).await?;
    let mut compose_text = tokio::fs::read_to_string(&resolved.compose_file).await?;
    compose_text = remap_host_binds_to_named_volumes(&compose_text)?;
    tokio::fs::write(&resolved.compose_file, &compose_text).await?;
    let web_service = resolved.manifest.compose.web_service.clone();
    validate_compose_subset(&compose_text, &web_service)?;
    let summaries = hostlet_contracts::compose::parse_compose_services(&compose_text, &web_service);
    anyhow::ensure!(!summaries.is_empty(), "compose file defines no services");
    anyhow::ensure!(
        summaries.len() <= 64,
        "compose file defines too many services"
    );

    let mut local_images = HashMap::new();
    let mut sources = HashMap::new();
    let mut override_yaml = String::from("services:\n");
    let marker = format!("${{{}}}", hostlet_contracts::compose::WEB_IMAGE_ENV);
    let generated_web = compose_text.contains(&marker);
    for service in &summaries {
        if service.build || (service.name == web_service && generated_web) {
            let local = format!(
                "hostlet/compose-{}-{}:{deployment_id}",
                app_id.simple(),
                app_slug(&service.name)
            );
            local_images.insert(service.name.clone(), local.clone());
            sources.insert(service.name.clone(), ArtifactImageSource::Built);
            override_yaml.push_str(&format!("  {}:\n    image: {}\n", service.name, local));
        }
    }
    if local_images.is_empty() {
        override_yaml = "services: {}\n".into();
    }
    let override_file = build_dir.join("compose.build.hostlet.yml");
    tokio::fs::write(&override_file, &override_yaml).await?;
    let mut compose_env = compose_interpolation_env(payload);
    if let Some(local) = local_images.get(&web_service).filter(|_| generated_web) {
        compose_env.push((
            hostlet_contracts::compose::WEB_IMAGE_ENV.to_string(),
            local.clone(),
        ));
        let app_name = app_slug(&format!("app-{app_id}"));
        let port = payload["container_port"]
            .as_i64()
            .context("container_port")?;
        super::pipeline::build_image(
            cfg,
            deployment_id,
            &app_name,
            local,
            project_dir,
            port,
            payload,
            git_sync_duration_ms,
        )
        .await?;
    }
    let env_refs = compose_env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    for service in summaries.iter().filter(|service| service.build) {
        let project = format!("hostlet-build-{}", deployment_id.simple());
        let args = compose_invocation(
            &project,
            &resolved.compose_file,
            &override_file,
            &["build", &service.name],
        )?;
        run_log_in_dir_env(cfg, deployment_id, project_dir, &env_refs, "docker", &args).await?;
    }
    let config = compose_config_json(
        cfg,
        deployment_id,
        project_dir,
        &resolved.compose_file,
        Some(&override_file),
        &compose_env,
    )
    .await?;
    let config_services = config
        .get("services")
        .and_then(Value::as_object)
        .context("resolved compose has no services")?;
    let platform = payload
        .get("required_platform")
        .and_then(Value::as_str)
        .context("required platform")?;
    let mut artifact_images = Vec::new();
    let mut release_images = HashMap::new();
    for service in &summaries {
        let source = sources
            .get(&service.name)
            .cloned()
            .unwrap_or(ArtifactImageSource::Mirrored);
        let local_image = if let Some(local) = local_images.get(&service.name) {
            local.clone()
        } else {
            let image = config_services
                .get(&service.name)
                .and_then(|value| value.get("image"))
                .and_then(Value::as_str)
                .with_context(|| format!("compose service {} has no image", service.name))?;
            run_log(
                cfg,
                deployment_id,
                "docker",
                &["pull", "--platform", platform, image],
            )
            .await?;
            image.to_string()
        };
        let artifact = push_image(
            cfg,
            deployment_id,
            &local_image,
            target,
            &staging_tag(deployment_id, &service.name),
        )
        .await?;
        release_images.insert(service.name.clone(), artifact.digest_ref.clone());
        artifact_images.push(ArtifactServiceImage {
            service: service.name.clone(),
            role: service.role.clone(),
            source,
            artifact,
        });
    }
    let release_compose = compose_release_yaml(&compose_text, &release_images)?;
    validate_release_compose(&release_compose, None)?;
    let port = resolved.manifest.compose.port.unwrap_or(u16::try_from(
        payload["container_port"].as_i64().context("port")?,
    )?);
    let health_path = resolved
        .manifest
        .compose
        .health_path
        .clone()
        .unwrap_or_else(|| {
            payload
                .get("health_path")
                .and_then(Value::as_str)
                .unwrap_or("/")
                .to_string()
        });
    Ok((
        artifact_images,
        ReleaseRuntime::Compose {
            compose_file: "compose.release.yml".into(),
            hostlet_config_path: resolved.manifest_path.to_string(),
            web_service,
            target_port: port,
            health_path,
            backing_spec_source: compose_text,
        },
        Some(release_compose),
    ))
}

async fn generated_topology_artifact(
    cfg: &Config,
    payload: &Value,
    deployment_id: Uuid,
    app_id: Uuid,
    project_dir: &Path,
    git_sync_duration_ms: u128,
    target: &RegistryTarget<'_>,
) -> anyhow::Result<(Vec<ArtifactServiceImage>, ReleaseRuntime, Option<String>)> {
    use super::pipeline::generated_topology as topology;
    let inventory = topology::checkout_inventory(project_dir).await?;
    let plan = hostlet_contracts::plan_repository_topology(&inventory);
    let config: GeneratedTopologyConfig = serde_json::from_value(
        payload
            .pointer("/runtime_config/generatedTopology")
            .cloned()
            .context("generated topology config is missing")?,
    )?;
    hostlet_contracts::validate_generated_topology_config(&config).map_err(anyhow::Error::msg)?;
    let services = topology::selected_services(&plan, &config)?;
    anyhow::ensure!(
        !services.is_empty() && services.len() <= 2,
        "generated topology must contain one service or one frontend/backend pair"
    );
    let lock_receipt = topology::repair_pnpm_lock_metadata(project_dir, &inventory).await?;
    topology::log_inference_plan(cfg, deployment_id, &plan, &services, lock_receipt.as_ref()).await;
    let domain = payload["domain"].as_str().context("domain")?;
    let public_origin = cfg.app_public_scheme.origin(domain);
    let public_ws_origin = cfg.app_public_scheme.websocket_origin(domain);
    let app_name = app_slug(&format!("app-{app_id}"));
    let mut artifact_images = Vec::new();
    let mut release_services = Vec::new();
    for service in &services {
        let service_slug = app_slug(&service.name);
        let local_image = format!("hostlet/{app_name}-{service_slug}:{deployment_id}");
        let mut service_payload = payload.clone();
        topology::configure_service_payload(
            &mut service_payload,
            service,
            &public_origin,
            &public_ws_origin,
            project_dir,
        )
        .await?;
        let built = super::pipeline::build_image(
            cfg,
            deployment_id,
            &format!("{app_name}-{service_slug}"),
            &local_image,
            project_dir,
            service.container_port.into(),
            &service_payload,
            git_sync_duration_ms,
        )
        .await?;
        let artifact = push_image(
            cfg,
            deployment_id,
            &local_image,
            target,
            &staging_tag(deployment_id, &service.name),
        )
        .await?;
        let role = match service.role {
            ServiceRole::Frontend => "frontend",
            ServiceRole::Backend => "backend",
            ServiceRole::Web => "web",
        };
        release_services.push(ReleaseService {
            name: service.name.clone(),
            role: role.into(),
            image_digest_ref: artifact.digest_ref.clone(),
            container_port: service.container_port,
            health_probe: service.health_probe.clone(),
            filesystem: filesystem_mode(built.hardening),
            env_policy: if service.role == ServiceRole::Frontend
                && service.output_directory.is_some()
            {
                RuntimeEnvPolicy::None
            } else {
                RuntimeEnvPolicy::All
            },
            public_env: service.public_env.clone(),
            runtime_metadata: built.runtime_metadata,
        });
        artifact_images.push(ArtifactServiceImage {
            service: service.name.clone(),
            role: role.into(),
            source: ArtifactImageSource::Built,
            artifact,
        });
    }
    let receipt = topology::inference_receipt(&plan, &services, &config, lock_receipt.as_ref());
    Ok((
        artifact_images,
        ReleaseRuntime::GeneratedTopology {
            services: release_services,
            inferred_services: services,
            config,
            inference_receipt: receipt,
        },
        None,
    ))
}

pub(super) async fn build_artifact(cfg: Config, payload: Value) -> anyhow::Result<Value> {
    let deployment_id =
        Uuid::parse_str(payload["deployment_id"].as_str().context("deployment_id")?)?;
    let app_id = Uuid::parse_str(payload["app_id"].as_str().context("app_id")?)?;
    let build_id = Uuid::parse_str(payload["build_id"].as_str().context("build_id")?)?;
    let commit_sha = payload["commit_sha"].as_str().context("commit_sha")?;
    let platform = payload["required_platform"]
        .as_str()
        .context("required_platform")?;
    status(&cfg, deployment_id, "building", None).await;
    let (checkout, git_sync_duration_ms) =
        sync_build_checkout(&cfg, &payload, deployment_id).await?;
    let root_directory = payload
        .get("root_directory")
        .and_then(Value::as_str)
        .unwrap_or(".");
    let project_dir = safe_project_dir(&checkout, root_directory).await?;
    let target = registry_target(&payload, true)?;
    let (images, runtime, compose) = if payload
        .pointer("/runtime_config/generatedTopology")
        .is_some()
    {
        generated_topology_artifact(
            &cfg,
            &payload,
            deployment_id,
            app_id,
            &project_dir,
            git_sync_duration_ms,
            &target,
        )
        .await?
    } else if payload.get("runtime_kind").and_then(Value::as_str) == Some("compose") {
        compose_artifact(
            &cfg,
            &payload,
            deployment_id,
            app_id,
            &project_dir,
            git_sync_duration_ms,
            &target,
        )
        .await?
    } else {
        single_artifact(
            &cfg,
            &payload,
            deployment_id,
            app_id,
            &project_dir,
            git_sync_duration_ms,
            &target,
        )
        .await?
    };
    status(&cfg, deployment_id, "publishing", None).await;
    let bundle = ReleaseBundleV1 {
        schema_version: RELEASE_BUNDLE_SCHEMA_VERSION,
        deployment_id,
        app_id,
        commit_sha: commit_sha.to_string(),
        platform: platform.to_string(),
        runtime,
    };
    let bundle_image = write_bundle_image(&cfg, deployment_id, &bundle, compose.as_deref()).await?;
    let bundle_descriptor = push_image(
        &cfg,
        deployment_id,
        &bundle_image,
        &target,
        &staging_tag(deployment_id, "release-bundle"),
    )
    .await?;
    let mut index_children = images
        .iter()
        .map(|image| {
            (
                image.artifact.clone(),
                BTreeMap::from([
                    ("io.hostlet.kind".into(), "service".into()),
                    ("io.hostlet.service".into(), image.service.clone()),
                    ("io.hostlet.role".into(), image.role.clone()),
                ]),
            )
        })
        .collect::<Vec<_>>();
    index_children.push((
        bundle_descriptor.clone(),
        BTreeMap::from([("io.hostlet.kind".into(), "release-bundle".into())]),
    ));
    let index = publish_index(
        &cfg,
        &target,
        deployment_id,
        app_id,
        commit_sha,
        &index_children,
    )
    .await?;
    let manifest = BuildArtifactManifestV2 {
        schema_version: BUILD_ARTIFACT_SCHEMA_VERSION,
        build_id,
        deployment_id,
        app_id,
        commit_sha: commit_sha.to_string(),
        platform: platform.to_string(),
        build_plan_digest: build_plan_digest(&payload)?,
        index,
        bundle: bundle_descriptor,
        images,
    };
    Ok(serde_json::to_value(manifest)?)
}

async fn pull_image(
    cfg: &Config,
    deployment_id: Uuid,
    payload: &Value,
    digest_ref: &str,
) -> anyhow::Result<()> {
    let target = registry_target(payload, false)?;
    anyhow::ensure!(
        digest_ref.starts_with(&format!("{}/{}@sha256:", target.host, target.repository)),
        "artifact digest is outside the assigned repository"
    );
    let docker_config = docker_login(cfg, deployment_id, &target).await?;
    let config_value = docker_config.to_string_lossy().to_string();
    let result = run_log_in_dir_env(
        cfg,
        deployment_id,
        &cfg.workdir,
        &[("DOCKER_CONFIG", config_value.as_str())],
        "docker",
        &["pull", digest_ref],
    )
    .await;
    let _ = tokio::fs::remove_dir_all(&docker_config).await;
    result
}

async fn extract_bundle(
    cfg: &Config,
    deployment_id: Uuid,
    digest_ref: &str,
) -> anyhow::Result<PathBuf> {
    let container = format!("hostlet-artifact-{}", deployment_id.simple());
    let directory = cfg
        .workdir
        .join("release-bundles")
        .join(deployment_id.to_string());
    let _ = run_quiet("docker", &["rm", "-f", &container]).await;
    let _ = tokio::fs::remove_dir_all(&directory).await;
    tokio::fs::create_dir_all(&directory).await?;
    run_log(
        cfg,
        deployment_id,
        "docker",
        &["create", "--name", &container, digest_ref],
    )
    .await?;
    let destination = directory.to_string_lossy().to_string();
    let copy_result = run_log(
        cfg,
        deployment_id,
        "docker",
        &["cp", &format!("{container}:/bundle/."), &destination],
    )
    .await;
    let _ = run_quiet("docker", &["rm", "-f", &container]).await;
    copy_result?;
    Ok(directory)
}

async fn read_bundle_file(directory: &Path, relative: &str) -> anyhow::Result<Vec<u8>> {
    validate_relative_file_path(relative)?;
    let root = tokio::fs::canonicalize(directory).await?;
    let path = directory.join(relative);
    let metadata = tokio::fs::symlink_metadata(&path).await?;
    anyhow::ensure!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "release bundle entry must be a regular file"
    );
    anyhow::ensure!(
        metadata.len() <= 2 * 1024 * 1024,
        "release bundle entry exceeds the 2 MiB limit"
    );
    let canonical = tokio::fs::canonicalize(&path).await?;
    anyhow::ensure!(
        canonical.starts_with(&root),
        "release bundle entry escaped its artifact directory"
    );
    tokio::fs::read(canonical).await.map_err(Into::into)
}

fn validate_bundle(
    payload: &Value,
    manifest: &BuildArtifactManifestV2,
    bundle: &ReleaseBundleV1,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        bundle.schema_version == RELEASE_BUNDLE_SCHEMA_VERSION,
        "unsupported release bundle schema"
    );
    anyhow::ensure!(
        bundle.deployment_id == manifest.deployment_id
            && bundle.app_id == manifest.app_id
            && bundle.commit_sha == manifest.commit_sha
            && bundle.platform == manifest.platform,
        "release bundle does not match artifact assignment"
    );
    let job_deployment = payload
        .get("deployment_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .context("release job deployment id")?;
    if job_deployment != bundle.deployment_id {
        anyhow::ensure!(
            payload.get("type").and_then(Value::as_str) == Some("rollback")
                && payload
                    .get("target_deployment_id")
                    .and_then(Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok())
                    == Some(bundle.deployment_id),
            "release bundle deployment does not match job"
        );
    }
    let expected = manifest
        .images
        .iter()
        .map(|image| (image.service.as_str(), image.artifact.digest_ref.as_str()))
        .collect::<HashMap<_, _>>();
    let services: Vec<&ReleaseService> = match &bundle.runtime {
        ReleaseRuntime::Single { service } => vec![service],
        ReleaseRuntime::GeneratedTopology { services, .. } => services.iter().collect(),
        ReleaseRuntime::Compose { .. } => Vec::new(),
    };
    for service in services {
        anyhow::ensure!(
            expected.get(service.name.as_str()).copied() == Some(service.image_digest_ref.as_str()),
            "release service image does not match artifact manifest"
        );
    }
    Ok(())
}

pub(super) async fn release_artifact(cfg: Config, mut payload: Value) -> anyhow::Result<()> {
    let deployment_id =
        Uuid::parse_str(payload["deployment_id"].as_str().context("deployment_id")?)?;
    if payload
        .pointer("/artifact_manifest/schemaVersion")
        .and_then(Value::as_u64)
        == Some(1)
    {
        let image = payload
            .pointer("/artifact_manifest/images")
            .and_then(Value::as_array)
            .and_then(|images| {
                images
                    .iter()
                    .find(|image| image.get("role").and_then(Value::as_str) == Some("web"))
            })
            .and_then(|image| image.get("digestRef"))
            .and_then(Value::as_str)
            .context("legacy artifact manifest is missing the web image digest")?
            .to_string();
        status(&cfg, deployment_id, "pulling", None).await;
        pull_image(&cfg, deployment_id, &payload, &image).await?;
        return super::pipeline::release_single_image(cfg, payload, image).await;
    }
    let manifest: BuildArtifactManifestV2 = serde_json::from_value(
        payload
            .get("artifact_manifest")
            .cloned()
            .context("release is missing its artifact manifest")?,
    )
    .context("artifact manifest is invalid")?;
    anyhow::ensure!(
        manifest.schema_version == BUILD_ARTIFACT_SCHEMA_VERSION,
        "unsupported artifact manifest"
    );
    status(&cfg, deployment_id, "pulling", None).await;
    pull_image(&cfg, deployment_id, &payload, &manifest.bundle.digest_ref).await?;
    for image in &manifest.images {
        pull_image(&cfg, deployment_id, &payload, &image.artifact.digest_ref).await?;
    }
    let directory = extract_bundle(&cfg, deployment_id, &manifest.bundle.digest_ref).await?;
    let bundle: ReleaseBundleV1 =
        serde_json::from_slice(&read_bundle_file(&directory, "release.json").await?)?;
    validate_bundle(&payload, &manifest, &bundle)?;
    match bundle.runtime {
        ReleaseRuntime::Single { service } => {
            if let Some(object) = payload.as_object_mut() {
                object.insert(
                    "artifact_manifest".into(),
                    json!({"runtimeMetadata": service.runtime_metadata}),
                );
            }
            super::pipeline::release_single_image(cfg, payload, service.image_digest_ref).await
        }
        ReleaseRuntime::Compose {
            compose_file,
            hostlet_config_path,
            web_service,
            target_port,
            health_path,
            backing_spec_source,
        } => {
            let compose = String::from_utf8(read_bundle_file(&directory, &compose_file).await?)
                .context("release compose is not UTF-8")?;
            validate_release_compose(&compose, Some(&manifest))?;
            let app_id = Uuid::parse_str(payload["app_id"].as_str().context("app_id")?)?;
            let app_name = app_slug(&format!("app-{app_id}"));
            let route_key = payload
                .get("route_key")
                .and_then(Value::as_str)
                .map(app_slug)
                .unwrap_or_else(|| app_name.clone());
            let domain = payload["domain"].as_str().context("domain")?.to_string();
            if let Some(object) = payload.as_object_mut() {
                object.insert(
                    "_hostlet_backing_spec_source".into(),
                    json!(backing_spec_source),
                );
                object.insert(
                    "_hostlet_artifact_config_path".into(),
                    json!(hostlet_config_path),
                );
                let mut runtime_config = object
                    .get("runtime_config")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let runtime_object = runtime_config
                    .as_object_mut()
                    .context("runtime_config must be an object")?;
                runtime_object.insert(
                    "generatedCompose".into(),
                    json!({
                        "composeFile": "compose.release.yml",
                        "webService": web_service,
                        "port": target_port,
                        "healthPath": health_path,
                        "compose": compose,
                    }),
                );
                object.insert("runtime_config".into(), runtime_config);
            }
            deploy_compose(
                cfg,
                payload,
                deployment_id,
                app_id,
                &app_name,
                &route_key,
                &directory,
                target_port.into(),
                &domain,
                &health_path,
                0,
                None,
                true,
            )
            .await
        }
        ReleaseRuntime::GeneratedTopology {
            services,
            inferred_services,
            config,
            inference_receipt,
        } => {
            super::pipeline::generated_topology::release_generated_topology(
                &cfg,
                &payload,
                services,
                inferred_services,
                config,
                inference_receipt,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_compose_replaces_builds_with_digests() {
        let compose = "services:\n  web:\n    build: .\n  db:\n    image: postgres:16\n";
        let output = compose_release_yaml(
            compose,
            &HashMap::from([
                ("web".into(), "registry/app@sha256:web".into()),
                ("db".into(), "registry/app@sha256:db".into()),
            ]),
        )
        .unwrap();
        assert!(!output.contains("build:"));
        assert!(output.contains("registry/app@sha256:web"));
        assert!(output.contains("registry/app@sha256:db"));
    }

    #[test]
    fn registry_target_rejects_cross_namespace_repositories() {
        let payload = json!({"artifact_registry": {
            "url": "https://registry.example.test",
            "host": "registry.example.test",
            "repository": "other/app",
            "tag": "deployment-abc",
            "pushUsername": "builder",
            "pushPassword": "secret"
        }});
        assert!(registry_target(&payload, true).is_err());
    }
}
