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
    fn runtime_env_args_injects_port_and_data_dirs() {
        let args = runtime_env_args(&serde_json::json!({"env":{"APP_ENV":"test"}}), 4173);
        assert!(args.contains(&"APP_ENV=test".to_string()));
        assert!(args.contains(&"PORT=4173".to_string()));
        assert!(args.contains(&"HOSTLET_DATA_DIR=/data".to_string()));
        assert!(args.contains(&"DATA_DIR=/data".to_string()));
    }

    #[test]
    fn runtime_env_args_preserves_user_port() {
        let args = runtime_env_args(&serde_json::json!({"env":{"PORT":"9000"}}), 4173);
        assert!(args.contains(&"PORT=9000".to_string()));
        assert!(!args.contains(&"PORT=4173".to_string()));
    }

    #[test]
    fn backing_services_get_caps_when_runtime_config_provides_them() {
        let override_yaml = compose_override_yaml(
            "services:\n  web:\n    build: .\n  db:\n    image: postgres:16\n",
            "web",
            3000,
            Uuid::nil(),
            Uuid::nil(),
            &serde_json::json!({
                "runtime_config": {"compose": {"backingMemoryLimitMb": 256, "backingCpuLimit": 0.25}}
            }),
        );
        // The backing service is capped; the web service is not (it gets its own
        // per-app --memory/--cpus from the single-service runtime).
        assert!(override_yaml.contains("mem_limit: 256m"));
        assert!(override_yaml.contains("cpus: \"0.25\""));
        assert_eq!(override_yaml.matches("mem_limit:").count(), 1);
        assert_eq!(override_yaml.matches("cpus:").count(), 1);
    }

    #[test]
    fn backing_services_have_no_caps_without_runtime_config() {
        let override_yaml = compose_override_yaml(
            "services:\n  web:\n    build: .\n  db:\n    image: postgres:16\n",
            "web",
            3000,
            Uuid::nil(),
            Uuid::nil(),
            &serde_json::json!({}),
        );
        assert!(!override_yaml.contains("mem_limit:"));
        assert!(!override_yaml.contains("cpus:"));
    }

    #[test]
    fn rejects_bad_job_signature() {
        assert!(!verify_signature("secret", b"{}", "sha256=bad"));
    }

    #[test]
    fn packaging_strategy_defaults_to_auto() {
        assert!(matches!(
            PackagingStrategy::from_payload(&serde_json::json!({})).unwrap(),
            PackagingStrategy::Auto
        ));
        assert!(matches!(
            PackagingStrategy::from_payload(&serde_json::json!({"packaging_strategy":"generated"}))
                .unwrap(),
            PackagingStrategy::Generated
        ));
    }

    #[test]
    fn buildx_args_use_local_cache_and_load() {
        let args = buildx_args(
            "hostlet/app:test",
            "/tmp/Dockerfile",
            "/tmp/app",
            "type=local,src=/tmp/cache",
            "type=local,dest=/tmp/cache-next,mode=max",
        );
        assert!(args.contains(&"buildx"));
        assert!(args.contains(&"--load"));
        assert!(args.contains(&"--cache-from"));
        assert!(args.contains(&"--cache-to"));
    }

    #[test]
    fn app_ports_bind_to_loopback_only() {
        assert_eq!(docker_port_map(3000), "127.0.0.1::3000");
        let override_yaml = compose_override_yaml(
            "services:\n  web:\n    build: .\n  db:\n    image: postgres:16\n",
            "web",
            3000,
            Uuid::nil(),
            Uuid::nil(),
            &serde_json::json!({}),
        );
        assert!(override_yaml.contains("host_ip: 127.0.0.1"));
        assert!(!override_yaml.contains("host_ip: 0.0.0.0"));
        assert!(override_yaml.contains("no-new-privileges:true"));
        assert!(override_yaml.contains("cap_drop:\n      - ALL"));
        assert!(override_yaml.contains("pids_limit: 256"));
        // Every service is hardened, but only the web service publishes a port
        // (to loopback) and only it drops all caps; the backing service keeps
        // Docker's default cap set so real database images can initialize.
        assert!(override_yaml.contains("hostlet.role: \"backing\""));
        assert_eq!(override_yaml.matches("ports:").count(), 1);
        assert_eq!(override_yaml.matches("pids_limit: 512").count(), 1);
        // no-new-privileges applies to both services (web + db).
        assert_eq!(override_yaml.matches("no-new-privileges:true").count(), 2);
    }

    #[test]
    fn compose_override_env_block_enforces_strict_contract_env_keys() {
        // The compose-override env block is the last gate before payload env
        // keys are rendered into YAML, so it must apply the strict contracts
        // rule (UPPERCASE | digit | _). A lowercase key the contract rejects
        // must be dropped, while a canonical UPPERCASE key is kept.
        let block = compose_override_env_block(
            Uuid::nil(),
            Uuid::nil(),
            &serde_json::json!({"env": {"lowercase": "x", "VALID_KEY": "y"}}),
        );
        assert!(block.contains("VALID_KEY=y"));
        assert!(!block.contains("lowercase=x"));
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
        let relative_bind_mount = r#"
services:
  web:
    build: .
    volumes:
      - ./data:/app/data
"#;
        assert!(validate_compose_subset(relative_bind_mount, "web").is_err());
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
        let long_relative_bind_mount = r#"
services:
  web:
    build: .
    volumes:
      - type: volume
        source: data/cache
        target: /app/data
"#;
        assert!(validate_compose_subset(long_relative_bind_mount, "web").is_err());
        let service_network = r#"
services:
  web:
    build: .
    networks:
      - hostlet
"#;
        assert!(validate_compose_subset(service_network, "web").is_err());
    }

    #[test]
    fn compose_validation_rejects_host_backed_named_volumes_and_socket_targets() {
        let driver_opts = r#"
services:
  web:
    build: .
    volumes:
      - host-root:/mnt/host
volumes:
  host-root:
    driver: local
    driver_opts:
      type: none
      o: bind
      device: /
"#;
        assert!(validate_compose_subset(driver_opts, "web").is_err());

        let socket_target = r#"
services:
  web:
    build: .
    volumes:
      - docker-sock:/var/run/docker.sock
volumes:
  docker-sock:
"#;
        assert!(validate_compose_subset(socket_target, "web").is_err());

        let long_socket_target = r#"
services:
  web:
    build: .
    volumes:
      - type: volume
        source: docker-sock
        target: /var/run/docker.sock
volumes:
  docker-sock:
"#;
        assert!(validate_compose_subset(long_socket_target, "web").is_err());
    }

    #[test]
    fn remap_moves_relative_bind_to_named_volume_and_passes_subset() {
        // Mirrors homebase: a single web service persisting to ./data.
        let compose = r#"
services:
  web:
    build: .
    volumes:
      - ./data:/app/data
"#;
        let remapped = remap_host_binds_to_named_volumes(compose).unwrap();
        // The relative host bind is gone; a managed named volume took its place.
        assert!(!remapped.contains("./data"));
        assert!(remapped.contains("hostlet-app-data:/app/data"));
        // The named volume is registered at the top level...
        let value: serde_yaml::Value = serde_yaml::from_str(&remapped).unwrap();
        assert!(value
            .get("volumes")
            .and_then(|v| v.as_mapping())
            .is_some_and(|m| yaml_contains_key(m, "hostlet-app-data")));
        // ...and the result now satisfies the very gate that rejected the bind.
        validate_compose_subset(&remapped, "web").unwrap();
    }

    #[test]
    fn remap_preserves_the_mode_suffix() {
        let compose =
            "services:\n  web:\n    build: .\n    volumes:\n      - ./cache:/app/cache:ro\n";
        let remapped = remap_host_binds_to_named_volumes(compose).unwrap();
        assert!(remapped.contains("hostlet-app-cache:/app/cache:ro"));
    }

    #[test]
    fn remap_leaves_absolute_binds_and_socket_for_the_subset_to_reject() {
        let absolute = "services:\n  web:\n    build: .\n    volumes:\n      - /etc:/host-etc\n";
        let remapped = remap_host_binds_to_named_volumes(absolute).unwrap();
        // Untouched, so the subset gate still rejects it.
        assert!(remapped.contains("/etc:/host-etc"));
        assert!(validate_compose_subset(&remapped, "web").is_err());

        let escaping = "services:\n  web:\n    build: .\n    volumes:\n      - ../secrets:/app/s\n";
        let remapped = remap_host_binds_to_named_volumes(escaping).unwrap();
        assert!(remapped.contains("../secrets"));
        assert!(validate_compose_subset(&remapped, "web").is_err());
    }

    #[test]
    fn remap_is_a_noop_for_named_volumes() {
        let compose = "services:\n  web:\n    build: .\n    volumes:\n      - app-data:/data\nvolumes:\n  app-data:\n";
        let remapped = remap_host_binds_to_named_volumes(compose).unwrap();
        assert_eq!(remapped, compose);
    }

    #[test]
    fn remap_volume_name_is_stable_for_the_same_target() {
        // Persistence depends on the same mount target always yielding the same
        // managed volume name across redeploys.
        let a = remap_host_binds_to_named_volumes(
            "services:\n  web:\n    build: .\n    volumes:\n      - ./data:/app/data\n",
        )
        .unwrap();
        let b = remap_host_binds_to_named_volumes(
            "services:\n  web:\n    build: .\n    volumes:\n      - ./data:/app/data\n",
        )
        .unwrap();
        assert!(a.contains("hostlet-app-data"));
        assert_eq!(a, b);
    }

    #[test]
    fn data_mount_path_honors_declared_path_and_defaults_to_data() {
        assert_eq!(data_mount_path(&serde_json::json!({})), "/data");
        assert_eq!(
            data_mount_path(&serde_json::json!({"runtime_config": {"dataMountPath": "/app/data"}})),
            "/app/data"
        );
        // An invalid declared path safely falls back to /data.
        assert_eq!(
            data_mount_path(
                &serde_json::json!({"runtime_config": {"dataMountPath": "relative/path"}})
            ),
            "/data"
        );
        assert_eq!(
            data_mount_path(&serde_json::json!({"runtime_config": {"dataMountPath": "/"}})),
            "/data"
        );
    }
}
