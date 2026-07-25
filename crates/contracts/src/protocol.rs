use crate::{DeploymentServiceReport, DeploymentStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Current durable deployment-execution protocol spoken by Core API and agent.
pub const DEPLOYMENT_PROTOCOL_VERSION: i32 = 2;

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
    pub runtime_metadata: Option<Value>,
    #[serde(default)]
    pub rolled_back: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit_json() -> Value {
        serde_json::json!({
            "jobId": Uuid::nil(),
            "claimToken": Uuid::nil(),
            "routeGeneration": 7,
            "localUrl": null,
            "rolledBack": false
        })
    }

    #[test]
    fn commit_activation_accepts_recovery_requests_without_final_metadata() {
        let request: CommitActivationRequest = serde_json::from_value(commit_json()).unwrap();
        assert_eq!(request.runtime_metadata, None);
    }

    #[test]
    fn commit_activation_carries_final_runtime_metadata() {
        let mut value = commit_json();
        value["runtimeMetadata"] = serde_json::json!({"routingDurationMs": 12});
        let request: CommitActivationRequest = serde_json::from_value(value).unwrap();
        assert_eq!(
            request.runtime_metadata,
            Some(serde_json::json!({"routingDurationMs": 12}))
        );
    }
}
