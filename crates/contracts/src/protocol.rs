use crate::{DeploymentServiceReport, DeploymentStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Current durable deployment-execution protocol spoken by Core API and agent.
pub const DEPLOYMENT_PROTOCOL_VERSION: i32 = 3;

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
