use super::*;

#[derive(Clone)]
pub(super) struct HealthTarget {
    pub(super) app_id: Uuid,
    pub(super) deployment_id: Uuid,
    pub(super) container_name: String,
    container_port: u16,
    pub(crate) published_port: u16,
    health_path: String,
    tcp_probe: bool,
    domain: Option<String>,
    route_key: Option<String>,
    route_generation: Option<i64>,
    split_route: Option<SplitRoute>,
}

#[derive(Clone)]
struct SplitRoute {
    backend_container_name: String,
    backend_container_port: u16,
    backend_published_port: u16,
    backend_path_prefixes: Vec<String>,
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
        "backend_container_name": target
            .split_route
            .as_ref()
            .map(|route| route.backend_container_name.clone()),
        "backend_published_port": target
            .split_route
            .as_ref()
            .map(|route| route.backend_published_port),
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

fn select_current_health_target(
    payload: &Value,
    targets: Vec<HealthTarget>,
) -> Option<HealthTarget> {
    let app_id = payload
        .get("app_id")
        .or_else(|| payload.get("appId"))
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())?;
    let deployment_id = payload
        .get("deployment_id")
        .or_else(|| payload.get("deploymentId"))
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())?;
    select_health_target_by_id(app_id, deployment_id, targets)
}

fn select_health_target_by_id(
    app_id: Uuid,
    deployment_id: Uuid,
    targets: Vec<HealthTarget>,
) -> Option<HealthTarget> {
    targets
        .into_iter()
        .find(|target| target.app_id == app_id && target.deployment_id == deployment_id)
}

/// Resolve interactive jobs through the API's current, canonical target list.
/// Job payloads can wait in the queue while a deployment changes and older
/// payload schemas do not carry split-route facts, so they are never trusted
/// for a route-affecting probe.
pub(super) async fn current_health_target(
    cfg: &Config,
    payload: &Value,
) -> anyhow::Result<Option<HealthTarget>> {
    Ok(select_current_health_target(
        payload,
        health_targets(cfg).await?,
    ))
}

async fn current_health_target_by_id(
    cfg: &Config,
    app_id: Uuid,
    deployment_id: Uuid,
) -> anyhow::Result<Option<HealthTarget>> {
    Ok(select_health_target_by_id(
        app_id,
        deployment_id,
        health_targets(cfg).await?,
    ))
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
    let tcp_probe = value
        .get("probeKind")
        .or_else(|| value.get("probe_kind"))
        .and_then(Value::as_str)
        == Some("tcp");
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
        .and_then(Value::as_i64)
        .filter(|generation| *generation >= 0);
    let split_route = match value.get("splitRoute").or_else(|| value.get("split_route")) {
        None => None,
        Some(value) => Some(parse_split_route(value)?),
    };
    if split_route.is_some()
        && (domain.is_none() || route_key.is_none() || route_generation.is_none())
    {
        return None;
    }
    Some(HealthTarget {
        app_id,
        deployment_id,
        container_name,
        container_port,
        published_port,
        health_path: health_path.to_string(),
        tcp_probe,
        domain,
        route_key,
        route_generation,
        split_route,
    })
}

fn parse_split_route(value: &Value) -> Option<SplitRoute> {
    let backend = value.get("backend")?;
    let backend_container_name = backend
        .get("containerName")
        .or_else(|| backend.get("container_name"))
        .and_then(Value::as_str)?
        .to_string();
    if !valid_container_name(&backend_container_name) {
        return None;
    }
    let backend_container_port = bounded_port(backend, "targetPort", "target_port")?;
    let backend_published_port = bounded_port(backend, "publishedPort", "published_port")?;
    let backend_path_prefixes = parse_backend_path_prefixes(
        value
            .get("backendPathPrefixes")
            .or_else(|| value.get("backend_path_prefixes"))?,
    )?;
    Some(SplitRoute {
        backend_container_name,
        backend_container_port,
        backend_published_port,
        backend_path_prefixes,
    })
}

fn bounded_port(value: &Value, camel: &str, snake: &str) -> Option<u16> {
    value
        .get(camel)
        .or_else(|| value.get(snake))
        .and_then(Value::as_i64)
        .and_then(|value| (1..=65_535).contains(&value).then_some(value as u16))
}

fn parse_backend_path_prefixes(value: &Value) -> Option<Vec<String>> {
    let values = value.as_array()?;
    if values.len() > 16 {
        return None;
    }
    let mut prefixes = Vec::with_capacity(values.len());
    for value in values {
        let prefix = value.as_str()?.to_string();
        if !valid_backend_path_prefix(&prefix) || prefixes.iter().any(|item| item == &prefix) {
            return None;
        }
        prefixes.push(prefix);
    }
    Some(prefixes)
}

