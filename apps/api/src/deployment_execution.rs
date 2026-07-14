//! Transactional protocol-v2 deployment execution.
//!
//! Docker and Caddy are necessarily changed outside Postgres.  These endpoints
//! provide the fencing and two-phase activation needed to make those effects
//! recoverable without allowing an expired worker to overwrite newer state.

use crate::{agent::authenticated_server_id, deploy::ACTIVE_DEPLOYMENT_STATUSES, state::AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use hostlet_contracts::{
    AgentJobHeartbeat, AgentJobHeartbeatReceipt, CommitActivationRequest, PrepareActivationReceipt,
    PrepareActivationRequest,
};
use sqlx::Row;
use uuid::Uuid;

const LEASE_MINUTES: i64 = 5;

pub async fn heartbeat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
    Json(request): Json<AgentJobHeartbeat>,
) -> impl IntoResponse {
    let Some(server_id) = authenticated_server_id(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !ACTIVE_DEPLOYMENT_STATUSES.contains(&request.phase.as_str()) {
        return (StatusCode::BAD_REQUEST, "invalid active deployment phase").into_response();
    }
    // SQLx cannot update two tables in one UPDATE. Use a transaction so the
    // renewed lease and visible deployment phase cannot drift apart.
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let job = sqlx::query(
        "UPDATE agent_jobs
         SET status='running', updated_at=now(),
             lease_expires_at=now() + make_interval(mins => $1)
         WHERE id=$2 AND server_id=$3 AND claim_token=$4
           AND status IN ('claimed','running')
           AND lease_expires_at >= now()
         RETURNING deployment_id,cancel_requested_at,lease_expires_at",
    )
    .bind(LEASE_MINUTES as i32)
    .bind(job_id)
    .bind(server_id)
    .bind(request.claim_token)
    .fetch_optional(&mut *tx)
    .await;
    let Ok(Some(job)) = job else {
        return StatusCode::CONFLICT.into_response();
    };
    if let Some(deployment_id) = job.get::<Option<Uuid>, _>("deployment_id") {
        if sqlx::query(
            "UPDATE deployments SET status=$1,last_heartbeat_at=now()
             WHERE id=$2 AND server_id=$3 AND status = ANY($4)",
        )
        .bind(request.phase.as_str())
        .bind(deployment_id)
        .bind(server_id)
        .bind(ACTIVE_DEPLOYMENT_STATUSES)
        .execute(&mut *tx)
        .await
        .is_err()
        {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    if tx.commit().await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let expires: chrono::DateTime<chrono::Utc> = job.get("lease_expires_at");
    Json(AgentJobHeartbeatReceipt {
        cancel_requested: job
            .get::<Option<chrono::DateTime<chrono::Utc>>, _>("cancel_requested_at")
            .is_some(),
        lease_expires_at: expires.to_rfc3339(),
    })
    .into_response()
}

pub async fn prepare_activation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(deployment_id): Path<Uuid>,
    Json(request): Json<PrepareActivationRequest>,
) -> impl IntoResponse {
    let Some(server_id) = authenticated_server_id(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let row = sqlx::query(
        "SELECT d.app_id,a.current_deployment_id,a.pending_deployment_id,a.route_generation,
                j.cancel_requested_at
         FROM deployments d
         JOIN apps a ON a.id=d.app_id
         JOIN agent_jobs j ON j.id=$1 AND j.deployment_id=d.id
         WHERE d.id=$2 AND d.server_id=$3 AND j.server_id=$3
           AND j.claim_token=$4 AND j.status IN ('claimed','running')
           AND j.lease_expires_at >= now()
         FOR UPDATE OF d,a,j",
    )
    .bind(request.job_id)
    .bind(deployment_id)
    .bind(server_id)
    .bind(request.claim_token)
    .fetch_optional(&mut *tx)
    .await;
    let Ok(Some(row)) = row else {
        return StatusCode::CONFLICT.into_response();
    };
    if row
        .get::<Option<chrono::DateTime<chrono::Utc>>, _>("cancel_requested_at")
        .is_some()
    {
        return (
            StatusCode::CONFLICT,
            "deployment cancellation was requested",
        )
            .into_response();
    }
    let current = row.get::<Option<Uuid>, _>("current_deployment_id");
    if current != request.expected_current_deployment_id {
        return (StatusCode::CONFLICT, "current deployment changed").into_response();
    }
    let pending = row.get::<Option<Uuid>, _>("pending_deployment_id");
    if pending.is_some() && pending != Some(deployment_id) {
        return (StatusCode::CONFLICT, "another activation is pending").into_response();
    }
    let generation = if pending == Some(deployment_id) {
        row.get::<i64, _>("route_generation")
    } else {
        row.get::<i64, _>("route_generation") + 1
    };
    let candidate = match serde_json::to_value(&request.candidate) {
        Ok(value) => value,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let app_id = row.get::<Uuid, _>("app_id");
    if sqlx::query(
        "UPDATE apps SET pending_deployment_id=$1,route_generation=$2,updated_at=now()
         WHERE id=$3",
    )
    .bind(deployment_id)
    .bind(generation)
    .bind(app_id)
    .execute(&mut *tx)
    .await
    .is_err()
        || sqlx::query(
            "UPDATE deployments SET status='routing',expected_current_deployment_id=$1,
                    activation_generation=$2,last_heartbeat_at=now(),
                    image_tag=COALESCE($3,image_tag),container_name=$4,published_port=$5,
                    compose_project=COALESCE($6,compose_project),runtime_metadata=$7
             WHERE id=$8 AND status = ANY($9)",
        )
        .bind(current)
        .bind(generation)
        .bind(request.candidate.image_tag.as_deref())
        .bind(&request.candidate.container_name)
        .bind(request.candidate.published_port)
        .bind(request.candidate.compose_project.as_deref())
        .bind(&request.candidate.runtime_metadata)
        .bind(deployment_id)
        .bind(ACTIVE_DEPLOYMENT_STATUSES)
        .execute(&mut *tx)
        .await
        .is_err()
        || sqlx::query("UPDATE agent_jobs SET result_json=$1,updated_at=now() WHERE id=$2")
            .bind(candidate)
            .bind(request.job_id)
            .execute(&mut *tx)
            .await
            .is_err()
        || sqlx::query(
            "INSERT INTO audit_events(actor_type,actor_id,event_type,app_id,deployment_id,job_id,metadata_json)
             VALUES ('agent',$1,'deployment_activation_prepared',$2,$3,$4,jsonb_build_object('routeGeneration',$5))",
        )
        .bind(server_id.to_string())
        .bind(app_id)
        .bind(deployment_id)
        .bind(request.job_id)
        .bind(generation)
        .execute(&mut *tx)
        .await
        .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if tx.commit().await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    Json(PrepareActivationReceipt {
        route_generation: generation,
    })
    .into_response()
}

pub async fn commit_activation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(deployment_id): Path<Uuid>,
    Json(request): Json<CommitActivationRequest>,
) -> impl IntoResponse {
    let Some(server_id) = authenticated_server_id(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let row = sqlx::query(
        "SELECT d.app_id,d.status,a.current_deployment_id,a.pending_deployment_id,
                a.route_generation,j.status AS job_status,j.claim_token,j.result_json
         FROM deployments d JOIN apps a ON a.id=d.app_id
         JOIN agent_jobs j ON j.id=$1 AND j.deployment_id=d.id
         WHERE d.id=$2 AND d.server_id=$3 AND j.server_id=$3
         FOR UPDATE OF d,a,j",
    )
    .bind(request.job_id)
    .bind(deployment_id)
    .bind(server_id)
    .fetch_optional(&mut *tx)
    .await;
    let Ok(Some(row)) = row else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if matches!(
        row.get::<String, _>("status").as_str(),
        "success" | "rolled_back"
    ) && row.get::<String, _>("job_status") == "success"
        && row.get::<Option<Uuid>, _>("current_deployment_id") == Some(deployment_id)
    {
        return StatusCode::NO_CONTENT.into_response();
    }
    if row.get::<Option<Uuid>, _>("claim_token") != Some(request.claim_token)
        || row.get::<Option<Uuid>, _>("pending_deployment_id") != Some(deployment_id)
        || row.get::<i64, _>("route_generation") != request.route_generation
    {
        return StatusCode::CONFLICT.into_response();
    }
    let app_id = row.get::<Uuid, _>("app_id");
    let terminal_status = if request.rolled_back {
        "rolled_back"
    } else {
        "success"
    };
    let candidate = row
        .get::<Option<serde_json::Value>, _>("result_json")
        .and_then(|value| {
            serde_json::from_value::<hostlet_contracts::CandidateRuntime>(value).ok()
        });
    if sqlx::query(
        "UPDATE apps SET current_deployment_id=$1,pending_deployment_id=NULL,
                domain=COALESCE($2,domain),updated_at=now() WHERE id=$3",
    )
    .bind(deployment_id)
    .bind(request.local_url.as_deref())
    .bind(app_id)
    .execute(&mut *tx)
    .await
    .is_err()
        || sqlx::query(
            "UPDATE deployments SET status=$2,failure_summary=NULL,failure_code=NULL,
                    finished_at=now(),last_heartbeat_at=now() WHERE id=$1",
        )
        .bind(deployment_id)
        .bind(terminal_status)
        .execute(&mut *tx)
        .await
        .is_err()
        || sqlx::query(
            "UPDATE agent_jobs SET status='success',payload_json=payload_json-'env'-'github_token',
                    lease_expires_at=NULL,updated_at=now(),finished_at=now() WHERE id=$1",
        )
        .bind(request.job_id)
        .execute(&mut *tx)
        .await
        .is_err()
        || sqlx::query(
            "INSERT INTO audit_events(actor_type,actor_id,event_type,app_id,deployment_id,job_id,metadata_json)
             VALUES ('agent',$1,'deployment_activation_committed',$2,$3,$4,jsonb_build_object('routeGeneration',$5))",
        )
        .bind(server_id.to_string())
        .bind(app_id)
        .bind(deployment_id)
        .bind(request.job_id)
        .bind(request.route_generation)
        .execute(&mut *tx)
        .await
        .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if let Some(candidate) = candidate {
        let rollback_target = request
            .rolled_back
            .then(|| {
                candidate
                    .runtime_metadata
                    .get("rollbackTargetDeploymentId")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok())
            })
            .flatten();
        let stable_compose = candidate
            .runtime_metadata
            .get("stableProject")
            .and_then(serde_json::Value::as_str)
            .zip(
                candidate
                    .runtime_metadata
                    .get("backingSpecHash")
                    .and_then(serde_json::Value::as_str),
            )
            .map(|(project, hash)| (project.to_string(), hash.to_string()));
        if sqlx::query("DELETE FROM deployment_services WHERE deployment_id=$1")
            .bind(deployment_id)
            .execute(&mut *tx)
            .await
            .is_err()
        {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        for service in candidate.services.into_iter().take(64) {
            if service.name.is_empty()
                || service.name.len() > 64
                || !matches!(service.role.as_str(), "web" | "backing")
                || service
                    .container_name
                    .as_deref()
                    .is_some_and(|name| !hostlet_contracts::valid_container_name(name))
            {
                continue;
            }
            if sqlx::query(
                "INSERT INTO deployment_services
                   (deployment_id,app_id,service_name,role,container_name,image_tag,target_port,published_port,status,health_status)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            )
            .bind(deployment_id)
            .bind(app_id)
            .bind(service.name)
            .bind(service.role)
            .bind(service.container_name)
            .bind(service.image_tag)
            .bind(service.target_port)
            .bind(service.published_port)
            .bind(service.status)
            .bind(service.health_status)
            .execute(&mut *tx)
            .await
            .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
        if let Some(target) = rollback_target {
            if sqlx::query(
                "INSERT INTO deployment_services
                   (deployment_id,app_id,service_name,role,container_name,image_tag,target_port,published_port,status,health_status,last_healthy_at)
                 SELECT $1,$2,service_name,role,container_name,image_tag,target_port,published_port,status,health_status,last_healthy_at
                 FROM deployment_services WHERE deployment_id=$3",
            )
            .bind(deployment_id)
            .bind(app_id)
            .bind(target)
            .execute(&mut *tx)
            .await
            .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
        if let Some((stable_project, backing_hash)) = stable_compose {
            let stable_network = format!("{stable_project}_default");
            if sqlx::query(
                "INSERT INTO app_compose_runtime
                   (app_id,stable_project,stable_network,backing_spec_hash,backing_status,last_applied_deployment_id,updated_at)
                 VALUES ($1,$2,$3,$4,'ready',$5,now())
                 ON CONFLICT (app_id) DO UPDATE SET
                   stable_project=EXCLUDED.stable_project,
                   stable_network=EXCLUDED.stable_network,
                   backing_spec_hash=EXCLUDED.backing_spec_hash,
                   backing_status='ready',
                   last_applied_deployment_id=EXCLUDED.last_applied_deployment_id,
                   updated_at=now()",
            )
            .bind(app_id)
            .bind(stable_project)
            .bind(stable_network)
            .bind(backing_hash)
            .bind(deployment_id)
            .execute(&mut *tx)
            .await
            .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    }
    match tx.commit().await {
        Ok(()) => {
            if let Err(err) =
                crate::screenshots::enqueue_auto_screenshot_for_deployment(&state, deployment_id)
                    .await
            {
                tracing::warn!(error = %err, %deployment_id, "failed to enqueue automatic screenshot after activation");
            }
            if let Err(err) = crate::cleanup::auto_cleanup_for_server(&state, server_id).await {
                tracing::warn!(error = %err, %server_id, "failed to enqueue cleanup after activation");
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
