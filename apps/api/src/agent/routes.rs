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
               d.id AS deployment_id,
               d.container_name,
               d.published_port
        FROM apps a
        JOIN deployments d ON d.id = a.current_deployment_id
        WHERE a.server_id=$1
          AND d.server_id=$1
          AND d.status IN ('success','rolled_back')
          AND d.container_name IS NOT NULL
          AND d.published_port IS NOT NULL
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
                    serde_json::json!({
                        "appId": row.get::<Uuid, _>("app_id"),
                        "deploymentId": row.get::<Uuid, _>("deployment_id"),
                        "containerName": row.get::<String, _>("container_name"),
                        "publishedPort": row.get::<i32, _>("published_port"),
                        "healthPath": row.get::<String, _>("health_path"),
                    })
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

    // Free up any of this server's own jobs whose lease expired before we look
    // for new work, so a crashed-and-restarted agent can re-claim them.
    requeue_expired_jobs_for_server(&state, server_id).await;

    match claim_next_queued_job(&state, server_id, agent_id).await {
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
const RETRYABLE_EXPIRED_JOBS_PREDICATE: &str = "status IN ('claimed','running')
           AND lease_expires_at < now()
           AND attempt < max_attempts";

/// SET clause that returns an expired job to the queue, clearing claim/lease state.
const REQUEUE_JOB_SET_CLAUSE: &str = "SET status='queued',
             claimed_by=NULL,
             claimed_at=NULL,
             lease_expires_at=NULL,
             updated_at=now()";

/// Requeue this server's own expired-but-retryable jobs (scoped lease recovery).
async fn requeue_expired_jobs_for_server(state: &AppState, server_id: Uuid) {
    let _ = sqlx::query(&format!(
        "UPDATE agent_jobs
         {REQUEUE_JOB_SET_CLAUSE}
         WHERE server_id=$1
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
            attempt=attempt + 1,
            claimed_by=$2,
            claimed_at=now(),
            lease_expires_at=now() + interval '5 minutes',
            started_at=COALESCE(started_at, now()),
            updated_at=now()
        WHERE id = (
            SELECT id
            FROM agent_jobs
            WHERE server_id=$1
              AND status='queued'
              AND COALESCE(payload_json, '{}'::jsonb) <> '{}'::jsonb
              AND (job_type <> 'docker_cleanup' OR NOT EXISTS (
                SELECT 1 FROM deployments d
                WHERE d.server_id=$1 AND d.status = ANY($3)
              ))
            ORDER BY priority ASC, created_at ASC
            FOR UPDATE SKIP LOCKED
            LIMIT 1
        )
        RETURNING id, job_type, app_id, deployment_id, payload_json, attempt
        "#,
    )
    .bind(server_id)
    .bind(agent_id)
    .bind(crate::deploy::ACTIVE_DEPLOYMENT_STATUSES)
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
    let mut payload = row.get::<serde_json::Value, _>("payload_json");
    if let Some(object) = payload.as_object_mut() {
        object.insert("job_id".into(), serde_json::json!(row.get::<Uuid, _>("id")));
        object.insert(
            "job_type".into(),
            serde_json::json!(row.get::<String, _>("job_type")),
        );
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
    if !matches!(request.status.as_str(), "success" | "failed") {
        return (StatusCode::BAD_REQUEST, "invalid job status").into_response();
    }
    let result = sqlx::query(
        "UPDATE agent_jobs
         SET status=$1,
             failure_summary=$2,
             last_error=$2,
             result_json=$3,
             lease_expires_at=NULL,
             updated_at=now(),
             finished_at=now()
         WHERE id=$4 AND server_id=$5 AND status IN ('claimed','running')
         RETURNING job_type",
    )
    .bind(&request.status)
    .bind(request.failure.as_deref())
    .bind(request.result.unwrap_or_else(|| serde_json::json!({})))
    .bind(id)
    .bind(server_id)
    .fetch_optional(&state.db)
    .await;

    match result {
        Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::warn!(error = %err, job_id = %id, "failed to complete agent job");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn recover_stale_agent_jobs(state: &AppState) -> anyhow::Result<u64> {
    // Expired jobs with retries left go back to the queue (same rule as the
    // per-server requeue in `claim_job`, shared via the constant predicate).
    let retried = sqlx::query(&format!(
        "UPDATE agent_jobs
         {REQUEUE_JOB_SET_CLAUSE}
         WHERE {RETRYABLE_EXPIRED_JOBS_PREDICATE}"
    ))
    .execute(&state.db)
    .await?
    .rows_affected();

    // Expired jobs that have exhausted their attempts are marked failed.
    let failed = sqlx::query(
        "UPDATE agent_jobs
         SET status='failed',
             failure_summary=COALESCE(failure_summary, 'Agent job lease expired and retry limit was reached.'),
             last_error=COALESCE(last_error, 'Agent job lease expired and retry limit was reached.'),
             lease_expires_at=NULL,
             updated_at=now(),
             finished_at=now()
         WHERE status IN ('claimed','running')
           AND lease_expires_at < now()
           AND attempt >= max_attempts",
    )
    .execute(&state.db)
    .await?
    .rows_affected();

    Ok(retried + failed)
}
