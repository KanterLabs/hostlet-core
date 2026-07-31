use crate::{DeploymentServiceReport, DeploymentStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Current durable deployment-execution protocol spoken by Core API and agent.
pub const DEPLOYMENT_PROTOCOL_VERSION: i32 = 4;
/// Maximum service topology retained from a deployment status report.
///
/// This is deliberately independent from the much smaller runtime-log target
/// cap: diagnostics may sample a deployment without truncating its durable
/// topology, health, routing, or cleanup state.
pub const DEPLOYMENT_SERVICE_REPORT_MAX: usize = 64;

/// Runtime diagnostics are intentionally a small, ephemeral tail rather than a
/// durable log archive. Both the API and agent enforce these limits so neither
/// side has to trust the other to keep the response bounded.
pub const RUNTIME_LOG_MAX_LINES: usize = 500;
pub const RUNTIME_LOG_MAX_BYTES: usize = 256 * 1024;
pub const RUNTIME_LOG_MAX_LINE_BYTES: usize = 8 * 1024;
pub const RUNTIME_LOG_MAX_TARGETS: usize = 16;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLogTarget {
    pub service: String,
    pub container: String,
}

/// One-shot request from the API to the connected agent. It deliberately
/// contains no environment or secret values.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLogRequest {
    pub request_id: Uuid,
    pub deployment_id: Uuid,
    pub targets: Vec<RuntimeLogTarget>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLogLine {
    pub timestamp: Option<String>,
    pub service: String,
    pub stream: String,
    pub line: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLogServiceError {
    pub service: String,
    pub message: String,
}

/// One-shot response from the agent. The API forwards the bounded lines to the
/// requesting owner and then drops them; the response is never persisted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLogResponse {
    pub request_id: Uuid,
    pub deployment_id: Uuid,
    pub truncated: bool,
    #[serde(default)]
    pub lines: Vec<RuntimeLogLine>,
    #[serde(default)]
    pub unavailable_services: Vec<RuntimeLogServiceError>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentJobHeartbeat {
    pub claim_token: Uuid,
    pub phase: DeploymentStatus,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentJobHeartbeatReceipt {
    pub cancel_requested: bool,
    pub lease_expires_at: String,
}

/// Bounded host-level telemetry attached to the agent's websocket heartbeat.
/// It contains capacity facts only—never process arguments, environment
/// values, paths, or tenant identifiers.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostResourceSnapshot {
    pub memory_total_mib: u64,
    pub memory_available_mib: u64,
    pub swap_used_mib: u64,
    pub disk_total_mib: u64,
    pub disk_free_mib: u64,
    pub load_one: f64,
    pub load_five: f64,
    pub load_fifteen: f64,
    pub running_containers: u32,
}

/// Runtime facts durably prepared by the agent before a route can be changed.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateRuntime {
    pub container_name: String,
    pub published_port: i32,
    pub image_tag: Option<String>,
    pub compose_project: Option<String>,
    #[serde(default)]
    pub runtime_metadata: Value,
    #[serde(default)]
    pub services: Vec<DeploymentServiceReport>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareActivationRequest {
    pub job_id: Uuid,
    pub claim_token: Uuid,
    pub expected_current_deployment_id: Option<Uuid>,
    pub candidate: CandidateRuntime,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareActivationReceipt {
    pub route_generation: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitActivationRequest {
    pub job_id: Uuid,
    pub claim_token: Uuid,
    pub route_generation: i64,
    pub local_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub rolled_back: bool,
}
