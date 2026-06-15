use serde_json::Value;

pub struct DeploymentStatusEvent<'a> {
    pub status: &'a str,
    pub runtime_metadata: Option<&'a Value>,
}

pub enum DeploymentStatusDecision {
    Accept,
    Fail {
        failure: String,
        runtime_metadata: Option<Value>,
    },
}

pub trait DeploymentStatusPolicy: Send + Sync {
    fn evaluate(&self, event: DeploymentStatusEvent<'_>) -> DeploymentStatusDecision;
}

pub struct NoopDeploymentStatusPolicy;

impl DeploymentStatusPolicy for NoopDeploymentStatusPolicy {
    fn evaluate(&self, _event: DeploymentStatusEvent<'_>) -> DeploymentStatusDecision {
        DeploymentStatusDecision::Accept
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn noop_deployment_status_policy_accepts_metadata() {
        let policy = NoopDeploymentStatusPolicy;
        let metadata = json!({"imageBudgetStatus": "over_budget"});

        let decision = policy.evaluate(DeploymentStatusEvent {
            status: "success",
            runtime_metadata: Some(&metadata),
        });

        assert!(matches!(decision, DeploymentStatusDecision::Accept));
    }
}
