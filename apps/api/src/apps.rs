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
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|_| CreateAppError::Internal)?;
    let row = sqlx::query(
        "INSERT INTO apps (user_id,server_id,name,repo_full_name,branch,container_port,\
         health_path,domain,runtime_kind,hostlet_config_path,runtime_config,packaging_strategy,\
         root_directory,install_command,build_command,start_command,memory_limit_mb,cpu_limit,\
         public_exposure,auto_deploy) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20) \
         RETURNING id",
    )
    .bind(record.user_id)
    .bind(record.server_id)
    .bind(record.name)
    .bind(record.repo_full_name)
    .bind(record.branch)
    .bind(record.container_port)
    .bind(record.health_path)
    .bind(record.domain)
    .bind(record.runtime_kind)
    .bind(record.hostlet_config_path)
    .bind(record.runtime_config)
    .bind(record.packaging_strategy)
    .bind(record.root_directory)
    .bind(record.install_command)
    .bind(record.build_command)
    .bind(record.start_command)
    .bind(record.memory_limit_mb)
    .bind(record.cpu_limit)
    .bind(record.public_exposure)
    .bind(record.auto_deploy)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| CreateAppError::Insert)?;
    let app_id: Uuid = row.get("id");
    for ev in record.env {
        let ciphertext = state
            .crypto
            .encrypt(&ev.value)
            .map_err(|_| CreateAppError::Internal)?;
        sqlx::query("INSERT INTO app_env_vars (app_id,key,value_ciphertext) VALUES ($1,$2,$3)")
            .bind(app_id)
            .bind(ev.key)
            .bind(ciphertext)
            .execute(&mut *tx)
            .await
            .map_err(|_| CreateAppError::Internal)?;
    }
    tx.commit().await.map_err(|_| CreateAppError::Internal)?;
    Ok(app_id)
}