/// Keep this in lockstep with `GeneratedTopologyConfig`'s route-prefix
/// validation. Split-route facts are untrusted API input and must not allow a
/// Caddy matcher to escape the topology's path-prefix constraints.
fn valid_backend_path_prefix(value: &str) -> bool {
    value.starts_with('/')
        && value != "/"
        && value.len() <= 128
        && !value.ends_with('/')
        && !value.contains("..")
        && !value.contains('*')
        && !value.contains('?')
        && !value.contains('#')
        && !value.chars().any(|ch| ch.is_control() || ch == '\\')
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
    if let Err(err) = refresh_published_ports(cfg, target).await {
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
    if target.tcp_probe {
        return match tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::TcpStream::connect(("127.0.0.1", target.published_port)),
        )
        .await
        {
            Ok(Ok(_)) => HealthProbeResult {
                healthy: true,
                url,
                http_status: None,
                latency_ms: started.elapsed().as_millis(),
                error: None,
                container_state: Some(container_state),
            },
            Ok(Err(err)) => HealthProbeResult {
                healthy: false,
                url,
                http_status: None,
                latency_ms: started.elapsed().as_millis(),
                error: Some(err.to_string()),
                container_state: Some(container_state),
            },
            Err(_) => HealthProbeResult {
                healthy: false,
                url,
                http_status: None,
                latency_ms: started.elapsed().as_millis(),
                error: Some("TCP health probe timed out".into()),
                container_state: Some(container_state),
            },
        };
    }
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

async fn refresh_published_ports(cfg: &Config, target: &mut HealthTarget) -> anyhow::Result<()> {
    let actual_frontend =
        docker_published_port(&target.container_name, target.container_port).await?;
    let actual_backend = match target.split_route.as_ref() {
        Some(route) => Some(
            docker_published_port(&route.backend_container_name, route.backend_container_port)
                .await?,
        ),
        None => None,
    };
    if !split_route_ports_changed(target, actual_frontend, actual_backend) {
        return Ok(());
    }
    let Some(current) =
        current_health_target_by_id(cfg, target.app_id, target.deployment_id).await?
    else {
        bail!("health target no longer belongs to the current deployment");
    };
    if !same_route_owner(target, &current) {
        bail!("health route ownership changed while published ports were inspected");
    }
    if !split_route_ports_changed(&current, actual_frontend, actual_backend) {
        target.published_port = actual_frontend;
        if let (Some(actual), Some(route)) = (actual_backend, target.split_route.as_mut()) {
            route.backend_published_port = actual;
        }
        return Ok(());
    }
    let backend_summary = actual_backend
        .zip(current.split_route.as_ref())
        .map(|(actual, route)| {
            format!(
                " and backend {} {} to {}",
                route.backend_container_name, route.backend_published_port, actual
            )
        })
        .unwrap_or_default();
    log(
        cfg,
        target.deployment_id,
        "stdout",
        &format!(
            "Detected Docker-published port drift for {}; updating route from {} to {}{}.",
            current.container_name, current.published_port, actual_frontend, backend_summary
        ),
    )
    .await;
    refresh_route(cfg, &current, actual_frontend, actual_backend).await?;
    target.published_port = actual_frontend;
    if let (Some(actual), Some(route)) = (actual_backend, target.split_route.as_mut()) {
        route.backend_published_port = actual;
    }
    Ok(())
}

fn same_route_owner(left: &HealthTarget, right: &HealthTarget) -> bool {
    left.container_name == right.container_name
        && left.container_port == right.container_port
        && left.domain == right.domain
        && left.route_key == right.route_key
        && left.route_generation == right.route_generation
        && match (&left.split_route, &right.split_route) {
            (None, None) => true,
            (Some(left), Some(right)) => {
                left.backend_container_name == right.backend_container_name
                    && left.backend_container_port == right.backend_container_port
                    && left.backend_path_prefixes == right.backend_path_prefixes
            }
            _ => false,
        }
}

fn published_port_changed(stored: u16, actual: u16) -> bool {
    stored != actual
}

fn split_route_ports_changed(
    target: &HealthTarget,
    actual_frontend: u16,
    actual_backend: Option<u16>,
) -> bool {
    if published_port_changed(target.published_port, actual_frontend) {
        return true;
    }
    target.split_route.as_ref().is_some_and(|route| {
        actual_backend
            .is_some_and(|actual| published_port_changed(route.backend_published_port, actual))
    })
}

async fn refresh_route(
    cfg: &Config,
    target: &HealthTarget,
    frontend_port: u16,
    backend_port: Option<u16>,
) -> anyhow::Result<()> {
    let Some(route_key) = target.route_key.as_deref() else {
        return Ok(());
    };
    let Some(domain) = target.domain.as_deref() else {
        return Ok(());
    };
    if let (Some(route), Some(backend_port)) = (target.split_route.as_ref(), backend_port) {
        let Some(generation) = target.route_generation else {
            anyhow::bail!("split route refresh requires a route generation");
        };
        if cfg.local_mode {
            if let Some(router) = &cfg.local_router {
                return apply_local_caddy_split_route_versioned(
                    cfg,
                    target.deployment_id,
                    router,
                    route_key,
                    domain,
                    frontend_port,
                    backend_port,
                    &route.backend_path_prefixes,
                    generation,
                )
                .await;
            }
            return Ok(());
        }
        return apply_caddy_split_route_versioned(
            cfg,
            target.deployment_id,
            route_key,
            domain,
            frontend_port,
            backend_port,
            &route.backend_path_prefixes,
            generation,
        )
        .await;
    }
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
                        frontend_port,
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
                        frontend_port,
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
                frontend_port,
                generation,
            )
            .await
        }
        None => {
            apply_caddy_route(cfg, target.deployment_id, route_key, domain, frontend_port).await
        }
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
#[path = "health_tests.rs"]
mod tests;
