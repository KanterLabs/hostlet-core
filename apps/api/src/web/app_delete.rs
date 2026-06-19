use super::apps::request_context_or_response;
use super::*;

pub async fn delete_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match request_context_or_response(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
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
        return delete_app_synchronously(&state, id, user_id, &domain, public_exposure).await;
    }
    enqueue_app_teardown(
        &state,
        &app,
        id,
        user_id,
        &domain,
        public_exposure,
        &deployment_rows,
    )
    .await
}

/// Tear an app down immediately when it has no deployments to clean up: close
/// any public DNS, then delete its records in one transaction.
async fn delete_app_synchronously(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
    domain: &str,
    public_exposure: bool,
) -> Response {
    if public_exposure {
        if let Err(response) = close_public_app_dns(state, id, domain).await {
            return response;
        }
    }
    match delete_app_records(state, id, user_id, &[]).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::warn!(error = %err, app_id = %id, "failed to delete app records");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Enqueue an asynchronous teardown job for an app that has deployments (and
/// therefore containers/images that an agent must remove off-box first).
async fn enqueue_app_teardown(
    state: &AppState,
    app: &sqlx::postgres::PgRow,
    id: Uuid,
    user_id: Uuid,
    domain: &str,
    public_exposure: bool,
    deployment_rows: &[sqlx::postgres::PgRow],
) -> Response {
    if public_exposure && state.cloudflare_api_token.is_none() {
        tracing::warn!(app_id = %id, domain = %domain, "public app deletion will require Cloudflare DNS cleanup but Cloudflare is not configured");
    }
    let containers = dedup_column(deployment_rows, "container_name");
    let images = dedup_column(deployment_rows, "image_tag");
    let server_id = app.get::<Uuid, _>("server_id");
    let payload = serde_json::json!({
        "type": "delete_app",
        "app_id": id,
        "route_key": format!("app-{id}"),
        "domain": domain,
        "user_id": user_id,
        "public_exposure": public_exposure,
        "compose_project": format!("hostlet-app-{}", id.simple()),
        "containers": containers,
        "images": images,
    });
    let job_id =
        match deploy::enqueue_agent_job(state, server_id, Some(id), None, "delete_app", payload, 5)
            .await
        {
            Ok(job_id) => job_id,
            Err(err) => {
                tracing::warn!(error = %err, app_id = %id, "failed to enqueue app teardown job");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };
    record_audit_event(
        state,
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

/// Collect a sorted, de-duplicated list of the non-null string values in
/// `column` across all `rows`.
fn dedup_column(rows: &[sqlx::postgres::PgRow], column: &str) -> Vec<String> {
    let mut values = rows
        .iter()
        .filter_map(|row| row.get::<Option<String>, _>(column))
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

/// Close the public tunnel DNS for a deleting app, mapping a failure onto the
/// `502 Bad Gateway` response the handler returns to the caller.
async fn close_public_app_dns(state: &AppState, id: Uuid, domain: &str) -> Result<(), Response> {
    delete_cloudflare_app_dns(state, id, domain).await.map_err(|err| {
        tracing::warn!(error = %err, domain = %domain, "failed to remove public tunnel DNS while deleting app");
        (
            StatusCode::BAD_GATEWAY,
            "failed to close public tunnel for app domain",
        )
            .into_response()
    })
}

pub async fn finalize_delete_app_from_job(state: &AppState, job_id: Uuid) -> anyhow::Result<bool> {
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
            crate::job_control::mark_agent_job_failed(state, job_id, &err.to_string()).await;
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
            crate::job_control::mark_agent_job_failed(
                state,
                job_id,
                "app disappeared before deletion completed",
            )
            .await;
            Ok(false)
        }
        Err(err) => {
            tracing::warn!(error = %err, app_id = %app_id, "failed to delete app records after cleanup");
            crate::job_control::mark_agent_job_failed(state, job_id, &err.to_string()).await;
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
    let mut tx = state.db.begin().await?;
    if !containers.is_empty()
        && sqlx::query("DELETE FROM app_resource_snapshots WHERE container_name = ANY($1)")
            .bind(containers)
            .execute(&mut *tx)
            .await
            .is_err()
    {
        anyhow::bail!("failed to delete resource snapshots");
    }
    // Collect screenshot storage paths inside the transaction before the app
    // row (and its cascaded app_screenshots rows) are deleted. The paths are
    // used for best-effort file cleanup after commit.
    let screenshot_paths: Vec<String> =
        sqlx::query_scalar("SELECT storage_path FROM app_screenshots WHERE app_id=$1")
            .bind(app_id)
            .fetch_all(&mut *tx)
            .await?;
    let res = sqlx::query("DELETE FROM apps WHERE id=$1 AND user_id=$2")
        .bind(app_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    let deleted = res.rows_affected() > 0;
    tx.commit().await?;
    // Best-effort file cleanup; only runs when the app row was actually deleted.
    // Paths containing path separators are rejected as a defensive measure.
    if deleted {
        for path in screenshot_paths {
            if path.contains('/') || path.contains('\\') {
                continue;
            }
            match tokio::fs::remove_file(state.screenshot_dir.join(&path)).await {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        path = %path,
                        "failed to remove screenshot file during app deletion"
                    );
                }
            }
        }
    }
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
    let sql = if except_app_id.is_some() {
        "SELECT 1 FROM apps WHERE lower(domain)=lower($1) AND id<>$2 LIMIT 1"
    } else {
        "SELECT 1 FROM apps WHERE lower(domain)=lower($1) LIMIT 1"
    };
    let mut query = sqlx::query(sql).bind(domain);
    if let Some(app_id) = except_app_id {
        query = query.bind(app_id);
    }
    matches!(query.fetch_optional(&state.db).await, Ok(Some(_)))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal test state: real DB (skipped when `HOSTLET_DB_TEST_URL` is
    /// absent) with a temp screenshot directory.
    async fn test_state() -> Option<AppState> {
        let mut state = crate::state::db_test_state_from_env().await?;
        state.screenshot_dir =
            std::env::temp_dir().join(format!("hostlet-del-test-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&state.screenshot_dir)
            .await
            .ok()?;
        Some(state)
    }

    async fn reset_db(state: &AppState) {
        sqlx::query(
            "TRUNCATE app_screenshots, agent_jobs, deployments, app_env_vars, apps, users CASCADE",
        )
        .execute(&state.db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn db_delete_app_records_removes_screenshot_file() {
        let Some(state) = test_state().await else {
            return;
        };
        reset_db(&state).await;

        let user_id: Uuid = sqlx::query_scalar(
            "INSERT INTO users (github_id, login) VALUES (9901, 'del-test-user') RETURNING id",
        )
        .fetch_one(&state.db)
        .await
        .unwrap();

        let app_id: Uuid = sqlx::query_scalar(
            "INSERT INTO apps
               (user_id,server_id,name,repo_full_name,branch,container_port,health_path,domain,runtime_kind,root_directory,public_exposure,auto_deploy)
             VALUES ($1,$2,'del-app','hostlet-ci/test','main',3000,'/health','del.example.test','single','.',false,false)
             RETURNING id",
        )
        .bind(user_id)
        .bind(state.local_server_id)
        .fetch_one(&state.db)
        .await
        .unwrap();

        let deployment_id: Uuid = sqlx::query_scalar(
            "INSERT INTO deployments
               (app_id,server_id,status,commit_sha,started_at,finished_at,runtime_kind,container_name,published_port)
             VALUES ($1,$2,'success','HEAD',now(),now(),'single','hostlet-del-test',32100)
             RETURNING id",
        )
        .bind(app_id)
        .bind(state.local_server_id)
        .fetch_one(&state.db)
        .await
        .unwrap();

        let job_id: Uuid = sqlx::query_scalar(
            "INSERT INTO agent_jobs
               (server_id,app_id,deployment_id,job_type,status,payload_json)
             VALUES ($1,$2,$3,'capture_screenshot','running','{}'::jsonb)
             RETURNING id",
        )
        .bind(state.local_server_id)
        .bind(app_id)
        .bind(deployment_id)
        .fetch_one(&state.db)
        .await
        .unwrap();

        let ss_id = Uuid::new_v4();
        let storage_path = format!("{ss_id}.jpg");
        let file_path = state.screenshot_dir.join(&storage_path);
        tokio::fs::write(&file_path, b"test").await.unwrap();
        sqlx::query(
            "INSERT INTO app_screenshots
               (id,app_id,deployment_id,agent_job_id,source,content_type,byte_size,storage_path,capture_url)
             VALUES ($1,$2,$3,$4,'generated','image/jpeg',4,$5,'u://x')",
        )
        .bind(ss_id)
        .bind(app_id)
        .bind(deployment_id)
        .bind(job_id)
        .bind(&storage_path)
        .execute(&state.db)
        .await
        .unwrap();

        let deleted = delete_app_records(&state, app_id, user_id, &[])
            .await
            .unwrap();
        assert!(deleted);

        let row_count: i64 = sqlx::query_scalar("SELECT count(*) FROM app_screenshots WHERE id=$1")
            .bind(ss_id)
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(row_count, 0, "screenshot row should be gone (CASCADE)");
        assert!(!file_path.exists(), "screenshot file should be deleted");
    }
}
