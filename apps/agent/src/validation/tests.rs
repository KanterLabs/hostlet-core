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
    let prefixes = vec!["/api".to_string(), "/graphql".to_string()];
    let split = render_caddy_split_route("app", "app.example.com", 12345, 12346, &prefixes);
    assert!(split.contains("handle @hostletWebsocket"));
    assert!(split.contains("path /api /api/* /graphql /graphql/*"));
    assert!(split.contains("reverse_proxy 127.0.0.1:12345"));
    assert!(split.contains("reverse_proxy 127.0.0.1:12346"));
    let local = render_local_caddy_split_route("app", "app.example.com", 12345, 12346, &prefixes);
    assert!(local.contains("header Connection *Upgrade*"));
    assert!(local.contains("path /api /api/* /graphql /graphql/*"));
    assert!(local.contains("reverse_proxy @appBackend 127.0.0.1:12346"));
    assert!(local.contains("reverse_proxy @appFrontend 127.0.0.1:12345"));
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
fn compose_named_volumes_use_stable_project_names() {
    let compose = "services:\n  web:\n    image: app\nvolumes:\n  cache-data:\n  app-data:\n";
    assert_eq!(
        compose_named_volume_names(compose, "hostlet-app-123").unwrap(),
        vec![
            "hostlet-app-123_app-data".to_string(),
            "hostlet-app-123_cache-data".to_string()
        ]
    );
}

#[test]
fn remap_preserves_the_mode_suffix() {
    let compose = "services:\n  web:\n    build: .\n    volumes:\n      - ./cache:/app/cache:ro\n";
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
        data_mount_path(&serde_json::json!({"runtime_config": {"dataMountPath": "relative/path"}})),
        "/data"
    );
    assert_eq!(
        data_mount_path(&serde_json::json!({"runtime_config": {"dataMountPath": "/"}})),
        "/data"
    );
}
