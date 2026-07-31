const OCI_IMAGE_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
const OCI_IMAGE_INDEX: &str = "application/vnd.oci.image.index.v1+json";

struct RegistryTarget<'a> {
    url: &'a str,
    host: &'a str,
    repository: &'a str,
    tag: &'a str,
    username: &'a str,
    password: &'a str,
}

impl RegistryTarget<'_> {
    fn tagged_ref(&self, tag: &str) -> String {
        format!("{}/{}:{tag}", self.host, self.repository)
    }

    fn digest_ref(&self, digest: &str) -> String {
        format!("{}/{}@{digest}", self.host, self.repository)
    }

    fn manifest_url(&self, reference: &str) -> String {
        format!(
            "{}/v2/{}/manifests/{reference}",
            self.url.trim_end_matches('/'),
            self.repository
        )
    }
}

fn registry_target(payload: &Value, push: bool) -> anyhow::Result<RegistryTarget<'_>> {
    let registry = payload
        .get("artifact_registry")
        .context("build is missing artifact registry configuration")?;
    let (username_key, password_key) = if push {
        ("pushUsername", "pushPassword")
    } else {
        ("pullUsername", "pullPassword")
    };
    let target = RegistryTarget {
        url: registry
            .get("url")
            .and_then(Value::as_str)
            .context("artifact registry URL")?,
        host: registry
            .get("host")
            .and_then(Value::as_str)
            .context("artifact registry host")?,
        repository: registry
            .get("repository")
            .and_then(Value::as_str)
            .context("artifact registry repository")?,
        tag: registry
            .get("tag")
            .and_then(Value::as_str)
            .context("artifact registry tag")?,
        username: registry
            .get(username_key)
            .and_then(Value::as_str)
            .context("artifact registry username")?,
        password: registry
            .get(password_key)
            .and_then(Value::as_str)
            .context("artifact registry password")?,
    };
    anyhow::ensure!(
        target.host.len() <= 253
            && target
                .host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':')),
        "artifact registry host is invalid"
    );
    anyhow::ensure!(
        target.repository.starts_with("hostlet/apps/")
            && target.repository.len() <= 300
            && target.repository.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '/' | '-' | '_')
            }),
        "artifact repository is invalid"
    );
    anyhow::ensure!(
        target.tag.starts_with("deployment-")
            && target
                .tag
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
        "artifact tag is invalid"
    );
    Ok(target)
}

async fn docker_login(
    cfg: &Config,
    deployment_id: Uuid,
    target: &RegistryTarget<'_>,
) -> anyhow::Result<PathBuf> {
    let docker_config = cfg
        .workdir
        .join("registry-auth")
        .join(deployment_id.to_string());
    let _ = tokio::fs::remove_dir_all(&docker_config).await;
    tokio::fs::create_dir_all(&docker_config).await?;
    let auth = base64::engine::general_purpose::STANDARD
        .encode(format!("{}:{}", target.username, target.password));
    let config_path = docker_config.join("config.json");
    tokio::fs::write(
        &config_path,
        serde_json::to_vec(&json!({"auths": {target.host: {"auth": auth}}}))?,
    )
    .await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).await?;
    }
    Ok(docker_config)
}

fn manifest_accept() -> &'static str {
    "application/vnd.oci.image.index.v1+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.docker.distribution.manifest.v2+json"
}

async fn registry_descriptor(
    cfg: &Config,
    target: &RegistryTarget<'_>,
    reference: &str,
) -> anyhow::Result<OciArtifactDescriptor> {
    let response = cfg
        .http
        .get(target.manifest_url(reference))
        .basic_auth(target.username, Some(target.password))
        .header(reqwest::header::ACCEPT, manifest_accept())
        .send()
        .await?
        .error_for_status()?;
    let headers = response.headers().clone();
    let body = response.bytes().await?;
    let digest = headers
        .get("docker-content-digest")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| format!("sha256:{:x}", Sha256::digest(&body)));
    anyhow::ensure!(
        digest
            .strip_prefix("sha256:")
            .is_some_and(|value| value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit())),
        "registry returned an invalid manifest digest"
    );
    let media_type = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .unwrap_or(OCI_IMAGE_MANIFEST)
        .to_string();
    Ok(OciArtifactDescriptor {
        digest_ref: target.digest_ref(&digest),
        media_type,
        size_bytes: body.len() as u64,
    })
}

