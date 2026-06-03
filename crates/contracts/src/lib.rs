use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Defines a `snake_case` status enum whose wire string is shared by serde, the
/// database, and the `as_str`/`FromStr` round trip.
///
/// Each variant lists its canonical string exactly once, so `as_str` and
/// `from_str` can never drift apart (the previous hand-written impls duplicated
/// every arm twice and had to be kept in sync by hand). The `#[serde(rename_all
/// = "snake_case")]` attribute is applied so the JSON wire shape stays identical
/// to the canonical strings, which are the same values persisted in Postgres.
macro_rules! string_status_enum {
    (
        $(#[$enum_meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$variant_meta:meta])* $variant:ident => $wire:literal ),+ $(,)?
        }
    ) => {
        $(#[$enum_meta])*
        #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
        #[serde(rename_all = "snake_case")]
        $vis enum $name {
            $( $(#[$variant_meta])* $variant ),+
        }

        impl $name {
            /// Returns the canonical wire/database string for this status.
            pub fn as_str(&self) -> &'static str {
                match self {
                    $( Self::$variant => $wire ),+
                }
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = ();

            fn from_str(value: &str) -> ::std::result::Result<Self, Self::Err> {
                match value {
                    $( $wire => Ok(Self::$variant), )+
                    _ => Err(()),
                }
            }
        }
    };
}

/// Validates a GitHub `owner/repo` identifier.
///
/// Mirrors GitHub's naming rules: exactly one `/` separator, each segment
/// non-empty and at most 100 chars, restricted to alphanumerics plus `.`, `_`
/// and `-`, and never beginning or ending with a dot.
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

/// Validates a Git branch (ref) name against the subset of `git check-ref-format`
/// rules that matter for deploys.
///
/// Rejects empty names, names longer than 255 chars, a leading `-` (which Git
/// would treat as an option), leading/trailing `/`, the `..` and `@{` sequences
/// that Git forbids in refs, and any character outside alphanumerics plus
/// `/ . _ -`.
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

/// Validates a deploy domain, which is either a bare hostname or `host:port`.
///
/// When a trailing `:port` is present the port must be a non-empty, parseable
/// `u16` (the range Postgres/TCP allow); the host portion is checked with
/// [`valid_hostname`].
pub fn valid_domain(value: &str) -> bool {
    let Some((host, port)) = value.rsplit_once(':') else {
        return valid_hostname(value);
    };
    valid_hostname(host) && !port.is_empty() && port.parse::<u16>().is_ok()
}

/// Validates a DNS hostname per RFC 1035/1123 length and character limits.
///
/// The full name is capped at 253 chars and each dot-separated label at 63;
/// labels must be non-empty, may not start or end with `-`, and may contain only
/// alphanumerics and `-`. Leading/trailing dots are rejected.
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

/// Validates an HTTP health-check path.
///
/// Must be an absolute path (leading `/`), at most 256 chars, and free of
/// control characters and backslashes (which would be ambiguous or unsafe in a
/// URL path).
pub fn valid_health_path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 256
        && !value.chars().any(|c| c.is_control() || c == '\\')
}

string_status_enum! {
    /// Lifecycle status of a single deployment, as persisted in the
    /// `deployments` table and sent over the wire.
    pub enum DeploymentStatus {
        Queued => "queued",
        Running => "running",
        Building => "building",
        Starting => "starting",
        HealthChecking => "health_checking",
        Routing => "routing",
        Success => "success",
        Failed => "failed",
        RolledBack => "rolled_back",
        Canceled => "canceled",
    }
}

string_status_enum! {
    /// Lifecycle status of a durable agent job, as persisted in the
    /// `agent_jobs` table and sent over the wire.
    ///
    /// Note both `Canceled` (`"canceled"`) and `Cancelled` (`"cancelled"`)
    /// exist on purpose: the agent-job pipeline persists the British spelling
    /// `"cancelled"` (see `web/jobs.rs` and `web/cleanup.rs`), while
    /// `"canceled"` is accepted/recognized for parity with [`DeploymentStatus`]
    /// and the agent status validator. They are distinct wire strings, so
    /// neither can be dropped without changing deserialization of existing rows.
    pub enum AgentJobStatus {
        Queued => "queued",
        Claimed => "claimed",
        Running => "running",
        Success => "success",
        Failed => "failed",
        Canceled => "canceled",
        Cancelled => "cancelled",
        Expired => "expired",
    }
}

string_status_enum! {
    /// Health classification reported for a running container.
    pub enum RuntimeHealthStatus {
        Healthy => "healthy",
        Degraded => "degraded",
        Unhealthy => "unhealthy",
        Unknown => "unknown",
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

/// Full work order handed to an agent to build and launch one deployment.
///
/// The fields fall into a few logical groups (kept in a flat struct so the JSON
/// wire shape is unchanged): identity/routing, source, networking, runtime
/// configuration, build/run commands, resource limits, and secrets.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DeployJob {
    // Identity and routing.
    pub deployment_id: Uuid,
    pub app_id: Uuid,
    pub route_key: String,
    pub app_name: String,

    // Source to build from.
    pub repo: String,
    pub branch: String,
    pub commit_sha: String,

    // Networking and health.
    pub container_port: i64,
    pub health_path: String,
    pub domain: String,

    // Runtime configuration.
    pub env: BTreeMap<String, String>,
    pub runtime_kind: String,
    pub hostlet_config_path: String,
    pub runtime_config: Value,
    pub packaging_strategy: String,
    pub root_directory: String,

    // Build and run commands (defaulted when absent).
    pub install_command: Option<String>,
    pub build_command: Option<String>,
    pub start_command: Option<String>,

    // Resource limits.
    pub memory_limit_mb: Option<i32>,
    pub cpu_limit: Option<f64>,

    // Secrets.
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
