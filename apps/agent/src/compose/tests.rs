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
fn compose_health_failure_message_reports_cleanup_result() {
    let health_err = anyhow::anyhow!("timeout");
    let cleanup_err = anyhow::anyhow!("docker failed");

    let cleaned = compose_health_failure_message(&health_err, None);
    assert!(cleaned.contains("Removed the unhealthy Compose project"));
    assert!(cleaned.contains("preserved the previous working route"));

    let failed = compose_health_failure_message(&health_err, Some(&cleanup_err));
    assert!(failed.contains("Failed to remove the unhealthy Compose project"));
    assert!(failed.contains("docker failed"));
}

#[tokio::test]
async fn harden_host_command_env_strips_inherited_agent_secrets() {
    // Simulate the agent process holding its host-wide secret, then confirm
    // a hardened Docker/Compose spawn cannot see it: `env_clear` must wipe
    // the inherited environment so tenant `${HOSTLET_AGENT_TOKEN}`
    // interpolation resolves to empty instead of the real token.
    std::env::set_var("HOSTLET_AGENT_TOKEN", "super-secret-token");

    let mut cmd = Command::new("sh");
    harden_host_command_env(&mut cmd);
    cmd.args(["-c", "printf '%s' \"${HOSTLET_AGENT_TOKEN:-}\""]);
    let output = cmd.output().await.expect("spawn sh");

    std::env::remove_var("HOSTLET_AGENT_TOKEN");

    assert!(output.status.success(), "sh probe should exit cleanly");
    assert!(
        output.stdout.is_empty(),
        "HOSTLET_AGENT_TOKEN leaked into the child env: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn compose_interpolation_env_rejects_host_process_control_keys() {
    let env = compose_interpolation_env(&json!({
        "env": {
            "DATABASE_URL": "postgres://db",
            "PATH": ".:/usr/bin",
            "LD_PRELOAD": "/tmp/hook.so",
            "DOCKER_HOST": "tcp://attacker",
            "COMPOSE_FILE": "owned.yml"
        }
    }));

    assert_eq!(
        env,
        vec![("DATABASE_URL".to_string(), "postgres://db".to_string())]
    );
}

#[test]
fn docker_cleanup_removes_unkept_hostlet_app_containers() {
    let keep_containers = HashSet::from(["hostlet-app-current".to_string()]);

    let should_remove =
        cleanup_should_remove_container("hostlet-app-stale-restarting", &keep_containers, false)
            .unwrap();

    assert!(should_remove);
}

#[test]
fn docker_cleanup_keeps_protected_and_compose_containers() {
    let keep_containers = HashSet::from(["hostlet-app-current".to_string()]);

    assert!(
        !cleanup_should_remove_container("hostlet-app-current", &keep_containers, false).unwrap()
    );
    assert!(
        !cleanup_should_remove_container("hostlet-app-compose", &keep_containers, true).unwrap()
    );
}

#[test]
fn docker_cleanup_rejects_invalid_unkept_container_names() {
    let err = cleanup_should_remove_container("hostlet-app-bad name", &HashSet::new(), false)
        .unwrap_err();

    assert!(err
        .to_string()
        .contains("refusing to clean invalid managed container name"));
}

#[test]
fn docker_cleanup_never_touches_non_deployment_containers() {
    // Cleanup runs automatically after every successful deploy; on a shared
    // docker daemon (CI runner) these belong to other concurrent work.
    for name in [
        "hostlet-railpack-buildkit",
        "hostlet-railpack-buildkit-ci-27372703297-602616",
        "hostlet-ci-self-api-postgres-27372659875-576983",
        "hostlet-caddy",
        "not-hostlet-app",
    ] {
        assert!(
            !cleanup_should_remove_container(name, &HashSet::new(), false).unwrap(),
            "{name} must never be reaped"
        );
    }
}

#[test]
fn docker_cleanup_image_predicate_only_targets_deployment_images() {
    let keep = HashSet::from(["hostlet/app-keep:current".to_string()]);

    assert!(cleanup_should_remove_image("hostlet/app-stale:old", &keep).unwrap());
    assert!(!cleanup_should_remove_image("hostlet/app-keep:current", &keep).unwrap());
    for image in [
        "hostlet/railpack-fixture-go:27372703297-602616",
        "hostlet/builder-base:latest",
        "ghcr.io/shanekanterman04/hostlet-api:v0.2.7",
    ] {
        assert!(
            !cleanup_should_remove_image(image, &keep).unwrap(),
            "{image} must never be reaped"
        );
    }
}
