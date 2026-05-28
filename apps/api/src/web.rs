use crate::{
    auth::{
        cloud_compute_allowed_for_context, cloud_request_ready, current_cloud_user_id,
        current_user_id, request_context, RequestContext,
    },
    crypto::verify_token,
    deploy, github,
    state::{AppState, HostletMode},
};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
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

#[derive(Deserialize)]
pub struct BillingPlanRequest {
    plan: String,
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
    let user_id = match cloud_request_ready(&headers, &state).await {
        Ok(user_id) => user_id,
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
    if state.mode == HostletMode::Cloud {
        return (
            StatusCode::FORBIDDEN,
            "cloud users cannot run global cleanup",
        )
            .into_response();
    }
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
    if let Err(err) = cloud_compute_allowed_for_context(&state, context).await {
        return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response();
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

pub async fn restart_app_container(
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
    if let Err(err) = cloud_compute_allowed_for_context(&state, context).await {
        return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response();
    }
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
        "type": "restart_container",
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
        "restart_container",
        payload,
    )
    .await
}

async fn enqueue_interactive_agent_job(
    state: &AppState,
    server_id: Uuid,
    app_id: Uuid,
    deployment_id: Option<Uuid>,
    job_type: &str,
    payload: serde_json::Value,
) -> axum::response::Response {
    match deploy::enqueue_agent_job(
        state,
        server_id,
        Some(app_id),
        deployment_id,
        job_type,
        payload,
        20,
    )
    .await
    {
        Ok(job_id) => {
            record_audit_event(
                state,
                AuditEventInput {
                    actor_type: "owner",
                    actor_id: None,
                    event_type: &format!("{job_type}_requested"),
                    app_id: Some(app_id),
                    deployment_id,
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
        Err(err) => {
            tracing::warn!(error = %err, app_id = %app_id, job_type, "failed to enqueue agent job");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn health_summary(
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
        SELECT COALESCE(hs.status, 'unknown') AS status, count(*) AS count
        FROM apps a
        LEFT JOIN app_health_snapshots hs ON hs.app_id = a.id
        WHERE a.user_id=$1
        GROUP BY COALESCE(hs.status, 'unknown')
        "#,
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => Json(health_counts_json(rows)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn system_health_counts(state: &AppState) -> serde_json::Value {
    let rows = sqlx::query(
        r#"
        SELECT COALESCE(hs.status, 'unknown') AS status, count(*) AS count
        FROM apps a
        LEFT JOIN app_health_snapshots hs ON hs.app_id = a.id
        GROUP BY COALESCE(hs.status, 'unknown')
        "#,
    )
    .fetch_all(&state.db)
    .await;
    health_counts_json(rows.unwrap_or_default())
}

fn health_counts_json(rows: Vec<sqlx::postgres::PgRow>) -> serde_json::Value {
    let mut counts = serde_json::json!({
        "healthy": 0,
        "degraded": 0,
        "unhealthy": 0,
        "unknown": 0
    });
    for row in rows {
        let status: String = row.get("status");
        if let Some(value) = counts.get_mut(&status) {
            *value = serde_json::json!(row.get::<i64, _>("count"));
        }
    }
    counts
}

fn stripe_price_for_plan<'a>(state: &'a AppState, plan: &str) -> Option<&'a str> {
    match plan.trim().to_ascii_lowercase().as_str() {
        "student" => state.stripe_price_student.as_deref(),
        "starter" => state.stripe_price_starter.as_deref(),
        "pro" => state.stripe_price_pro.as_deref(),
        _ => None,
    }
}

async fn ensure_stripe_customer(
    state: &AppState,
    secret_key: &str,
    cloud_user_id: Uuid,
) -> anyhow::Result<String> {
    if let Some(customer_id) = sqlx::query_scalar::<_, String>(
        "SELECT stripe_customer_id FROM cloud_stripe_customers WHERE cloud_user_id=$1",
    )
    .bind(cloud_user_id)
    .fetch_optional(&state.db)
    .await?
    {
        return Ok(customer_id);
    }
    let row = sqlx::query("SELECT login, email FROM cloud_users WHERE id=$1")
        .bind(cloud_user_id)
        .fetch_one(&state.db)
        .await?;
    let login = row.get::<String, _>("login");
    let email = row.get::<Option<String>, _>("email");
    let mut form = vec![
        ("name", login.as_str()),
        ("metadata[cloud_user_id]", ""),
        ("metadata[github_login]", login.as_str()),
    ];
    let cloud_user_id_string = cloud_user_id.to_string();
    form[1] = ("metadata[cloud_user_id]", cloud_user_id_string.as_str());
    if let Some(email) = email.as_deref() {
        form.push(("email", email));
    }
    let payload = state
        .http
        .post("https://api.stripe.com/v1/customers")
        .bearer_auth(secret_key)
        .form(&form)
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    let customer_id = payload
        .get("id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("Stripe customer response was missing id"))?
        .to_string();
    sqlx::query(
        "INSERT INTO cloud_stripe_customers (cloud_user_id, stripe_customer_id)
         VALUES ($1,$2)
         ON CONFLICT (cloud_user_id) DO UPDATE SET
           stripe_customer_id=EXCLUDED.stripe_customer_id,
           updated_at=now()",
    )
    .bind(cloud_user_id)
    .bind(&customer_id)
    .execute(&state.db)
    .await?;
    Ok(customer_id)
}

fn valid_stripe_signature(headers: &HeaderMap, body: &[u8], webhook_secret: &str) -> bool {
    let Some(signature) = headers
        .get("stripe-signature")
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in signature.split(',') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        match key {
            "t" => timestamp = Some(value),
            "v1" => signatures.push(value),
            _ => {}
        }
    }
    let Some(timestamp) = timestamp else {
        return false;
    };
    let Ok(timestamp_seconds) = timestamp.parse::<i64>() else {
        return false;
    };
    let age_seconds = (chrono::Utc::now().timestamp() - timestamp_seconds).abs();
    if age_seconds > 300 {
        return false;
    }
    let signed_payload = [timestamp.as_bytes(), b".", body].concat();
    let mut mac = Hmac::<Sha256>::new_from_slice(webhook_secret.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(&signed_payload);
    let expected = hex_bytes(&mac.finalize().into_bytes());
    signatures
        .iter()
        .any(|candidate| crate::crypto::constant_time_eq(candidate.as_bytes(), expected.as_bytes()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StripeWebhookAction {
    CheckoutCompleted,
    SubscriptionChanged,
    Ignore,
}

fn stripe_webhook_action(event_type: &str) -> StripeWebhookAction {
    match event_type {
        "checkout.session.completed" => StripeWebhookAction::CheckoutCompleted,
        "customer.subscription.created"
        | "customer.subscription.updated"
        | "customer.subscription.deleted" => StripeWebhookAction::SubscriptionChanged,
        _ => StripeWebhookAction::Ignore,
    }
}

async fn handle_checkout_completed(
    state: &AppState,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let object = payload
        .get("data")
        .and_then(|value| value.get("object"))
        .ok_or_else(|| anyhow::anyhow!("Stripe checkout event missing object"))?;
    let cloud_user_id = stripe_cloud_user_id(object)?;
    let customer_id = object
        .get("customer")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if !customer_id.is_empty() {
        sqlx::query(
            "INSERT INTO cloud_stripe_customers (cloud_user_id, stripe_customer_id)
             VALUES ($1,$2)
             ON CONFLICT (cloud_user_id) DO UPDATE SET
               stripe_customer_id=EXCLUDED.stripe_customer_id,
               updated_at=now()",
        )
        .bind(cloud_user_id)
        .bind(customer_id)
        .execute(&state.db)
        .await?;
    }
    let subscription_id = stripe_checkout_subscription_id(object)?;
    let plan = stripe_plan_code(object)?;
    sqlx::query(
        "INSERT INTO cloud_subscriptions (cloud_user_id, stripe_subscription_id, plan_code, status)
         VALUES ($1,$2,$3,'pending')
         ON CONFLICT (stripe_subscription_id) DO UPDATE SET
           plan_code=EXCLUDED.plan_code,
           status=CASE
             WHEN cloud_subscriptions.status IN ('active','trialing') THEN cloud_subscriptions.status
             ELSE 'pending'
           END,
           updated_at=now()",
    )
    .bind(cloud_user_id)
    .bind(subscription_id)
    .bind(plan)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn handle_subscription_event(
    state: &AppState,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let object = payload
        .get("data")
        .and_then(|value| value.get("object"))
        .ok_or_else(|| anyhow::anyhow!("Stripe subscription event missing object"))?;
    let subscription_id = object
        .get("id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("Stripe subscription missing id"))?;
    let cloud_user_id = match stripe_cloud_user_id(object) {
        Ok(id) => id,
        Err(_) => {
            let customer_id = object
                .get("customer")
                .and_then(|value| value.as_str())
                .ok_or_else(|| anyhow::anyhow!("Stripe subscription missing customer"))?;
            sqlx::query_scalar::<_, Uuid>(
                "SELECT cloud_user_id FROM cloud_stripe_customers WHERE stripe_customer_id=$1",
            )
            .bind(customer_id)
            .fetch_one(&state.db)
            .await?
        }
    };
    let status = object
        .get("status")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let plan = stripe_plan_code(object)?;
    let current_period_start = stripe_timestamp(object, "current_period_start");
    let current_period_end = stripe_timestamp(object, "current_period_end");
    let cancel_at_period_end = object
        .get("cancel_at_period_end")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    sqlx::query(
        "INSERT INTO cloud_subscriptions
           (cloud_user_id, stripe_subscription_id, plan_code, status, current_period_start, current_period_end, cancel_at_period_end)
         VALUES ($1,$2,$3,$4,$5,$6,$7)
         ON CONFLICT (stripe_subscription_id) DO UPDATE SET
           plan_code=EXCLUDED.plan_code,
           status=EXCLUDED.status,
           current_period_start=EXCLUDED.current_period_start,
           current_period_end=EXCLUDED.current_period_end,
           cancel_at_period_end=EXCLUDED.cancel_at_period_end,
           updated_at=now()",
    )
    .bind(cloud_user_id)
    .bind(subscription_id)
    .bind(plan)
    .bind(status)
    .bind(current_period_start)
    .bind(current_period_end)
    .bind(cancel_at_period_end)
    .execute(&state.db)
    .await?;
    Ok(())
}

fn stripe_cloud_user_id(object: &serde_json::Value) -> anyhow::Result<Uuid> {
    let value = object
        .get("metadata")
        .and_then(|value| value.get("cloud_user_id"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("Stripe object missing cloud_user_id metadata"))?;
    Ok(Uuid::parse_str(value)?)
}

fn stripe_checkout_subscription_id(object: &serde_json::Value) -> anyhow::Result<&str> {
    object
        .get("subscription")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("Stripe checkout session missing subscription id"))
}

fn stripe_plan_code(object: &serde_json::Value) -> anyhow::Result<&str> {
    let plan = object
        .get("metadata")
        .and_then(|value| value.get("plan"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .ok_or_else(|| anyhow::anyhow!("Stripe object missing plan metadata"))?;
    if matches!(plan, "student" | "starter" | "pro") {
        Ok(plan)
    } else {
        anyhow::bail!("Stripe object contains unsupported plan metadata")
    }
}

fn stripe_timestamp(
    object: &serde_json::Value,
    key: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    object
        .get(key)
        .and_then(|value| value.as_i64())
        .and_then(|timestamp| chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0))
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub async fn system_version(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(_user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let update = cached_update_check(&state).await;
    Json(serde_json::json!({
        "currentVersion": env!("CARGO_PKG_VERSION"),
        "mode": state.mode.as_str(),
        "updateChecksEnabled": state.update_checks_enabled,
        "update": update,
    }))
    .into_response()
}

pub async fn cloud_status(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let Some(_user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    Json(serde_json::json!({
        "mode": state.mode.as_str(),
        "enabled": state.mode == crate::state::HostletMode::Cloud,
        "publicWebUrl": state.public_web_url,
        "publicApiUrl": state.public_api_url,
        "baseDomain": state.base_domain.as_deref(),
        "githubOAuth": {
            "clientIdConfigured": !state.github_client_id.trim().is_empty(),
            "clientSecretConfigured": state.github_client_secret.is_some()
        },
        "githubApp": {
            "appIdConfigured": state.github_app_id.is_some(),
            "slugConfigured": state.github_app_slug.is_some(),
            "clientIdConfigured": state.github_app_client_id.is_some(),
            "clientSecretConfigured": state.github_app_client_secret.is_some(),
            "privateKeyConfigured": state.github_app_private_key_pem.is_some(),
            "webhookSecretConfigured": state.github_app_webhook_secret.is_some()
        },
        "stripe": {
            "secretKeyConfigured": state.stripe_secret_key.is_some(),
            "publishableKeyConfigured": state.stripe_publishable_key.is_some(),
            "webhookSecretConfigured": state.stripe_webhook_secret.is_some(),
            "studentPriceConfigured": state.stripe_price_student.is_some(),
            "starterPriceConfigured": state.stripe_price_starter.is_some(),
            "proPriceConfigured": state.stripe_price_pro.is_some()
        }
    }))
    .into_response()
}

pub async fn cloud_usage(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if state.mode != HostletMode::Cloud {
        return StatusCode::NOT_FOUND.into_response();
    }
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let app_count: i64 = match sqlx::query_scalar("SELECT count(*) FROM apps WHERE user_id=$1")
        .bind(context.user_id)
        .fetch_one(&state.db)
        .await
    {
        Ok(count) => count,
        Err(err) => {
            tracing::warn!(error = %err, "failed to load cloud app usage");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let subscription = match context.cloud_user_id {
        Some(cloud_user_id) => {
            sqlx::query(
                r#"
            SELECT s.plan_code,
                   s.status,
                   s.current_period_start,
                   s.current_period_end,
                   s.cancel_at_period_end,
                   e.app_limit
            FROM cloud_subscriptions s
            JOIN cloud_plan_entitlements e ON e.plan_code=s.plan_code
            WHERE s.cloud_user_id=$1
              AND s.status IN ('active','trialing')
              AND (s.current_period_end IS NULL OR s.current_period_end > now())
            ORDER BY s.created_at DESC
            LIMIT 1
            "#,
            )
            .bind(cloud_user_id)
            .fetch_optional(&state.db)
            .await
        }
        None => Ok(None),
    };
    let subscription = match subscription {
        Ok(row) => row,
        Err(err) => {
            tracing::warn!(error = %err, "failed to load cloud usage subscription");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let app_limit = subscription
        .as_ref()
        .map(|row| row.get::<i32, _>("app_limit"))
        .unwrap_or(0);
    Json(serde_json::json!({
        "planCode": subscription.as_ref().map(|row| row.get::<String, _>("plan_code")),
        "subscriptionStatus": subscription.as_ref().map(|row| row.get::<String, _>("status")),
        "currentPeriodStart": subscription.as_ref().and_then(|row| row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("current_period_start")),
        "currentPeriodEnd": subscription.as_ref().and_then(|row| row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("current_period_end")),
        "cancelAtPeriodEnd": subscription.as_ref().map(|row| row.get::<bool, _>("cancel_at_period_end")).unwrap_or(false),
        "apps": {
            "used": app_count,
            "limit": app_limit,
            "remaining": (i64::from(app_limit) - app_count).max(0)
        }
    }))
    .into_response()
}

pub async fn cloud_billing_checkout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BillingPlanRequest>,
) -> impl IntoResponse {
    if state.mode != HostletMode::Cloud {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(cloud_user_id) = current_cloud_user_id(&headers, &state).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(secret_key) = state.stripe_secret_key.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Stripe secret key is not configured",
        )
            .into_response();
    };
    let Some(price_id) = stripe_price_for_plan(&state, &body.plan) else {
        return (
            StatusCode::BAD_REQUEST,
            "unknown plan or missing Stripe price",
        )
            .into_response();
    };
    let customer_id = match ensure_stripe_customer(&state, secret_key, cloud_user_id).await {
        Ok(id) => id,
        Err(err) => {
            tracing::warn!(error = %err, "failed to prepare Stripe customer");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    let success_url = format!(
        "{}/?billing=success",
        state.public_web_url.trim_end_matches('/')
    );
    let cancel_url = format!(
        "{}/?billing=cancelled",
        state.public_web_url.trim_end_matches('/')
    );
    let res = state
        .http
        .post("https://api.stripe.com/v1/checkout/sessions")
        .bearer_auth(secret_key)
        .form(&[
            ("mode", "subscription"),
            ("customer", customer_id.as_str()),
            ("line_items[0][price]", price_id),
            ("line_items[0][quantity]", "1"),
            ("success_url", success_url.as_str()),
            ("cancel_url", cancel_url.as_str()),
            ("client_reference_id", &cloud_user_id.to_string()),
            ("metadata[cloud_user_id]", &cloud_user_id.to_string()),
            ("metadata[plan]", body.plan.as_str()),
            (
                "subscription_data[metadata][cloud_user_id]",
                &cloud_user_id.to_string(),
            ),
            ("subscription_data[metadata][plan]", body.plan.as_str()),
        ])
        .send()
        .await;
    match res {
        Ok(resp) => match resp.error_for_status() {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(value) => Json(serde_json::json!({
                    "url": value.get("url").and_then(|value| value.as_str())
                }))
                .into_response(),
                Err(err) => {
                    tracing::warn!(error = %err, "Stripe checkout response was not valid JSON");
                    (
                        StatusCode::BAD_GATEWAY,
                        "Stripe checkout could not be started",
                    )
                        .into_response()
                }
            },
            Err(err) => {
                tracing::warn!(error = %err, "Stripe checkout request failed");
                (
                    StatusCode::BAD_GATEWAY,
                    "Stripe checkout could not be started",
                )
                    .into_response()
            }
        },
        Err(err) => {
            tracing::warn!(error = %err, "Stripe checkout request could not be sent");
            (
                StatusCode::BAD_GATEWAY,
                "Stripe checkout could not be started",
            )
                .into_response()
        }
    }
}

pub async fn cloud_billing_portal(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if state.mode != HostletMode::Cloud {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(cloud_user_id) = current_cloud_user_id(&headers, &state).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(secret_key) = state.stripe_secret_key.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Stripe secret key is not configured",
        )
            .into_response();
    };
    let customer_id: Option<String> = sqlx::query_scalar(
        "SELECT stripe_customer_id FROM cloud_stripe_customers WHERE cloud_user_id=$1",
    )
    .bind(cloud_user_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    let Some(customer_id) = customer_id else {
        return (
            StatusCode::PRECONDITION_REQUIRED,
            "Stripe customer is missing",
        )
            .into_response();
    };
    let return_url = format!("{}/", state.public_web_url.trim_end_matches('/'));
    let res = state
        .http
        .post("https://api.stripe.com/v1/billing_portal/sessions")
        .bearer_auth(secret_key)
        .form(&[
            ("customer", customer_id.as_str()),
            ("return_url", return_url.as_str()),
        ])
        .send()
        .await;
    match res {
        Ok(resp) => match resp.error_for_status() {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(value) => Json(serde_json::json!({
                    "url": value.get("url").and_then(|value| value.as_str())
                }))
                .into_response(),
                Err(err) => {
                    tracing::warn!(error = %err, "Stripe portal response was not valid JSON");
                    (
                        StatusCode::BAD_GATEWAY,
                        "Stripe billing portal could not be opened",
                    )
                        .into_response()
                }
            },
            Err(err) => {
                tracing::warn!(error = %err, "Stripe portal request failed");
                (
                    StatusCode::BAD_GATEWAY,
                    "Stripe billing portal could not be opened",
                )
                    .into_response()
            }
        },
        Err(err) => {
            tracing::warn!(error = %err, "Stripe portal request could not be sent");
            (
                StatusCode::BAD_GATEWAY,
                "Stripe billing portal could not be opened",
            )
                .into_response()
        }
    }
}

pub async fn cloud_billing_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let Some(webhook_secret) = state.stripe_webhook_secret.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Stripe webhook secret is not configured",
        )
            .into_response();
    };
    if !valid_stripe_signature(&headers, &body, webhook_secret) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let event_id = payload
        .get("id")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let event_type = payload
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let inserted = sqlx::query(
        "INSERT INTO cloud_webhook_events (provider, provider_event_id, payload)
         VALUES ('stripe',$1,$2)
         ON CONFLICT (provider, provider_event_id) DO NOTHING",
    )
    .bind(event_id)
    .bind(&payload)
    .execute(&state.db)
    .await;
    let Ok(done) = inserted else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if done.rows_affected() == 0 {
        return StatusCode::NO_CONTENT.into_response();
    }
    let result = match stripe_webhook_action(event_type) {
        StripeWebhookAction::CheckoutCompleted => handle_checkout_completed(&state, &payload).await,
        StripeWebhookAction::SubscriptionChanged => {
            handle_subscription_event(&state, &payload).await
        }
        StripeWebhookAction::Ignore => Ok(()),
    };
    if let Err(err) = result {
        tracing::warn!(error = %err, event_id, event_type, "failed to process Stripe webhook");
        return StatusCode::BAD_GATEWAY.into_response();
    }
    let _ = sqlx::query(
        "UPDATE cloud_webhook_events SET processed_at=now() WHERE provider='stripe' AND provider_event_id=$1",
    )
    .bind(event_id)
    .execute(&state.db)
    .await;
    StatusCode::NO_CONTENT.into_response()
}

pub async fn system_update_check(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(_user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !state.update_checks_enabled {
        return (
            StatusCode::BAD_REQUEST,
            "Hostlet update checks are disabled by HOSTLET_UPDATE_CHECKS=false",
        )
            .into_response();
    }
    match refresh_update_check(&state).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => (StatusCode::BAD_GATEWAY, err.to_string()).into_response(),
    }
}

pub async fn operator_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !operator_token_valid(&state, &headers).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let health = system_health_counts(&state).await;
    let servers = sqlx::query("SELECT status,count(*) AS count FROM servers GROUP BY status")
        .fetch_all(&state.db)
        .await;
    let route_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM apps WHERE public_exposure=true AND current_deployment_id IS NOT NULL",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);
    let mut server_counts = serde_json::json!({});
    if let Ok(rows) = servers {
        for row in rows {
            let status: String = row.get("status");
            server_counts[status] = serde_json::json!(row.get::<i64, _>("count"));
        }
    }
    Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "mode": state.mode.as_str(),
        "service": {
            "imageTag": std::env::var("HOSTLET_IMAGE_TAG").ok(),
            "revision": std::env::var("HOSTLET_IMAGE_REVISION")
                .ok()
                .or_else(|| option_env!("HOSTLET_BUILD_REVISION").map(str::to_string)),
            "registry": std::env::var("HOSTLET_IMAGE_REGISTRY").ok(),
        },
        "database": {
            "connected": true,
        },
        "routing": {
            "publicAppRouteCount": route_count,
        },
        "health": health,
        "servers": server_counts,
    }))
    .into_response()
}

pub async fn operator_cleanup_preview(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !operator_token_valid(&state, &headers).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match cleanup_plan(&state, Uuid::nil()).await {
        Ok(plan) => Json(plan).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "failed to build operator cleanup preview");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn operator_run_cleanup(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !operator_token_valid(&state, &headers).await {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    run_cleanup_inner(&state, None).await
}

async fn operator_token_valid(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(token) = headers
        .get("x-hostlet-agent-token")
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let row = sqlx::query(
        "SELECT agent_token_hash FROM servers WHERE kind='local' ORDER BY created_at ASC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await;
    let Ok(Some(row)) = row else {
        return false;
    };
    let expected: Option<String> = row.get("agent_token_hash");
    expected
        .as_deref()
        .is_some_and(|hash| verify_token(token, hash))
}

pub async fn refresh_update_check_if_stale(state: &AppState) -> anyhow::Result<()> {
    let stale = sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
        "SELECT updated_at FROM settings WHERE key='system_update_check'",
    )
    .fetch_optional(&state.db)
    .await?
    .flatten()
    .map(|updated_at| {
        chrono::Utc::now().signed_duration_since(updated_at) > chrono::Duration::hours(24)
    })
    .unwrap_or(true);
    if stale {
        let _ = refresh_update_check(state).await?;
    }
    Ok(())
}

pub async fn app_resources(
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
        SELECT d.container_name, s.kind,
               rs.cpu_percent, rs.memory_usage, rs.memory_percent,
               rs.network_io, rs.block_io, rs.pids, rs.sampled_at
        FROM apps a
        JOIN servers s ON s.id = a.server_id
        LEFT JOIN deployments d ON d.id = a.current_deployment_id
        LEFT JOIN app_resource_snapshots rs ON rs.container_name = d.container_name
        WHERE a.id=$1 AND a.user_id=$2
        "#,
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await;
    let Ok(Some(row)) = row else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if row.get::<String, _>("kind") != "local" {
        return (
            StatusCode::BAD_REQUEST,
            "resource usage is currently available for local apps only",
        )
            .into_response();
    }
    let Some(container) = row.get::<Option<String>, _>("container_name") else {
        return (
            StatusCode::NOT_FOUND,
            "app does not have a running container yet",
        )
            .into_response();
    };

    let sampled_at = row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("sampled_at");
    let Some(sampled_at) = sampled_at else {
        return (
            StatusCode::ACCEPTED,
            "resource usage is waiting for the local agent",
        )
            .into_response();
    };
    if chrono::Utc::now().signed_duration_since(sampled_at) > chrono::Duration::seconds(45) {
        return (
            StatusCode::ACCEPTED,
            "resource usage is waiting for a fresh local agent sample",
        )
            .into_response();
    }
    Json(serde_json::json!({
        "container": container,
        "name": container,
        "cpuPercent": row.get::<Option<String>, _>("cpu_percent").unwrap_or_else(|| "0%".into()),
        "memoryUsage": row.get::<Option<String>, _>("memory_usage").unwrap_or_else(|| "0B / 0B".into()),
        "memoryPercent": row.get::<Option<String>, _>("memory_percent").unwrap_or_else(|| "0%".into()),
        "networkIo": row.get::<Option<String>, _>("network_io").unwrap_or_else(|| "0B / 0B".into()),
        "blockIo": row.get::<Option<String>, _>("block_io").unwrap_or_else(|| "0B / 0B".into()),
        "pids": row.get::<Option<String>, _>("pids").unwrap_or_else(|| "0".into()),
        "sampledAt": sampled_at
    }))
    .into_response()
}

pub async fn create_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateApp>,
) -> impl IntoResponse {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(err) if err.to_string() == "sign in required" => {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(err) => return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response(),
    };
    if let Err(err) = cloud_compute_allowed_for_context(&state, context).await {
        return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response();
    };
    let user_id = context.user_id;
    let app_name = body.name.trim();
    let repo_full_name = body.repo_full_name.trim();
    let branch = body.branch.trim();
    if app_name.is_empty()
        || repo_full_name.is_empty()
        || branch.is_empty()
        || !(1..=65_535).contains(&body.container_port)
    {
        return (
            StatusCode::BAD_REQUEST,
            "app name, repo, branch, and valid port are required",
        )
            .into_response();
    }
    if !valid_app_name(app_name) {
        return (
            StatusCode::BAD_REQUEST,
            "app name contains unsupported characters",
        )
            .into_response();
    }
    if !valid_repo_full_name(repo_full_name) {
        return (
            StatusCode::BAD_REQUEST,
            "repo must be a GitHub owner/repo name",
        )
            .into_response();
    }
    if !valid_branch(branch) {
        return (
            StatusCode::BAD_REQUEST,
            "branch name contains unsupported characters",
        )
            .into_response();
    }
    if state.mode != HostletMode::Cloud && !valid_memory_limit(body.memory_limit_mb) {
        return (
            StatusCode::BAD_REQUEST,
            "memory limit must be between 64 and 262144 MB",
        )
            .into_response();
    }
    if state.mode != HostletMode::Cloud && !valid_cpu_limit(body.cpu_limit) {
        return (
            StatusCode::BAD_REQUEST,
            "CPU limit must be between 0.1 and 128",
        )
            .into_response();
    }
    let runtime_kind = match clean_runtime_kind(body.runtime_kind.as_deref()) {
        Ok(value) => value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    if state.mode == HostletMode::Cloud && runtime_kind == "compose" {
        return (
            StatusCode::BAD_REQUEST,
            "Docker Compose is not supported on Hostlet Cloud yet",
        )
            .into_response();
    }
    if state.mode == HostletMode::Cloud && cloud_create_contains_unsupported_settings(&body) {
        return (
            StatusCode::BAD_REQUEST,
            "unsupported Hostlet Cloud app setting",
        )
            .into_response();
    }
    let hostlet_config_path = match clean_hostlet_config_path(body.hostlet_config_path.as_deref()) {
        Ok(value) => value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let runtime_config = body.runtime_config.unwrap_or_else(|| serde_json::json!({}));
    if let Err(message) = clean_runtime_config(&runtime_config) {
        return (StatusCode::BAD_REQUEST, message).into_response();
    }
    let packaging_strategy = match clean_packaging_strategy(body.packaging_strategy.as_deref()) {
        Ok(value) => value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let server_id = if state.mode == HostletMode::Cloud {
        Uuid::parse_str(
            &std::env::var("LOCAL_SERVER_ID")
                .unwrap_or_else(|_| "00000000-0000-0000-0000-000000000001".into()),
        )
        .unwrap()
    } else {
        match body.server_id {
            Some(id) => id,
            None => Uuid::parse_str(
                &std::env::var("LOCAL_SERVER_ID")
                    .unwrap_or_else(|_| "00000000-0000-0000-0000-000000000001".into()),
            )
            .unwrap(),
        }
    };
    let server = sqlx::query("SELECT id FROM servers WHERE id=$1 AND kind='local'")
        .bind(server_id)
        .fetch_optional(&state.db)
        .await;
    let Ok(Some(_)) = server else {
        return (StatusCode::BAD_REQUEST, "server is not available").into_response();
    };
    let domain = if state.mode == HostletMode::Cloud {
        match cloud_app_domain(&state, app_name).await {
            Ok(domain) => domain,
            Err((status, message)) => return (status, message).into_response(),
        }
    } else if body.domain.trim().is_empty() {
        match &state.base_domain {
            Some(base_domain) => format!("{}.{}", app_slug(app_name), base_domain),
            None => format!("localhost:{}", 20000 + (body.container_port as u16 % 20000)),
        }
    } else {
        body.domain.trim().to_ascii_lowercase()
    };
    if !valid_domain(&domain) {
        return (
            StatusCode::BAD_REQUEST,
            "domain must be a hostname with optional port",
        )
            .into_response();
    }
    if app_domain_in_use(&state, &domain, None).await {
        return (
            StatusCode::CONFLICT,
            "domain is already assigned to another app",
        )
            .into_response();
    }
    let public_exposure = if state.mode == HostletMode::Cloud {
        true
    } else {
        body.public_exposure.unwrap_or(false)
    };
    if public_exposure && state.mode != HostletMode::Cloud {
        if let Err(err) = hostlet_public_cloudflare_host(&state, &domain) {
            return (StatusCode::BAD_REQUEST, err.to_string()).into_response();
        }
    }
    let health_path = {
        let value = body.health_path.trim();
        if value.is_empty() {
            "/".to_string()
        } else {
            value.to_string()
        }
    };
    if !valid_health_path(&health_path) {
        return (
            StatusCode::BAD_REQUEST,
            "health path must start with / and cannot contain control characters",
        )
            .into_response();
    }
    let root_directory = clean_optional(body.root_directory).unwrap_or_else(|| ".".into());
    if !valid_root_directory(&root_directory) {
        return (
            StatusCode::BAD_REQUEST,
            "root directory cannot be absolute or contain ..",
        )
            .into_response();
    }
    let install_command = match clean_command(body.install_command) {
        Ok(value) => value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let build_command = match clean_command(body.build_command) {
        Ok(value) => value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let start_command = match clean_command(body.start_command) {
        Ok(value) => value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    if let Err(message) = validate_env_vars(&body.env) {
        return (StatusCode::BAD_REQUEST, message).into_response();
    }
    if let Err(err) = enforce_cloud_app_entitlements(&state, context).await {
        return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response();
    }
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let auto_deploy = if state.mode == HostletMode::Cloud {
        true
    } else {
        body.auto_deploy.unwrap_or(false)
    };
    if auto_deploy && state.mode != HostletMode::Cloud {
        if let Err(err) = github::ensure_repo_webhook(&state, user_id, repo_full_name).await {
            tracing::warn!(error = %err, repo = %repo_full_name, "failed to ensure GitHub webhook");
            return (
                StatusCode::BAD_GATEWAY,
                "GitHub webhook could not be configured",
            )
                .into_response();
        }
    }
    let row = sqlx::query("INSERT INTO apps (user_id,server_id,name,repo_full_name,branch,container_port,health_path,domain,runtime_kind,hostlet_config_path,runtime_config,packaging_strategy,root_directory,install_command,build_command,start_command,memory_limit_mb,cpu_limit,public_exposure,auto_deploy) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20) RETURNING id")
        .bind(user_id).bind(server_id).bind(app_name).bind(repo_full_name).bind(branch).bind(body.container_port).bind(health_path).bind(&domain)
        .bind(runtime_kind).bind(hostlet_config_path).bind(runtime_config).bind(packaging_strategy).bind(root_directory).bind(install_command).bind(build_command).bind(start_command)
        .bind(if state.mode == HostletMode::Cloud { Some(512) } else { body.memory_limit_mb })
        .bind(if state.mode == HostletMode::Cloud { Some(0.5) } else { body.cpu_limit })
        .bind(public_exposure).bind(auto_deploy)
        .fetch_one(&mut *tx).await;
    let Ok(row) = row else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let app_id: Uuid = row.get("id");
    for ev in body.env {
        let enc = match state.crypto.encrypt(&ev.value) {
            Ok(v) => v,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        if sqlx::query("INSERT INTO app_env_vars (app_id,key,value_ciphertext) VALUES ($1,$2,$3)")
            .bind(app_id)
            .bind(ev.key)
            .bind(enc)
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
    if public_exposure && state.mode != HostletMode::Cloud {
        if let Err(err) = ensure_cloudflare_app_dns(&state, app_id, &domain).await {
            tracing::warn!(error = %err, domain = %domain, "failed to open public tunnel");
            delete_created_app_row(&state, app_id).await;
            return (
                StatusCode::BAD_GATEWAY,
                "failed to open public tunnel for app domain",
            )
                .into_response();
        }
        if sqlx::query("UPDATE apps SET public_exposure=true, updated_at=now() WHERE id=$1")
            .bind(app_id)
            .execute(&state.db)
            .await
            .is_err()
        {
            let _ = delete_cloudflare_app_dns(&state, app_id, &domain).await;
            delete_created_app_row(&state, app_id).await;
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    record_audit_event(
        &state,
        AuditEventInput {
            actor_type: "owner",
            actor_id: Some(user_id.to_string()),
            event_type: "app_created",
            app_id: Some(app_id),
            deployment_id: None,
            job_id: None,
            metadata: serde_json::json!({
                "repo": repo_full_name,
                "branch": branch,
                "publicExposure": public_exposure,
                "autoDeploy": auto_deploy,
            }),
        },
    )
    .await;
    if public_exposure {
        record_audit_event(
            &state,
            AuditEventInput {
                actor_type: "owner",
                actor_id: Some(user_id.to_string()),
                event_type: "public_url_published",
                app_id: Some(app_id),
                deployment_id: None,
                job_id: None,
                metadata: serde_json::json!({"domain": domain}),
            },
        )
        .await;
    }
    let deployment_id = if body.deploy_after_create.unwrap_or(false) {
        match deploy::create_and_send_deploy(&state, user_id, app_id, "HEAD").await {
            Ok(id) => Some(id),
            Err(err) => return (StatusCode::BAD_GATEWAY, err.to_string()).into_response(),
        }
    } else {
        None
    };
    Json(serde_json::json!({"id": app_id, "deploymentId": deployment_id})).into_response()
}

pub async fn update_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateApp>,
) -> impl IntoResponse {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(err) if err.to_string() == "sign in required" => {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(err) => return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response(),
    };
    if let Err(err) = cloud_compute_allowed_for_context(&state, context).await {
        return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response();
    };
    let user_id = context.user_id;
    if state.mode == HostletMode::Cloud && cloud_update_contains_unsupported_settings(&body) {
        return (
            StatusCode::BAD_REQUEST,
            "unsupported Hostlet Cloud app setting",
        )
            .into_response();
    }
    let row = sqlx::query(
        "SELECT id, domain, public_exposure, repo_full_name, auto_deploy FROM apps WHERE id=$1 AND user_id=$2",
    )
            .bind(id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
    let Some(row) = row else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let old_domain = row.get::<String, _>("domain");
    let old_public_exposure = row.get::<bool, _>("public_exposure");
    let repo_full_name = row.get::<String, _>("repo_full_name");
    let old_auto_deploy = row.get::<bool, _>("auto_deploy");
    let domain_changed = body.domain.is_some();
    let mut app_domain = old_domain.clone();
    if let Some(domain) = &body.domain {
        let domain = domain.trim().to_ascii_lowercase();
        if domain.is_empty() {
            return (StatusCode::BAD_REQUEST, "domain is required").into_response();
        }
        if !valid_domain(&domain) {
            return (
                StatusCode::BAD_REQUEST,
                "domain must be a hostname with optional port",
            )
                .into_response();
        }
        if app_domain_in_use(&state, &domain, Some(id)).await {
            return (
                StatusCode::CONFLICT,
                "domain is already assigned to another app",
            )
                .into_response();
        }
        app_domain = domain;
    }
    let desired_public_exposure = body.public_exposure.unwrap_or(old_public_exposure);
    if desired_public_exposure {
        if let Err(err) = hostlet_public_cloudflare_host(&state, &app_domain) {
            return (StatusCode::BAD_REQUEST, err.to_string()).into_response();
        }
    }
    let health_path = match body.health_path {
        Some(path) => {
            let path = path.trim().to_string();
            if !valid_health_path(&path) {
                return (
                    StatusCode::BAD_REQUEST,
                    "health path must start with / and cannot contain control characters",
                )
                    .into_response();
            }
            Some(path)
        }
        None => None,
    };
    let root_directory = match body.root_directory {
        Some(root_directory) => {
            let root_directory = clean_optional(Some(root_directory)).unwrap_or_else(|| ".".into());
            if !valid_root_directory(&root_directory) {
                return (
                    StatusCode::BAD_REQUEST,
                    "root directory cannot be absolute or contain ..",
                )
                    .into_response();
            }
            Some(root_directory)
        }
        None => None,
    };
    let runtime_kind = match body.runtime_kind.as_deref() {
        Some(value) => Some(match clean_runtime_kind(Some(value)) {
            Ok(value) => value,
            Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
        }),
        None => None,
    };
    let hostlet_config_path = match body.hostlet_config_path.as_deref() {
        Some(value) => Some(match clean_hostlet_config_path(Some(value)) {
            Ok(value) => value,
            Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
        }),
        None => None,
    };
    let runtime_config = match body.runtime_config {
        Some(value) => {
            if let Err(message) = clean_runtime_config(&value) {
                return (StatusCode::BAD_REQUEST, message).into_response();
            }
            Some(value)
        }
        None => None,
    };
    let packaging_strategy = match body.packaging_strategy.as_deref() {
        Some(value) => Some(match clean_packaging_strategy(Some(value)) {
            Ok(value) => value,
            Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
        }),
        None => None,
    };
    let install_command = match body.install_command {
        Some(command) => Some(match command {
            Some(value) => match clean_command(Some(value)) {
                Ok(value) => value,
                Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
            },
            None => None,
        }),
        None => None,
    };
    let build_command = match body.build_command {
        Some(command) => Some(match command {
            Some(value) => match clean_command(Some(value)) {
                Ok(value) => value,
                Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
            },
            None => None,
        }),
        None => None,
    };
    let start_command = match body.start_command {
        Some(command) => Some(match command {
            Some(value) => match clean_command(Some(value)) {
                Ok(value) => value,
                Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
            },
            None => None,
        }),
        None => None,
    };
    if let Some(container_port) = body.container_port {
        if !(1..=65_535).contains(&container_port) {
            return (StatusCode::BAD_REQUEST, "container port must be 1-65535").into_response();
        }
    }
    if state.mode != HostletMode::Cloud {
        if let Some(memory_limit_mb) = body.memory_limit_mb {
            if !valid_memory_limit(memory_limit_mb) {
                return (
                    StatusCode::BAD_REQUEST,
                    "memory limit must be between 64 and 262144 MB",
                )
                    .into_response();
            }
        }
        if let Some(cpu_limit) = body.cpu_limit {
            if !valid_cpu_limit(cpu_limit) {
                return (
                    StatusCode::BAD_REQUEST,
                    "CPU limit must be between 0.1 and 128",
                )
                    .into_response();
            }
        }
    }
    if let Some(env) = &body.env {
        if let Err(message) = validate_env_vars(env) {
            return (StatusCode::BAD_REQUEST, message).into_response();
        }
    }
    if body.auto_deploy == Some(true) && !old_auto_deploy {
        if let Err(err) = github::ensure_repo_webhook(&state, user_id, &repo_full_name).await {
            tracing::warn!(error = %err, repo = %repo_full_name, "failed to ensure GitHub webhook");
            return (
                StatusCode::BAD_GATEWAY,
                "GitHub webhook could not be configured",
            )
                .into_response();
        }
    }
    let env_replaced = body.env.is_some();
    if desired_public_exposure {
        if let Err(err) = ensure_cloudflare_app_dns(&state, id, &app_domain).await {
            tracing::warn!(
                error = %err,
                domain = %app_domain,
                "failed to open public tunnel during app update"
            );
            return (
                StatusCode::BAD_GATEWAY,
                "failed to open public tunnel for app domain",
            )
                .into_response();
        }
    }
    let should_close_old_dns =
        old_public_exposure && (!desired_public_exposure || old_domain != app_domain);
    if should_close_old_dns {
        if let Err(err) = delete_cloudflare_app_dns(&state, id, &old_domain).await {
            tracing::warn!(
                error = %err,
                domain = %old_domain,
                "failed to close old public tunnel during app update"
            );
            return (
                StatusCode::BAD_GATEWAY,
                "failed to close public tunnel for app domain",
            )
                .into_response();
        }
    }
    let update_result: anyhow::Result<()> = async {
        let mut tx = state.db.begin().await?;
        if domain_changed {
            sqlx::query("UPDATE apps SET domain=$1, updated_at=now() WHERE id=$2")
                .bind(&app_domain)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(path) = health_path {
            sqlx::query("UPDATE apps SET health_path=$1, updated_at=now() WHERE id=$2")
                .bind(path)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(root_directory) = root_directory {
            sqlx::query("UPDATE apps SET root_directory=$1, updated_at=now() WHERE id=$2")
                .bind(root_directory)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(runtime_kind) = runtime_kind {
            sqlx::query("UPDATE apps SET runtime_kind=$1, updated_at=now() WHERE id=$2")
                .bind(runtime_kind)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(hostlet_config_path) = hostlet_config_path {
            sqlx::query("UPDATE apps SET hostlet_config_path=$1, updated_at=now() WHERE id=$2")
                .bind(hostlet_config_path)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(runtime_config) = runtime_config {
            sqlx::query("UPDATE apps SET runtime_config=$1, updated_at=now() WHERE id=$2")
                .bind(runtime_config)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(packaging_strategy) = packaging_strategy {
            sqlx::query("UPDATE apps SET packaging_strategy=$1, updated_at=now() WHERE id=$2")
                .bind(packaging_strategy)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(command) = install_command {
            sqlx::query("UPDATE apps SET install_command=$1, updated_at=now() WHERE id=$2")
                .bind(command)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(command) = build_command {
            sqlx::query("UPDATE apps SET build_command=$1, updated_at=now() WHERE id=$2")
                .bind(command)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(command) = start_command {
            sqlx::query("UPDATE apps SET start_command=$1, updated_at=now() WHERE id=$2")
                .bind(command)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(container_port) = body.container_port {
            sqlx::query("UPDATE apps SET container_port=$1, updated_at=now() WHERE id=$2")
                .bind(container_port)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if state.mode != HostletMode::Cloud {
            if let Some(memory_limit_mb) = body.memory_limit_mb {
                sqlx::query("UPDATE apps SET memory_limit_mb=$1, updated_at=now() WHERE id=$2")
                    .bind(memory_limit_mb)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
            if let Some(cpu_limit) = body.cpu_limit {
                sqlx::query("UPDATE apps SET cpu_limit=$1, updated_at=now() WHERE id=$2")
                    .bind(cpu_limit)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        if let Some(env) = body.env {
            sqlx::query("DELETE FROM app_env_vars WHERE app_id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            for ev in env {
                let enc = state.crypto.encrypt(&ev.value)?;
                sqlx::query(
                    "INSERT INTO app_env_vars (app_id,key,value_ciphertext) VALUES ($1,$2,$3)",
                )
                .bind(id)
                .bind(ev.key)
                .bind(enc)
                .execute(&mut *tx)
                .await?;
            }
        }
        if body.public_exposure.is_some() {
            sqlx::query("UPDATE apps SET public_exposure=$1, updated_at=now() WHERE id=$2")
                .bind(desired_public_exposure)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(auto_deploy) = body.auto_deploy {
            sqlx::query("UPDATE apps SET auto_deploy=$1, updated_at=now() WHERE id=$2")
                .bind(auto_deploy)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    .await;
    if let Err(err) = update_result {
        tracing::warn!(error = %err, app_id = %id, "failed to persist app update after DNS changes");
        compensate_failed_app_update_dns(
            &state,
            &old_domain,
            &app_domain,
            id,
            old_public_exposure,
            desired_public_exposure,
        )
        .await;
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    record_audit_event(
        &state,
        AuditEventInput {
            actor_type: "owner",
            actor_id: Some(user_id.to_string()),
            event_type: "app_updated",
            app_id: Some(id),
            deployment_id: None,
            job_id: None,
            metadata: serde_json::json!({
                "domainChanged": domain_changed,
                "publicExposureChanged": body.public_exposure.is_some(),
                "autoDeployChanged": body.auto_deploy.is_some(),
                "envReplaced": env_replaced,
            }),
        },
    )
    .await;
    if body.public_exposure.is_some() && desired_public_exposure != old_public_exposure {
        record_audit_event(
            &state,
            AuditEventInput {
                actor_type: "owner",
                actor_id: Some(user_id.to_string()),
                event_type: if desired_public_exposure {
                    "public_url_published"
                } else {
                    "public_url_made_private"
                },
                app_id: Some(id),
                deployment_id: None,
                job_id: None,
                metadata: serde_json::json!({"domain": app_domain}),
            },
        )
        .await;
    }
    StatusCode::NO_CONTENT.into_response()
}

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
    if let Err(err) = cloud_compute_allowed_for_context(&state, context).await {
        return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response();
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
    if let Err(err) = cloud_compute_allowed_for_context(&state, context).await {
        return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response();
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
    .bind(state.mode == HostletMode::Cloud)
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
    .bind(state.mode == HostletMode::Cloud)
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
    if let Err(err) = cloud_compute_allowed_for_context(&state, context).await {
        return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response();
    }
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
    .bind(state.mode == HostletMode::Cloud)
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
    if let Err(err) = cloud_compute_allowed_for_context(&state, context).await {
        return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response();
    }
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
    .bind(state.mode == HostletMode::Cloud)
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
    if let Err(err) = cloud_compute_allowed_for_context(&state, context).await {
        return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response();
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

async fn enforce_cloud_app_entitlements(
    state: &AppState,
    context: RequestContext,
) -> anyhow::Result<()> {
    if state.mode != HostletMode::Cloud {
        return Ok(());
    }
    let Some(cloud_user_id) = context.cloud_user_id else {
        anyhow::bail!("Hostlet Cloud session is required");
    };
    let entitlement = sqlx::query(
        r#"
        SELECT e.app_limit
        FROM cloud_subscriptions s
        JOIN cloud_plan_entitlements e ON e.plan_code=s.plan_code
        WHERE s.cloud_user_id=$1
          AND s.status IN ('active','trialing')
          AND (s.current_period_end IS NULL OR s.current_period_end > now())
        ORDER BY s.created_at DESC
        LIMIT 1
        "#,
    )
    .bind(cloud_user_id)
    .fetch_optional(&state.db)
    .await?;
    let Some(entitlement) = entitlement else {
        anyhow::bail!("An active Hostlet Cloud subscription is required before creating apps");
    };
    let active_apps: i64 = sqlx::query_scalar("SELECT count(*) FROM apps WHERE user_id=$1")
        .bind(context.user_id)
        .fetch_one(&state.db)
        .await?;
    if active_apps >= i64::from(entitlement.get::<i32, _>("app_limit")) {
        anyhow::bail!("Hostlet Cloud app limit reached for this plan");
    }
    Ok(())
}

async fn cloud_app_domain(
    state: &AppState,
    app_name: &str,
) -> Result<String, (StatusCode, &'static str)> {
    let Some(base_domain) = state.base_domain.as_ref() else {
        return Err((
            StatusCode::BAD_REQUEST,
            "HOSTLET_BASE_DOMAIN is required in cloud mode",
        ));
    };
    let slug = cloud_app_domain_label(app_name);
    let clean = format!("{slug}.{base_domain}");
    if !reserved_public_domain_label(&slug) && !app_domain_in_use(state, &clean, None).await {
        return Ok(clean);
    }
    for _ in 0..10 {
        let label = format!(
            "{}-{}",
            slug,
            crate::crypto::random_token(6).to_ascii_lowercase()
        );
        let domain = format!("{label}.{base_domain}");
        if !app_domain_in_use(state, &domain, None).await {
            return Ok(domain);
        }
    }
    Err((
        StatusCode::CONFLICT,
        "could not allocate a unique app domain",
    ))
}

fn cloud_app_domain_label(app_name: &str) -> String {
    app_slug(app_name)
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

fn app_json(r: sqlx::postgres::PgRow) -> serde_json::Value {
    serde_json::json!({
        "id": r.get::<Uuid,_>("id"), "name": r.get::<String,_>("name"), "repoFullName": r.get::<String,_>("repo_full_name"),
        "branch": r.get::<String,_>("branch"), "domain": r.get::<String,_>("domain"), "currentDeploymentId": r.get::<Option<Uuid>,_>("current_deployment_id"),
        "runtimeKind": r.try_get::<String,_>("runtime_kind").unwrap_or_else(|_| "single".into()),
        "hostletConfigPath": r.try_get::<String,_>("hostlet_config_path").unwrap_or_else(|_| "hostlet.yml".into()),
        "runtimeConfig": r.try_get::<serde_json::Value,_>("runtime_config").unwrap_or_else(|_| serde_json::json!({})),
        "packagingStrategy": r.try_get::<String,_>("packaging_strategy").unwrap_or_else(|_| "auto".into()),
        "rootDirectory": r.try_get::<String,_>("root_directory").unwrap_or_else(|_| ".".into()),
        "installCommand": r.try_get::<Option<String>,_>("install_command").unwrap_or(None),
        "buildCommand": r.try_get::<Option<String>,_>("build_command").unwrap_or(None),
        "startCommand": r.try_get::<Option<String>,_>("start_command").unwrap_or(None),
        "containerPort": r.try_get::<i32,_>("container_port").ok(),
        "healthPath": r.try_get::<String,_>("health_path").ok(),
        "memoryLimitMb": r.try_get::<Option<i32>,_>("memory_limit_mb").unwrap_or(None),
        "cpuLimit": r.try_get::<Option<f64>,_>("cpu_limit").unwrap_or(None),
        "publicExposure": r.try_get::<bool,_>("public_exposure").unwrap_or(false),
        "autoDeploy": r.try_get::<bool,_>("auto_deploy").unwrap_or(false),
        "createdAt": r.try_get::<chrono::DateTime<chrono::Utc>,_>("created_at").ok(),
        "server": r.try_get::<Uuid,_>("server_id").ok().map(|id| serde_json::json!({
            "id": id,
            "name": r.try_get::<String,_>("server_name").unwrap_or_else(|_| "Server".into()),
            "publicIp": r.try_get::<Option<String>,_>("server_public_ip").unwrap_or(None),
            "kind": r.try_get::<String,_>("server_kind").unwrap_or_else(|_| "remote".into()),
            "status": r.try_get::<String,_>("server_status").unwrap_or_else(|_| "offline".into()),
            "lastSeenAt": r.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("server_last_seen_at").unwrap_or(None)
        })),
        "latestDeployment": r.try_get::<Option<Uuid>,_>("latest_deployment_id").unwrap_or(None).map(|id| serde_json::json!({
            "id": id,
            "status": r.try_get::<Option<String>,_>("latest_deployment_status").unwrap_or(None),
            "commitSha": r.try_get::<Option<String>,_>("latest_commit_sha").unwrap_or(None),
            "failure": r.try_get::<Option<String>,_>("latest_failure_summary").unwrap_or(None),
            "startedAt": r.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("latest_started_at").unwrap_or(None),
            "finishedAt": r.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("latest_finished_at").unwrap_or(None),
            "runtimeMetadata": r.try_get::<Option<serde_json::Value>,_>("latest_runtime_metadata").unwrap_or(None).unwrap_or_else(|| serde_json::json!({}))
        })),
        "currentDeployment": r.try_get::<Option<String>,_>("current_deployment_status").unwrap_or(None).map(|status| serde_json::json!({
            "status": status,
            "publishedPort": r.try_get::<Option<i32>,_>("current_published_port").unwrap_or(None),
            "finishedAt": r.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("current_deployment_finished_at").unwrap_or(None)
        })),
        "latestWebhook": r.try_get::<Option<String>,_>("latest_webhook_status").unwrap_or(None).map(|status| serde_json::json!({
            "status": status,
            "ignoredReason": r.try_get::<Option<String>,_>("latest_webhook_ignored_reason").unwrap_or(None),
            "commitSha": r.try_get::<Option<String>,_>("latest_webhook_commit_sha").unwrap_or(None),
            "branch": r.try_get::<Option<String>,_>("latest_webhook_branch").unwrap_or(None),
            "deploymentId": r.try_get::<Option<Uuid>,_>("latest_webhook_deployment_id").unwrap_or(None),
            "createdAt": r.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("latest_webhook_created_at").unwrap_or(None)
        })),
        "health": r.try_get::<Option<String>,_>("health_status").unwrap_or(None).map(|status| serde_json::json!({
            "status": status,
            "httpStatus": r.try_get::<Option<i32>,_>("health_http_status").unwrap_or(None),
            "latencyMs": r.try_get::<Option<i32>,_>("health_latency_ms").unwrap_or(None),
            "failureCount": r.try_get::<Option<i32>,_>("health_failure_count").unwrap_or(None).unwrap_or(0),
            "successCount": r.try_get::<Option<i32>,_>("health_success_count").unwrap_or(None).unwrap_or(0),
            "lastError": r.try_get::<Option<String>,_>("health_last_error").unwrap_or(None),
            "lastCheckedAt": r.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("health_last_checked_at").unwrap_or(None),
            "lastHealthyAt": r.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("health_last_healthy_at").unwrap_or(None),
            "updatedAt": r.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("health_updated_at").unwrap_or(None)
        }))
    })
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

fn cloud_create_contains_unsupported_settings(body: &CreateApp) -> bool {
    !body.domain.trim().is_empty()
}

fn cloud_update_contains_unsupported_settings(body: &UpdateApp) -> bool {
    body.domain.is_some()
        || body.public_exposure.is_some()
        || body.auto_deploy.is_some()
        || body.memory_limit_mb.is_some()
        || body.cpu_limit.is_some()
        || body
            .runtime_kind
            .as_deref()
            .is_some_and(|value| value.trim() == "compose")
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

fn clean_runtime_config(value: &serde_json::Value) -> Result<(), &'static str> {
    if !value.is_object() {
        return Err("runtime config must be an object");
    }
    if value.to_string().len() > 32_000 {
        return Err("runtime config is too large");
    }
    Ok(())
}

fn app_slug(value: &str) -> String {
    let slug = value
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if slug.is_empty() {
        "app".into()
    } else {
        slug
    }
}

fn valid_app_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' '))
}

fn valid_repo_full_name(value: &str) -> bool {
    let mut parts = value.split('/');
    let Some(owner) = parts.next() else {
        return false;
    };
    let Some(repo) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    [owner, repo].into_iter().all(|part| {
        !part.is_empty()
            && part.len() <= 100
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            && !part.starts_with('.')
            && !part.ends_with('.')
    })
}

fn valid_branch(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('-')
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("..")
        && !value.contains("@{")
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
}

fn valid_domain(value: &str) -> bool {
    let Some((host, port)) = value.rsplit_once(':') else {
        return valid_hostname(value);
    };
    valid_hostname(host) && !port.is_empty() && port.parse::<u16>().is_ok()
}

fn valid_hostname(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && !value.starts_with('.')
        && !value.ends_with('.')
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

fn valid_health_path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 256
        && !value.chars().any(|c| c.is_control() || c == '\\')
}

fn valid_root_directory(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 256
        && !value.starts_with('/')
        && !value.starts_with('\\')
        && !value.split('/').any(|part| part == "..")
        && !value.chars().any(|c| c.is_control() || c == '\\')
}

fn valid_memory_limit(value: Option<i32>) -> bool {
    value.map(|v| (64..=262_144).contains(&v)).unwrap_or(true)
}

fn valid_cpu_limit(value: Option<f64>) -> bool {
    value
        .map(|v| v.is_finite() && (0.1..=128.0).contains(&v))
        .unwrap_or(true)
}

pub async fn cloudflare_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(_user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let configured = state.cloudflare_api_token.is_some()
        && state.cloudflare_zone_id.is_some()
        && state.cloudflare_tunnel_target.is_some()
        && state.base_domain.is_some();
    let Some(token) = state.cloudflare_api_token.as_ref() else {
        return Json(serde_json::json!({
            "configured": false,
            "tokenValid": null,
            "baseDomain": state.base_domain.as_deref(),
            "domainPrefix": state.domain_prefix,
            "defaultDomainPattern": default_domain_pattern(&state),
            "tunnelTargetConfigured": state.cloudflare_tunnel_target.is_some(),
            "message": "CLOUDFLARE_API_TOKEN is not set."
        }))
        .into_response();
    };
    let Some(zone_id) = state.cloudflare_zone_id.as_ref() else {
        return Json(serde_json::json!({
            "configured": false,
            "tokenValid": null,
            "baseDomain": state.base_domain.as_deref(),
            "domainPrefix": state.domain_prefix,
            "defaultDomainPattern": default_domain_pattern(&state),
            "tunnelTargetConfigured": state.cloudflare_tunnel_target.is_some(),
            "message": "CLOUDFLARE_ZONE_ID is not set."
        }))
        .into_response();
    };
    let resp = state
        .http
        .get(format!(
            "https://api.cloudflare.com/client/v4/zones/{zone_id}"
        ))
        .bearer_auth(token)
        .send()
        .await;
    match resp {
        Ok(resp) if resp.status().is_success() => Json(serde_json::json!({
            "configured": configured,
            "tokenValid": true,
            "baseDomain": state.base_domain.as_deref(),
            "domainPrefix": state.domain_prefix,
            "defaultDomainPattern": default_domain_pattern(&state),
            "tunnelTargetConfigured": state.cloudflare_tunnel_target.is_some(),
            "message": "Cloudflare API token can access the configured zone."
        }))
        .into_response(),
        Ok(resp) => Json(serde_json::json!({
            "configured": configured,
            "tokenValid": false,
            "baseDomain": state.base_domain.as_deref(),
            "domainPrefix": state.domain_prefix,
            "defaultDomainPattern": default_domain_pattern(&state),
            "tunnelTargetConfigured": state.cloudflare_tunnel_target.is_some(),
            "message": format!("Cloudflare zone check failed with status {}.", resp.status())
        }))
        .into_response(),
        Err(_) => Json(serde_json::json!({
            "configured": configured,
            "tokenValid": false,
            "baseDomain": state.base_domain.as_deref(),
            "domainPrefix": state.domain_prefix,
            "defaultDomainPattern": default_domain_pattern(&state),
            "tunnelTargetConfigured": state.cloudflare_tunnel_target.is_some(),
            "message": "Could not reach Cloudflare from the API container."
        }))
        .into_response(),
    }
}

async fn ensure_cloudflare_app_dns(
    state: &AppState,
    app_id: Uuid,
    domain: &str,
) -> anyhow::Result<()> {
    let host = hostlet_public_cloudflare_host(state, domain)?;
    let (Some(token), Some(zone_id), Some(target)) = (
        &state.cloudflare_api_token,
        &state.cloudflare_zone_id,
        &state.cloudflare_tunnel_target,
    ) else {
        anyhow::bail!("Cloudflare DNS is not configured");
    };

    let client = &state.http;
    let base = format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records");
    let existing = client
        .get(&base)
        .bearer_auth(token)
        .query(&[("type", "CNAME"), ("name", host.as_str())])
        .send()
        .await?
        .error_for_status()?
        .json::<CloudflareListResponse>()
        .await?;

    let owned = sqlx::query(
        "SELECT app_id, cloudflare_record_id
         FROM app_public_dns_records
         WHERE zone_id=$1 AND hostname=$2",
    )
    .bind(zone_id)
    .bind(&host)
    .fetch_optional(&state.db)
    .await?;

    let payload = CloudflareDnsRecord {
        record_type: "CNAME",
        name: &host,
        content: target,
        proxied: true,
    };

    if let Some(owner) = owned.as_ref() {
        let owner_app_id = owner.get::<Uuid, _>("app_id");
        if owner_app_id != app_id {
            anyhow::bail!("{host} is already managed by another Hostlet app");
        }
    }

    let record_id = if let Some(record) = existing.result.first() {
        if owned.is_none() && !hostlet_legacy_prefixed_host(state, &host) {
            anyhow::bail!(
                "{host} already has a Cloudflare CNAME record not managed by this Hostlet app"
            );
        }
        client
            .patch(format!("{base}/{}", record.id))
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;
        record.id.clone()
    } else {
        client
            .post(&base)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await?
            .error_for_status()?
            .json::<CloudflareMutationResponse>()
            .await?
            .result
            .id
    };

    sqlx::query(
        "INSERT INTO app_public_dns_records (app_id, zone_id, hostname, cloudflare_record_id, target)
         VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (zone_id, hostname)
         DO UPDATE SET app_id=$1, cloudflare_record_id=$4, target=$5, updated_at=now()",
    )
    .bind(app_id)
    .bind(zone_id)
    .bind(&host)
    .bind(record_id)
    .bind(target)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn delete_cloudflare_app_dns(
    state: &AppState,
    app_id: Uuid,
    domain: &str,
) -> anyhow::Result<()> {
    let Ok(host) = hostlet_public_cloudflare_host(state, domain) else {
        return Ok(());
    };
    let (Some(token), Some(zone_id)) = (&state.cloudflare_api_token, &state.cloudflare_zone_id)
    else {
        anyhow::bail!("Cloudflare DNS is not configured");
    };

    let client = &state.http;
    let base = format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records");
    let owned = sqlx::query(
        "SELECT cloudflare_record_id
         FROM app_public_dns_records
         WHERE app_id=$1 AND zone_id=$2 AND hostname=$3",
    )
    .bind(app_id)
    .bind(zone_id)
    .bind(&host)
    .fetch_optional(&state.db)
    .await?;

    if let Some(record) = owned {
        let record_id = record.get::<String, _>("cloudflare_record_id");
        let resp = client
            .delete(format!("{base}/{record_id}"))
            .bearer_auth(token)
            .send()
            .await?;
        if !resp.status().is_success() && resp.status() != StatusCode::NOT_FOUND {
            resp.error_for_status()?;
        }
        sqlx::query(
            "DELETE FROM app_public_dns_records WHERE app_id=$1 AND zone_id=$2 AND hostname=$3",
        )
        .bind(app_id)
        .bind(zone_id)
        .bind(&host)
        .execute(&state.db)
        .await?;
        return Ok(());
    }

    if !hostlet_legacy_prefixed_host(state, &host) {
        return Ok(());
    }

    let existing = client
        .get(&base)
        .bearer_auth(token)
        .query(&[("type", "CNAME"), ("name", host.as_str())])
        .send()
        .await?
        .error_for_status()?
        .json::<CloudflareListResponse>()
        .await?;

    for record in existing.result {
        let resp = client
            .delete(format!("{base}/{}", record.id))
            .bearer_auth(token)
            .send()
            .await?;
        if !resp.status().is_success() && resp.status() != StatusCode::NOT_FOUND {
            resp.error_for_status()?;
        }
    }

    Ok(())
}

fn default_domain_pattern(state: &AppState) -> Option<String> {
    state
        .base_domain
        .as_ref()
        .map(|base_domain| format!("{{app}}.{base_domain}"))
}

fn hostlet_public_cloudflare_host(state: &AppState, domain: &str) -> anyhow::Result<String> {
    if domain.contains(':') {
        anyhow::bail!("public app domain cannot include a port");
    }
    let Some(host) = domain_host(domain) else {
        anyhow::bail!("app domain is not a valid hostname");
    };
    let host = host.to_ascii_lowercase();
    if !valid_hostname(&host) {
        anyhow::bail!("app domain is not a valid hostname");
    }
    let Some(base_domain) = state.base_domain.as_ref() else {
        anyhow::bail!("HOSTLET_BASE_DOMAIN is not configured");
    };
    let Some(label) = host.strip_suffix(&format!(".{base_domain}")) else {
        anyhow::bail!("app domain must end with .{base_domain}");
    };
    if label.is_empty() {
        anyhow::bail!("app domain must use a label before {base_domain}");
    }
    if label.contains('.') {
        anyhow::bail!("app domain must use a single label before {base_domain}");
    }
    if reserved_public_domain_label(label) {
        anyhow::bail!("{label}.{base_domain} is reserved");
    }
    Ok(host)
}

fn hostlet_legacy_prefixed_host(state: &AppState, host: &str) -> bool {
    let Some(base_domain) = state.base_domain.as_ref() else {
        return false;
    };
    host.strip_suffix(&format!(".{base_domain}"))
        .is_some_and(|label| label.starts_with(&state.domain_prefix) && !label.contains('.'))
}

fn reserved_public_domain_label(label: &str) -> bool {
    matches!(
        label.to_ascii_lowercase().as_str(),
        "@" | "admin"
            | "api"
            | "app"
            | "apps"
            | "blog"
            | "cloudflare"
            | "cpanel"
            | "dns"
            | "ftp"
            | "hostlet"
            | "imap"
            | "mail"
            | "mx"
            | "ns1"
            | "ns2"
            | "pop"
            | "smtp"
            | "ssh"
            | "status"
            | "support"
            | "www"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        app_env_vars, app_slug, cancel_agent_job, check_app_health_now, cloud_app_domain_label,
        cloud_billing_webhook, cloud_create_contains_unsupported_settings,
        cloud_update_contains_unsupported_settings, create_app, delete_app, delete_app_env_var,
        get_app, reserved_public_domain_label, restart_app_container, retry_agent_job, run_cleanup,
        set_app_env_var, stripe_checkout_subscription_id, stripe_plan_code, stripe_webhook_action,
        update_app, valid_stripe_signature, AppState, Bytes, CreateApp, EnvValue, State,
        StatusCode, StripeWebhookAction, UpdateApp, Uuid,
    };
    use axum::{extract::Path, http::HeaderMap, response::IntoResponse, Json};
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    #[test]
    fn cloud_domain_label_prefers_clean_app_slug() {
        assert_eq!(cloud_app_domain_label("RunComp"), "runcomp");
        assert_eq!(
            cloud_app_domain_label("My Class Project"),
            "my-class-project"
        );
    }

    #[test]
    fn cloud_domain_label_preserves_reserved_detection_for_fallback() {
        let label = cloud_app_domain_label("Status");
        assert_eq!(label, "status");
        assert!(reserved_public_domain_label(&label));
    }

    #[test]
    fn app_slug_falls_back_for_empty_names() {
        assert_eq!(app_slug("!!!"), "app");
    }

    #[test]
    fn cloud_create_ignores_managed_exposure_and_auto_deploy_inputs() {
        let base = CreateApp {
            name: "Demo".into(),
            repo_full_name: "owner/repo".into(),
            branch: "main".into(),
            server_id: None,
            container_port: 3000,
            health_path: "/".into(),
            domain: "".into(),
            runtime_kind: Some("single".into()),
            hostlet_config_path: None,
            runtime_config: None,
            packaging_strategy: None,
            root_directory: None,
            install_command: None,
            build_command: None,
            start_command: None,
            memory_limit_mb: None,
            cpu_limit: None,
            public_exposure: None,
            auto_deploy: None,
            deploy_after_create: None,
            env: vec![],
        };
        assert!(!cloud_create_contains_unsupported_settings(&base));

        let mut custom_domain = base;
        custom_domain.domain = "demo.example.com".into();
        assert!(cloud_create_contains_unsupported_settings(&custom_domain));

        let mut custom_resources = custom_domain;
        custom_resources.domain = "".into();
        custom_resources.memory_limit_mb = Some(1024);
        assert!(!cloud_create_contains_unsupported_settings(
            &custom_resources
        ));

        let mut managed_false = custom_resources;
        managed_false.public_exposure = Some(false);
        managed_false.auto_deploy = Some(false);
        assert!(!cloud_create_contains_unsupported_settings(&managed_false));

        let mut managed_true = managed_false;
        managed_true.public_exposure = Some(true);
        managed_true.auto_deploy = Some(true);
        assert!(!cloud_create_contains_unsupported_settings(&managed_true));
    }

    #[test]
    fn cloud_update_rejects_cloud_restricted_settings() {
        let allowed = UpdateApp {
            domain: None,
            runtime_kind: Some("single".into()),
            hostlet_config_path: None,
            runtime_config: None,
            packaging_strategy: None,
            health_path: Some("/health".into()),
            root_directory: None,
            install_command: None,
            build_command: None,
            start_command: None,
            container_port: Some(3000),
            memory_limit_mb: None,
            cpu_limit: None,
            public_exposure: None,
            auto_deploy: None,
            env: None,
        };
        assert!(!cloud_update_contains_unsupported_settings(&allowed));

        let mut compose = allowed;
        compose.runtime_kind = Some(" compose ".into());
        assert!(cloud_update_contains_unsupported_settings(&compose));

        let mut public_toggle = compose;
        public_toggle.runtime_kind = None;
        public_toggle.public_exposure = Some(false);
        assert!(cloud_update_contains_unsupported_settings(&public_toggle));

        let mut custom_resources = public_toggle;
        custom_resources.public_exposure = None;
        custom_resources.memory_limit_mb = Some(Some(1024));
        assert!(cloud_update_contains_unsupported_settings(
            &custom_resources
        ));
    }

    #[test]
    fn checkout_completion_requires_subscription_before_pending_record() {
        let object = serde_json::json!({
            "metadata": {
                "cloud_user_id": "00000000-0000-0000-0000-000000000001",
                "plan": "starter"
            }
        });
        assert!(stripe_checkout_subscription_id(&object).is_err());

        let object = serde_json::json!({
            "subscription": "sub_123",
            "metadata": {
                "cloud_user_id": "00000000-0000-0000-0000-000000000001",
                "plan": "starter"
            }
        });
        assert_eq!(stripe_checkout_subscription_id(&object).unwrap(), "sub_123");
    }

    #[test]
    fn stripe_plan_metadata_is_required_and_known() {
        assert_eq!(
            stripe_plan_code(&serde_json::json!({"metadata": {"plan": "pro"}})).unwrap(),
            "pro"
        );
        assert!(stripe_plan_code(&serde_json::json!({"metadata": {}})).is_err());
        assert!(
            stripe_plan_code(&serde_json::json!({"metadata": {"plan": "enterprise"}})).is_err()
        );
    }

    #[test]
    fn stripe_signature_requires_recent_valid_v1_signature() {
        let body = br#"{"id":"evt_123"}"#;
        let secret = "whsec_test";
        let timestamp = chrono::Utc::now().timestamp();
        let mut headers = HeaderMap::new();
        headers.insert(
            "stripe-signature",
            stripe_signature_header(body, secret, timestamp)
                .parse()
                .unwrap(),
        );
        assert!(valid_stripe_signature(&headers, body, secret));

        headers.insert(
            "stripe-signature",
            stripe_signature_header(body, secret, timestamp - 301)
                .parse()
                .unwrap(),
        );
        assert!(!valid_stripe_signature(&headers, body, secret));
    }

    #[test]
    fn stripe_signature_is_provider_auth_not_browser_origin() {
        let body = br#"{"id":"evt_123"}"#;
        let secret = "whsec_test";
        let timestamp = chrono::Utc::now().timestamp();
        let mut headers = HeaderMap::new();
        headers.insert(
            "stripe-signature",
            stripe_signature_header(body, secret, timestamp)
                .parse()
                .unwrap(),
        );
        assert!(valid_stripe_signature(&headers, body, secret));

        headers.insert("origin", "https://hostlet.cloud".parse().unwrap());
        headers.insert("stripe-signature", "t=1,v1=bad".parse().unwrap());
        assert!(!valid_stripe_signature(&headers, body, secret));
    }

    #[test]
    fn stripe_webhook_routes_all_subscription_lifecycle_events() {
        assert_eq!(
            stripe_webhook_action("checkout.session.completed"),
            StripeWebhookAction::CheckoutCompleted
        );
        for event_type in [
            "customer.subscription.created",
            "customer.subscription.updated",
            "customer.subscription.deleted",
        ] {
            assert_eq!(
                stripe_webhook_action(event_type),
                StripeWebhookAction::SubscriptionChanged
            );
        }
        assert_eq!(
            stripe_webhook_action("invoice.paid"),
            StripeWebhookAction::Ignore
        );
    }

    #[tokio::test]
    async fn cloud_db_stripe_webhooks_dedupe_and_update_subscription_lifecycle() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_cloud_db(&state).await;
        let cloud_user_id = insert_cloud_user(&state, 303, "stripe-user").await;
        let secret = state.stripe_webhook_secret.as_deref().unwrap();

        let checkout = stripe_event(
            "evt_checkout",
            "checkout.session.completed",
            serde_json::json!({
                "id": "cs_test_1",
                "customer": "cus_test_1",
                "subscription": "sub_test_1",
                "metadata": {
                    "cloud_user_id": cloud_user_id.to_string(),
                    "plan": "starter"
                }
            }),
        );
        assert_eq!(
            post_stripe_webhook(&state, secret, checkout.clone()).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            post_stripe_webhook(&state, secret, checkout).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM cloud_webhook_events WHERE provider='stripe' AND provider_event_id='evt_checkout'",
            )
            .fetch_one(&state.db)
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            subscription_status(&state, "sub_test_1").await.as_deref(),
            Some("pending")
        );

        for (event_id, event_type, status) in [
            (
                "evt_sub_created",
                "customer.subscription.created",
                "trialing",
            ),
            ("evt_sub_updated", "customer.subscription.updated", "active"),
            (
                "evt_sub_deleted",
                "customer.subscription.deleted",
                "canceled",
            ),
        ] {
            let payload = stripe_event(
                event_id,
                event_type,
                serde_json::json!({
                    "id": "sub_test_1",
                    "customer": "cus_test_1",
                    "status": status,
                    "metadata": {
                        "plan": "starter"
                    },
                    "current_period_start": 1_700_000_000i64,
                    "current_period_end": 1_700_086_400i64,
                    "cancel_at_period_end": status == "canceled"
                }),
            );
            assert_eq!(
                post_stripe_webhook(&state, secret, payload).await,
                StatusCode::NO_CONTENT
            );
            assert_eq!(
                subscription_status(&state, "sub_test_1").await.as_deref(),
                Some(status)
            );
        }
    }

    #[tokio::test]
    async fn cloud_db_stripe_webhook_rejects_missing_metadata_without_activating() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_cloud_db(&state).await;
        let cloud_user_id = insert_cloud_user(&state, 404, "stripe-missing").await;
        let secret = state.stripe_webhook_secret.as_deref().unwrap();

        let missing_subscription = stripe_event(
            "evt_missing_subscription",
            "checkout.session.completed",
            serde_json::json!({
                "id": "cs_missing_sub",
                "customer": "cus_missing_sub",
                "metadata": {
                    "cloud_user_id": cloud_user_id.to_string(),
                    "plan": "starter"
                }
            }),
        );
        assert_eq!(
            post_stripe_webhook(&state, secret, missing_subscription).await,
            StatusCode::BAD_GATEWAY
        );

        let missing_plan = stripe_event(
            "evt_missing_plan",
            "customer.subscription.updated",
            serde_json::json!({
                "id": "sub_missing_plan",
                "customer": "cus_missing_sub",
                "status": "active",
                "metadata": {
                    "cloud_user_id": cloud_user_id.to_string()
                }
            }),
        );
        assert_eq!(
            post_stripe_webhook(&state, secret, missing_plan).await,
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM cloud_subscriptions WHERE cloud_user_id=$1 AND status IN ('active','trialing')",
            )
            .bind(cloud_user_id)
            .fetch_one(&state.db)
            .await
            .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn cloud_db_operator_cleanup_rejects_cloud_customer_session() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_cloud_db(&state).await;
        let user_id = insert_app_user(&state, 505, "cleanup-user").await;
        let cloud_user_id = insert_cloud_user(&state, 505, "cleanup-user").await;
        insert_cloud_session(&state, cloud_user_id, "cleanup-token").await;

        let mut headers = HeaderMap::new();
        headers.insert(
            "cookie",
            crate::auth::test_cloud_cookie_header(&state.session_secret, user_id, "cleanup-token")
                .parse()
                .unwrap(),
        );
        assert_eq!(
            run_cleanup(State(state), headers)
                .await
                .into_response()
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn cloud_db_compute_mutations_require_ready_account() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_cloud_db(&state).await;
        let user_id = insert_app_user(&state, 606, "gated-user").await;
        let cloud_user_id = insert_cloud_user(&state, 606, "gated-user").await;
        insert_cloud_session(&state, cloud_user_id, "gated-token").await;
        let headers = cloud_headers(&state, user_id, "gated-token");
        let app_id = insert_cloud_app(&state, user_id, "gated-app").await;
        let deployment_id = insert_successful_deployment(&state, app_id).await;
        let job_id = insert_agent_job(&state, app_id, Some(deployment_id), "failed").await;

        for status in [
            create_app(
                State(state.clone()),
                headers.clone(),
                Json(create_app_payload("blocked-create")),
            )
            .await
            .into_response()
            .status(),
            update_app(
                State(state.clone()),
                headers.clone(),
                Path(app_id),
                Json(update_app_payload()),
            )
            .await
            .into_response()
            .status(),
            set_app_env_var(
                State(state.clone()),
                headers.clone(),
                Path((app_id, "SECRET".to_string())),
                Json(EnvValue {
                    value: "blocked".into(),
                }),
            )
            .await
            .into_response()
            .status(),
            delete_app_env_var(
                State(state.clone()),
                headers.clone(),
                Path((app_id, "SECRET".to_string())),
            )
            .await
            .into_response()
            .status(),
            check_app_health_now(State(state.clone()), headers.clone(), Path(app_id))
                .await
                .into_response()
                .status(),
            restart_app_container(State(state.clone()), headers.clone(), Path(app_id))
                .await
                .into_response()
                .status(),
            retry_agent_job(State(state.clone()), headers.clone(), Path(job_id))
                .await
                .into_response()
                .status(),
            cancel_agent_job(State(state.clone()), headers.clone(), Path(job_id))
                .await
                .into_response()
                .status(),
            delete_app(State(state.clone()), headers.clone(), Path(app_id))
                .await
                .into_response()
                .status(),
        ] {
            assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
        }

        activate_cloud_account(&state, cloud_user_id).await;
        assert_eq!(
            check_app_health_now(State(state.clone()), headers.clone(), Path(app_id))
                .await
                .into_response()
                .status(),
            StatusCode::ACCEPTED
        );
        assert_eq!(
            restart_app_container(State(state.clone()), headers.clone(), Path(app_id))
                .await
                .into_response()
                .status(),
            StatusCode::ACCEPTED
        );
    }

    #[tokio::test]
    async fn cloud_db_customer_data_is_isolated_by_user() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_cloud_db(&state).await;
        let owner_user = insert_app_user(&state, 707, "owner-user").await;
        let other_user = insert_app_user(&state, 808, "other-user").await;
        let owner_cloud = insert_cloud_user(&state, 707, "owner-user").await;
        let other_cloud = insert_cloud_user(&state, 808, "other-user").await;
        insert_cloud_session(&state, owner_cloud, "owner-token").await;
        insert_cloud_session(&state, other_cloud, "other-token").await;
        activate_cloud_account(&state, owner_cloud).await;
        activate_cloud_account(&state, other_cloud).await;

        let app_id = insert_cloud_app(&state, owner_user, "owner-app").await;
        let deployment_id = insert_successful_deployment(&state, app_id).await;
        insert_env_var(&state, app_id, "OWNER_SECRET", "secret").await;
        let job_id = insert_agent_job(&state, app_id, Some(deployment_id), "failed").await;
        let owner_headers = cloud_headers(&state, owner_user, "owner-token");
        let other_headers = cloud_headers(&state, other_user, "other-token");

        assert_eq!(
            get_app(State(state.clone()), owner_headers.clone(), Path(app_id))
                .await
                .into_response()
                .status(),
            StatusCode::OK
        );
        for status in [
            get_app(State(state.clone()), other_headers.clone(), Path(app_id))
                .await
                .into_response()
                .status(),
            app_env_vars(State(state.clone()), other_headers.clone(), Path(app_id))
                .await
                .into_response()
                .status(),
            crate::deploy::get_deployment(
                State(state.clone()),
                other_headers.clone(),
                Path(deployment_id),
            )
            .await
            .into_response()
            .status(),
            crate::deploy::deployment_logs(
                State(state.clone()),
                other_headers.clone(),
                Path(deployment_id),
            )
            .await
            .into_response()
            .status(),
            retry_agent_job(State(state.clone()), other_headers.clone(), Path(job_id))
                .await
                .into_response()
                .status(),
            cancel_agent_job(State(state.clone()), other_headers.clone(), Path(job_id))
                .await
                .into_response()
                .status(),
        ] {
            assert_eq!(status, StatusCode::NOT_FOUND);
        }
    }

    fn stripe_signature_header(body: &[u8], secret: &str, timestamp: i64) -> String {
        let timestamp = timestamp.to_string();
        let signed_payload = [timestamp.as_bytes(), b".", body].concat();
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(&signed_payload);
        let signature = super::hex_bytes(&mac.finalize().into_bytes());
        format!("t={timestamp},v1={signature}")
    }

    async fn post_stripe_webhook(
        state: &AppState,
        secret: &str,
        payload: serde_json::Value,
    ) -> StatusCode {
        let body = serde_json::to_vec(&payload).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "stripe-signature",
            stripe_signature_header(&body, secret, chrono::Utc::now().timestamp())
                .parse()
                .unwrap(),
        );
        cloud_billing_webhook(State(state.clone()), headers, Bytes::from(body))
            .await
            .into_response()
            .status()
    }

    fn stripe_event(id: &str, event_type: &str, object: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "type": event_type,
            "data": {
                "object": object
            }
        })
    }

    async fn reset_cloud_db(state: &AppState) {
        sqlx::query(
            "TRUNCATE cloud_webhook_events, cloud_usage_buckets, cloud_subscriptions,
             cloud_stripe_customers, cloud_github_installations, cloud_sessions,
             cloud_users, users CASCADE",
        )
        .execute(&state.db)
        .await
        .unwrap();
    }

    async fn insert_cloud_user(state: &AppState, github_id: i64, login: &str) -> Uuid {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO cloud_users (github_id, login) VALUES ($1,$2) RETURNING id",
        )
        .bind(github_id)
        .bind(login)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    async fn insert_app_user(state: &AppState, github_id: i64, login: &str) -> Uuid {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO users (github_id, login) VALUES ($1,$2) RETURNING id",
        )
        .bind(github_id)
        .bind(login)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    async fn insert_cloud_session(state: &AppState, cloud_user_id: Uuid, token: &str) {
        sqlx::query(
            "INSERT INTO cloud_sessions (cloud_user_id, token_hash, expires_at)
             VALUES ($1,$2,now() + interval '1 hour')",
        )
        .bind(cloud_user_id)
        .bind(crate::crypto::hash_token(token))
        .execute(&state.db)
        .await
        .unwrap();
    }

    fn cloud_headers(state: &AppState, user_id: Uuid, cloud_token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "cookie",
            crate::auth::test_cloud_cookie_header(&state.session_secret, user_id, cloud_token)
                .parse()
                .unwrap(),
        );
        headers
    }

    async fn activate_cloud_account(state: &AppState, cloud_user_id: Uuid) {
        sqlx::query(
            "INSERT INTO cloud_github_installations
               (cloud_user_id, installation_id, account_login, account_type, permissions_json, repository_selection)
             VALUES ($1,$2,'ci-user','User','{}'::jsonb,'selected')",
        )
        .bind(cloud_user_id)
        .bind(rand_installation_id())
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO cloud_subscriptions (cloud_user_id, stripe_subscription_id, plan_code, status)
             VALUES ($1,$2,'starter','active')",
        )
        .bind(cloud_user_id)
        .bind(format!("sub_{}", Uuid::new_v4().simple()))
        .execute(&state.db)
        .await
        .unwrap();
    }

    async fn insert_cloud_app(state: &AppState, user_id: Uuid, name: &str) -> Uuid {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO apps
               (user_id,server_id,name,repo_full_name,branch,container_port,health_path,domain,runtime_kind,root_directory,memory_limit_mb,cpu_limit,public_exposure,auto_deploy)
             VALUES ($1,'00000000-0000-0000-0000-000000000001',$2,'hostlet-ci/node-hello','main',3000,'/health',$3,'single','.',512,0.5,true,false)
             RETURNING id",
        )
        .bind(user_id)
        .bind(name)
        .bind(format!("{name}.hostlet.cloud"))
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    async fn insert_successful_deployment(state: &AppState, app_id: Uuid) -> Uuid {
        let id = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO deployments
               (app_id,server_id,status,commit_sha,started_at,finished_at,container_name,published_port,runtime_kind)
             VALUES ($1,'00000000-0000-0000-0000-000000000001','success','HEAD',now(),now(),$2,32001,'single')
             RETURNING id",
        )
        .bind(app_id)
        .bind(format!("hostlet-app-{app_id}"))
        .fetch_one(&state.db)
        .await
        .unwrap();
        sqlx::query("UPDATE apps SET current_deployment_id=$1 WHERE id=$2")
            .bind(id)
            .bind(app_id)
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO deployment_logs (deployment_id,stream,line) VALUES ($1,'stdout','ready')",
        )
        .bind(id)
        .execute(&state.db)
        .await
        .unwrap();
        id
    }

    async fn insert_agent_job(
        state: &AppState,
        app_id: Uuid,
        deployment_id: Option<Uuid>,
        status: &str,
    ) -> Uuid {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO agent_jobs
               (server_id,app_id,deployment_id,job_type,status,payload_json)
             VALUES ('00000000-0000-0000-0000-000000000001',$1,$2,'health_check',$3,'{\"type\":\"health_check\"}'::jsonb)
             RETURNING id",
        )
        .bind(app_id)
        .bind(deployment_id)
        .bind(status)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    async fn insert_env_var(state: &AppState, app_id: Uuid, key: &str, value: &str) {
        sqlx::query("INSERT INTO app_env_vars (app_id,key,value_ciphertext) VALUES ($1,$2,$3)")
            .bind(app_id)
            .bind(key)
            .bind(state.crypto.encrypt(value).unwrap())
            .execute(&state.db)
            .await
            .unwrap();
    }

    fn create_app_payload(name: &str) -> CreateApp {
        CreateApp {
            name: name.into(),
            repo_full_name: "hostlet-ci/node-hello".into(),
            branch: "main".into(),
            server_id: None,
            container_port: 3000,
            health_path: "/health".into(),
            domain: "".into(),
            runtime_kind: Some("single".into()),
            hostlet_config_path: None,
            runtime_config: None,
            packaging_strategy: None,
            root_directory: Some(".".into()),
            install_command: None,
            build_command: None,
            start_command: None,
            memory_limit_mb: None,
            cpu_limit: None,
            public_exposure: None,
            auto_deploy: None,
            deploy_after_create: Some(false),
            env: vec![],
        }
    }

    fn update_app_payload() -> UpdateApp {
        UpdateApp {
            domain: None,
            runtime_kind: None,
            hostlet_config_path: None,
            runtime_config: None,
            packaging_strategy: None,
            health_path: Some("/ready".into()),
            root_directory: None,
            install_command: None,
            build_command: None,
            start_command: None,
            container_port: Some(3000),
            memory_limit_mb: None,
            cpu_limit: None,
            public_exposure: None,
            auto_deploy: None,
            env: None,
        }
    }

    fn rand_installation_id() -> i64 {
        let bytes = *Uuid::new_v4().as_bytes();
        i64::from_be_bytes(bytes[..8].try_into().unwrap()).abs()
    }

    async fn subscription_status(state: &AppState, subscription_id: &str) -> Option<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM cloud_subscriptions WHERE stripe_subscription_id=$1",
        )
        .bind(subscription_id)
        .fetch_optional(&state.db)
        .await
        .unwrap()
    }
}

struct UpdateCheck {
    latest_version: String,
    release_notes_url: String,
    released_at: Option<String>,
    minimum_supported_version: Option<String>,
    compose_migrations: bool,
    database_migrations: bool,
}

async fn fetch_latest_release(state: &AppState) -> anyhow::Result<UpdateCheck> {
    let value: serde_json::Value = state
        .http
        .get("https://api.github.com/repos/ShaneKanterman04/Hostlet/releases/latest")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let latest_version = value
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0")
        .trim_start_matches('v')
        .to_string();
    let release_notes_url = value
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://github.com/ShaneKanterman04/Hostlet/releases/latest")
        .to_string();
    let mut update = UpdateCheck {
        latest_version,
        release_notes_url,
        released_at: value
            .get("published_at")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        minimum_supported_version: None,
        compose_migrations: false,
        database_migrations: false,
    };
    if let Some(manifest_url) = value
        .get("assets")
        .and_then(|v| v.as_array())
        .and_then(|assets| {
            assets.iter().find_map(|asset| {
                let name = asset.get("name")?.as_str()?;
                (name == "hostlet-release.json")
                    .then(|| {
                        asset
                            .get("browser_download_url")?
                            .as_str()
                            .map(str::to_string)
                    })
                    .flatten()
            })
        })
    {
        apply_update_manifest(state, &mut update, &manifest_url).await?;
    }
    Ok(update)
}

async fn apply_update_manifest(
    state: &AppState,
    update: &mut UpdateCheck,
    manifest_url: &str,
) -> anyhow::Result<()> {
    let value: serde_json::Value = state
        .http
        .get(manifest_url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if let Some(version) = value.get("version").and_then(|v| v.as_str()) {
        update.latest_version = version.trim_start_matches('v').to_string();
    }
    update.released_at = value
        .get("released_at")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| update.released_at.clone());
    update.minimum_supported_version = value
        .get("minimum_supported_version")
        .and_then(|v| v.as_str())
        .map(|value| value.trim_start_matches('v').to_string());
    update.compose_migrations = value
        .get("compose_migrations")
        .and_then(|v| v.as_bool())
        .unwrap_or(update.compose_migrations);
    update.database_migrations = value
        .get("database_migrations")
        .and_then(|v| v.as_bool())
        .unwrap_or(update.database_migrations);
    if let Some(notes_url) = value.get("notes_url").and_then(|v| v.as_str()) {
        update.release_notes_url = notes_url.to_string();
    }
    Ok(())
}

async fn cached_update_check(state: &AppState) -> Option<serde_json::Value> {
    let row = sqlx::query("SELECT value,updated_at FROM settings WHERE key='system_update_check'")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()?;
    let value: String = row.get("value");
    let mut json = serde_json::from_str::<serde_json::Value>(&value).ok()?;
    if let serde_json::Value::Object(ref mut object) = json {
        object.insert(
            "checkedAt".into(),
            serde_json::json!(row.get::<chrono::DateTime<chrono::Utc>, _>("updated_at")),
        );
    }
    Some(json)
}

async fn refresh_update_check(state: &AppState) -> anyhow::Result<serde_json::Value> {
    let update = fetch_latest_release(state).await?;
    let value = serde_json::json!({
        "latestVersion": update.latest_version,
        "releaseNotesUrl": update.release_notes_url,
        "releasedAt": update.released_at,
        "minimumSupportedVersion": update.minimum_supported_version,
        "composeMigrations": update.compose_migrations,
        "databaseMigrations": update.database_migrations,
        "updateAvailable": version_is_newer(env!("CARGO_PKG_VERSION"), &update.latest_version),
        "unsupportedDirectUpdate": update.minimum_supported_version.as_ref().is_some_and(|minimum| version_is_newer(minimum, env!("CARGO_PKG_VERSION"))),
    });
    let _ = sqlx::query(
        "INSERT INTO settings (key,value,updated_at) VALUES ('system_update_check',$1,now())
         ON CONFLICT (key) DO UPDATE SET value=EXCLUDED.value, updated_at=now()",
    )
    .bind(value.to_string())
    .execute(&state.db)
    .await;
    Ok(value)
}

fn version_is_newer(current: &str, latest: &str) -> bool {
    version_parts(latest) > version_parts(current)
}

fn version_parts(value: &str) -> (u64, u64, u64) {
    let mut parts = value
        .trim_start_matches('v')
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

fn domain_host(value: &str) -> Option<&str> {
    if let Some((host, port)) = value.rsplit_once(':') {
        if port.parse::<u16>().is_ok() {
            return Some(host);
        }
    }
    Some(value)
}

#[derive(Deserialize)]
struct CloudflareListResponse {
    result: Vec<CloudflareRecord>,
}

#[derive(Deserialize)]
struct CloudflareRecord {
    id: String,
}

#[derive(Deserialize)]
struct CloudflareMutationResponse {
    result: CloudflareRecord,
}

#[derive(Serialize)]
struct CloudflareDnsRecord<'a> {
    #[serde(rename = "type")]
    record_type: &'a str,
    name: &'a str,
    content: &'a str,
    proxied: bool,
}
