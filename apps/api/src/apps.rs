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

#[cfg(test)]
mod tests {
    use super::*;

    /// Shared server id seeded by `db_test_state_from_env` via `seed_local_server`.
    const TEST_SERVER_ID: Uuid = Uuid::from_u128(1);

    async fn reset_db(state: &AppState) {
        // Truncate derived tables before apps; users is cleared with DELETE so
        // the NULL-FK servers row (seeded by seed_local_server) is untouched.
        sqlx::query("TRUNCATE app_env_vars, apps CASCADE")
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users")
            .execute(&state.db)
            .await
            .unwrap();
    }

    async fn insert_user(state: &AppState, github_id: i64, login: &str) -> Uuid {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO users (github_id, login) VALUES ($1,$2) RETURNING id",
        )
        .bind(github_id)
        .bind(login)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    fn minimal_record(user_id: Uuid, name: &str, domain: &str) -> NewAppRecord {
        NewAppRecord {
            user_id,
            server_id: TEST_SERVER_ID,
            name: name.into(),
            repo_full_name: "hostlet-ci/node-hello".into(),
            branch: "main".into(),
            container_port: 3000,
            health_path: "/health".into(),
            domain: domain.into(),
            runtime_kind: "single".into(),
            hostlet_config_path: ".hostlet.yml".into(),
            runtime_config: serde_json::json!({}),
            packaging_strategy: "generated".into(),
            root_directory: ".".into(),
            install_command: None,
            build_command: None,
            start_command: None,
            memory_limit_mb: None,
            cpu_limit: None,
            public_exposure: true,
            auto_deploy: false,
            env: vec![],
        }
    }

    /// Success path: `create_app_record` persists the row and returns a valid UUID.
    #[tokio::test]
    async fn db_create_app_record_persists_row_and_returns_id() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_db(&state).await;
        let user_id = insert_user(&state, 88801, "apps-create-user").await;

        let app_id = create_app_record(
            &state,
            minimal_record(user_id, "test-app", "test-app.example.test"),
        )
        .await
        .expect("create_app_record should succeed");

        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM apps WHERE id=$1 AND user_id=$2)")
                .bind(app_id)
                .bind(user_id)
                .fetch_one(&state.db)
                .await
                .unwrap();
        assert!(exists, "app row should be persisted in the database");
    }

    /// Env-var path: env vars are encrypted at rest, not stored as plaintext.
    #[tokio::test]
    async fn db_create_app_record_env_var_is_stored_encrypted() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_db(&state).await;
        let user_id = insert_user(&state, 88802, "apps-env-user").await;
        let mut record = minimal_record(user_id, "env-app", "env-app.example.test");
        record.env = vec![AppEnvVarInput {
            key: "MY_SECRET".into(),
            value: "top-secret-value".into(),
        }];

        let app_id = create_app_record(&state, record)
            .await
            .expect("create_app_record with env var should succeed");

        let ciphertext: String = sqlx::query_scalar(
            "SELECT value_ciphertext FROM app_env_vars WHERE app_id=$1 AND key='MY_SECRET'",
        )
        .bind(app_id)
        .fetch_one(&state.db)
        .await
        .expect("env var row should be persisted");
        // Must not be stored as plaintext.
        assert_ne!(
            ciphertext, "top-secret-value",
            "value must be encrypted at rest"
        );
        // Decrypting must recover the original value.
        let decrypted = state
            .crypto
            .decrypt(&ciphertext)
            .expect("ciphertext should decrypt");
        assert_eq!(decrypted, "top-secret-value");
    }

    /// Error path: a non-existent user_id causes an FK violation → `CreateAppError::Insert`.
    #[tokio::test]
    async fn db_create_app_record_nonexistent_user_returns_insert_error() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_db(&state).await;
        // Deliberately use a UUID that has no matching row in `users`.
        let bogus_user_id = Uuid::new_v4();

        let result = create_app_record(
            &state,
            minimal_record(bogus_user_id, "fk-app", "fk.example.test"),
        )
        .await;

        assert!(
            matches!(result, Err(CreateAppError::Insert)),
            "FK violation should map to CreateAppError::Insert"
        );
    }
}
