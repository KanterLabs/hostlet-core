use super::*;

pub async fn cleanup_preview(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user_id = match request_context(&headers, &state).await {
        Ok(context) => context.user_id,
        Err(err) if err.to_string() == "sign in required" => {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(err) => return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response(),
    };
    match cleanup_plan(&state, user_id).await {
        Ok(plan) => Json(plan).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "failed to build cleanup preview");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn run_cleanup(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(err) if err.to_string() == "sign in required" => {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(err) => return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response(),
    };
    run_cleanup_inner(&state, Some(context.user_id)).await
}

pub(in crate::web) async fn run_cleanup_inner(state: &AppState, user_id: Option<Uuid>) -> axum::response::Response {
    let plan = match cleanup_plan(state, user_id.unwrap_or_else(Uuid::nil)).await {
        Ok(plan) => plan,
        Err(err) => {
            tracing::warn!(error = %err, "failed to build cleanup plan");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let db_deleted = match apply_database_cleanup(state).await {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(error = %err, "database cleanup failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let job_id = if let Some(server_id) = plan.local_server_id {
        match deploy::enqueue_agent_job(
            state,
            server_id,
            None,
            None,
            "docker_cleanup",
            serde_json::json!({
                "type": "docker_cleanup",
                "keep_containers": plan.keep_containers,
                "keep_images": plan.keep_images,
                "dry_run": false,
            }),
            50,
        )
        .await
        {
            Ok(job_id) => Some(job_id),
            Err(err) => {
                tracing::warn!(error = %err, "failed to enqueue Docker cleanup job");
                None
            }
        }
    } else {
        None
    };
    record_audit_event(
        state,
        AuditEventInput {
            actor_type: user_id.map(|_| "owner").unwrap_or("cli"),
            actor_id: user_id.map(|id| id.to_string()),
            event_type: "cleanup_requested",
            app_id: None,
            deployment_id: None,
            job_id,
            metadata: serde_json::json!({"databaseDeleted": db_deleted}),
        },
    )
    .await;
    Json(serde_json::json!({
        "databaseDeleted": db_deleted,
        "dockerCleanupJobId": job_id,
    }))
    .into_response()
}

#[derive(Serialize)]
pub(in crate::web) struct CleanupPlan {
    retention: CleanupRetention,
    database: CleanupDatabasePreview,
    docker: CleanupDockerPreview,
    #[serde(skip_serializing)]
    local_server_id: Option<Uuid>,
    #[serde(skip_serializing)]
    keep_containers: Vec<String>,
    #[serde(skip_serializing)]
    keep_images: Vec<String>,
}

#[derive(Serialize)]
struct CleanupRetention {
    deployment_log_days: i64,
    deployments_per_app: i64,
    health_event_days: i64,
    health_events_per_app: i64,
    resource_snapshot_days: i64,
    resource_snapshots_per_app: i64,
    webhook_event_days: i64,
    completed_agent_job_days: i64,
    failed_agent_job_days: i64,
}

#[derive(Serialize)]
struct CleanupDatabasePreview {
    deployment_logs: i64,
    health_events: i64,
    resource_snapshots: i64,
    webhook_events: i64,
    completed_agent_jobs: i64,
    failed_agent_jobs: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CleanupDockerPreview {
    keep_containers: usize,
    keep_images: usize,
    job_will_run: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CleanupDatabaseDeleted {
    deployment_logs: u64,
    health_events: u64,
    resource_snapshots: u64,
    webhook_events: u64,
    completed_agent_jobs: u64,
    failed_agent_jobs: u64,
}

const RETENTION: CleanupRetention = CleanupRetention {
    deployment_log_days: 30,
    deployments_per_app: 20,
    health_event_days: 7,
    health_events_per_app: 500,
    resource_snapshot_days: 7,
    resource_snapshots_per_app: 1000,
    webhook_event_days: 14,
    completed_agent_job_days: 30,
    failed_agent_job_days: 90,
};

pub(in crate::web) async fn cleanup_plan(state: &AppState, _user_id: Uuid) -> anyhow::Result<CleanupPlan> {
    let database = CleanupDatabasePreview {
        deployment_logs: cleanup_count(state, CLEANUP_DEPLOYMENT_LOGS).await?,
        health_events: cleanup_count(state, CLEANUP_HEALTH_EVENTS).await?,
        resource_snapshots: cleanup_count(state, CLEANUP_RESOURCE_SNAPSHOTS).await?,
        webhook_events: cleanup_count(state, CLEANUP_WEBHOOK_EVENTS).await?,
        completed_agent_jobs: cleanup_count(state, CLEANUP_COMPLETED_AGENT_JOBS).await?,
        failed_agent_jobs: cleanup_count(state, CLEANUP_FAILED_AGENT_JOBS).await?,
    };
    let keep_rows = sqlx::query(
        r#"
        WITH ranked AS (
          SELECT d.container_name,
                 d.image_tag,
                 row_number() OVER (
                   PARTITION BY d.app_id
                   ORDER BY
                     CASE WHEN a.current_deployment_id=d.id THEN 0 ELSE 1 END,
                     d.finished_at DESC NULLS LAST,
                     d.created_at DESC
                 ) AS rn
          FROM deployments d
          JOIN apps a ON a.id=d.app_id
          WHERE d.status IN ('success','rolled_back')
        )
        SELECT container_name,image_tag
        FROM ranked
        WHERE rn <= 2
        "#,
    )
    .fetch_all(&state.db)
    .await?;
    let mut keep_containers = keep_rows
        .iter()
        .filter_map(|row| row.get::<Option<String>, _>("container_name"))
        .collect::<Vec<_>>();
    keep_containers.sort();
    keep_containers.dedup();
    let mut keep_images = keep_rows
        .iter()
        .filter_map(|row| row.get::<Option<String>, _>("image_tag"))
        .collect::<Vec<_>>();
    keep_images.sort();
    keep_images.dedup();
    let local_server_id =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM servers WHERE kind='local' LIMIT 1")
            .fetch_optional(&state.db)
            .await?;
    Ok(CleanupPlan {
        retention: RETENTION,
        database,
        docker: CleanupDockerPreview {
            keep_containers: keep_containers.len(),
            keep_images: keep_images.len(),
            job_will_run: local_server_id.is_some(),
        },
        local_server_id,
        keep_containers,
        keep_images,
    })
}

async fn cleanup_count(state: &AppState, sql: &str) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(sql).fetch_one(&state.db).await?)
}

async fn cleanup_delete(state: &AppState, sql: &str) -> anyhow::Result<u64> {
    Ok(sqlx::query(sql).execute(&state.db).await?.rows_affected())
}

async fn apply_database_cleanup(state: &AppState) -> anyhow::Result<CleanupDatabaseDeleted> {
    Ok(CleanupDatabaseDeleted {
        deployment_logs: cleanup_delete(state, DELETE_DEPLOYMENT_LOGS).await?,
        health_events: cleanup_delete(state, DELETE_HEALTH_EVENTS).await?,
        resource_snapshots: cleanup_delete(state, DELETE_RESOURCE_SNAPSHOTS).await?,
        webhook_events: cleanup_delete(state, DELETE_WEBHOOK_EVENTS).await?,
        completed_agent_jobs: cleanup_delete(state, DELETE_COMPLETED_AGENT_JOBS).await?,
        failed_agent_jobs: cleanup_delete(state, DELETE_FAILED_AGENT_JOBS).await?,
    })
}

const CLEANUP_DEPLOYMENT_LOGS: &str = r#"
SELECT count(*)::bigint
FROM deployment_logs l
JOIN deployments d ON d.id=l.deployment_id
WHERE l.created_at < now() - interval '30 days'
  AND d.id NOT IN (
    SELECT id FROM (
      SELECT id,row_number() OVER (PARTITION BY app_id ORDER BY created_at DESC) AS rn
      FROM deployments
    ) ranked WHERE rn <= 20
  )
  AND NOT EXISTS (
    SELECT 1 FROM agent_jobs j
    WHERE j.deployment_id=d.id AND j.status IN ('queued','claimed','running')
  )
"#;

const DELETE_DEPLOYMENT_LOGS: &str = r#"
DELETE FROM deployment_logs l
USING deployments d
WHERE d.id=l.deployment_id
  AND l.created_at < now() - interval '30 days'
  AND d.id NOT IN (
    SELECT id FROM (
      SELECT id,row_number() OVER (PARTITION BY app_id ORDER BY created_at DESC) AS rn
      FROM deployments
    ) ranked WHERE rn <= 20
  )
  AND NOT EXISTS (
    SELECT 1 FROM agent_jobs j
    WHERE j.deployment_id=d.id AND j.status IN ('queued','claimed','running')
  )
"#;

const CLEANUP_HEALTH_EVENTS: &str = r#"
SELECT count(*)::bigint
FROM app_health_events e
WHERE e.created_at < now() - interval '7 days'
   OR e.id IN (
      SELECT id FROM (
        SELECT id,row_number() OVER (PARTITION BY app_id ORDER BY created_at DESC) AS rn
        FROM app_health_events
      ) ranked WHERE rn > 500
   )
"#;

const DELETE_HEALTH_EVENTS: &str = r#"
DELETE FROM app_health_events e
WHERE e.created_at < now() - interval '7 days'
   OR e.id IN (
      SELECT id FROM (
        SELECT id,row_number() OVER (PARTITION BY app_id ORDER BY created_at DESC) AS rn
        FROM app_health_events
      ) ranked WHERE rn > 500
   )
"#;

const CLEANUP_RESOURCE_SNAPSHOTS: &str = r#"
SELECT count(*)::bigint
FROM app_resource_snapshots s
WHERE s.sampled_at < now() - interval '7 days'
  AND NOT EXISTS (
    SELECT 1 FROM deployments d
    JOIN apps a ON a.current_deployment_id=d.id
    WHERE d.container_name=s.container_name
  )
"#;

const DELETE_RESOURCE_SNAPSHOTS: &str = r#"
DELETE FROM app_resource_snapshots s
WHERE s.sampled_at < now() - interval '7 days'
  AND NOT EXISTS (
    SELECT 1 FROM deployments d
    JOIN apps a ON a.current_deployment_id=d.id
    WHERE d.container_name=s.container_name
  )
"#;

const CLEANUP_WEBHOOK_EVENTS: &str = r#"
SELECT count(*)::bigint
FROM webhook_events e
WHERE e.created_at < now() - interval '14 days'
"#;

const DELETE_WEBHOOK_EVENTS: &str = r#"
DELETE FROM webhook_events e
WHERE e.created_at < now() - interval '14 days'
"#;

const CLEANUP_COMPLETED_AGENT_JOBS: &str = r#"
SELECT count(*)::bigint
FROM agent_jobs j
WHERE j.status IN ('success','cancelled')
  AND COALESCE(j.finished_at,j.updated_at,j.created_at) < now() - interval '30 days'
"#;

const DELETE_COMPLETED_AGENT_JOBS: &str = r#"
DELETE FROM agent_jobs j
WHERE j.status IN ('success','cancelled')
  AND COALESCE(j.finished_at,j.updated_at,j.created_at) < now() - interval '30 days'
"#;

const CLEANUP_FAILED_AGENT_JOBS: &str = r#"
SELECT count(*)::bigint
FROM agent_jobs j
WHERE j.status IN ('failed','expired')
  AND COALESCE(j.finished_at,j.updated_at,j.created_at) < now() - interval '90 days'
"#;

const DELETE_FAILED_AGENT_JOBS: &str = r#"
DELETE FROM agent_jobs j
WHERE j.status IN ('failed','expired')
  AND COALESCE(j.finished_at,j.updated_at,j.created_at) < now() - interval '90 days'
"#;

