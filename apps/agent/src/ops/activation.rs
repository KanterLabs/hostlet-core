use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn prepare_candidate_activation(
    cfg: &Config,
    payload: &Value,
    deployment_id: Uuid,
    image_tag: Option<&str>,
    container_name: &str,
    published_port: u16,
    compose_project: Option<&str>,
    runtime_metadata: Value,
    services: Vec<hostlet_contracts::DeploymentServiceReport>,
) -> anyhow::Result<i64> {
    let job_id = payload_uuid(payload, "job_id").context("deploy job missing job_id")?;
    let claim_token =
        payload_uuid(payload, "claim_token").context("deploy job missing claim_token")?;
    let expected_current_deployment_id = payload
        .get("expected_current_deployment_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    let request = hostlet_contracts::PrepareActivationRequest {
        job_id,
        claim_token,
        expected_current_deployment_id,
        candidate: hostlet_contracts::CandidateRuntime {
            container_name: container_name.to_string(),
            published_port: published_port.into(),
            image_tag: image_tag.map(str::to_string),
            compose_project: compose_project.map(str::to_string),
            runtime_metadata,
            services,
        },
    };
    let response = cfg
        .http
        .post(format!(
            "{}/api/agent/deployments/{deployment_id}/prepare-activation",
            cfg.api_url
        ))
        .header("x-hostlet-server-id", cfg.server_id.to_string())
        .header("x-hostlet-agent-token", &cfg.agent_token)
        .json(&request)
        .send()
        .await
        .map_err(unacknowledged_activation)?;
    if !response.status().is_success() {
        return Err(unacknowledged_activation(format!(
            "prepare returned {}",
            response.status()
        )));
    }
    let receipt = response
        .json::<hostlet_contracts::PrepareActivationReceipt>()
        .await
        .map_err(unacknowledged_activation)?;
    record_activation_generation(
        cfg,
        deployment_id,
        receipt.route_generation,
        payload.get("type").and_then(Value::as_str) == Some("rollback"),
    )
    .await?;
    Ok(receipt.route_generation)
}

pub(crate) async fn commit_candidate_activation(
    cfg: &Config,
    payload: &Value,
    deployment_id: Uuid,
    route_generation: i64,
    local_url: Option<&str>,
    runtime_metadata: Option<&Value>,
    rolled_back: bool,
) -> anyhow::Result<()> {
    let job_id = payload_uuid(payload, "job_id").context("deploy job missing job_id")?;
    let claim_token =
        payload_uuid(payload, "claim_token").context("deploy job missing claim_token")?;
    record_deployment_phase(cfg, deployment_id, "route_switched").await?;
    let request = hostlet_contracts::CommitActivationRequest {
        job_id,
        claim_token,
        route_generation,
        local_url: local_url.map(str::to_string),
        runtime_metadata: runtime_metadata.cloned(),
        rolled_back,
    };
    let response = cfg
        .http
        .post(format!(
            "{}/api/agent/deployments/{deployment_id}/commit-activation",
            cfg.api_url
        ))
        .header("x-hostlet-server-id", cfg.server_id.to_string())
        .header("x-hostlet-agent-token", &cfg.agent_token)
        .json(&request)
        .send()
        .await
        .map_err(unacknowledged_activation)?;
    if !response.status().is_success() {
        return Err(unacknowledged_activation(format!(
            "commit returned {}",
            response.status()
        )));
    }
    record_deployment_phase(cfg, deployment_id, "committed").await?;
    Ok(())
}
