use super::*;

#[derive(Clone)]
pub(super) struct HealthTarget {
    pub(super) app_id: Uuid,
    pub(super) deployment_id: Uuid,
    pub(super) container_name: String,
    container_port: u16,
    pub(crate) published_port: u16,
    health_path: String,
    domain: Option<String>,
    route_key: Option<String>,
    route_generation: Option<i64>,
}

pub(super) fn health_status_event(
    target: &HealthTarget,
    result: &HealthProbeResult,
    status: &str,
    failure_count: u32,
    success_count: u32,
) -> Value {
    json!({
        "type": "health_status",
        "app_id": target.app_id,
        "deployment_id": target.deployment_id,
        "container_name": target.container_name,
        "published_port": target.published_port,
        "status": status,
        "checked_url": result.url,
        "http_status": result.http_status,
        "latency_ms": result.latency_ms,
        "failure_count": failure_count,
        "success_count": success_count,
        "error": result.error,
    })
}

pub(super) fn single_probe_health_event(
    target: &HealthTarget,
    result: &HealthProbeResult,
) -> Value {
    let status = if result.healthy {
        "healthy"
    } else {
        "degraded"
    };
    let (failure_count, success_count) = if result.healthy { (0, 1) } else { (1, 0) };
    health_status_event(target, result, status, failure_count, success_count)
}

pub(super) async fn health_targets(cfg: &Config) -> anyhow::Result<Vec<HealthTarget>> {
    let raw = cfg
        .http
        .get(format!("{}/api/agent/health-targets", cfg.api_url))
        .header("x-hostlet-server-id", cfg.server_id.to_string())
        .header("x-hostlet-agent-token", &cfg.agent_token)
        .send()
        .await?
        .error_for_status()?
        .json::<Vec<Value>>()
        .await?;
    Ok(raw
        .iter()
        .filter_map(health_target_from_payload)
        .collect::<Vec<_>>())
}

pub(super) fn health_target_from_payload(value: &Value) -> Option<HealthTarget> {
    let app_id = value
        .get("appId")
        .or_else(|| value.get("app_id"))
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok())?;
    let deployment_id = value
        .get("deploymentId")
        .or_else(|| value.get("deployment_id"))
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok())?;
    let container_name = value
        .get("containerName")
        .or_else(|| value.get("container_name"))
        .and_then(|v| v.as_str())?
        .to_string();
    if !valid_container_name(&container_name) {
        return None;
    }
    let published_port = value
        .get("publishedPort")
        .or_else(|| value.get("published_port"))
        .and_then(|v| v.as_i64())
        .and_then(|v| (1..=65_535).contains(&v).then_some(v as u16))?;
    let container_port = value
        .get("containerPort")
        .or_else(|| value.get("container_port"))
        .and_then(|v| v.as_i64())
        .and_then(|v| (1..=65_535).contains(&v).then_some(v as u16))
        .unwrap_or(published_port);
    let health_path = value
        .get("healthPath")
        .or_else(|| value.get("health_path"))
        .and_then(|v| v.as_str())
        .unwrap_or("/");
    if validate_health_path(health_path).is_err() {
        return None;
    }
    let domain = value
        .get("domain")
        .and_then(|v| v.as_str())
        .filter(|value| validate_domain(value).is_ok())
        .map(str::to_string);
    let route_key = value
        .get("routeKey")
        .or_else(|| value.get("route_key"))
        .and_then(|v| v.as_str())
        .and_then(clean_route_key);
    let route_generation = value
        .get("routeGeneration")
        .or_else(|| value.get("route_generation"))
        .and_then(Value::as_i64);
    Some(HealthTarget {
        app_id,
        deployment_id,
        container_name,
        container_port,
        published_port,
        health_path: health_path.to_string(),
        domain,
        route_key,
        route_generation,
    })
}

fn clean_route_key(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || app_slug(trimmed) != trimmed {
        return None;
    }
    Some(trimmed.to_string())
}

pub(super) struct HealthProbeResult {
    pub(super) healthy: bool,
    pub(super) url: String,
    pub(super) http_status: Option<u16>,
    pub(super) latency_ms: u128,
    pub(super) error: Option<String>,
    pub(super) container_state: Option<ContainerState>,
}

