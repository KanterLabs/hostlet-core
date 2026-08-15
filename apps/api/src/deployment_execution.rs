//! Transactional protocol-v2 deployment execution.
//!
//! Docker and Caddy are necessarily changed outside Postgres.  These endpoints
//! provide the fencing and two-phase activation needed to make those effects
//! recoverable without allowing an expired worker to overwrite newer state.

use crate::{
    agent::{authenticated_server_id, locks},
    deploy::ACTIVE_DEPLOYMENT_STATUSES,
    state::AppState,
};
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
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

const LEASE_MINUTES: i64 = 5;

/// Lock activation state in the shared lifecycle order before any authority
/// mutation.  The joined `FOR UPDATE` queries that used to serve this purpose
/// left lock acquisition to the planner, which allowed activation and owner
/// cancellation to take deployment/app/job rows in opposite orders.
async fn lock_activation_rows(
    tx: &mut Transaction<'_, Postgres>,
    deployment_id: Uuid,
    job_id: Uuid,
    server_id: Uuid,
) -> anyhow::Result<Option<Uuid>> {
    let app_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT app_id FROM deployments WHERE id=$1 AND server_id=$2",
    )
    .bind(deployment_id)
    .bind(server_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(app_id) = app_id else {
        return Ok(None);
    };
    crate::agent::locks::app(tx, app_id).await?;
    crate::agent::locks::deployment(tx, deployment_id, app_id).await?;
    crate::agent::locks::job(tx, job_id, Some(app_id), Some(deployment_id)).await?;
    Ok(Some(app_id))
}

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
    let ids = sqlx::query(
        "SELECT app_id,deployment_id
         FROM agent_jobs
         WHERE id=$1 AND server_id=$2 AND claim_token=$3",
    )
    .bind(job_id)
    .bind(server_id)
    .bind(request.claim_token)
    .fetch_optional(&mut *tx)
    .await;
    let Some(ids) = (match ids {
        Ok(ids) => ids,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }) else {
        return StatusCode::CONFLICT.into_response();
    };
    let app_id = ids.get::<Option<Uuid>, _>("app_id");
    let deployment_id = ids.get::<Option<Uuid>, _>("deployment_id");
    if let Some(app_id) = app_id {
        if locks::app(&mut tx, app_id).await.is_err() {
            return StatusCode::CONFLICT.into_response();
        }
        if let Some(deployment_id) = deployment_id {
            if locks::deployment(&mut tx, deployment_id, app_id)
                .await
                .is_err()
            {
                return StatusCode::CONFLICT.into_response();
            }
        }
    }
    let job = sqlx::query(
        "UPDATE agent_jobs
         SET status='running', updated_at=now(),
             lease_expires_at=CASE
               WHEN cancel_requested_at IS NULL
               THEN now() + make_interval(mins => $1)
               ELSE lease_expires_at
             END
         WHERE id=$2 AND server_id=$3 AND claim_token=$4
           AND status IN ('claimed','running')
           AND lease_expires_at > clock_timestamp()
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
    if job
        .get::<Option<chrono::DateTime<chrono::Utc>>, _>("cancel_requested_at")
        .is_none()
    {
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
    if !matches!(
        lock_activation_rows(&mut tx, deployment_id, request.job_id, server_id).await,
        Ok(Some(_))
    ) {
        return StatusCode::CONFLICT.into_response();
    }
    let row = sqlx::query(
        "SELECT d.app_id,a.current_deployment_id,a.pending_deployment_id,a.route_generation,
                a.suspended_at,j.cancel_requested_at
         FROM deployments d
         JOIN apps a ON a.id=d.app_id
         JOIN agent_jobs j ON j.id=$1 AND j.deployment_id=d.id
         WHERE d.id=$2 AND d.server_id=$3 AND j.server_id=$3
           AND j.claim_token=$4 AND j.status IN ('claimed','running')
           AND j.lease_expires_at > clock_timestamp()",
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
    if row
        .get::<Option<chrono::DateTime<chrono::Utc>>, _>("suspended_at")
        .is_some()
    {
        return (StatusCode::CONFLICT, "app is paused").into_response();
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
    match lock_activation_rows(&mut tx, deployment_id, request.job_id, server_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
    let row = sqlx::query(
        "SELECT d.app_id,d.status,a.current_deployment_id,a.pending_deployment_id,
                a.route_generation,a.suspended_at,j.status AS job_status,j.claim_token,
                j.cancel_requested_at,j.lease_expires_at,j.result_json,
                COALESCE(j.lease_expires_at > clock_timestamp(),false) AS lease_current
         FROM deployments d JOIN apps a ON a.id=d.app_id
         JOIN agent_jobs j ON j.id=$1 AND j.deployment_id=d.id
         WHERE d.id=$2 AND d.server_id=$3 AND j.server_id=$3",
    )
    .bind(request.job_id)
    .bind(deployment_id)
    .bind(server_id)
    .fetch_optional(&mut *tx)
    .await;
    let Ok(Some(row)) = row else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let terminal_status = if request.rolled_back {
        "rolled_back"
    } else {
        "success"
    };
    if row.get::<String, _>("status") == terminal_status
        && row.get::<String, _>("job_status") == "success"
        && row.get::<Option<Uuid>, _>("current_deployment_id") == Some(deployment_id)
        && row.get::<Option<Uuid>, _>("claim_token") == Some(request.claim_token)
        && row.get::<i64, _>("route_generation") == request.route_generation
    {
        return StatusCode::NO_CONTENT.into_response();
    }
    if !matches!(
        row.get::<String, _>("job_status").as_str(),
        "claimed" | "running"
    ) {
        return (StatusCode::CONFLICT, "job is no longer active").into_response();
    }
    if row.get::<Option<Uuid>, _>("claim_token") != Some(request.claim_token)
        || row.get::<Option<Uuid>, _>("pending_deployment_id") != Some(deployment_id)
        || row.get::<i64, _>("route_generation") != request.route_generation
    {
        return StatusCode::CONFLICT.into_response();
    }
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
    if row
        .get::<Option<chrono::DateTime<chrono::Utc>>, _>("suspended_at")
        .is_some()
    {
        return (StatusCode::CONFLICT, "app is paused").into_response();
    }
    let lease_current = row.get::<bool, _>("lease_current");
    if !lease_current {
        return (StatusCode::CONFLICT, "job lease has expired").into_response();
    }
    let app_id = row.get::<Uuid, _>("app_id");
    let candidate = row
        .get::<Option<serde_json::Value>, _>("result_json")
        .and_then(|value| {
            serde_json::from_value::<hostlet_contracts::CandidateRuntime>(value).ok()
        });
    // Consume the execution authority before changing the app or deployment.
    // The strict lease predicate is repeated in the write, so a lease that
    // expires after the SELECT cannot still cross the activation boundary.
    let job_completed = sqlx::query(
        "UPDATE agent_jobs
         SET status='success',payload_json=payload_json-'env'-'github_token'-'artifact_registry',
             lease_expires_at=NULL,updated_at=now(),finished_at=now()
         WHERE id=$1 AND server_id=$2 AND status IN ('claimed','running')
           AND claim_token=$3 AND lease_expires_at > clock_timestamp()
           AND cancel_requested_at IS NULL",
    )
    .bind(request.job_id)
    .bind(server_id)
    .bind(request.claim_token)
    .execute(&mut *tx)
    .await;
    match job_completed {
        Ok(done) if done.rows_affected() == 1 => {}
        Ok(_) => return (StatusCode::CONFLICT, "activation authority expired").into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
    let app_updated = sqlx::query(
        "UPDATE apps SET current_deployment_id=$1,pending_deployment_id=NULL,
                domain=COALESCE($2,domain),updated_at=now()
         WHERE id=$3 AND pending_deployment_id=$1 AND route_generation=$4",
    )
    .bind(deployment_id)
    .bind(request.local_url.as_deref())
    .bind(app_id)
    .bind(request.route_generation)
    .execute(&mut *tx)
    .await;
    match app_updated {
        Ok(done) if done.rows_affected() == 1 => {}
        Ok(_) => return (StatusCode::CONFLICT, "pending activation changed").into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
    let deployment_updated = sqlx::query(
        "UPDATE deployments SET status=$2,failure_summary=NULL,failure_code=NULL,
                runtime_metadata=COALESCE($3,runtime_metadata),
                finished_at=now(),last_heartbeat_at=now()
         WHERE id=$1 AND status = ANY($4)",
    )
    .bind(deployment_id)
    .bind(terminal_status)
    .bind(&request.runtime_metadata)
    .bind(ACTIVE_DEPLOYMENT_STATUSES)
    .execute(&mut *tx)
    .await;
    match deployment_updated {
        Ok(done) if done.rows_affected() == 1 => {}
        Ok(_) => return (StatusCode::CONFLICT, "deployment activation changed").into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
    if sqlx::query(
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
        for service in candidate
            .services
            .into_iter()
            .take(hostlet_contracts::DEPLOYMENT_SERVICE_REPORT_MAX)
        {
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
    let previous_deployment_id = row.get::<Option<Uuid>, _>("current_deployment_id");
    if schedule_previous_runtime_stop(
        &mut tx,
        server_id,
        app_id,
        previous_deployment_id,
        deployment_id,
    )
    .await
    .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
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

async fn schedule_previous_runtime_stop(
    tx: &mut Transaction<'_, Postgres>,
    server_id: Uuid,
    app_id: Uuid,
    previous_deployment_id: Option<Uuid>,
    new_deployment_id: Uuid,
) -> anyhow::Result<()> {
    // A rollback can make the target of an older delayed stop current again.
    // Cancel every still-queued delayed stop before scheduling the newly
    // superseded runtime.
    sqlx::query(
        "UPDATE agent_jobs
         SET status='cancelled',
             failure_summary='Superseded by a newer activation decision.',
             payload_json='{}'::jsonb,
             updated_at=now(),finished_at=now()
         WHERE app_id=$1 AND job_type='stop_previous_deployment' AND status='queued'",
    )
    .bind(app_id)
    .execute(&mut **tx)
    .await?;
    let Some(previous_deployment_id) = previous_deployment_id.filter(|id| *id != new_deployment_id)
    else {
        return Ok(());
    };
    let target = sqlx::query(
        "SELECT d.container_name,d.compose_project,d.published_port,
                a.container_port,a.health_path,a.domain
         FROM deployments d
         JOIN apps a ON a.id=d.app_id
         WHERE d.id=$1 AND d.app_id=$2 AND d.server_id=$3",
    )
    .bind(previous_deployment_id)
    .bind(app_id)
    .bind(server_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(target) = target else {
        return Ok(());
    };
    let (Some(container_name), Some(published_port)) = (
        target.get::<Option<String>, _>("container_name"),
        target.get::<Option<i32>, _>("published_port"),
    ) else {
        return Ok(());
    };
    let payload = serde_json::json!({
        "type": "stop_previous_deployment",
        "app_id": app_id,
        "deployment_id": previous_deployment_id,
        "container_name": container_name,
        "compose_project": target.get::<Option<String>, _>("compose_project"),
        "container_port": target.get::<i32, _>("container_port"),
        "published_port": published_port,
        "health_path": target.get::<String, _>("health_path"),
        "domain": target.get::<String, _>("domain"),
        "route_key": format!("app-{app_id}"),
    });
    let job_id = crate::deploy::insert_agent_job_in_transaction(
        tx,
        server_id,
        Some(app_id),
        Some(previous_deployment_id),
        "stop_previous_deployment",
        payload,
        20,
    )
    .await?;
    sqlx::query(
        "UPDATE agent_jobs
         SET available_at=now() + interval '15 minutes'
         WHERE id=$1",
    )
    .bind(job_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
