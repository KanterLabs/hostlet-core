use super::*;

mod docker_resources;
mod redaction;

pub(crate) use docker_resources::*;
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

pub(crate) fn safe_name(s: &str) -> String {
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

pub(crate) fn compose_project_name(app_id: Uuid) -> String {
    format!("hostlet-app-{}", app_id.simple())
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
            if valid_env_key(key) {
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

pub(crate) fn compose_override_yaml(
    web_service: &str,
    port: u16,
    app_id: Uuid,
    deployment_id: Uuid,
    payload: &Value,
) -> String {
    let env_yaml = compose_override_env_block(app_id, deployment_id, payload);
    format!(
        "services:\n  {web_service}:\n    labels:\n      hostlet.app_id: \"{app_id}\"\n      hostlet.deployment_id: \"{deployment_id}\"\n      hostlet.role: \"web\"\n    environment:\n{env_yaml}\n    security_opt:\n      - no-new-privileges:true\n    cap_drop:\n      - ALL\n    pids_limit: 256\n    ports:\n      - target: {port}\n        host_ip: 127.0.0.1\n        protocol: tcp\n"
    )
}

/// Look up a string key in a YAML mapping without repeatedly allocating a
/// `serde_yaml::Value::String` at every call site.
fn yaml_get<'a>(mapping: &'a serde_yaml::Mapping, key: &str) -> Option<&'a serde_yaml::Value> {
    mapping.get(serde_yaml::Value::String(key.to_string()))
}

fn yaml_contains_key(mapping: &serde_yaml::Mapping, key: &str) -> bool {
    mapping.contains_key(serde_yaml::Value::String(key.to_string()))
}

pub(crate) fn validate_compose_subset(contents: &str, web_service: &str) -> anyhow::Result<()> {
    let value: serde_yaml::Value =
        serde_yaml::from_str(contents).context("compose file is not valid YAML")?;
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
            if yaml_contains_key(service, key) {
                bail!("compose service {service_name} uses unsupported field {key}");
            }
        }
        if let Some(volumes) = yaml_get(service, "volumes").and_then(|v| v.as_sequence()) {
            for volume in volumes {
                if let Some(value) = volume.as_str() {
                    if value.starts_with('/') || value.contains("../") {
                        bail!("compose service {service_name} uses an unsupported host bind mount");
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
                    if volume_type == "bind" || source.starts_with('/') || source.contains("../") {
                        bail!("compose service {service_name} uses an unsupported host bind mount");
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

pub(crate) fn valid_env_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
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

pub(crate) fn runtime_env_args(p: &Value, port: i64) -> Vec<String> {
    let mut pairs = env_args(p);
    if !env_pairs_has_key(&pairs, "PORT") {
        pairs.push(format!("PORT={port}"));
    }
    if !env_pairs_has_key(&pairs, "HOSTLET_DATA_DIR") {
        pairs.push("HOSTLET_DATA_DIR=/data".into());
    }
    if !env_pairs_has_key(&pairs, "DATA_DIR") {
        pairs.push("DATA_DIR=/data".into());
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
    valid_prefixed_name(value, "hostlet-app-", 64, |c| {
        c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
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
