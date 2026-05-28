use crate::{
    auth::{current_user_id, request_context, RequestContext},
    crypto::verify_token,
    deploy, github,
    state::AppState,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct CreateApp {
    name: String,
    repo_full_name: String,
    branch: String,
    server_id: Option<Uuid>,
    container_port: i32,
    health_path: String,
    domain: String,
    runtime_kind: Option<String>,
    hostlet_config_path: Option<String>,
    runtime_config: Option<serde_json::Value>,
    packaging_strategy: Option<String>,
    root_directory: Option<String>,
    install_command: Option<String>,
    build_command: Option<String>,
    start_command: Option<String>,
    memory_limit_mb: Option<i32>,
    cpu_limit: Option<f64>,
    public_exposure: Option<bool>,
    auto_deploy: Option<bool>,
    deploy_after_create: Option<bool>,
    env: Vec<EnvVar>,
}

#[derive(Deserialize)]
pub struct EnvVar {
    key: String,
    value: String,
}

#[derive(Deserialize)]
pub struct UpdateApp {
    domain: Option<String>,
    runtime_kind: Option<String>,
    hostlet_config_path: Option<String>,
    runtime_config: Option<serde_json::Value>,
    packaging_strategy: Option<String>,
    health_path: Option<String>,
    root_directory: Option<String>,
    install_command: Option<Option<String>>,
    build_command: Option<Option<String>>,
    start_command: Option<Option<String>>,
    container_port: Option<i32>,
    memory_limit_mb: Option<Option<i32>>,
    cpu_limit: Option<Option<f64>>,
    public_exposure: Option<bool>,
    auto_deploy: Option<bool>,
    env: Option<Vec<EnvVar>>,
}

#[derive(Deserialize)]
pub struct EnvValue {
    value: String,
}

async fn customer_context(
    headers: &HeaderMap,
    state: &AppState,
) -> Result<RequestContext, Response> {
    request_context(headers, state)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED.into_response())
}

