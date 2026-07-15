use super::*;

mod docker_resources;
mod redaction;

pub(crate) use docker_resources::*;
pub(crate) use hostlet_contracts::app_slug;
pub(crate) use redaction::*;

pub(crate) fn valid_hostlet_image(value: &str) -> bool {
    valid_prefixed_name(value, "hostlet/", 300, |c| {
        c.is_ascii_alphanumeric() || matches!(c, '/' | ':' | '.' | '_' | '-')
    })
}

pub(crate) fn env(key: &str) -> anyhow::Result<String> {
    std::env::var(key).with_context(|| format!("{key} is required"))
}

pub(crate) fn local_router_config() -> anyhow::Result<Option<LocalRouter>> {
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

pub(crate) fn compose_project_name(app_id: Uuid) -> String {
    format!("hostlet-app-{}", app_id.simple())
}

pub(crate) fn compose_release_project_name(deployment_id: Uuid) -> String {
    format!("hostlet-release-{}", deployment_id.simple())
}

/// Derives the release-only override from the already-hardened base override.
/// The web container joins the stable app network and every declared named
/// volume resolves to the stable project's explicit volume name.
pub(crate) fn compose_release_override_yaml(
    base_override: &str,
    compose_text: &str,
    web_service: &str,
    stable_project: &str,
) -> anyhow::Result<String> {
    let mut value: serde_yaml::Value = serde_yaml::from_str(base_override)?;
    let root = value
        .as_mapping_mut()
        .context("compose override must be a mapping")?;
    let services = yaml_get_mut(root, "services")
        .and_then(serde_yaml::Value::as_mapping_mut)
        .context("compose override must define services")?;
    let web = services
        .get_mut(serde_yaml::Value::String(web_service.to_string()))
        .and_then(serde_yaml::Value::as_mapping_mut)
        .context("compose override is missing web service")?;
    web.insert(
        serde_yaml::Value::String("networks".into()),
        serde_yaml::Value::Sequence(vec![serde_yaml::Value::String("hostlet-stable".into())]),
    );
    let mut network = serde_yaml::Mapping::new();
    network.insert(serde_yaml::Value::String("external".into()), true.into());
    network.insert(
        serde_yaml::Value::String("name".into()),
        format!("{stable_project}_default").into(),
    );
    let mut networks = serde_yaml::Mapping::new();
    networks.insert("hostlet-stable".into(), network.into());
    root.insert("networks".into(), networks.into());

    let source: serde_yaml::Value = serde_yaml::from_str(compose_text)?;
    if let Some(source_volumes) = source
        .get("volumes")
        .and_then(serde_yaml::Value::as_mapping)
    {
        let mut volumes = serde_yaml::Mapping::new();
        for name in source_volumes.keys().filter_map(serde_yaml::Value::as_str) {
            validate_service_name(name)?;
            let mut definition = serde_yaml::Mapping::new();
            definition.insert("name".into(), format!("{stable_project}_{name}").into());
            definition.insert("external".into(), true.into());
            volumes.insert(name.into(), definition.into());
        }
        root.insert("volumes".into(), volumes.into());
    }
    serde_yaml::to_string(&value).context("failed to serialize release compose override")
}

pub(crate) fn compose_named_volume_names(
    compose_text: &str,
    stable_project: &str,
) -> anyhow::Result<Vec<String>> {
    let source: serde_yaml::Value = serde_yaml::from_str(compose_text)?;
    let mut names = source
        .get("volumes")
        .and_then(serde_yaml::Value::as_mapping)
        .into_iter()
        .flatten()
        .filter_map(|(key, _)| key.as_str())
        .map(|name| {
            validate_service_name(name)?;
            Ok(format!("{stable_project}_{name}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    names.sort();
    names.dedup();
    Ok(names)
}

/// Builds the `environment:` block body for the compose override: the
/// Hostlet-injected variables followed by the validated env entries from the
/// payload, each rendered as a YAML-escaped `- KEY=VALUE` list item.
fn compose_override_env_block(app_id: Uuid, deployment_id: Uuid, payload: &Value) -> String {
    let mut env = vec![
        format!("HOSTLET_APP_ID={app_id}"),
        format!("HOSTLET_DEPLOYMENT_ID={deployment_id}"),
        "HOSTLET_DATA_DIR=/data".to_string(),
        "DATA_DIR=/data".to_string(),
    ];
    if let Some(map) = payload.get("env").and_then(|v| v.as_object()) {
        for (key, value) in map {
            if hostlet_contracts::valid_env_key(key) {
                let value = value.as_str().unwrap_or_default();
                env.push(format!("{}={}", key, value.replace('\n', "\\n")));
            }
        }
    }
    env.iter()
        .map(|item| format!("      - {}", serde_yaml::to_string(item).unwrap().trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Builds the Hostlet Compose override (`compose.hostlet.yml`) that is merged
/// over the app's compose file at deploy time.
///
/// **Every** service is hardened so a backing container cannot escalate
/// privileges or fork-bomb a shared host: `no-new-privileges` and a `pids_limit`
/// are applied to all services. The web service additionally drops all Linux
/// capabilities (`cap_drop: ALL`) — Hostlet's generated web runtimes never need
/// them — while backing services keep Docker's already-curated default
/// capability set, because standard database/cache images (e.g. Postgres, Redis)
/// chown/setuid during init and would fail to start under `cap_drop: ALL`.
///
/// Only the web service receives the injected environment and a published port,
/// and that publish is bound to `127.0.0.1` (loopback) so Caddy stays the sole
/// public edge; backing services get no host port at all.
pub(crate) fn compose_override_yaml(
    compose_text: &str,
    web_service: &str,
    port: u16,
    app_id: Uuid,
    deployment_id: Uuid,
    payload: &Value,
) -> String {
    let env_yaml = compose_override_env_block(app_id, deployment_id, payload);
    let mut service_names: Vec<String> =
        hostlet_contracts::compose::parse_compose_services(compose_text, web_service)
            .into_iter()
            .map(|service| service.name)
            .collect();
    if !service_names.iter().any(|name| name == web_service) {
        service_names.push(web_service.to_string());
    }
    let mut out = String::from("services:\n");
    for name in &service_names {
        let is_web = name == web_service;
        let role = if is_web { "web" } else { "backing" };
        out.push_str(&format!(
            "  {name}:\n    labels:\n      hostlet.app_id: \"{app_id}\"\n      hostlet.deployment_id: \"{deployment_id}\"\n      hostlet.role: \"{role}\"\n"
        ));
        if is_web {
            out.push_str(&format!("    environment:\n{env_yaml}\n"));
        }
        out.push_str("    security_opt:\n      - no-new-privileges:true\n");
        if is_web {
            out.push_str("    cap_drop:\n      - ALL\n");
        }
        out.push_str(&format!(
            "    pids_limit: {}\n",
            if is_web { 256 } else { 512 }
        ));
        // Backing services may carry per-service memory/CPU caps (e.g. Hostlet
        // Cloud applies its plan's backing limits so add-ons can't crowd the
        // shared host). The web service keeps its own per-app `--memory`/`--cpus`
        // applied by the single-service runtime. Absent caps (self-hosted
        // default) leave the backing service uncapped.
        if !is_web {
            if let Some(mb) = payload
                .pointer("/runtime_config/compose/backingMemoryLimitMb")
                .and_then(|value| value.as_i64())
            {
                out.push_str(&format!("    mem_limit: {mb}m\n"));
            }
            if let Some(cpus) = payload
                .pointer("/runtime_config/compose/backingCpuLimit")
                .and_then(|value| value.as_f64())
            {
                out.push_str(&format!("    cpus: \"{cpus:.2}\"\n"));
            }
        }
        if is_web {
            out.push_str(&format!(
                "    ports:\n      - target: {port}\n        host_ip: 127.0.0.1\n        protocol: tcp\n"
            ));
        }
    }
    out
}

/// Look up a string key in a YAML mapping without repeatedly allocating a
/// `serde_yaml::Value::String` at every call site.
fn yaml_get<'a>(mapping: &'a serde_yaml::Mapping, key: &str) -> Option<&'a serde_yaml::Value> {
    mapping.get(serde_yaml::Value::String(key.to_string()))
}

fn yaml_contains_key(mapping: &serde_yaml::Mapping, key: &str) -> bool {
    mapping.contains_key(serde_yaml::Value::String(key.to_string()))
}

fn validate_compose_top_level_volumes(value: &serde_yaml::Value) -> anyhow::Result<()> {
    let volumes = value
        .as_mapping()
        .context("compose top-level volumes must be a mapping")?;
    for (name, volume) in volumes {
        let Some(volume_name) = name.as_str() else {
            bail!("compose volume names must be strings");
        };
        match volume {
            serde_yaml::Value::Null => {}
            serde_yaml::Value::Mapping(mapping) => {
                for key in hostlet_contracts::compose::FORBIDDEN_TOP_LEVEL_VOLUME_FIELDS
                    .iter()
                    .copied()
                {
                    if yaml_contains_key(mapping, key) {
                        bail!("compose volume {volume_name} uses unsupported field {key}");
                    }
                }
            }
            _ => bail!("compose volume {volume_name} must be an object"),
        }
    }
    Ok(())
}

fn is_docker_socket_path(value: &str) -> bool {
    value == "/var/run/docker.sock"
}

fn is_host_bind_source(value: &str) -> bool {
    value.starts_with('/') || value.starts_with('.') || value.contains('/') || value.contains('\\')
}

fn yaml_get_mut<'a>(
    mapping: &'a mut serde_yaml::Mapping,
    key: &str,
) -> Option<&'a mut serde_yaml::Value> {
    mapping.get_mut(serde_yaml::Value::String(key.to_string()))
}

/// A relative, within-repo host bind (`./data`, `cache/x`) that Hostlet can move
/// onto a managed named volume. Absolute paths, parent-escaping (`..`) paths, and
/// the Docker socket are deliberately excluded so they keep failing the subset.
fn is_mappable_relative_bind(source: &str) -> bool {
    is_host_bind_source(source)
        && !source.starts_with('/')
        && !source.split('/').any(|part| part == "..")
}

/// A stable managed volume name derived from the container mount target, so the
/// same path keeps the same volume (and thus its data) across redeploys.
fn managed_volume_name(target: &str) -> String {
    let slug: String = target
        .trim_start_matches('/')
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "hostlet-data".to_string()
    } else {
        format!("hostlet-{slug}")
    }
}

/// Rewrites relative host bind mounts (e.g. `./data:/app/data`) to Hostlet-managed
/// named volumes mounted at the same container path, registering each under the
/// top-level `volumes:`. Repos that persist to a project-relative directory then
/// deploy unchanged while the host filesystem stays isolated. Absolute binds,
/// `..`-escaping paths, and the Docker socket are left intact so
/// [`validate_compose_subset`] still rejects them. Returns the input untouched
/// when there is nothing to remap.
pub(crate) fn remap_host_binds_to_named_volumes(contents: &str) -> anyhow::Result<String> {
    let mut value: serde_yaml::Value =
        serde_yaml::from_str(contents).context("compose file is not valid YAML")?;
    let mut added: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    {
        let Some(root) = value.as_mapping_mut() else {
            return Ok(contents.to_string());
        };
        if let Some(services) = yaml_get_mut(root, "services").and_then(|v| v.as_mapping_mut()) {
            for (_, service) in services.iter_mut() {
                let Some(service) = service.as_mapping_mut() else {
                    continue;
                };
                let Some(volumes) =
                    yaml_get_mut(service, "volumes").and_then(|v| v.as_sequence_mut())
                else {
                    continue;
                };
                for volume in volumes.iter_mut() {
                    let Some(text) = volume.as_str() else {
                        continue;
                    };
                    let mut parts = text.splitn(3, ':');
                    let source = parts.next().unwrap_or("");
                    let Some(target) = parts.next() else {
                        continue;
                    };
                    if !is_mappable_relative_bind(source) {
                        continue;
                    }
                    let name = managed_volume_name(target);
                    let replacement = match parts.next() {
                        Some(mode) => format!("{name}:{target}:{mode}"),
                        None => format!("{name}:{target}"),
                    };
                    *volume = serde_yaml::Value::String(replacement);
                    added.insert(name);
                }
            }
        }
    }
    if added.is_empty() {
        return Ok(contents.to_string());
    }
    let root = value
        .as_mapping_mut()
        .context("compose file must be a mapping")?;
    let volumes = root
        .entry(serde_yaml::Value::String("volumes".to_string()))
        .or_insert_with(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
    let volumes = volumes
        .as_mapping_mut()
        .context("compose top-level volumes must be a mapping")?;
    for name in added {
        let key = serde_yaml::Value::String(name);
        if !volumes.contains_key(&key) {
            volumes.insert(key, serde_yaml::Value::Null);
        }
    }
    serde_yaml::to_string(&value).context("failed to serialize remapped compose")
}

pub(crate) fn validate_compose_subset(contents: &str, web_service: &str) -> anyhow::Result<()> {
    let value: serde_yaml::Value =
        serde_yaml::from_str(contents).context("compose file is not valid YAML")?;
    if let Some(volumes) = value.get("volumes") {
        validate_compose_top_level_volumes(volumes)?;
    }
    let services = value
        .get("services")
        .and_then(|v| v.as_mapping())
        .context("compose file must define services")?;
    if !yaml_contains_key(services, web_service) {
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
        for key in hostlet_contracts::compose::FORBIDDEN_SERVICE_FIELDS
            .iter()
            .copied()
        {
            if yaml_contains_key(service, key) {
                bail!("compose service {service_name} uses unsupported field {key}");
            }
        }
        if let Some(volumes) = yaml_get(service, "volumes").and_then(|v| v.as_sequence()) {
            for volume in volumes {
                if let Some(value) = volume.as_str() {
                    let source = value.split(':').next().unwrap_or("");
                    if is_host_bind_source(source) {
                        bail!("compose service {service_name} uses an unsupported host bind mount");
                    }
                    if value.split(':').nth(1).is_some_and(is_docker_socket_path) {
                        bail!("compose service {service_name} mounts the Docker socket");
                    }
                    continue;
                }
                if let Some(mapping) = volume.as_mapping() {
                    let volume_type = yaml_get(mapping, "type")
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    let source = yaml_get(mapping, "source")
                        .or_else(|| yaml_get(mapping, "src"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    if volume_type == "bind" || is_host_bind_source(source) {
                        bail!("compose service {service_name} uses an unsupported host bind mount");
                    }
                    let target = yaml_get(mapping, "target")
                        .or_else(|| yaml_get(mapping, "dst"))
                        .or_else(|| yaml_get(mapping, "destination"))
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    if is_docker_socket_path(target) {
                        bail!("compose service {service_name} mounts the Docker socket");
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_relative_file_path(value: &str) -> anyhow::Result<()> {
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

pub(crate) fn validate_service_name(value: &str) -> anyhow::Result<()> {
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

pub(crate) fn env_args(p: &Value) -> Vec<String> {
    p.get("env")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("")))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn env_pairs_has_key(pairs: &[String], key: &str) -> bool {
    pairs
        .iter()
        .filter_map(|pair| pair.split_once('='))
        .any(|(existing, _)| existing == key)
}

/// The container path where the single-service managed data volume is mounted.
/// Apps that declare a persistent data path in their compose (e.g.
/// `./data:/app/data`) get the volume there; everything else defaults to `/data`.
/// An invalid declared path safely falls back to `/data`. Single source of truth
/// for both the volume mount target and the `HOSTLET_DATA_DIR`/`DATA_DIR` env.
pub(crate) fn data_mount_path(p: &Value) -> String {
    p.pointer("/runtime_config/dataMountPath")
        .and_then(|v| v.as_str())
        .filter(|path| hostlet_contracts::compose::valid_container_mount_path(path))
        .unwrap_or("/data")
        .to_string()
}

pub(crate) fn runtime_env_args(p: &Value, port: i64) -> Vec<String> {
    let mut pairs = env_args(p);
    if !env_pairs_has_key(&pairs, "PORT") {
        pairs.push(format!("PORT={port}"));
    }
    let data_dir = data_mount_path(p);
    if !env_pairs_has_key(&pairs, "HOSTLET_DATA_DIR") {
        pairs.push(format!("HOSTLET_DATA_DIR={data_dir}"));
    }
    if !env_pairs_has_key(&pairs, "DATA_DIR") {
        pairs.push(format!("DATA_DIR={data_dir}"));
    }
    pairs
}

/// Shared shape for Hostlet-managed Docker resource names: a required prefix,
/// a maximum length, and a per-character allowlist.
fn valid_prefixed_name(
    value: &str,
    prefix: &str,
    max_len: usize,
    char_allowed: impl Fn(char) -> bool,
) -> bool {
    value.starts_with(prefix) && value.len() <= max_len && value.chars().all(char_allowed)
}

pub(crate) fn valid_compose_project_name(value: &str) -> bool {
    ["hostlet-app-", "hostlet-release-"].iter().any(|prefix| {
        valid_prefixed_name(value, prefix, 64, |c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
        })
    })
}

pub(crate) fn valid_compose_volume_name(value: &str) -> bool {
    valid_prefixed_name(value, "hostlet-app-", 128, |c| {
        c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
    })
}

pub(crate) fn validate_repo(value: &str) -> anyhow::Result<()> {
    if !hostlet_contracts::valid_repo_full_name(value) {
        bail!("repo must be a GitHub owner/repo name");
    }
    Ok(())
}

pub(crate) fn validate_branch(value: &str) -> anyhow::Result<()> {
    if !hostlet_contracts::valid_branch(value) {
        bail!("branch name contains unsupported characters");
    }
    Ok(())
}

pub(crate) fn validate_commit_sha(value: &str) -> anyhow::Result<()> {
    if value == "HEAD" {
        return Ok(());
    }
    if value.len() == 40 && value.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(());
    }
    bail!("commit sha must be HEAD or a 40-character hex SHA");
}

pub(crate) fn validate_port(value: i64) -> anyhow::Result<()> {
    if !(1..=65_535).contains(&value) {
        bail!("container port must be between 1 and 65535");
    }
    Ok(())
}

pub(crate) fn validate_domain(value: &str) -> anyhow::Result<()> {
    if !hostlet_contracts::valid_domain(value) {
        bail!("domain must be a hostname with optional port");
    }
    Ok(())
}

pub(crate) fn validate_health_path(value: &str) -> anyhow::Result<()> {
    if !hostlet_contracts::valid_health_path(value) {
        bail!("health path must start with / and cannot contain control characters");
    }
    Ok(())
}

pub(crate) fn validate_dockerfile_command(value: &str) -> anyhow::Result<()> {
    if value.len() > 500 || value.chars().any(|c| matches!(c, '\n' | '\r' | '\0')) {
        bail!("commands cannot contain newlines, NUL bytes, or more than 500 characters");
    }
    Ok(())
}

#[cfg(test)]
#[path = "validation/tests.rs"]
mod tests;