pub(super) async fn probe_health_target(
    cfg: &Config,
    target: &mut HealthTarget,
) -> HealthProbeResult {
    let url = health_url(cfg, target);
    let started = Instant::now();
    let container_state = match container_state(&target.container_name).await {
        Ok(state) => state,
        Err(err) => {
            return HealthProbeResult {
                healthy: false,
                url,
                http_status: None,
                latency_ms: started.elapsed().as_millis(),
                error: Some(err.to_string()),
                container_state: None,
            };
        }
    };
    if container_state != ContainerState::Running {
        return HealthProbeResult {
            healthy: false,
            url,
            http_status: None,
            latency_ms: started.elapsed().as_millis(),
            error: Some(container_state.error_message()),
            container_state: Some(container_state),
        };
    }
    if let Err(err) = refresh_published_port(cfg, target).await {
        return HealthProbeResult {
            healthy: false,
            url,
            http_status: None,
            latency_ms: started.elapsed().as_millis(),
            error: Some(err.to_string()),
            container_state: Some(container_state),
        };
    }
    let url = health_url(cfg, target);
    match cfg
        .http
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            HealthProbeResult {
                healthy: status.is_success() || status.is_redirection(),
                url,
                http_status: Some(status.as_u16()),
                latency_ms: started.elapsed().as_millis(),
                error: health_error_for_status(status),
                container_state: Some(container_state),
            }
        }
        Err(err) => HealthProbeResult {
            healthy: false,
            url,
            http_status: None,
            latency_ms: started.elapsed().as_millis(),
            error: Some(err.to_string()),
            container_state: Some(container_state),
        },
    }
}

async fn refresh_published_port(cfg: &Config, target: &mut HealthTarget) -> anyhow::Result<()> {
    let actual = docker_published_port(&target.container_name, target.container_port).await?;
    if !published_port_changed(target.published_port, actual) {
        return Ok(());
    }
    log(
        cfg,
        target.deployment_id,
        "stdout",
        &format!(
            "Detected Docker-published port drift for {}; updating route from {} to {}.",
            target.container_name, target.published_port, actual
        ),
    )
    .await;
    refresh_route(cfg, target, actual).await?;
    target.published_port = actual;
    Ok(())
}

fn published_port_changed(stored: u16, actual: u16) -> bool {
    stored != actual
}

async fn refresh_route(cfg: &Config, target: &HealthTarget, port: u16) -> anyhow::Result<()> {
    let Some(route_key) = target.route_key.as_deref() else {
        return Ok(());
    };
    let Some(domain) = target.domain.as_deref() else {
        return Ok(());
    };
    if cfg.local_mode {
        if let Some(router) = &cfg.local_router {
            return match target.route_generation {
                Some(generation) => {
                    apply_local_caddy_route_versioned(
                        cfg,
                        target.deployment_id,
                        router,
                        route_key,
                        domain,
                        port,
                        generation,
                    )
                    .await
                }
                None => {
                    apply_local_caddy_route(
                        cfg,
                        target.deployment_id,
                        router,
                        route_key,
                        domain,
                        port,
                    )
                    .await
                }
            };
        }
        return Ok(());
    }
    match target.route_generation {
        Some(generation) => {
            apply_caddy_route_versioned(
                cfg,
                target.deployment_id,
                route_key,
                domain,
                port,
                generation,
            )
            .await
        }
        None => apply_caddy_route(cfg, target.deployment_id, route_key, domain, port).await,
    }
}

pub(super) fn failed_health_probe(
    cfg: &Config,
    target: &HealthTarget,
    error: String,
) -> HealthProbeResult {
    HealthProbeResult {
        healthy: false,
        url: health_url(cfg, target),
        http_status: None,
        latency_ms: 0,
        error: Some(error),
        container_state: None,
    }
}

fn health_url(cfg: &Config, target: &HealthTarget) -> String {
    format!(
        "http://{}:{}{}",
        cfg.health_host, target.published_port, target.health_path
    )
}

fn health_error_for_status(status: StatusCode) -> Option<String> {
    if status.is_success() || status.is_redirection() {
        None
    } else {
        Some(format!("HTTP {status}"))
    }
}