pub async fn list_servers(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let Some(_user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match sqlx::query("SELECT id,name,public_ip,kind,status,last_seen_at,created_at FROM servers WHERE kind='local' ORDER BY created_at ASC")
        .fetch_all(&state.db).await {
        Ok(rows) => Json(rows.into_iter().map(|r| serde_json::json!({
            "id": r.get::<Uuid,_>("id"), "name": r.get::<String,_>("name"), "publicIp": r.get::<Option<String>,_>("public_ip"),
            "kind": r.get::<String,_>("kind"), "status": r.get::<String,_>("status"), "lastSeenAt": r.get::<Option<chrono::DateTime<chrono::Utc>>,_>("last_seen_at")
        })).collect::<Vec<_>>()).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn audit_events(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let rows = sqlx::query(
        r#"
        SELECT e.id,
               e.actor_type,
               e.actor_id,
               e.event_type,
               e.app_id,
               e.deployment_id,
               e.job_id,
               e.metadata_json,
               e.created_at
        FROM audit_events e
        WHERE e.app_id IS NULL
           OR EXISTS (
                SELECT 1 FROM apps a
                WHERE a.id=e.app_id AND a.user_id=$1
           )
        ORDER BY e.created_at DESC
        LIMIT 200
        "#,
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| {
                    serde_json::json!({
                        "id": row.get::<Uuid, _>("id"),
                        "actorType": row.get::<String, _>("actor_type"),
                        "actorId": row.get::<Option<String>, _>("actor_id"),
                        "eventType": row.get::<String, _>("event_type"),
                        "appId": row.get::<Option<Uuid>, _>("app_id"),
                        "deploymentId": row.get::<Option<Uuid>, _>("deployment_id"),
                        "jobId": row.get::<Option<Uuid>, _>("job_id"),
                        "metadata": row.get::<serde_json::Value, _>("metadata_json"),
                        "createdAt": row.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
                    })
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "failed to list audit events");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn backup_metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(_user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let row = sqlx::query("SELECT value FROM settings WHERE key='latest_backup_metadata'")
        .fetch_optional(&state.db)
        .await;
    match row {
        Ok(Some(row)) => {
            let value = row.get::<String, _>("value");
            match serde_json::from_str::<serde_json::Value>(&value) {
                Ok(value) => Json(value).into_response(),
                Err(_) => StatusCode::NO_CONTENT.into_response(),
            }
        }
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "failed to load backup metadata");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

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

async fn run_cleanup_inner(state: &AppState, user_id: Option<Uuid>) -> axum::response::Response {
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
struct CleanupPlan {
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

async fn cleanup_plan(state: &AppState, _user_id: Uuid) -> anyhow::Result<CleanupPlan> {
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

struct AuditEventInput<'a> {
    actor_type: &'a str,
    actor_id: Option<String>,
    event_type: &'a str,
    app_id: Option<Uuid>,
    deployment_id: Option<Uuid>,
    job_id: Option<Uuid>,
    metadata: serde_json::Value,
}

async fn record_audit_event(state: &AppState, event: AuditEventInput<'_>) {
    let _ = sqlx::query(
        "INSERT INTO audit_events
           (actor_type,actor_id,event_type,app_id,deployment_id,job_id,metadata_json)
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(event.actor_type)
    .bind(event.actor_id)
    .bind(event.event_type)
    .bind(event.app_id)
    .bind(event.deployment_id)
    .bind(event.job_id)
    .bind(event.metadata)
    .execute(&state.db)
    .await;
}

pub async fn create_server() -> impl IntoResponse {
    (
        StatusCode::GONE,
        "remote VPS agents are deferred in this release; deploy to this Hostlet machine",
    )
        .into_response()
}

pub async fn server_install_command() -> impl IntoResponse {
    (
        StatusCode::GONE,
        "remote VPS agents are deferred in this release; deploy to this Hostlet machine",
    )
        .into_response()
}

pub async fn list_apps(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let rows = sqlx::query(
        r#"
        SELECT
          a.id,
          a.name,
          a.repo_full_name,
          a.branch,
          a.domain,
          a.current_deployment_id,
          a.root_directory,
          a.runtime_kind,
          a.hostlet_config_path,
          a.runtime_config,
          a.packaging_strategy,
          a.install_command,
          a.build_command,
          a.start_command,
          a.container_port,
          a.health_path,
          a.memory_limit_mb,
          a.cpu_limit,
          a.public_exposure,
          a.auto_deploy,
          a.created_at,
          s.id AS server_id,
          s.name AS server_name,
          s.public_ip AS server_public_ip,
          s.kind AS server_kind,
          s.status AS server_status,
          s.last_seen_at AS server_last_seen_at,
          latest.id AS latest_deployment_id,
          latest.status AS latest_deployment_status,
          latest.commit_sha AS latest_commit_sha,
          latest.failure_summary AS latest_failure_summary,
          latest.started_at AS latest_started_at,
          latest.finished_at AS latest_finished_at,
          latest.runtime_metadata AS latest_runtime_metadata,
          current.status AS current_deployment_status,
          current.published_port AS current_published_port,
          current.finished_at AS current_deployment_finished_at,
          latest_webhook.status AS latest_webhook_status,
          latest_webhook.ignored_reason AS latest_webhook_ignored_reason,
          latest_webhook.commit_sha AS latest_webhook_commit_sha,
          latest_webhook.branch AS latest_webhook_branch,
          latest_webhook.deployment_id AS latest_webhook_deployment_id,
          latest_webhook.created_at AS latest_webhook_created_at,
          hs.status AS health_status,
          hs.http_status AS health_http_status,
          hs.latency_ms AS health_latency_ms,
          hs.failure_count AS health_failure_count,
          hs.success_count AS health_success_count,
          hs.last_error AS health_last_error,
          hs.last_checked_at AS health_last_checked_at,
          hs.last_healthy_at AS health_last_healthy_at,
          hs.updated_at AS health_updated_at
        FROM apps a
        JOIN servers s ON s.id = a.server_id
        LEFT JOIN LATERAL (
          SELECT id,status,commit_sha,failure_summary,started_at,finished_at,runtime_metadata
          FROM deployments
          WHERE app_id = a.id
          ORDER BY created_at DESC
          LIMIT 1
        ) latest ON true
        LEFT JOIN deployments current ON current.id = a.current_deployment_id
        LEFT JOIN LATERAL (
          SELECT status,ignored_reason,commit_sha,branch,deployment_id,created_at
          FROM webhook_app_events
          WHERE app_id = a.id
          ORDER BY created_at DESC
          LIMIT 1
        ) latest_webhook ON true
        LEFT JOIN app_health_snapshots hs ON hs.app_id = a.id
        WHERE a.user_id=$1
        ORDER BY a.created_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => Json(rows.into_iter().map(app_json).collect::<Vec<_>>()).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn get_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let app = sqlx::query(
        r#"
        SELECT
          a.id,
          a.name,
          a.repo_full_name,
          a.branch,
          a.domain,
          a.current_deployment_id,
          a.root_directory,
          a.runtime_kind,
          a.hostlet_config_path,
          a.runtime_config,
          a.packaging_strategy,
          a.install_command,
          a.build_command,
          a.start_command,
          a.container_port,
          a.health_path,
          a.memory_limit_mb,
          a.cpu_limit,
          a.public_exposure,
          a.auto_deploy,
          a.created_at,
          s.id AS server_id,
          s.name AS server_name,
          s.public_ip AS server_public_ip,
          s.kind AS server_kind,
          s.status AS server_status,
          s.last_seen_at AS server_last_seen_at,
          latest.id AS latest_deployment_id,
          latest.status AS latest_deployment_status,
          latest.commit_sha AS latest_commit_sha,
          latest.failure_summary AS latest_failure_summary,
          latest.started_at AS latest_started_at,
          latest.finished_at AS latest_finished_at,
          latest.runtime_metadata AS latest_runtime_metadata,
          current.status AS current_deployment_status,
          current.published_port AS current_published_port,
          current.finished_at AS current_deployment_finished_at,
          latest_webhook.status AS latest_webhook_status,
          latest_webhook.ignored_reason AS latest_webhook_ignored_reason,
          latest_webhook.commit_sha AS latest_webhook_commit_sha,
          latest_webhook.branch AS latest_webhook_branch,
          latest_webhook.deployment_id AS latest_webhook_deployment_id,
          latest_webhook.created_at AS latest_webhook_created_at,
          hs.status AS health_status,
          hs.http_status AS health_http_status,
          hs.latency_ms AS health_latency_ms,
          hs.failure_count AS health_failure_count,
          hs.success_count AS health_success_count,
          hs.last_error AS health_last_error,
          hs.last_checked_at AS health_last_checked_at,
          hs.last_healthy_at AS health_last_healthy_at,
          hs.updated_at AS health_updated_at
        FROM apps a
        JOIN servers s ON s.id = a.server_id
        LEFT JOIN LATERAL (
          SELECT id,status,commit_sha,failure_summary,started_at,finished_at,runtime_metadata
          FROM deployments
          WHERE app_id = a.id
          ORDER BY created_at DESC
          LIMIT 1
        ) latest ON true
        LEFT JOIN deployments current ON current.id = a.current_deployment_id
        LEFT JOIN LATERAL (
          SELECT status,ignored_reason,commit_sha,branch,deployment_id,created_at
          FROM webhook_app_events
          WHERE app_id = a.id
          ORDER BY created_at DESC
          LIMIT 1
        ) latest_webhook ON true
        LEFT JOIN app_health_snapshots hs ON hs.app_id = a.id
        WHERE a.id=$1 AND a.user_id=$2
        "#,
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await;
    match app {
        Ok(Some(row)) => Json(app_json(row)).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn app_health(
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
        SELECT a.id,
               hs.deployment_id,
               hs.container_name,
               COALESCE(hs.status, 'unknown') AS status,
               hs.checked_url,
               hs.http_status,
               hs.latency_ms,
               COALESCE(hs.failure_count, 0) AS failure_count,
               COALESCE(hs.success_count, 0) AS success_count,
               hs.last_error,
               hs.last_checked_at,
               hs.last_healthy_at,
               hs.updated_at
        FROM apps a
        LEFT JOIN app_health_snapshots hs ON hs.app_id = a.id
        WHERE a.id=$1 AND a.user_id=$2
        "#,
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await;
    match row {
        Ok(Some(row)) => Json(health_json(row)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn app_health_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let rows = sqlx::query(
        r#"
        SELECT e.id,
               e.deployment_id,
               e.container_name,
               e.status,
               e.checked_url,
               e.http_status,
               e.latency_ms,
               e.error,
               e.created_at
        FROM app_health_events e
        JOIN apps a ON a.id = e.app_id
        WHERE e.app_id=$1 AND a.user_id=$2
        ORDER BY e.created_at DESC
        LIMIT 100
        "#,
    )
    .bind(id)
    .bind(user_id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| {
                    serde_json::json!({
                        "id": row.get::<Uuid, _>("id"),
                        "deploymentId": row.get::<Option<Uuid>, _>("deployment_id"),
                        "containerName": row.get::<Option<String>, _>("container_name"),
                        "status": row.get::<String, _>("status"),
                        "checkedUrl": row.get::<Option<String>, _>("checked_url"),
                        "httpStatus": row.get::<Option<i32>, _>("http_status"),
                        "latencyMs": row.get::<Option<i32>, _>("latency_ms"),
                        "error": row.get::<Option<String>, _>("error"),
                        "createdAt": row.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
                    })
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn check_app_health_now(
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
    let row = sqlx::query(
        r#"
        SELECT a.server_id,
               a.health_path,
               d.id AS deployment_id,
               d.container_name,
               d.published_port
        FROM apps a
        LEFT JOIN deployments d ON d.id = a.current_deployment_id
        WHERE a.id=$1 AND a.user_id=$2
        "#,
    )
    .bind(id)
    .bind(context.user_id)
    .fetch_optional(&state.db)
    .await;
    let Ok(Some(row)) = row else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(deployment_id) = row.get::<Option<Uuid>, _>("deployment_id") else {
        return (
            StatusCode::BAD_REQUEST,
            "app does not have a current deployment",
        )
            .into_response();
    };
    let Some(container_name) = row.get::<Option<String>, _>("container_name") else {
        return (
            StatusCode::BAD_REQUEST,
            "app does not have a current container",
        )
            .into_response();
    };
    let Some(published_port) = row.get::<Option<i32>, _>("published_port") else {
        return (
            StatusCode::BAD_REQUEST,
            "app does not have a published runtime port",
        )
            .into_response();
    };
    let payload = serde_json::json!({
        "type": "health_check",
        "app_id": id,
        "deployment_id": deployment_id,
        "container_name": container_name,
        "published_port": published_port,
        "health_path": row.get::<String, _>("health_path"),
    });
    enqueue_interactive_agent_job(
        &state,
        row.get::<Uuid, _>("server_id"),
        id,
        Some(deployment_id),
        "health_check",
        payload,
    )
    .await
}

