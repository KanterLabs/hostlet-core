use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::str::FromStr;
use uuid::Uuid;

pub fn valid_repo_full_name(value: &str) -> bool {
    let mut parts = value.split('/');
    let Some(owner) = parts.next() else {
        return false;
    };
    let Some(repo) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    [owner, repo].into_iter().all(|part| {
        !part.is_empty()
            && part.len() <= 100
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            && !part.starts_with('.')
            && !part.ends_with('.')
    })
}

pub fn valid_branch(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('-')
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("..")
        && !value.contains("@{")
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
}

pub fn valid_domain(value: &str) -> bool {
    let Some((host, port)) = value.rsplit_once(':') else {
        return valid_hostname(value);
    };
    valid_hostname(host) && !port.is_empty() && port.parse::<u16>().is_ok()
}

pub fn valid_hostname(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && !value.starts_with('.')
        && !value.ends_with('.')
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

pub fn valid_health_path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 256
        && !value.chars().any(|c| c.is_control() || c == '\\')
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentStatus {
    Queued,
    Running,
    Building,
    Starting,
    HealthChecking,
    Routing,
    Success,
    Failed,
    RolledBack,
    Canceled,
}

impl DeploymentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Building => "building",
            Self::Starting => "starting",
            Self::HealthChecking => "health_checking",
            Self::Routing => "routing",
            Self::Success => "success",
            Self::Failed => "failed",
            Self::RolledBack => "rolled_back",
            Self::Canceled => "canceled",
        }
    }
}

