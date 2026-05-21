use crate::{
    auth::current_user_id,
    crypto::{hash_token, random_token},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tokio::process::Command;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct CreateServer {
    name: String,
    public_ip: Option<String>,
}

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
    env: Option<Vec<EnvVar>>,
}

pub async fn list_servers(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match sqlx::query("SELECT id,name,public_ip,kind,status,last_seen_at,created_at FROM servers WHERE user_id=$1 OR kind='local' ORDER BY kind ASC, created_at DESC")
        .bind(user_id).fetch_all(&state.db).await {
        Ok(rows) => Json(rows.into_iter().map(|r| serde_json::json!({
            "id": r.get::<Uuid,_>("id"), "name": r.get::<String,_>("name"), "publicIp": r.get::<Option<String>,_>("public_ip"),
            "kind": r.get::<String,_>("kind"), "status": r.get::<String,_>("status"), "lastSeenAt": r.get::<Option<chrono::DateTime<chrono::Utc>>,_>("last_seen_at")
        })).collect::<Vec<_>>()).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn create_server(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateServer>,
) -> impl IntoResponse {
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if body.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "server name is required").into_response();
    }
    let token = random_token(64);
    let row = sqlx::query("INSERT INTO servers (user_id,name,public_ip,kind,install_token_hash) VALUES ($1,$2,$3,'remote',$4) RETURNING id")
        .bind(user_id).bind(body.name.trim()).bind(body.public_ip).bind(hash_token(&token))
        .fetch_one(&state.db).await;
    match row {
        Ok(r) => {
            let id = r.get::<Uuid, _>("id");
            Json(serde_json::json!({
                "id": id,
                "installToken": token,
                "installCommand": format!(
                    "curl -fsSL {}/install-agent.sh | sudo HOSTLET_API_URL={} HOSTLET_SERVER_ID={} HOSTLET_INSTALL_TOKEN={} bash",
                    state.public_api_url, state.public_api_url, id, token
                )
            })).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn server_install_command(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let row = sqlx::query("SELECT install_token_hash FROM servers WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await;
    match row {
        Ok(Some(_)) => Json(serde_json::json!({"command": format!("curl -fsSL {}/install-agent.sh | sudo HOSTLET_API_URL={} HOSTLET_SERVER_ID={} bash", state.public_api_url, state.public_api_url, id)})).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
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
          current.finished_at AS current_deployment_finished_at
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
          current.finished_at AS current_deployment_finished_at
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
        SELECT d.container_name, s.kind
        FROM apps a
        JOIN servers s ON s.id = a.server_id
        LEFT JOIN deployments d ON d.id = a.current_deployment_id
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

    match docker_stats(&container).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => (StatusCode::BAD_GATEWAY, err.to_string()).into_response(),
    }
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
    let server = sqlx::query("SELECT id FROM servers WHERE id=$1 AND (user_id=$2 OR kind='local')")
        .bind(server_id)
        .bind(user_id)
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
    if let Err(err) = ensure_cloudflare_app_dns(&state, &domain).await {
        tracing::warn!(error = %err, domain = %domain, "failed to provision Cloudflare DNS");
        return (
            StatusCode::BAD_GATEWAY,
            "failed to provision DNS for app domain",
        )
            .into_response();
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
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let row = sqlx::query("INSERT INTO apps (user_id,server_id,name,repo_full_name,branch,container_port,health_path,domain,root_directory,install_command,build_command,start_command,memory_limit_mb,cpu_limit) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) RETURNING id")
        .bind(user_id).bind(server_id).bind(app_name).bind(repo_full_name).bind(branch).bind(body.container_port).bind(health_path).bind(domain)
        .bind(root_directory).bind(install_command).bind(build_command).bind(start_command)
        .bind(body.memory_limit_mb).bind(body.cpu_limit)
        .fetch_one(&mut *tx).await;
    let Ok(row) = row else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let app_id: Uuid = row.get("id");
    for ev in body.env {
        if !valid_env_key(&ev.key) {
            return (StatusCode::BAD_REQUEST, "invalid env var key").into_response();
        }
        let enc = match state.crypto.encrypt(&ev.value) {
            Ok(v) => v,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        let _ =
            sqlx::query("INSERT INTO app_env_vars (app_id,key,value_ciphertext) VALUES ($1,$2,$3)")
                .bind(app_id)
                .bind(ev.key)
                .bind(enc)
                .execute(&mut *tx)
                .await;
    }
    if tx.commit().await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
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
    let exists = sqlx::query("SELECT id FROM apps WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);
    if exists.is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Some(domain) = body.domain {
        let domain = domain.trim().to_string();
        if !valid_domain(&domain) {
            return (
                StatusCode::BAD_REQUEST,
                "domain must be a hostname with optional port",
            )
                .into_response();
        }
        let _ = sqlx::query("UPDATE apps SET domain=$1, updated_at=now() WHERE id=$2")
            .bind(domain)
            .bind(id)
            .execute(&state.db)
            .await;
    }
    if let Some(path) = body.health_path {
        let path = path.trim().to_string();
        if !valid_health_path(&path) {
            return (
                StatusCode::BAD_REQUEST,
                "health path must start with / and cannot contain control characters",
            )
                .into_response();
        }
        let _ = sqlx::query("UPDATE apps SET health_path=$1, updated_at=now() WHERE id=$2")
            .bind(path)
            .bind(id)
            .execute(&state.db)
            .await;
    }
    if let Some(env) = body.env {
        if env.iter().any(|ev| !valid_env_key(&ev.key)) {
            return (StatusCode::BAD_REQUEST, "invalid env var key").into_response();
        }
        let _ = sqlx::query("DELETE FROM app_env_vars WHERE app_id=$1")
            .bind(id)
            .execute(&state.db)
            .await;
        for ev in env {
            let Ok(enc) = state.crypto.encrypt(&ev.value) else {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            };
            let _ = sqlx::query(
                "INSERT INTO app_env_vars (app_id,key,value_ciphertext) VALUES ($1,$2,$3)",
            )
            .bind(id)
            .bind(ev.key)
            .bind(enc)
            .execute(&state.db)
            .await;
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

pub async fn delete_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let res = sqlx::query("DELETE FROM apps WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user_id)
        .execute(&state.db)
        .await;
    match res {
        Ok(done) if done.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
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

async fn ensure_cloudflare_app_dns(state: &AppState, domain: &str) -> anyhow::Result<()> {
    let Some(host) = domain_host(domain) else {
        return Ok(());
    };
    let Some(base_domain) = &state.base_domain else {
        return Ok(());
    };
    let Some(label) = host.strip_suffix(&format!(".{base_domain}")) else {
        return Ok(());
    };
    if label.contains('.') || !label.starts_with(&state.domain_prefix) {
        return Ok(());
    }
    let (Some(token), Some(zone_id), Some(target)) = (
        &state.cloudflare_api_token,
        &state.cloudflare_zone_id,
        &state.cloudflare_tunnel_target,
    ) else {
        return Ok(());
    };

    let client = reqwest::Client::new();
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

async fn docker_stats(container: &str) -> anyhow::Result<serde_json::Value> {
    let output = Command::new("docker")
        .args(["stats", "--no-stream", "--format", "json", container])
        .output()
        .await
        .map_err(|err| anyhow::anyhow!("Docker stats is unavailable: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Docker could not read container stats: {}", stderr.trim());
    }
    let stdout = String::from_utf8(output.stdout)?;
    let raw: serde_json::Value = serde_json::from_str(stdout.trim())?;
    Ok(serde_json::json!({
        "container": container,
        "name": raw.get("Name").and_then(|v| v.as_str()).unwrap_or(container),
        "cpuPercent": raw.get("CPUPerc").and_then(|v| v.as_str()).unwrap_or("0%"),
        "memoryUsage": raw.get("MemUsage").and_then(|v| v.as_str()).unwrap_or("0B / 0B"),
        "memoryPercent": raw.get("MemPerc").and_then(|v| v.as_str()).unwrap_or("0%"),
        "networkIo": raw.get("NetIO").and_then(|v| v.as_str()).unwrap_or("0B / 0B"),
        "blockIo": raw.get("BlockIO").and_then(|v| v.as_str()).unwrap_or("0B / 0B"),
        "pids": raw.get("PIDs").and_then(|v| v.as_str()).unwrap_or("0"),
        "sampledAt": chrono::Utc::now()
    }))
}
