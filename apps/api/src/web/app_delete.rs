use super::*;

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

pub(in crate::web) async fn finalize_delete_app_from_job(
    state: &AppState,
    job_id: Uuid,
) -> anyhow::Result<bool> {
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

pub(in crate::web) async fn delete_app_records(
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

pub(in crate::web) async fn app_belongs_to_user(
    state: &AppState,
    app_id: Uuid,
    user_id: Uuid,
) -> bool {
    matches!(
        sqlx::query("SELECT 1 FROM apps WHERE id=$1 AND user_id=$2")
            .bind(app_id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await,
        Ok(Some(_))
    )
}

pub(in crate::web) async fn app_domain_in_use(
    state: &AppState,
    domain: &str,
    except_app_id: Option<Uuid>,
) -> bool {
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

pub(in crate::web) async fn delete_created_app_row(state: &AppState, app_id: Uuid) {
    let _ = sqlx::query("DELETE FROM apps WHERE id=$1")
        .bind(app_id)
        .execute(&state.db)
        .await;
}

pub(in crate::web) async fn compensate_failed_app_update_dns(
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

pub(in crate::web) async fn mark_agent_job_failed(state: &AppState, job_id: Uuid, failure: &str) {
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
