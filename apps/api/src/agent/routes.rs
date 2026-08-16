use super::health_target_payload::{route_shape, split_route_payload, RouteShape};
use super::*;

pub async fn register() -> impl IntoResponse {
    (
        StatusCode::GONE,
        "remote agent registration is deferred in this release; use the local Hostlet agent",
    )
        .into_response()
}

pub async fn ws(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let Some(server_id) = authenticated_server_id(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    ws.on_upgrade(move |socket| handle_socket(state, server_id, socket))
}

pub async fn event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(value): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(server_id) = authenticated_server_id(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    handle_agent_message(&state, server_id, value).await;
    StatusCode::ACCEPTED.into_response()
}

pub async fn health_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(server_id) = authenticated_server_id(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let rows = sqlx::query(
        r#"
        SELECT a.id AS app_id,
               a.health_path,
               a.container_port,
               a.domain,
               d.id AS deployment_id,
               d.container_name,
               d.published_port,
               COALESCE(primary_ds.target_port, a.container_port) AS target_port,
               primary_ds.service_name,
               d.runtime_metadata,
               a.route_generation,
               inferred_backend.service_count AS backend_service_count,
               inferred_backend.service_name AS backend_service_name,
               backend_ds.container_name AS backend_container_name,
               backend_ds.target_port AS backend_target_port,
               backend_ds.published_port AS backend_published_port
        FROM apps a
        JOIN deployments d
          ON d.id = a.current_deployment_id
         AND d.app_id = a.id
        LEFT JOIN LATERAL (
          SELECT ds.service_name, ds.target_port
          FROM deployment_services ds
          WHERE ds.deployment_id = d.id
            AND ds.app_id = d.app_id
            AND ds.container_name = d.container_name
            AND ds.role = 'web'
          ORDER BY ds.id ASC
          LIMIT 1
        ) primary_ds ON TRUE
        LEFT JOIN LATERAL (
          SELECT COUNT(*)::bigint AS service_count,
                 CASE WHEN COUNT(*)=1 THEN MIN(service->>'name') END AS service_name
          FROM jsonb_array_elements(
            CASE
              WHEN jsonb_typeof(d.runtime_metadata #> '{inferenceReceipt,services}') = 'array'
              THEN d.runtime_metadata #> '{inferenceReceipt,services}'
              ELSE '[]'::jsonb
            END
          ) AS service
          WHERE service->>'role' = 'backend'
        ) inferred_backend ON TRUE
        LEFT JOIN LATERAL (
          SELECT ds.container_name, ds.target_port, ds.published_port
          FROM deployment_services ds
          WHERE ds.deployment_id = d.id
            AND ds.app_id = d.app_id
            AND ds.service_name = inferred_backend.service_name
            AND ds.role = 'web'
          ORDER BY ds.id ASC
          LIMIT 1
        ) backend_ds ON TRUE
        WHERE a.server_id=$1
          AND d.server_id=$1
          AND d.status IN ('success','rolled_back')
          AND d.container_name IS NOT NULL
          AND d.published_port IS NOT NULL
          AND a.suspended_at IS NULL
          AND a.pending_deployment_id IS NULL
        ORDER BY a.created_at ASC
        "#,
    )
    .bind(server_id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| {
                    let service_name = row.get::<Option<String>, _>("service_name");
                    let metadata = row.get::<serde_json::Value, _>("runtime_metadata");
                    let default_health_path = row.get::<String, _>("health_path");
                    let inferred_probe = service_name.as_deref().and_then(|name| {
                        metadata
                            .pointer("/inferenceReceipt/services")?
                            .as_array()?
                            .iter()
                            .find(|service| {
                                service.get("name").and_then(serde_json::Value::as_str)
                                    == Some(name)
                            })
                            .and_then(|service| service.get("healthProbe"))
                    });
                    let probe_kind = inferred_probe
                        .and_then(|probe| probe.get("kind"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("http");
                    let health_path = inferred_probe
                        .and_then(|probe| probe.get("path"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&default_health_path);
                    let mut target = serde_json::json!({
                        "appId": row.get::<Uuid, _>("app_id"),
                        "deploymentId": row.get::<Uuid, _>("deployment_id"),
                        "containerName": row.get::<String, _>("container_name"),
                        "containerPort": row.get::<i32, _>("target_port"),
                        "publishedPort": row.get::<i32, _>("published_port"),
                        "healthPath": health_path,
                        "probeKind": probe_kind,
                        "serviceName": service_name,
                        "domain": row.get::<String, _>("domain"),
                        "routeKey": format!("app-{}", row.get::<Uuid, _>("app_id")),
                        "routeGeneration": row.get::<i64, _>("route_generation"),
                    });

                    match route_shape(&metadata, &row) {
                        RouteShape::Single => {}
                        RouteShape::Split => {
                            target["splitRoute"] =
                                split_route_payload(&row).unwrap_or(serde_json::Value::Null);
                        }
                        // A present null marker makes the agent reject the
                        // entire target rather than downgrade an immutable
                        // generated-topology route to a single proxy.
                        RouteShape::Invalid => {
                            target["splitRoute"] = serde_json::Value::Null;
                        }
                    }
                    target
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
pub struct ClaimJobRequest {
    pub(crate) agent_id: Option<String>,
    #[serde(default = "default_agent_protocol_version")]
    pub(crate) protocol_version: i32,
}

fn default_agent_protocol_version() -> i32 {
    1
}

pub async fn claim_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ClaimJobRequest>,
) -> impl IntoResponse {
    let Some(server_id) = authenticated_server_id(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let agent_id = request
        .agent_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("local-agent");
    let protocol_version = request
        .protocol_version
        .clamp(1, hostlet_contracts::DEPLOYMENT_PROTOCOL_VERSION);
    let _ =
        sqlx::query("UPDATE servers SET agent_protocol_version=$1,last_seen_at=now() WHERE id=$2")
            .bind(protocol_version)
            .bind(server_id)
            .execute(&state.db)
            .await;

    // Free up any of this server's own jobs whose lease expired before we look
    // for new work, so a crashed-and-restarted agent can re-claim them.
    let _ = reconcile_teardown_fenced_jobs(&state, Some(server_id)).await;
    requeue_expired_jobs_for_server(&state, server_id).await;

    match claim_next_queued_job(&state, server_id, agent_id, protocol_version).await {
        Ok(Some(row)) => claim_job_response(&state, server_id, row).await,
        Ok(None) => Json(serde_json::json!({"job": null})).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, %server_id, "failed to claim agent job");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Predicate identifying jobs whose lease has lapsed but still have retries left.
/// Shared verbatim with `recover_stale_agent_jobs` so the requeue rule cannot drift.
pub(crate) const RETRYABLE_EXPIRED_JOBS_PREDICATE: &str = "j.status IN ('claimed','running')
           AND j.lease_expires_at < clock_timestamp()
           AND j.attempt < j.max_attempts
           AND j.cancel_requested_at IS NULL
           AND (
             j.job_type IN ('docker_cleanup','delete_app')
             OR (
               j.app_id IS NOT NULL
               AND EXISTS (SELECT 1 FROM apps a WHERE a.id=j.app_id)
               AND NOT EXISTS (
                 SELECT 1 FROM agent_jobs deletion
                 WHERE deletion.app_id=j.app_id
                   AND deletion.job_type='delete_app'
                   AND (
                     deletion.status IN ('queued','claimed','running','success')
                     OR deletion.payload_json->>'teardown_fence'='true'
                   )
               )
             )
           )";

/// SET clause that returns an expired job to the queue, clearing claim/lease state.
pub(crate) const REQUEUE_JOB_SET_CLAUSE: &str = "SET status='queued',
             server_id=CASE WHEN build_pool_id IS NOT NULL THEN NULL ELSE server_id END,
             claimed_by=NULL,
             claimed_at=NULL,
             claim_token=NULL,
             lease_expires_at=NULL,
             available_at=now() + (LEAST(attempt, 6) * interval '10 seconds'),
             updated_at=now()";

/// Requeue this server's own expired-but-retryable jobs (scoped lease recovery).
async fn requeue_expired_jobs_for_server(state: &AppState, server_id: Uuid) {
    let _ = sqlx::query(&format!(
        "UPDATE agent_jobs j
         {REQUEUE_JOB_SET_CLAUSE}
         WHERE j.server_id=$1
           AND {RETRYABLE_EXPIRED_JOBS_PREDICATE}"
    ))
    .bind(server_id)
    .execute(&state.db)
    .await;
}

/// Atomically claim the highest-priority queued job for this server using
/// `FOR UPDATE SKIP LOCKED` so concurrent claimers never contend on the same row.
/// Jobs with an empty payload are skipped (`payload_json <> '{}'`) since they have
/// nothing for the agent to execute.
async fn claim_next_queued_job(
    state: &AppState,
    server_id: Uuid,
    agent_id: &str,
    protocol_version: i32,
) -> Result<Option<sqlx::postgres::PgRow>, sqlx::Error> {
    // The $3 parameter is ACTIVE_DEPLOYMENT_STATUSES.  The docker_cleanup job
    // payload freezes the keep lists at enqueue time; claiming it while a
    // deployment is in flight on this server could cause the agent to reap the
    // brand-new live container.  Defer docker_cleanup jobs until no deployment
    // is active.  All other job types are unaffected.
    sqlx::query(
        r#"
        UPDATE agent_jobs
        SET status='claimed',
            server_id=COALESCE(server_id,$1),
            attempt=attempt + 1,
            claim_token=uuid_generate_v4(),
            claimed_by=$2,
            claimed_at=now(),
            lease_expires_at=now() + interval '5 minutes',
            started_at=COALESCE(started_at, now()),
            updated_at=now()
        WHERE id = (
            SELECT id
            FROM agent_jobs j
            WHERE (
                j.server_id=$1
                OR (
                  j.server_id IS NULL
                  AND j.job_type='build'
                  AND j.build_pool_id=(SELECT s.build_pool_id FROM servers s WHERE s.id=$1)
                  AND (j.payload_json->>'required_platform') = ANY(
                    SELECT unnest(s.platforms) FROM servers s WHERE s.id=$1
                  )
                  AND EXISTS (
                    SELECT 1 FROM servers s
                    WHERE s.id=$1
                      AND s.status<>'revoked'
                      AND NOT s.draining
                      AND s.capabilities @> ARRAY['builder']::TEXT[]
                      AND (
                        SELECT COUNT(*) FROM agent_jobs active
                        WHERE active.server_id=$1
                          AND active.job_type='build'
                          AND active.status IN ('claimed','running')
                      ) < s.max_concurrent_builds
                  )
                )
              )
              AND j.status='queued'
              AND j.available_at <= now()
              AND j.protocol_version <= $4
              AND COALESCE(j.payload_json, '{}'::jsonb) <> '{}'::jsonb
              AND (
                j.job_type='docker_cleanup'
                OR (
                  j.job_type='delete_app'
                  AND (
                    j.app_id IS NULL
                    OR NOT EXISTS (
                      SELECT 1
                      FROM agent_jobs active
                      WHERE active.app_id=j.app_id
                        AND active.id<>j.id
                        AND active.job_type<>'delete_app'
                        AND active.status IN ('claimed','running')
                    )
                  )
                )
                OR (
                  j.job_type NOT IN ('docker_cleanup','delete_app')
                  AND j.app_id IS NOT NULL
                  AND EXISTS (SELECT 1 FROM apps a WHERE a.id=j.app_id)
                  AND NOT EXISTS (
                    SELECT 1
                    FROM agent_jobs deletion
                    WHERE deletion.app_id=j.app_id
                      AND deletion.job_type='delete_app'
                      AND (
                        deletion.status IN ('queued','claimed','running','success')
                        OR deletion.payload_json->>'teardown_fence'='true'
                      )
                  )
                )
              )
              AND (j.job_type <> 'docker_cleanup' OR NOT EXISTS (
                SELECT 1 FROM deployments d
                WHERE d.server_id=$1 AND d.status = ANY($3)
              ))
            ORDER BY j.priority ASC, j.created_at ASC
            FOR UPDATE SKIP LOCKED
            LIMIT 1
        )
        RETURNING id, job_type, app_id, deployment_id, payload_json, attempt,
                  claim_token,lease_expires_at,protocol_version
        "#,
    )
    .bind(server_id)
    .bind(agent_id)
    .bind(crate::deploy::ACTIVE_DEPLOYMENT_STATUSES)
    .bind(protocol_version)
    .fetch_optional(&state.db)
    .await
}

/// Shape a claimed job row into the signed JSON envelope returned to the agent:
/// inject `job_id`/`job_type` into the payload, sign the serialized payload, and
/// surface DB/secret/serialization failures as 500s.
async fn claim_job_response(
    state: &AppState,
    server_id: Uuid,
    row: sqlx::postgres::PgRow,
) -> axum::response::Response {
    if row.get::<String, _>("job_type") == "build" {
        if let Some(deployment_id) = row.get::<Option<Uuid>, _>("deployment_id") {
            let _ = sqlx::query(
                "UPDATE deployment_builds SET status='leased',waiting_reason=NULL,
                        started_at=COALESCE(started_at,now()),updated_at=now()
                 WHERE deployment_id=$1 AND status IN ('queued','retry_wait')",
            )
            .bind(deployment_id)
            .execute(&state.db)
            .await;
            let _ = sqlx::query(
                "UPDATE deployments SET status='building',last_heartbeat_at=now() WHERE id=$1",
            )
            .bind(deployment_id)
            .execute(&state.db)
            .await;
            let _ = sqlx::query("UPDATE servers SET last_build_assigned_at=now() WHERE id=$1")
                .bind(server_id)
                .execute(&state.db)
                .await;
        }
    }
    let mut payload = row.get::<serde_json::Value, _>("payload_json");
    if let Some(object) = payload.as_object_mut() {
        object.insert("job_id".into(), serde_json::json!(row.get::<Uuid, _>("id")));
        object.insert(
            "claim_token".into(),
            serde_json::json!(row.get::<Uuid, _>("claim_token")),
        );
        object.insert(
            "job_type".into(),
            serde_json::json!(row.get::<String, _>("job_type")),
        );
        if let Some(deployment_id) = row.get::<Option<Uuid>, _>("deployment_id") {
            let expected: Option<Uuid> = sqlx::query_scalar(
                "SELECT expected_current_deployment_id FROM deployments
                 WHERE id=$1 AND server_id=$2",
            )
            .bind(deployment_id)
            .bind(server_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .flatten();
            object.insert(
                "expected_current_deployment_id".into(),
                serde_json::json!(expected),
            );
        }
    }
    let secret = match crate::deploy::job_signing_secret_for_server(state, server_id).await {
        Ok(secret) => secret,
        Err(err) => {
            tracing::warn!(error = %err, %server_id, "failed to load job signing secret");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let body = match serde_json::to_vec(&payload) {
        Ok(body) => body,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    Json(serde_json::json!({
        "job": {
            "id": row.get::<Uuid, _>("id"),
            "type": row.get::<String, _>("job_type"),
            "appId": row.get::<Option<Uuid>, _>("app_id"),
            "deploymentId": row.get::<Option<Uuid>, _>("deployment_id"),
            "attempt": row.get::<i32, _>("attempt"),
            "claimToken": row.get::<Uuid, _>("claim_token"),
            "leaseExpiresAt": row.get::<chrono::DateTime<chrono::Utc>, _>("lease_expires_at"),
            "protocolVersion": row.get::<i32, _>("protocol_version"),
            "payload": payload,
            "signature": sign(&secret, &body),
        }
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct CompleteJobRequest {
    pub(crate) status: String,
    pub(crate) failure: Option<String>,
    pub(crate) result: Option<serde_json::Value>,
    #[serde(rename = "claimToken", default)]
    pub(crate) claim_token: Option<Uuid>,
}

pub async fn complete_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<CompleteJobRequest>,
) -> impl IntoResponse {
    let Some(server_id) = authenticated_server_id(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !matches!(request.status.as_str(), "success" | "failed" | "cancelled") {
        return (StatusCode::BAD_REQUEST, "invalid job status").into_response();
    }
    let Some(claim_token) = request.claim_token else {
        return (StatusCode::BAD_REQUEST, "claim token is required").into_response();
    };
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let selected = sqlx::query(
        "SELECT job_type,deployment_id,app_id,payload_json,status,claim_token,
                lease_expires_at,cancel_requested_at
         FROM agent_jobs
         WHERE id=$1 AND server_id=$2
         ",
    )
    .bind(id)
    .bind(server_id)
    .fetch_optional(&mut *tx)
    .await;
    let row = match selected {
        Ok(Some(row)) => row,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::warn!(error = %err, job_id = %id, "failed to load completed agent job");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let app_id = row.get::<Option<Uuid>, _>("app_id");
    let deployment_id = row.get::<Option<Uuid>, _>("deployment_id");
    if let Some(app_id) = app_id {
        if crate::agent::locks::app(&mut tx, app_id).await.is_err() {
            return StatusCode::CONFLICT.into_response();
        }
        if let Some(deployment_id) = deployment_id {
            if crate::agent::locks::deployment(&mut tx, deployment_id, app_id)
                .await
                .is_err()
            {
                return StatusCode::CONFLICT.into_response();
            }
        }
    }
    if crate::agent::locks::job(&mut tx, id, app_id, deployment_id)
        .await
        .is_err()
    {
        return StatusCode::CONFLICT.into_response();
    }
    let app_suspended = if let Some(app_id) = app_id {
        sqlx::query_scalar::<_, bool>("SELECT suspended_at IS NOT NULL FROM apps WHERE id=$1")
            .bind(app_id)
            .fetch_optional(&mut *tx)
            .await
            .unwrap_or(Some(true))
            == Some(true)
    } else {
        false
    };
    let current_status = row.get::<String, _>("status");
    if row.get::<Option<Uuid>, _>("claim_token") != Some(claim_token) {
        return (StatusCode::CONFLICT, "claim token is stale").into_response();
    }
    if matches!(current_status.as_str(), "success" | "failed" | "cancelled") {
        if current_status == request.status {
            // Activation commits the job before the agent's final REST
            // acknowledgement. A lost response must therefore be harmless,
            // but only for this exact terminal status and claim authority.
            return StatusCode::NO_CONTENT.into_response();
        }
        return (StatusCode::CONFLICT, "terminal job status does not match").into_response();
    }
    if !matches!(current_status.as_str(), "claimed" | "running") {
        return (StatusCode::CONFLICT, "job is no longer active").into_response();
    }
    let lease_expires_at = row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("lease_expires_at");
    let lease_current = match lease_expires_at {
        Some(expires_at) => {
            sqlx::query_scalar::<_, bool>("SELECT $1::timestamptz > clock_timestamp()")
                .bind(expires_at)
                .fetch_one(&mut *tx)
                .await
                .unwrap_or(false)
        }
        None => false,
    };
    if !lease_current {
        return (StatusCode::CONFLICT, "job lease has expired").into_response();
    }
    let cancellation_requested = row
        .get::<Option<chrono::DateTime<chrono::Utc>>, _>("cancel_requested_at")
        .is_some();
    if cancellation_requested && request.status != "cancelled" {
        return (StatusCode::CONFLICT, "job cancellation was requested").into_response();
    }
    let job_type = row.get::<String, _>("job_type");
    let original_payload = row.get::<serde_json::Value, _>("payload_json");
    if request.status == "success"
        && matches!(
            job_type.as_str(),
            "deploy" | "rollback" | "build" | "release"
        )
        && app_suspended
    {
        return (
            StatusCode::CONFLICT,
            "app is paused; successful execution is not permitted",
        )
            .into_response();
    }
    let completion_result = request
        .result
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));
    if job_type == "build" && request.status == "success" {
        if let Err(err) =
            super::build_execution::validate_build_manifest(&original_payload, &completion_result)
        {
            return (StatusCode::BAD_REQUEST, err.to_string()).into_response();
        }
    }

    let updated = sqlx::query(
        "UPDATE agent_jobs
         SET status=$1,
             failure_summary=$2,
             last_error=$2,
             result_json=$3,
             payload_json=payload_json - 'env' - 'github_token' - 'artifact_registry',
             lease_expires_at=NULL,
             updated_at=now(),
             finished_at=now()
         WHERE id=$4 AND server_id=$5
           AND status IN ('claimed','running')
           AND claim_token=$6
           AND lease_expires_at > clock_timestamp()
           AND (
             ($1 IN ('success','failed') AND cancel_requested_at IS NULL)
             OR ($1='cancelled' AND cancel_requested_at IS NOT NULL)
           )",
    )
    .bind(&request.status)
    .bind(request.failure.as_deref())
    .bind(&completion_result)
    .bind(id)
    .bind(server_id)
    .bind(claim_token)
    .execute(&mut *tx)
    .await;
    match updated {
        Ok(done) if done.rows_affected() == 1 => {
            if crate::browser_health::record_job_result(
                &mut tx,
                &job_type,
                app_id,
                deployment_id,
                &request.status,
                request.failure.as_deref(),
            )
            .await
            .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            if job_type == "build" && request.status == "success" {
                let (Some(deployment_id), Some(app_id)) = (deployment_id, app_id) else {
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                };
                if let Err(err) = super::build_execution::complete_successful_build(
                    &state,
                    &mut tx,
                    deployment_id,
                    app_id,
                    &original_payload,
                    &completion_result,
                )
                .await
                {
                    tracing::warn!(error = %err, job_id = %id, "failed to enqueue release job");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
            if let Some(deployment_id) = deployment_id {
                if request.status != "success" {
                    let deployment_status = if request.status == "cancelled" {
                        "canceled"
                    } else {
                        "failed"
                    };
                    if (job_type == "build"
                        && super::build_execution::fail_build(
                            &mut tx,
                            deployment_id,
                            &request.status,
                            request.failure.as_deref(),
                        )
                        .await
                        .is_err())
                        || sqlx::query(
                        "UPDATE deployments SET status=$1,failure_summary=$2,
                                failure_code=CASE WHEN $1='canceled' THEN 'cancelled_by_owner' ELSE failure_code END,
                                finished_at=now()
                         WHERE id=$3 AND ($4='build' OR server_id=$5) AND status = ANY($6)",
                    )
                    .bind(deployment_status)
                    .bind(request.failure.as_deref())
                    .bind(deployment_id)
                    .bind(&job_type)
                    .bind(server_id)
                    .bind(crate::deploy::ACTIVE_DEPLOYMENT_STATUSES)
                    .execute(&mut *tx)
                    .await
                    .is_err()
                        || sqlx::query(
                            "UPDATE apps SET pending_deployment_id=NULL
                             WHERE pending_deployment_id=$1",
                        )
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
                    if request.status == "success" && job_type == "delete_app" {
                        match crate::web::finalize_delete_app_from_job(&state, id).await {
                            Ok(_) => {}
                            Err(err) => {
                                tracing::warn!(
                                    error = %err,
                                    job_id = %id,
                                    "delete job completed but app finalization remains pending"
                                );
                            }
                        }
                    }
                    StatusCode::NO_CONTENT.into_response()
                }
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Ok(_) => (StatusCode::CONFLICT, "job authority changed").into_response(),
        Err(err) => {
            tracing::warn!(error = %err, job_id = %id, "failed to complete agent job");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn recover_stale_agent_jobs(state: &AppState) -> anyhow::Result<u64> {
    let teardown_cancelled = reconcile_teardown_fenced_jobs(state, None).await?;
    let cancelled = recovery_helpers::terminalize_expired_cancellations(state).await?;
    // Both expiry paths acquire app -> deployment -> job before changing the
    // job. This keeps recovery from inverting cancellation or activation.
    let retried = recovery_helpers::recover_expired_jobs(state, false).await?;
    let failed = recovery_helpers::recover_expired_jobs(state, true).await?;

    let scrubbed = recovery_helpers::scrub_terminal_job_payload_secrets(state).await?;
    if scrubbed > 0 {
        tracing::warn!(
            scrubbed,
            "scrubbed secrets from terminal agent job payloads"
        );
    }

    Ok(teardown_cancelled + cancelled + retried + failed)
}

pub(crate) async fn reconcile_teardown_fenced_jobs(
    state: &AppState,
    server_id: Option<Uuid>,
) -> anyhow::Result<u64> {
    let mut tx = state.db.begin().await?;
    let candidates = sqlx::query(
        "SELECT j.id,j.app_id,j.deployment_id,COALESCE(j.app_id,d.app_id) AS lifecycle_app_id
         FROM agent_jobs j
         LEFT JOIN deployments d ON d.id=j.deployment_id
         WHERE ($1::uuid IS NULL OR j.server_id=$1)
           AND j.job_type NOT IN ('docker_cleanup','delete_app')
           AND (
             j.app_id IS NULL
             OR EXISTS (
               SELECT 1
               FROM agent_jobs deletion
               WHERE deletion.app_id=j.app_id
                 AND deletion.job_type='delete_app'
                 AND (
                   deletion.status IN ('queued','claimed','running','success')
                   OR deletion.payload_json->>'teardown_fence'='true'
                 )
             )
           )
           AND j.status IN ('queued','claimed','running')
         ORDER BY j.id",
    )
    .bind(server_id)
    .fetch_all(&mut *tx)
    .await?;
    let job_ids = candidates
        .iter()
        .map(|row| row.get::<Uuid, _>("id"))
        .collect::<Vec<_>>();
    if job_ids.is_empty() {
        tx.commit().await?;
        return Ok(0);
    }
    recovery_helpers::lock_lifecycle_rows(
        &mut tx,
        candidates
            .iter()
            .filter_map(|row| row.get::<Option<Uuid>, _>("lifecycle_app_id"))
            .collect(),
        candidates
            .iter()
            .filter_map(|row| row.get::<Option<Uuid>, _>("deployment_id"))
            .collect(),
        job_ids.clone(),
    )
    .await?;
    let terminal = sqlx::query(
        "UPDATE agent_jobs j
         SET status='cancelled',
             failure_summary=COALESCE(
               failure_summary,
               'App was deleted or fenced before this job could run.'
             ),
             last_error=COALESCE(
               last_error,
               'App was deleted or fenced before this job could run.'
             ),
             payload_json='{}'::jsonb,
             claim_token=NULL,lease_expires_at=NULL,cancel_requested_at=NULL,
             updated_at=clock_timestamp(),finished_at=clock_timestamp()
         WHERE j.id=ANY($2)
           AND ($1::uuid IS NULL OR j.server_id=$1)
           AND j.job_type NOT IN ('docker_cleanup','delete_app')
           AND (
             j.app_id IS NULL
             OR EXISTS (
               SELECT 1
               FROM agent_jobs deletion
               WHERE deletion.app_id=j.app_id
                 AND deletion.job_type='delete_app'
                 AND (
                   deletion.status IN ('queued','claimed','running','success')
                   OR deletion.payload_json->>'teardown_fence'='true'
                 )
             )
           )
           AND (
             j.status='queued'
             OR (
               j.status IN ('claimed','running')
               AND j.lease_expires_at < clock_timestamp()
             )
           )
         RETURNING deployment_id",
    )
    .bind(server_id)
    .bind(&job_ids)
    .fetch_all(&mut *tx)
    .await?;
    let terminal_count = terminal.len() as u64;
    let mut deployment_ids = terminal
        .into_iter()
        .filter_map(|row| row.get::<Option<Uuid>, _>("deployment_id"))
        .collect::<Vec<_>>();
    deployment_ids.sort_unstable();
    deployment_ids.dedup();
    if !deployment_ids.is_empty() {
        sqlx::query(
            "UPDATE deployments
             SET status='canceled',failure_code='cancelled_by_owner',
                 failure_summary='App deletion fenced this deployment.',
                 finished_at=clock_timestamp()
             WHERE id = ANY($1) AND status = ANY($2)",
        )
        .bind(&deployment_ids)
        .bind(crate::deploy::ACTIVE_DEPLOYMENT_STATUSES)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE apps
             SET pending_deployment_id=NULL,updated_at=clock_timestamp()
             WHERE pending_deployment_id = ANY($1)",
        )
        .bind(&deployment_ids)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "UPDATE agent_jobs j
         SET cancel_requested_at=COALESCE(cancel_requested_at,clock_timestamp()),
             updated_at=clock_timestamp()
         WHERE j.id=ANY($2)
           AND ($1::uuid IS NULL OR j.server_id=$1)
           AND j.job_type NOT IN ('docker_cleanup','delete_app')
           AND j.status IN ('claimed','running')
           AND (j.lease_expires_at IS NULL OR j.lease_expires_at >= clock_timestamp())
           AND (
             j.app_id IS NULL
             OR EXISTS (
               SELECT 1
               FROM agent_jobs deletion
               WHERE deletion.app_id=j.app_id
                 AND deletion.job_type='delete_app'
                 AND (
                   deletion.status IN ('queued','claimed','running','success')
                   OR deletion.payload_json->>'teardown_fence'='true'
                 )
             )
           )",
    )
    .bind(server_id)
    .bind(&job_ids)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(terminal_count)
}
