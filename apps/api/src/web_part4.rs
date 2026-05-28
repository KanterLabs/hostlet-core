pub async fn app_env_vars(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    if !app_belongs_to_user(&state, id, user_id).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    match sqlx::query("SELECT key FROM app_env_vars WHERE app_id=$1 ORDER BY key ASC")
        .bind(id)
        .fetch_all(&state.db)
        .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| serde_json::json!({"key": row.get::<String, _>("key")}))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn set_app_env_var(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, key)): Path<(Uuid, String)>,
    Json(body): Json<EnvValue>,
) -> impl IntoResponse {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(err) if err.to_string() == "sign in required" => {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(err) => return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response(),
    };
    let user_id = context.user_id;
    if !app_belongs_to_user(&state, id, user_id).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !valid_env_key(&key) {
        return (StatusCode::BAD_REQUEST, "invalid env var key").into_response();
    }
    if body.value.len() > 65_536 {
        return (StatusCode::BAD_REQUEST, "env var value is too large").into_response();
    }
    let Ok(enc) = state.crypto.encrypt(&body.value) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let res = sqlx::query(
        "INSERT INTO app_env_vars (app_id,key,value_ciphertext)
         VALUES ($1,$2,$3)
         ON CONFLICT (app_id,key) DO UPDATE SET value_ciphertext=EXCLUDED.value_ciphertext, updated_at=now()",
    )
    .bind(id)
    .bind(&key)
    .bind(enc)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => {
            record_audit_event(
                &state,
                AuditEventInput {
                    actor_type: "owner",
                    actor_id: Some(user_id.to_string()),
                    event_type: "app_env_var_changed",
                    app_id: Some(id),
                    deployment_id: None,
                    job_id: None,
                    metadata: serde_json::json!({"key": key}),
                },
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn delete_app_env_var(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, key)): Path<(Uuid, String)>,
) -> impl IntoResponse {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(err) if err.to_string() == "sign in required" => {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(err) => return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response(),
    };
    let user_id = context.user_id;
    if !app_belongs_to_user(&state, id, user_id).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !valid_env_key(&key) {
        return (StatusCode::BAD_REQUEST, "invalid env var key").into_response();
    }
    let res = sqlx::query("DELETE FROM app_env_vars WHERE app_id=$1 AND key=$2")
        .bind(id)
        .bind(&key)
        .execute(&state.db)
        .await;
    match res {
        Ok(_) => {
            record_audit_event(
                &state,
                AuditEventInput {
                    actor_type: "owner",
                    actor_id: Some(user_id.to_string()),
                    event_type: "app_env_var_deleted",
                    app_id: Some(id),
                    deployment_id: None,
                    job_id: None,
                    metadata: serde_json::json!({"key": key}),
                },
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn agent_job_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let row = sqlx::query(
        r#"
        SELECT j.id,j.job_type,j.app_id,j.status,j.failure_summary,j.finished_at
        FROM agent_jobs j
        JOIN servers s ON s.id = j.server_id
        WHERE j.id=$1
          AND (
            EXISTS (SELECT 1 FROM apps a WHERE a.id=j.app_id AND a.user_id=$2)
            OR EXISTS (
              SELECT 1 FROM deployments d
              JOIN apps a ON a.id=d.app_id
              WHERE d.id=j.deployment_id AND a.user_id=$2
            )
            OR (
              $3 = false
              AND j.app_id IS NULL
              AND j.deployment_id IS NULL
              AND (s.user_id=$2 OR s.kind='local')
            )
          )
        "#,
    )
    .bind(id)
    .bind(user_id)
    .bind(false)
    .fetch_optional(&state.db)
    .await;
    match row {
        Ok(Some(row)) => {
            let mut finalized_delete = false;
            if row.get::<String, _>("status") == "success"
                && row.get::<String, _>("job_type") == "delete_app"
                && row.get::<Option<Uuid>, _>("app_id").is_some()
            {
                finalized_delete = finalize_delete_app_from_job(&state, id)
                    .await
                    .unwrap_or(false);
            }
            let mut status = row.get::<String, _>("status");
            if status == "success"
                && row.get::<String, _>("job_type") == "delete_app"
                && row.get::<Option<Uuid>, _>("app_id").is_some()
                && !finalized_delete
            {
                status = "running".into();
            }
            Json(serde_json::json!({
            "id": row.get::<Uuid, _>("id"),
            "status": status,
            "failure": row.get::<Option<String>, _>("failure_summary"),
            "finishedAt": row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("finished_at")
            }))
            .into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn list_agent_jobs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let rows = sqlx::query(
        r#"
        SELECT j.id,j.job_type,j.app_id,j.deployment_id,j.status,j.failure_summary,
               j.attempt,j.max_attempts,j.claimed_by,j.created_at,j.updated_at,j.finished_at
        FROM agent_jobs j
        JOIN servers s ON s.id = j.server_id
        WHERE
          EXISTS (SELECT 1 FROM apps a WHERE a.id=j.app_id AND a.user_id=$1)
          OR EXISTS (
            SELECT 1 FROM deployments d
            JOIN apps a ON a.id=d.app_id
            WHERE d.id=j.deployment_id AND a.user_id=$1
          )
          OR (
            $2 = false
            AND j.app_id IS NULL
            AND j.deployment_id IS NULL
            AND (s.user_id=$1 OR s.kind='local')
          )
        ORDER BY j.created_at DESC
        LIMIT 200
        "#,
    )
    .bind(user_id)
    .bind(false)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| {
                    serde_json::json!({
                        "id": row.get::<Uuid, _>("id"),
                        "type": row.get::<String, _>("job_type"),
                        "appId": row.get::<Option<Uuid>, _>("app_id"),
                        "deploymentId": row.get::<Option<Uuid>, _>("deployment_id"),
                        "status": row.get::<String, _>("status"),
                        "failure": row.get::<Option<String>, _>("failure_summary"),
                        "attempt": row.get::<i32, _>("attempt"),
                        "maxAttempts": row.get::<i32, _>("max_attempts"),
                        "claimedBy": row.get::<Option<String>, _>("claimed_by"),
                        "createdAt": row.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
                        "updatedAt": row.get::<chrono::DateTime<chrono::Utc>, _>("updated_at"),
                        "finishedAt": row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("finished_at"),
                    })
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "failed to list agent jobs");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn retry_agent_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let result = sqlx::query(
        r#"
        UPDATE agent_jobs j
        SET status='queued',
            failure_summary=NULL,
            last_error=NULL,
            claimed_by=NULL,
            claimed_at=NULL,
            lease_expires_at=NULL,
            finished_at=NULL,
            updated_at=now()
        FROM servers s
        WHERE j.id=$1
          AND s.id=j.server_id
          AND (
            EXISTS (SELECT 1 FROM apps a WHERE a.id=j.app_id AND a.user_id=$2)
            OR EXISTS (
              SELECT 1 FROM deployments d
              JOIN apps a ON a.id=d.app_id
              WHERE d.id=j.deployment_id AND a.user_id=$2
            )
            OR (
              $3 = false
              AND j.app_id IS NULL
              AND j.deployment_id IS NULL
              AND (s.user_id=$2 OR s.kind='local')
            )
          )
          AND j.status IN ('failed','expired','cancelled')
          AND COALESCE(j.payload_json, '{}'::jsonb) <> '{}'::jsonb
        RETURNING j.app_id,j.deployment_id
        "#,
    )
    .bind(id)
    .bind(user_id)
    .bind(false)
    .fetch_optional(&state.db)
    .await;
    match result {
        Ok(Some(row)) => {
            record_audit_event(
                &state,
                AuditEventInput {
                    actor_type: "owner",
                    actor_id: None,
                    event_type: "agent_job_retried",
                    app_id: row.get::<Option<Uuid>, _>("app_id"),
                    deployment_id: row.get::<Option<Uuid>, _>("deployment_id"),
                    job_id: Some(id),
                    metadata: serde_json::json!({}),
                },
            )
            .await;
            StatusCode::ACCEPTED.into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::warn!(error = %err, job_id = %id, "failed to retry agent job");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn cancel_agent_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let result = sqlx::query(
        r#"
        UPDATE agent_jobs j
        SET status='cancelled',
            failure_summary='Cancelled by owner before the agent started work.',
            last_error='Cancelled by owner before the agent started work.',
            finished_at=now(),
            updated_at=now()
        FROM servers s
        WHERE j.id=$1
          AND s.id=j.server_id
          AND (
            EXISTS (SELECT 1 FROM apps a WHERE a.id=j.app_id AND a.user_id=$2)
            OR EXISTS (
              SELECT 1 FROM deployments d
              JOIN apps a ON a.id=d.app_id
              WHERE d.id=j.deployment_id AND a.user_id=$2
            )
            OR (
              $3 = false
              AND j.app_id IS NULL
              AND j.deployment_id IS NULL
              AND (s.user_id=$2 OR s.kind='local')
            )
          )
          AND j.status='queued'
        RETURNING j.app_id,j.deployment_id
        "#,
    )
    .bind(id)
    .bind(user_id)
    .bind(false)
    .fetch_optional(&state.db)
    .await;
    match result {
        Ok(Some(row)) => {
            record_audit_event(
                &state,
                AuditEventInput {
                    actor_type: "owner",
                    actor_id: None,
                    event_type: "agent_job_cancelled",
                    app_id: row.get::<Option<Uuid>, _>("app_id"),
                    deployment_id: row.get::<Option<Uuid>, _>("deployment_id"),
                    job_id: Some(id),
                    metadata: serde_json::json!({}),
                },
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::warn!(error = %err, job_id = %id, "failed to cancel agent job");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn delete_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(err) if err.to_string() == "sign in required" => {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(err) => return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response(),
    };
    let user_id = context.user_id;
    let app =
        sqlx::query("SELECT server_id,domain,public_exposure FROM apps WHERE id=$1 AND user_id=$2")
            .bind(id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await;
    let Ok(Some(app)) = app else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let domain = app.get::<String, _>("domain");
    let public_exposure = app.get::<bool, _>("public_exposure");
    let deployment_rows = match sqlx::query(
        "SELECT container_name,image_tag FROM deployments WHERE app_id=$1 ORDER BY created_at DESC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(error = %err, app_id = %id, "failed to read deployment metadata before deleting app");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if deployment_rows.is_empty() {
        if public_exposure {
            if let Err(err) = delete_cloudflare_app_dns(&state, id, &domain).await {
                tracing::warn!(error = %err, domain = %domain, "failed to remove public tunnel DNS while deleting app");
                return (
                    StatusCode::BAD_GATEWAY,
                    "failed to close public tunnel for app domain",
                )
                    .into_response();
            }
        }
        return match delete_app_records(&state, id, user_id, &[]).await {
            Ok(true) => StatusCode::NO_CONTENT.into_response(),
            Ok(false) => StatusCode::NOT_FOUND.into_response(),
            Err(err) => {
                tracing::warn!(error = %err, app_id = %id, "failed to delete app records");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        };
    }
    if public_exposure && state.cloudflare_api_token.is_none() {
        tracing::warn!(app_id = %id, domain = %domain, "public app deletion will require Cloudflare DNS cleanup but Cloudflare is not configured");
    }
    let mut containers = deployment_rows
        .iter()
        .filter_map(|row| row.get::<Option<String>, _>("container_name"))
        .collect::<Vec<_>>();
    containers.sort();
    containers.dedup();
    let mut images = deployment_rows
        .iter()
        .filter_map(|row| row.get::<Option<String>, _>("image_tag"))
        .collect::<Vec<_>>();
    images.sort();
    images.dedup();
    let server_id = app.get::<Uuid, _>("server_id");
    let payload = serde_json::json!({
        "type": "delete_app",
        "app_id": id,
        "route_key": format!("app-{id}"),
        "domain": domain,
        "user_id": user_id,
        "public_exposure": public_exposure,
        "compose_project": format!("hostlet-app-{}", id.simple()),
        "containers": containers.clone(),
        "images": images,
    });
    let job_id = match deploy::enqueue_agent_job(
        &state,
        server_id,
        Some(id),
        None,
        "delete_app",
        payload,
        5,
    )
    .await
    {
        Ok(job_id) => job_id,
        Err(err) => {
            tracing::warn!(error = %err, app_id = %id, "failed to enqueue app teardown job");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    record_audit_event(
        &state,
        AuditEventInput {
            actor_type: "owner",
            actor_id: None,
            event_type: "delete_app_requested",
            app_id: Some(id),
            deployment_id: None,
            job_id: Some(job_id),
            metadata: serde_json::json!({}),
        },
    )
    .await;
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"jobId": job_id})),
    )
        .into_response()
}

async fn finalize_delete_app_from_job(state: &AppState, job_id: Uuid) -> anyhow::Result<bool> {
    let row = sqlx::query(
        "SELECT app_id,payload_json FROM agent_jobs WHERE id=$1 AND job_type='delete_app' AND status='success'",
    )
    .bind(job_id)
    .fetch_optional(&state.db)
    .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let Some(app_id) = row.get::<Option<Uuid>, _>("app_id") else {
        return Ok(false);
    };
    let payload = row
        .get::<Option<serde_json::Value>, _>("payload_json")
        .unwrap_or_else(|| serde_json::json!({}));
    let mut user_id = payload
        .get("user_id")
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok());
    if user_id.is_none() {
        user_id = sqlx::query_scalar::<_, Uuid>("SELECT user_id FROM apps WHERE id=$1")
            .bind(app_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    }
    let Some(user_id) = user_id else {
        return Ok(false);
    };
    let domain = payload
        .get("domain")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let public_exposure = payload
        .get("public_exposure")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let containers = payload
        .get("containers")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if public_exposure {
        if let Err(err) = delete_cloudflare_app_dns(state, app_id, &domain).await {
            tracing::warn!(error = %err, domain = %domain, "failed to remove public tunnel DNS while deleting app");
            mark_agent_job_failed(state, job_id, &err.to_string()).await;
            return Err(err);
        }
    }
    match delete_app_records(state, app_id, user_id, &containers).await {
        Ok(true) => {
            record_audit_event(
                state,
                AuditEventInput {
                    actor_type: "system",
                    actor_id: None,
                    event_type: "app_deleted",
                    app_id: Some(app_id),
                    deployment_id: None,
                    job_id: Some(job_id),
                    metadata: serde_json::json!({}),
                },
            )
            .await;
            Ok(true)
        }
        Ok(false) => {
            mark_agent_job_failed(state, job_id, "app disappeared before deletion completed").await;
            Ok(false)
        }
        Err(err) => {
            tracing::warn!(error = %err, app_id = %app_id, "failed to delete app records after cleanup");
            mark_agent_job_failed(state, job_id, &err.to_string()).await;
            Err(err)
        }
    }
}

pub async fn reconcile_completed_delete_jobs(state: &AppState) -> anyhow::Result<u64> {
    let rows = sqlx::query(
        "SELECT id FROM agent_jobs WHERE job_type='delete_app' AND status='success' AND app_id IS NOT NULL",
    )
    .fetch_all(&state.db)
    .await?;
    let mut finalized = 0;
    for row in rows {
        if finalize_delete_app_from_job(state, row.get::<Uuid, _>("id")).await? {
            finalized += 1;
        }
    }
    Ok(finalized)
}

async fn delete_app_records(
    state: &AppState,
    app_id: Uuid,
    user_id: Uuid,
    containers: &[String],
) -> anyhow::Result<bool> {
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(err) => return Err(err.into()),
    };
    if !containers.is_empty()
        && sqlx::query("DELETE FROM app_resource_snapshots WHERE container_name = ANY($1)")
            .bind(containers)
            .execute(&mut *tx)
            .await
            .is_err()
    {
        anyhow::bail!("failed to delete resource snapshots");
    }
    let res = sqlx::query("DELETE FROM apps WHERE id=$1 AND user_id=$2")
        .bind(app_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    let deleted = res.rows_affected() > 0;
    tx.commit().await?;
    Ok(deleted)
}

async fn app_belongs_to_user(state: &AppState, app_id: Uuid, user_id: Uuid) -> bool {
    matches!(
        sqlx::query("SELECT 1 FROM apps WHERE id=$1 AND user_id=$2")
            .bind(app_id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await,
        Ok(Some(_))
    )
}

async fn app_domain_in_use(state: &AppState, domain: &str, except_app_id: Option<Uuid>) -> bool {
    match except_app_id {
        Some(app_id) => matches!(
            sqlx::query("SELECT 1 FROM apps WHERE lower(domain)=lower($1) AND id<>$2 LIMIT 1")
                .bind(domain)
                .bind(app_id)
                .fetch_optional(&state.db)
                .await,
            Ok(Some(_))
        ),
        None => matches!(
            sqlx::query("SELECT 1 FROM apps WHERE lower(domain)=lower($1) LIMIT 1")
                .bind(domain)
                .fetch_optional(&state.db)
                .await,
            Ok(Some(_))
        ),
    }
}

async fn delete_created_app_row(state: &AppState, app_id: Uuid) {
    let _ = sqlx::query("DELETE FROM apps WHERE id=$1")
        .bind(app_id)
        .execute(&state.db)
        .await;
}

async fn compensate_failed_app_update_dns(
    state: &AppState,
    old_domain: &str,
    app_domain: &str,
    app_id: Uuid,
    old_public_exposure: bool,
    desired_public_exposure: bool,
) {
    let opened_new_dns =
        desired_public_exposure && (!old_public_exposure || old_domain != app_domain);
    let closed_old_dns =
        old_public_exposure && (!desired_public_exposure || old_domain != app_domain);
    if opened_new_dns {
        if let Err(err) = delete_cloudflare_app_dns(state, app_id, app_domain).await {
            tracing::warn!(error = %err, domain = %app_domain, "failed to compensate new public tunnel after DB update failure");
        }
    }
    if closed_old_dns {
        if let Err(err) = ensure_cloudflare_app_dns(state, app_id, old_domain).await {
            tracing::warn!(error = %err, domain = %old_domain, "failed to restore old public tunnel after DB update failure");
        }
    }
}

async fn mark_agent_job_failed(state: &AppState, job_id: Uuid, failure: &str) {
    let _ = sqlx::query(
        "UPDATE agent_jobs
         SET status='failed', failure_summary=$2, updated_at=now(), finished_at=now()
         WHERE id=$1",
    )
    .bind(job_id)
    .bind(failure)
    .execute(&state.db)
    .await;
}

fn health_json(row: sqlx::postgres::PgRow) -> serde_json::Value {
    serde_json::json!({
        "appId": row.get::<Uuid, _>("id"),
        "deploymentId": row.get::<Option<Uuid>, _>("deployment_id"),
        "containerName": row.get::<Option<String>, _>("container_name"),
        "status": row.get::<String, _>("status"),
        "checkedUrl": row.get::<Option<String>, _>("checked_url"),
        "httpStatus": row.get::<Option<i32>, _>("http_status"),
        "latencyMs": row.get::<Option<i32>, _>("latency_ms"),
        "failureCount": row.get::<i32, _>("failure_count"),
        "successCount": row.get::<i32, _>("success_count"),
        "lastError": row.get::<Option<String>, _>("last_error"),
        "lastCheckedAt": row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("last_checked_at"),
        "lastHealthyAt": row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("last_healthy_at"),
        "updatedAt": row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("updated_at"),
    })
}

fn valid_env_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn validate_env_vars(env: &[EnvVar]) -> Result<(), &'static str> {
    let mut keys = HashSet::new();
    for ev in env {
        if !valid_env_key(&ev.key) {
            return Err("invalid env var key");
        }
        if ev.value.len() > 65_536 {
            return Err("env var value is too large");
        }
        if !keys.insert(ev.key.as_str()) {
            return Err("env var keys must be unique");
        }
    }
    Ok(())
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn clean_command(value: Option<String>) -> Result<Option<String>, &'static str> {
    let Some(value) = clean_optional(value) else {
        return Ok(None);
    };
    if value.len() > 500 || value.chars().any(|c| matches!(c, '\n' | '\r' | '\0')) {
        return Err("commands cannot contain newlines, NUL bytes, or more than 500 characters");
    }
    Ok(Some(value))
}

fn clean_runtime_kind(value: Option<&str>) -> Result<String, &'static str> {
    let value = value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("single");
    match value {
        "single" | "compose" => Ok(value.to_string()),
        _ => Err("runtime kind must be single or compose"),
    }
}

fn clean_packaging_strategy(value: Option<&str>) -> Result<String, &'static str> {
    let value = value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("auto");
    match value {
        "auto" | "dockerfile" | "generated" => Ok(value.to_string()),
        _ => Err("packaging strategy must be auto, dockerfile, or generated"),
    }
}

fn clean_hostlet_config_path(value: Option<&str>) -> Result<String, &'static str> {
    let value = value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("hostlet.yml");
    if valid_root_directory(value) && (value.ends_with(".yml") || value.ends_with(".yaml")) {
        Ok(value.to_string())
    } else {
        Err("Hostlet config path must be a relative .yml or .yaml file")
    }
}