impl FromStr for DeploymentStatus {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "building" => Ok(Self::Building),
            "starting" => Ok(Self::Starting),
            "health_checking" => Ok(Self::HealthChecking),
            "routing" => Ok(Self::Routing),
            "success" => Ok(Self::Success),
            "failed" => Ok(Self::Failed),
            "rolled_back" => Ok(Self::RolledBack),
            "canceled" => Ok(Self::Canceled),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentJobStatus {
    Queued,
    Claimed,
    Running,
    Success,
    Failed,
    Canceled,
    Cancelled,
    Expired,
}

impl FromStr for AgentJobStatus {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "queued" => Ok(Self::Queued),
            "claimed" => Ok(Self::Claimed),
            "running" => Ok(Self::Running),
            "success" => Ok(Self::Success),
            "failed" => Ok(Self::Failed),
            "canceled" => Ok(Self::Canceled),
            "cancelled" => Ok(Self::Cancelled),
            "expired" => Ok(Self::Expired),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeHealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

impl FromStr for RuntimeHealthStatus {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "healthy" => Ok(Self::Healthy),
            "degraded" => Ok(Self::Degraded),
            "unhealthy" => Ok(Self::Unhealthy),
            "unknown" => Ok(Self::Unknown),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentJobPayload {
    Deploy(Box<DeployJob>),
    Rollback(Box<RollbackJob>),
    DeleteApp(Box<DeleteAppJob>),
    HealthCheck(Box<HealthCheckJob>),
    RestartContainer(Box<RestartContainerJob>),
    DockerCleanup(Box<DockerCleanupJob>),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DeployJob {
    pub deployment_id: Uuid,
    pub app_id: Uuid,
    pub route_key: String,
    pub app_name: String,
    pub repo: String,
    pub branch: String,
    pub commit_sha: String,
    pub container_port: i64,
    pub health_path: String,
    pub domain: String,
    pub env: BTreeMap<String, String>,
    pub runtime_kind: String,
    pub hostlet_config_path: String,
    pub runtime_config: Value,
    pub packaging_strategy: String,
    pub root_directory: String,
    pub install_command: Option<String>,
    pub build_command: Option<String>,
    pub start_command: Option<String>,
    pub memory_limit_mb: Option<i32>,
    pub cpu_limit: Option<f64>,
    pub github_token: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RollbackJob {
    pub deployment_id: Uuid,
    pub app_id: Uuid,
    pub route_key: String,
    pub target_deployment_id: Uuid,
    pub target_container: Option<String>,
    pub domain: String,
    pub container_port: i32,
    pub published_port: i32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DeleteAppJob {
    pub app_id: Uuid,
    pub route_key: String,
    pub domain: String,
    pub user_id: Uuid,
    pub public_exposure: bool,
    pub compose_project: String,
    pub containers: Vec<String>,
    pub images: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HealthCheckJob {
    pub app_id: Uuid,
    pub deployment_id: Uuid,
    pub container_name: String,
    pub published_port: i32,
    pub health_path: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RestartContainerJob {
    pub app_id: Uuid,
    pub deployment_id: Uuid,
    pub container_name: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DockerCleanupJob {
    pub dry_run: bool,
    pub keep_containers: Vec<String>,
    pub keep_images: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Heartbeat,
    DeploymentStatus(DeploymentStatusEvent),
    Log(LogEvent),
    ResourceStats(ResourceStatsEvent),
    HealthStatus(HealthStatusEvent),
    JobStatus(JobStatusEvent),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DeploymentStatusEvent {
    pub deployment_id: Uuid,
    pub status: DeploymentStatus,
    pub failure: Option<String>,
    pub image_tag: Option<String>,
    pub container_name: Option<String>,
    pub local_url: Option<String>,
    pub published_port: Option<i32>,
    pub compose_project: Option<String>,
    pub runtime_metadata: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogEvent {
    pub deployment_id: Uuid,
    pub stream: String,
    pub line: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceStatsEvent {
    pub container: String,
    pub cpu_percent: String,
    pub memory_usage: String,
    pub memory_percent: String,
    pub network_io: String,
    pub block_io: String,
    pub pids: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HealthStatusEvent {
    pub app_id: Uuid,
    pub deployment_id: Option<Uuid>,
    pub container_name: Option<String>,
    pub status: RuntimeHealthStatus,
    pub checked_url: Option<String>,
    pub http_status: Option<i32>,
    pub latency_ms: Option<i32>,
    pub failure_count: i32,
    pub success_count: i32,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct JobStatusEvent {
    pub job_id: Uuid,
    pub status: AgentJobStatus,
    pub failure: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deploy_payload_keeps_existing_wire_shape() {
        let deployment_id = Uuid::nil();
        let app_id = Uuid::from_u128(1);
        let payload = AgentJobPayload::Deploy(Box::new(DeployJob {
            deployment_id,
            app_id,
            route_key: "app-1".into(),
            app_name: "demo".into(),
            repo: "owner/repo".into(),
            branch: "main".into(),
            commit_sha: "HEAD".into(),
            container_port: 3000,
            health_path: "/".into(),
            domain: "demo.example.test".into(),
            env: BTreeMap::new(),
            runtime_kind: "single".into(),
            hostlet_config_path: "hostlet.yml".into(),
            runtime_config: serde_json::json!({}),
            packaging_strategy: "auto".into(),
            root_directory: ".".into(),
            install_command: None,
            build_command: None,
            start_command: None,
            memory_limit_mb: Some(512),
            cpu_limit: Some(0.5),
            github_token: None,
        }));
        let value = serde_json::to_value(&payload).unwrap();
        assert_eq!(value["type"], "deploy");
        assert_eq!(value["deployment_id"], deployment_id.to_string());
        assert_eq!(value["app_id"], app_id.to_string());
        assert_eq!(
            serde_json::from_value::<AgentJobPayload>(value).unwrap(),
            payload
        );
    }

    #[test]
    fn agent_events_keep_existing_type_tags() {
        let event = AgentEvent::Heartbeat;
        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["type"], "heartbeat");
    }

    #[test]
    fn deployment_status_strings_match_database_values() {
        assert_eq!(DeploymentStatus::HealthChecking.as_str(), "health_checking");
        assert_eq!(DeploymentStatus::RolledBack.as_str(), "rolled_back");
        assert_eq!(
            "health_checking".parse::<DeploymentStatus>().unwrap(),
            DeploymentStatus::HealthChecking
        );
    }
}
