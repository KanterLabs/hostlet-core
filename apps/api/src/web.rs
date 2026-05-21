use crate::{auth::current_user_id, deploy, state::AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
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
    root_directory: Option<String>,
    install_command: Option<String>,
    build_command: Option<String>,
    start_command: Option<String>,
    memory_limit_mb: Option<i32>,
    cpu_limit: Option<f64>,
    public_exposure: Option<bool>,
    auto_deploy: Option<bool>,
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
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
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
          s.kind AS server_kind,
          s.status AS server_status,
          s.last_seen_at AS server_last_seen_at,
          latest.id AS latest_deployment_id,
          latest.status AS latest_deployment_status,
          latest.commit_sha AS latest_commit_sha,
          latest.failure_summary AS latest_failure_summary,
          latest.started_at AS latest_started_at,
          latest.finished_at AS latest_finished_at,
          current.status AS current_deployment_status,
          current.finished_at AS current_deployment_finished_at,
          latest_webhook.status AS latest_webhook_status,
          latest_webhook.ignored_reason AS latest_webhook_ignored_reason,
          latest_webhook.commit_sha AS latest_webhook_commit_sha,
          latest_webhook.branch AS latest_webhook_branch,
          latest_webhook.deployment_id AS latest_webhook_deployment_id,
          latest_webhook.created_at AS latest_webhook_created_at
        FROM apps a
        JOIN servers s ON s.id = a.server_id
        LEFT JOIN LATERAL (
          SELECT id,status,commit_sha,failure_summary,started_at,finished_at
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
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
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
          s.kind AS server_kind,
          s.status AS server_status,
          s.last_seen_at AS server_last_seen_at,
          latest.id AS latest_deployment_id,
          latest.status AS latest_deployment_status,
          latest.commit_sha AS latest_commit_sha,
          latest.failure_summary AS latest_failure_summary,
          latest.started_at AS latest_started_at,
          latest.finished_at AS latest_finished_at,
          current.status AS current_deployment_status,
          current.finished_at AS current_deployment_finished_at,
          latest_webhook.status AS latest_webhook_status,
          latest_webhook.ignored_reason AS latest_webhook_ignored_reason,
          latest_webhook.commit_sha AS latest_webhook_commit_sha,
          latest_webhook.branch AS latest_webhook_branch,
          latest_webhook.deployment_id AS latest_webhook_deployment_id,
          latest_webhook.created_at AS latest_webhook_created_at
        FROM apps a
        JOIN servers s ON s.id = a.server_id
        LEFT JOIN LATERAL (
          SELECT id,status,commit_sha,failure_summary,started_at,finished_at
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

pub async fn app_resources(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
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
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
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
    if !valid_memory_limit(body.memory_limit_mb) {
        return (
            StatusCode::BAD_REQUEST,
            "memory limit must be between 64 and 262144 MB",
        )
            .into_response();
    }
    if !valid_cpu_limit(body.cpu_limit) {
        return (
            StatusCode::BAD_REQUEST,
            "CPU limit must be between 0.1 and 128",
        )
            .into_response();
    }
    let server_id = match body.server_id {
        Some(id) => id,
        None => Uuid::parse_str(
            &std::env::var("LOCAL_SERVER_ID")
                .unwrap_or_else(|_| "00000000-0000-0000-0000-000000000001".into()),
        )
        .unwrap(),
    };
    let server = sqlx::query("SELECT id FROM servers WHERE id=$1 AND kind='local'")
        .bind(server_id)
        .fetch_optional(&state.db)
        .await;
    let Ok(Some(_)) = server else {
        return (StatusCode::BAD_REQUEST, "server is not available").into_response();
    };
    let domain = if body.domain.trim().is_empty() {
        match &state.base_domain {
            Some(base_domain) => format!(
                "{}{}.{}",
                state.domain_prefix,
                app_slug(app_name),
                base_domain
            ),
            None => format!("localhost:{}", 20000 + (body.container_port as u16 % 20000)),
        }
    } else {
        body.domain.trim().to_string()
    };
    if !valid_domain(&domain) {
        return (
            StatusCode::BAD_REQUEST,
            "domain must be a hostname with optional port",
        )
            .into_response();
    }
    let public_exposure = body.public_exposure.unwrap_or(false);
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
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let auto_deploy = body.auto_deploy.unwrap_or(false);
    let row = sqlx::query("INSERT INTO apps (user_id,server_id,name,repo_full_name,branch,container_port,health_path,domain,root_directory,install_command,build_command,start_command,memory_limit_mb,cpu_limit,public_exposure,auto_deploy) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16) RETURNING id")
        .bind(user_id).bind(server_id).bind(app_name).bind(repo_full_name).bind(branch).bind(body.container_port).bind(health_path).bind(&domain)
        .bind(root_directory).bind(install_command).bind(build_command).bind(start_command)
        .bind(body.memory_limit_mb).bind(body.cpu_limit).bind(false).bind(auto_deploy)
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
    if public_exposure {
        if let Err(err) = ensure_cloudflare_app_dns(&state, &domain).await {
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
            let _ = delete_cloudflare_app_dns(&state, &domain).await;
            delete_created_app_row(&state, app_id).await;
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    Json(serde_json::json!({"id": app_id})).into_response()
}

pub async fn update_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateApp>,
) -> impl IntoResponse {
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let row =
        sqlx::query("SELECT id, domain, public_exposure FROM apps WHERE id=$1 AND user_id=$2")
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
    let domain_changed = body.domain.is_some();
    let mut app_domain = old_domain.clone();
    if let Some(domain) = &body.domain {
        let domain = domain.trim().to_string();
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
        app_domain = domain;
    }
    let desired_public_exposure = body.public_exposure.unwrap_or(old_public_exposure);
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
    if let Some(env) = &body.env {
        if let Err(message) = validate_env_vars(env) {
            return (StatusCode::BAD_REQUEST, message).into_response();
        }
    }
    if desired_public_exposure {
        if let Err(err) = ensure_cloudflare_app_dns(&state, &app_domain).await {
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
        if let Err(err) = delete_cloudflare_app_dns(&state, &old_domain).await {
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
            old_public_exposure,
            desired_public_exposure,
        )
        .await;
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

pub async fn app_env_vars(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
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
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
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
    .bind(key)
    .bind(enc)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn delete_app_env_var(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, key)): Path<(Uuid, String)>,
) -> impl IntoResponse {
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !app_belongs_to_user(&state, id, user_id).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !valid_env_key(&key) {
        return (StatusCode::BAD_REQUEST, "invalid env var key").into_response();
    }
    let res = sqlx::query("DELETE FROM app_env_vars WHERE app_id=$1 AND key=$2")
        .bind(id)
        .bind(key)
        .execute(&state.db)
        .await;
    match res {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn agent_job_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let row = sqlx::query(
        r#"
        SELECT j.id,j.job_type,j.app_id,j.status,j.failure_summary,j.finished_at
        FROM agent_jobs j
        JOIN servers s ON s.id = j.server_id
        WHERE j.id=$1 AND (s.user_id=$2 OR s.kind='local')
        "#,
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await;
    match row {
        Ok(Some(row)) => {
            let mut status = row.get::<String, _>("status");
            if status == "success"
                && row.get::<String, _>("job_type") == "delete_app"
                && row.get::<Option<Uuid>, _>("app_id").is_some()
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

pub async fn delete_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
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
            if let Err(err) = delete_cloudflare_app_dns(&state, &domain).await {
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
    let job_id = match sqlx::query(
        "INSERT INTO agent_jobs (server_id,app_id,job_type,status) VALUES ($1,$2,'delete_app','queued') RETURNING id",
    )
    .bind(server_id)
    .bind(id)
    .fetch_one(&state.db)
    .await
    {
        Ok(row) => row.get::<Uuid, _>("id"),
        Err(err) => {
            tracing::warn!(error = %err, app_id = %id, "failed to create delete app agent job");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let payload = serde_json::json!({
        "type": "delete_app",
        "job_id": job_id,
        "app_id": id,
        "route_key": format!("app-{id}"),
        "domain": domain,
        "containers": containers.clone(),
        "images": images,
    });
    let _ = sqlx::query("UPDATE agent_jobs SET status='running', updated_at=now() WHERE id=$1")
        .bind(job_id)
        .execute(&state.db)
        .await;
    if let Err(err) = deploy::send_agent_job(&state, server_id, payload).await {
        mark_agent_job_failed(&state, job_id, &err.to_string()).await;
        tracing::warn!(error = %err, app_id = %id, "failed to request app teardown from agent");
        return (
            StatusCode::BAD_GATEWAY,
            "failed to request app teardown from the server agent",
        )
            .into_response();
    }
    let finalize_state = state.clone();
    tokio::spawn(async move {
        finalize_delete_app(
            finalize_state,
            id,
            user_id,
            job_id,
            containers,
            domain,
            public_exposure,
        )
        .await;
    });
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"jobId": job_id})),
    )
        .into_response()
}

async fn finalize_delete_app(
    state: AppState,
    app_id: Uuid,
    user_id: Uuid,
    job_id: Uuid,
    containers: Vec<String>,
    domain: String,
    public_exposure: bool,
) {
    if let Err(err) = wait_for_agent_job(&state, job_id, Duration::from_secs(120)).await {
        tracing::warn!(error = %err, app_id = %app_id, "delete app agent job did not complete");
        mark_agent_job_failed(&state, job_id, &err.to_string()).await;
        return;
    }
    if public_exposure {
        if let Err(err) = delete_cloudflare_app_dns(&state, &domain).await {
            tracing::warn!(error = %err, domain = %domain, "failed to remove public tunnel DNS while deleting app");
            mark_agent_job_failed(&state, job_id, &err.to_string()).await;
            return;
        }
    }
    match delete_app_records(&state, app_id, user_id, &containers).await {
        Ok(true) => {
            mark_agent_job_success(&state, job_id).await;
        }
        Ok(false) => {
            mark_agent_job_failed(&state, job_id, "app disappeared before deletion completed")
                .await;
        }
        Err(err) => {
            tracing::warn!(error = %err, app_id = %app_id, "failed to delete app records after cleanup");
            mark_agent_job_failed(&state, job_id, &err.to_string()).await;
        }
    }
}

async fn mark_agent_job_success(state: &AppState, job_id: Uuid) {
    let _ = sqlx::query(
        "UPDATE agent_jobs
         SET status='success', failure_summary=NULL, updated_at=now(), finished_at=now()
         WHERE id=$1",
    )
    .bind(job_id)
    .execute(&state.db)
    .await;
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
    old_public_exposure: bool,
    desired_public_exposure: bool,
) {
    let opened_new_dns =
        desired_public_exposure && (!old_public_exposure || old_domain != app_domain);
    let closed_old_dns =
        old_public_exposure && (!desired_public_exposure || old_domain != app_domain);
    if opened_new_dns {
        if let Err(err) = delete_cloudflare_app_dns(state, app_domain).await {
            tracing::warn!(error = %err, domain = %app_domain, "failed to compensate new public tunnel after DB update failure");
        }
    }
    if closed_old_dns {
        if let Err(err) = ensure_cloudflare_app_dns(state, old_domain).await {
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

async fn wait_for_agent_job(
    state: &AppState,
    job_id: Uuid,
    timeout: Duration,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let row = sqlx::query("SELECT status,failure_summary FROM agent_jobs WHERE id=$1")
            .bind(job_id)
            .fetch_optional(&state.db)
            .await?;
        let Some(row) = row else {
            anyhow::bail!("agent job disappeared before completion");
        };
        match row.get::<String, _>("status").as_str() {
            "success" => return Ok(()),
            "failed" => {
                let failure = row
                    .get::<Option<String>, _>("failure_summary")
                    .unwrap_or_else(|| "agent reported cleanup failure".into());
                anyhow::bail!("{failure}");
            }
            _ if Instant::now() >= deadline => {
                anyhow::bail!(
                    "server agent did not confirm cleanup within {} seconds",
                    timeout.as_secs()
                );
            }
            _ => tokio::time::sleep(Duration::from_millis(300)).await,
        }
    }
}

fn app_json(r: sqlx::postgres::PgRow) -> serde_json::Value {
    serde_json::json!({
        "id": r.get::<Uuid,_>("id"), "name": r.get::<String,_>("name"), "repoFullName": r.get::<String,_>("repo_full_name"),
        "branch": r.get::<String,_>("branch"), "domain": r.get::<String,_>("domain"), "currentDeploymentId": r.get::<Option<Uuid>,_>("current_deployment_id"),
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
            "finishedAt": r.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("latest_finished_at").unwrap_or(None)
        })),
        "currentDeployment": r.try_get::<Option<String>,_>("current_deployment_status").unwrap_or(None).map(|status| serde_json::json!({
            "status": status,
            "finishedAt": r.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("current_deployment_finished_at").unwrap_or(None)
        })),
        "latestWebhook": r.try_get::<Option<String>,_>("latest_webhook_status").unwrap_or(None).map(|status| serde_json::json!({
            "status": status,
            "ignoredReason": r.try_get::<Option<String>,_>("latest_webhook_ignored_reason").unwrap_or(None),
            "commitSha": r.try_get::<Option<String>,_>("latest_webhook_commit_sha").unwrap_or(None),
            "branch": r.try_get::<Option<String>,_>("latest_webhook_branch").unwrap_or(None),
            "deploymentId": r.try_get::<Option<Uuid>,_>("latest_webhook_deployment_id").unwrap_or(None),
            "createdAt": r.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("latest_webhook_created_at").unwrap_or(None)
        }))
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
            "tunnelTargetConfigured": state.cloudflare_tunnel_target.is_some(),
            "message": "Cloudflare API token can access the configured zone."
        }))
        .into_response(),
        Ok(resp) => Json(serde_json::json!({
            "configured": configured,
            "tokenValid": false,
            "baseDomain": state.base_domain.as_deref(),
            "domainPrefix": state.domain_prefix,
            "tunnelTargetConfigured": state.cloudflare_tunnel_target.is_some(),
            "message": format!("Cloudflare zone check failed with status {}.", resp.status())
        }))
        .into_response(),
        Err(_) => Json(serde_json::json!({
            "configured": configured,
            "tokenValid": false,
            "baseDomain": state.base_domain.as_deref(),
            "domainPrefix": state.domain_prefix,
            "tunnelTargetConfigured": state.cloudflare_tunnel_target.is_some(),
            "message": "Could not reach Cloudflare from the API container."
        }))
        .into_response(),
    }
}

async fn ensure_cloudflare_app_dns(state: &AppState, domain: &str) -> anyhow::Result<()> {
    let Some(host) = hostlet_managed_cloudflare_host(state, domain) else {
        anyhow::bail!("app domain is not managed by Hostlet public tunnel DNS");
    };
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
        .query(&[("type", "CNAME"), ("name", host)])
        .send()
        .await?
        .error_for_status()?
        .json::<CloudflareListResponse>()
        .await?;

    let payload = CloudflareDnsRecord {
        record_type: "CNAME",
        name: host,
        content: target,
        proxied: true,
    };
    if let Some(record) = existing.result.first() {
        client
            .patch(format!("{base}/{}", record.id))
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;
    } else {
        client
            .post(&base)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;
    }
    Ok(())
}

async fn delete_cloudflare_app_dns(state: &AppState, domain: &str) -> anyhow::Result<()> {
    let Some(host) = hostlet_managed_cloudflare_host(state, domain) else {
        return Ok(());
    };
    let (Some(token), Some(zone_id)) = (&state.cloudflare_api_token, &state.cloudflare_zone_id)
    else {
        anyhow::bail!("Cloudflare DNS is not configured");
    };

    let client = &state.http;
    let base = format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records");
    let existing = client
        .get(&base)
        .bearer_auth(token)
        .query(&[("type", "CNAME"), ("name", host)])
        .send()
        .await?
        .error_for_status()?
        .json::<CloudflareListResponse>()
        .await?;

    for record in existing.result {
        client
            .delete(format!("{base}/{}", record.id))
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?;
    }
    Ok(())
}

fn hostlet_managed_cloudflare_host<'a>(state: &AppState, domain: &'a str) -> Option<&'a str> {
    let host = domain_host(domain)?;
    let base_domain = state.base_domain.as_ref()?;
    let label = host.strip_suffix(&format!(".{base_domain}"))?;
    if label.contains('.') || !label.starts_with(&state.domain_prefix) {
        return None;
    }
    Some(host)
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

#[derive(Serialize)]
struct CloudflareDnsRecord<'a> {
    #[serde(rename = "type")]
    record_type: &'a str,
    name: &'a str,
    content: &'a str,
    proxied: bool,
}