async fn push_image(
    cfg: &Config,
    deployment_id: Uuid,
    local_image: &str,
    target: &RegistryTarget<'_>,
    tag: &str,
) -> anyhow::Result<OciArtifactDescriptor> {
    let normalized = normalize_image_for_registry(cfg, deployment_id, local_image, tag).await?;
    let tagged = target.tagged_ref(tag);
    run_log(cfg, deployment_id, "docker", &["tag", &normalized, &tagged]).await?;
    let docker_config = docker_login(cfg, deployment_id, target).await?;
    let config_value = docker_config.to_string_lossy().to_string();
    let result = run_log_in_dir_env(
        cfg,
        deployment_id,
        &cfg.workdir,
        &[("DOCKER_CONFIG", config_value.as_str())],
        "docker",
        &["push", &tagged],
    )
    .await;
    let _ = tokio::fs::remove_dir_all(&docker_config).await;
    result?;
    registry_descriptor(cfg, target, tag).await
}

/// Railpack's Docker exporter can load a schema-2 manifest whose layer media
/// types are OCI. Strict distribution registries correctly reject that mixed
/// manifest. A no-op BuildKit export preserves the image config and layers
/// while emitting a consistent OCI manifest for every artifact source.
async fn normalize_image_for_registry(
    cfg: &Config,
    deployment_id: Uuid,
    local_image: &str,
    tag: &str,
) -> anyhow::Result<String> {
    anyhow::ensure!(
        !local_image.contains(['\n', '\r']) && !local_image.trim().is_empty(),
        "local artifact image reference is invalid"
    );
    let directory = cfg
        .workdir
        .join("registry-normalize")
        .join(deployment_id.to_string())
        .join(app_slug(tag));
    let _ = tokio::fs::remove_dir_all(&directory).await;
    tokio::fs::create_dir_all(&directory).await?;
    tokio::fs::write(
        directory.join("Dockerfile"),
        b"ARG BASE_IMAGE\nFROM ${BASE_IMAGE}\n",
    )
    .await?;
    let normalized = format!(
        "hostlet-artifact-normalized:{}-{}",
        deployment_id.simple(),
        app_slug(tag)
    );
    let build_arg = format!("BASE_IMAGE={local_image}");
    run_log_in_dir_env(
        cfg,
        deployment_id,
        &directory,
        &[],
        "docker",
        &[
            "build",
            "--provenance=false",
            "--build-arg",
            &build_arg,
            "-t",
            &normalized,
            ".",
        ],
    )
    .await?;
    let _ = tokio::fs::remove_dir_all(&directory).await;
    Ok(normalized)
}

async fn publish_index(
    cfg: &Config,
    target: &RegistryTarget<'_>,
    deployment_id: Uuid,
    app_id: Uuid,
    commit_sha: &str,
    descriptors: &[(OciArtifactDescriptor, BTreeMap<String, String>)],
) -> anyhow::Result<OciArtifactDescriptor> {
    let manifests = descriptors
        .iter()
        .map(|(descriptor, annotations)| {
            let digest = descriptor
                .digest_ref
                .rsplit_once('@')
                .map(|(_, digest)| digest)
                .context("artifact descriptor is not digest-qualified")?;
            Ok(json!({
                "mediaType": descriptor.media_type,
                "digest": digest,
                "size": descriptor.size_bytes,
                "annotations": annotations,
            }))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let body = serde_json::to_vec(&json!({
        "schemaVersion": 2,
        "mediaType": OCI_IMAGE_INDEX,
        "manifests": manifests,
        "annotations": {
            "io.hostlet.schema": BUILD_ARTIFACT_SCHEMA_VERSION.to_string(),
            "io.hostlet.deployment": deployment_id.to_string(),
            "io.hostlet.app": app_id.to_string(),
            "org.opencontainers.image.revision": commit_sha,
        }
    }))?;
    let response = cfg
        .http
        .put(target.manifest_url(target.tag))
        .basic_auth(target.username, Some(target.password))
        .header(reqwest::header::CONTENT_TYPE, OCI_IMAGE_INDEX)
        .body(body.clone())
        .send()
        .await?
        .error_for_status()?;
    let digest = response
        .headers()
        .get("docker-content-digest")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| format!("sha256:{:x}", Sha256::digest(&body)));
    Ok(OciArtifactDescriptor {
        digest_ref: target.digest_ref(&digest),
        media_type: OCI_IMAGE_INDEX.to_string(),
        size_bytes: body.len() as u64,
    })
}