pub(crate) const CONTAINER_STATE_INSPECT_FORMAT: &str =
    "{{.State.Running}} {{.State.Restarting}} {{.State.OOMKilled}} {{.State.ExitCode}}";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ContainerState {
    Running,
    Restarting(String),
    Stopped(String),
    OomKilled,
    Missing,
}

impl ContainerState {
    pub(super) fn error_message(&self) -> String {
        match self {
            Self::Running => String::new(),
            Self::Restarting(exit_code) => {
                format!("container is restarting after exit code {exit_code}")
            }
            Self::Stopped(exit_code) => {
                format!("container is not running; last exit code {exit_code}")
            }
            Self::OomKilled => "container was OOM-killed".into(),
            Self::Missing => "container does not exist".into(),
        }
    }
}

async fn container_state(container: &str) -> anyhow::Result<ContainerState> {
    let output = command_output(
        "docker",
        &["inspect", "-f", CONTAINER_STATE_INSPECT_FORMAT, container],
        Duration::from_secs(10),
    )
    .await?;
    if !output.status.success() {
        return Ok(ContainerState::Missing);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    inspect_container_state(stdout.trim()).context("docker inspect returned malformed state")
}

fn inspect_container_state(value: &str) -> Option<ContainerState> {
    let mut parts = value.split_whitespace();
    let running = parts.next()?;
    let restarting = parts.next()?;
    let oom_killed = parts.next()?;
    let exit_code = parts.next().unwrap_or("unknown");
    if restarting == "true" {
        return Some(ContainerState::Restarting(exit_code.to_string()));
    }
    if oom_killed == "true" {
        return Some(ContainerState::OomKilled);
    }
    if running == "true" {
        Some(ContainerState::Running)
    } else {
        Some(ContainerState::Stopped(exit_code.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_container_state_accepts_running_container() {
        assert_eq!(
            inspect_container_state("true false false 0"),
            Some(ContainerState::Running)
        );
    }

    #[test]
    fn inspect_container_state_reports_restart_loop() {
        let state = inspect_container_state("true true false 1").unwrap();

        assert_eq!(
            state.error_message(),
            "container is restarting after exit code 1"
        );
    }

    #[test]
    fn inspect_container_state_reports_oom_kill() {
        let state = inspect_container_state("false false true 137").unwrap();

        assert_eq!(state.error_message(), "container was OOM-killed");
    }

    #[test]
    fn inspect_container_state_reports_stopped_exit_code() {
        let state = inspect_container_state("false false false 2").unwrap();

        assert_eq!(
            state.error_message(),
            "container is not running; last exit code 2"
        );
    }

    #[test]
    fn inspect_container_state_rejects_malformed_output() {
        assert_eq!(inspect_container_state(""), None);
        assert_eq!(inspect_container_state("true false"), None);
    }

    #[test]
    fn health_target_payload_accepts_route_metadata() {
        let app_id = Uuid::from_u128(1);
        let deployment_id = Uuid::from_u128(2);
        let target = health_target_from_payload(&json!({
            "appId": app_id,
            "deploymentId": deployment_id,
            "containerName": "hostlet-app-demo",
            "containerPort": 3000,
            "publishedPort": 32000,
            "healthPath": "/health",
            "domain": "demo.example.com",
            "routeKey": "app-00000000-0000-0000-0000-000000000001"
        }))
        .unwrap();

        assert_eq!(target.domain.as_deref(), Some("demo.example.com"));
        assert_eq!(
            target.route_key.as_deref(),
            Some("app-00000000-0000-0000-0000-000000000001")
        );
    }

    #[test]
    fn health_target_payload_rejects_invalid_route_metadata_without_rejecting_target() {
        let app_id = Uuid::from_u128(1);
        let deployment_id = Uuid::from_u128(2);
        let target = health_target_from_payload(&json!({
            "app_id": app_id,
            "deployment_id": deployment_id,
            "container_name": "hostlet-app-demo",
            "container_port": 3000,
            "published_port": 32000,
            "health_path": "/health",
            "domain": "not a domain",
            "route_key": "../../bad"
        }))
        .unwrap();

        assert_eq!(target.domain, None);
        assert_eq!(target.route_key, None);
    }

    #[test]
    fn published_port_changed_detects_drift_only() {
        assert!(!published_port_changed(32000, 32000));
        assert!(published_port_changed(32000, 32001));
    }
}
