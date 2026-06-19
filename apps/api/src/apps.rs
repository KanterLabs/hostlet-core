//! Generic app-record persistence.
//!
//! This is a self-hosted-first, Cloud-independent primitive: it persists a new
//! `apps` row plus its environment variables in one transaction and returns the
//! new app id. Both the self-hosted `web::apps::create_app` handler and Hostlet
//! Cloud's project-creation workflow compose this instead of duplicating the
//! INSERT, so app persistence stays a reusable hosting primitive in core while
//! cloud-only policy (entitlements, domain allocation, showcases) lives in
//! `hostlet-cloud`.

use crate::state::AppState;
use sqlx::Row;
use uuid::Uuid;

pub mod serialization;

/// A plaintext environment variable to persist with the app. The value is
/// encrypted with the app's crypto provider before it is written.
pub struct AppEnvVarInput {
    pub key: String,
    pub value: String,
}

/// Every validated field required to persist a new app row. Callers perform
/// their own input validation and policy (domain allocation, entitlement and
/// limit checks, cloud-vs-self-hosted defaults) before constructing this.
pub struct NewAppRecord {
    pub user_id: Uuid,
    pub server_id: Uuid,
    pub name: String,
    pub repo_full_name: String,
    pub branch: String,
    pub container_port: i32,
    pub health_path: String,
    pub domain: String,
    pub runtime_kind: String,
    pub hostlet_config_path: String,
    pub runtime_config: serde_json::Value,
    pub packaging_strategy: String,
    pub root_directory: String,
    pub install_command: Option<String>,
    pub build_command: Option<String>,
    pub start_command: Option<String>,
    pub memory_limit_mb: Option<i32>,
    pub cpu_limit: Option<f64>,
    pub public_exposure: bool,
    pub auto_deploy: bool,
    pub env: Vec<AppEnvVarInput>,
}

/// Why persisting a new app failed, mapped to the HTTP status the handlers
/// previously returned inline.
///
/// Only two variants are exposed so callers stay a simple status mapping
/// (`Insert` → 400, `Internal` → 500); finer-grained diagnostics about *which*
/// internal step failed (transaction, env encryption, commit) are emitted via
/// `tracing` inside [`create_app_record`] instead of widening this enum, which
/// would force every caller's `match` to grow new arms.
#[derive(Debug)]
pub enum CreateAppError {
    /// The INSERT was rejected (e.g. a constraint violation). Handlers return
    /// `400 Bad Request`.
    Insert,
    /// An internal failure (transaction, env encryption, or commit). → `500`.
    Internal,
}

/// Persist a new app row and its environment variables atomically, returning
/// the new app id. Generic across self-hosted and Hostlet Cloud.
pub async fn create_app_record(
    state: &AppState,
    record: NewAppRecord,
) -> Result<Uuid, CreateAppError> {
    // Destructure up front so every field is a named local and each `.bind`
    // below references it by name: a reordered struct field can no longer
    // silently land in the wrong column, and an added/removed field is a
    // compile error here rather than silent data corruption.
    let NewAppRecord {
        user_id,
        server_id,
        name,
        repo_full_name,
        branch,
        container_port,
        health_path,
        domain,
        runtime_kind,
        hostlet_config_path,
        runtime_config,
        packaging_strategy,
        root_directory,
        install_command,
        build_command,
        start_command,
        memory_limit_mb,
        cpu_limit,
        public_exposure,
        auto_deploy,
        env,
    } = record;

    let mut tx = state.db.begin().await.map_err(|err| {
        tracing::error!(error = %err, "create_app_record: failed to begin transaction");
        CreateAppError::Internal
    })?;
    let row = sqlx::query(
        "INSERT INTO apps (user_id,server_id,name,repo_full_name,branch,container_port,\
         health_path,domain,runtime_kind,hostlet_config_path,runtime_config,packaging_strategy,\
         root_directory,install_command,build_command,start_command,memory_limit_mb,cpu_limit,\
         public_exposure,auto_deploy) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20) \
         RETURNING id",
    )
    .bind(user_id) // $1  user_id
    .bind(server_id) // $2  server_id
    .bind(name) // $3  name
    .bind(repo_full_name) // $4  repo_full_name
    .bind(branch) // $5  branch
    .bind(container_port) // $6  container_port
    .bind(health_path) // $7  health_path
    .bind(domain) // $8  domain
    .bind(runtime_kind) // $9  runtime_kind
    .bind(hostlet_config_path) // $10 hostlet_config_path
    .bind(runtime_config) // $11 runtime_config
    .bind(packaging_strategy) // $12 packaging_strategy
    .bind(root_directory) // $13 root_directory
    .bind(install_command) // $14 install_command
    .bind(build_command) // $15 build_command
    .bind(start_command) // $16 start_command
    .bind(memory_limit_mb) // $17 memory_limit_mb
    .bind(cpu_limit) // $18 cpu_limit
    .bind(public_exposure) // $19 public_exposure
    .bind(auto_deploy) // $20 auto_deploy
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| CreateAppError::Insert)?;
    let app_id: Uuid = row.get("id");
    for ev in env {
        let ciphertext = state.crypto.encrypt(&ev.value).map_err(|err| {
            tracing::error!(error = %err, "create_app_record: failed to encrypt env var");
            CreateAppError::Internal
        })?;
        sqlx::query("INSERT INTO app_env_vars (app_id,key,value_ciphertext) VALUES ($1,$2,$3)")
            .bind(app_id)
            .bind(ev.key)
            .bind(ciphertext)
            .execute(&mut *tx)
            .await
            .map_err(|err| {
                tracing::error!(error = %err, "create_app_record: failed to insert env var");
                CreateAppError::Internal
            })?;
    }
    tx.commit().await.map_err(|err| {
        tracing::error!(error = %err, "create_app_record: failed to commit transaction");
        CreateAppError::Internal
    })?;
    Ok(app_id)
}
