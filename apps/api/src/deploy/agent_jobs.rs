use crate::state::AppState;
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn insert_agent_job_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    server_id: Uuid,
    app_id: Option<Uuid>,
    deployment_id: Option<Uuid>,
    job_type: &str,
    payload: serde_json::Value,
    priority: i32,
) -> anyhow::Result<Uuid> {
    let protocol_version = required_protocol_version(job_type, &payload);
    let id = sqlx::query(
        "INSERT INTO agent_jobs
           (server_id,app_id,deployment_id,job_type,status,payload_json,priority,protocol_version)
         VALUES ($1,$2,$3,$4,'queued',$5,$6,$7)
         RETURNING id",
    )
    .bind(server_id)
    .bind(app_id)
    .bind(deployment_id)
    .bind(job_type)
    .bind(payload)
    .bind(priority)
    .bind(protocol_version)
    .fetch_one(&mut **transaction)
    .await?
    .get::<Uuid, _>("id");
    Ok(id)
}

pub(crate) fn required_protocol_version(job_type: &str, payload: &serde_json::Value) -> i32 {
    match job_type {
        "build" | "release" => 6,
        "suspend_app" | "resume_app" | "stop_previous_deployment" => 4,
        "deploy"
            if payload
                .pointer("/runtime_config/generatedTopology")
                .is_some() =>
        {
            3
        }
        "rollback"
            if payload
                .pointer("/target_runtime_metadata/inferenceReceipt/schemaVersion")
                .is_some()
                || payload
                    .pointer("/target_runtime_metadata/runtime")
                    .and_then(serde_json::Value::as_str)
                    == Some("generated_topology") =>
        {
            3
        }
        "deploy" | "rollback" => 2,
        _ => 1,
    }
}

pub async fn job_signing_secret_for_server(
    state: &AppState,
    server_id: Uuid,
) -> anyhow::Result<String> {
    let encrypted: Option<String> =
        sqlx::query_scalar("SELECT job_signing_secret_ciphertext FROM servers WHERE id=$1")
            .bind(server_id)
            .fetch_optional(&state.db)
            .await?
            .flatten();
    match encrypted {
        Some(value) => state.crypto.decrypt(&value),
        None => Ok(state.job_signing_secret.clone()),
    }
}

pub(crate) async fn send_job(
    state: &AppState,
    server_id: Uuid,
    deployment_id: Uuid,
    payload: serde_json::Value,
) -> anyhow::Result<()> {
    let job_type = payload
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("deployment")
        .to_string();
    let app_id = payload
        .get("app_id")
        .and_then(|value| value.as_str())
        .and_then(|value| Uuid::parse_str(value).ok());
    let required_protocol = required_protocol_version(&job_type, &payload);
    if required_protocol >= 3 {
        let advertised: Option<i32> =
            sqlx::query_scalar("SELECT agent_protocol_version FROM servers WHERE id=$1")
                .bind(server_id)
                .fetch_optional(&state.db)
                .await?;
        if advertised.unwrap_or(1) < required_protocol {
            anyhow::bail!(
                "agent_upgrade_required: this inferred topology requires Hostlet agent protocol v{required_protocol}"
            );
        }
    }
    let job_id = super::enqueue_agent_job(
        state,
        server_id,
        app_id,
        Some(deployment_id),
        &job_type,
        payload,
        10,
    )
    .await?;
    if let Some(app_id) = app_id {
        super::record_audit_event(
            state,
            &format!("{job_type}_job_queued"),
            Uuid::nil(),
            app_id,
            Some(deployment_id),
            Some(job_id),
        )
        .await;
    }
    let waiting_for_capacity: bool = sqlx::query_scalar(
        "SELECT COALESCE(payload_json->>'capacity_wait'='true',false)
         FROM agent_jobs WHERE id=$1",
    )
    .bind(job_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);
    // Best-effort: advance 'queued' → 'running' only once the capacity
    // scheduler has made the job claimable.
    if !waiting_for_capacity {
        super::mark_deployment_running(state, deployment_id).await;
    }
    Ok(())
}
