use super::super::*;
use super::request_context_or_response;
use sqlx::{postgres::PgRow, PgPool, Postgres, QueryBuilder};

/// Validated, ready-to-persist changes for an app update.
///
/// Every field is `Option`: `None` means "leave the column untouched", which
/// preserves the original handler's behaviour of only writing fields the
/// caller actually supplied.
#[derive(Default)]
struct AppUpdate {
    domain: Option<String>,
    health_path: Option<String>,
    root_directory: Option<String>,
    runtime_kind: Option<String>,
    hostlet_config_path: Option<String>,
    runtime_config: Option<serde_json::Value>,
    packaging_strategy: Option<String>,
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

impl AppUpdate {
    /// Persist all supplied columns plus the optional env replacement in one
    /// transaction. Scalar columns are written with a single dynamically built
    /// `UPDATE apps SET ... , updated_at=now() WHERE id=$N` statement instead
    /// of one statement per field.
    async fn persist(self, state: &AppState, id: Uuid) -> anyhow::Result<()> {
        let mut tx = state.db.begin().await?;
        if let Some(mut update) = self.build_apps_update(id) {
            update.build().execute(&mut *tx).await?;
        }
        if let Some(env) = self.env {
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
        tx.commit().await?;
        Ok(())
    }

    /// Build the `UPDATE apps SET ...` statement for the scalar columns, or
    /// `None` when no scalar column was supplied (so no query is issued, as in
    /// the original per-field code).
    fn build_apps_update(&self, id: Uuid) -> Option<QueryBuilder<'_, Postgres>> {
        let mut qb = QueryBuilder::<Postgres>::new("UPDATE apps SET ");
        let mut wrote = false;
        {
            let mut sep = qb.separated(", ");
            if let Some(value) = &self.domain {
                sep.push("domain=").push_bind_unseparated(value);
                wrote = true;
            }
            if let Some(value) = &self.health_path {
                sep.push("health_path=").push_bind_unseparated(value);
                wrote = true;
            }
            if let Some(value) = &self.root_directory {
                sep.push("root_directory=").push_bind_unseparated(value);
                wrote = true;
            }
            if let Some(value) = &self.runtime_kind {
                sep.push("runtime_kind=").push_bind_unseparated(value);
                wrote = true;
            }
            if let Some(value) = &self.hostlet_config_path {
                sep.push("hostlet_config_path=")
                    .push_bind_unseparated(value);
                wrote = true;
            }
            if let Some(value) = &self.runtime_config {
                sep.push("runtime_config=").push_bind_unseparated(value);
                wrote = true;
            }
            if let Some(value) = &self.packaging_strategy {
                sep.push("packaging_strategy=").push_bind_unseparated(value);
                wrote = true;
            }
            if let Some(value) = &self.install_command {
                sep.push("install_command=").push_bind_unseparated(value);
                wrote = true;
            }
            if let Some(value) = &self.build_command {
                sep.push("build_command=").push_bind_unseparated(value);
                wrote = true;
            }
            if let Some(value) = &self.start_command {
                sep.push("start_command=").push_bind_unseparated(value);
                wrote = true;
            }
            if let Some(value) = self.container_port {
                sep.push("container_port=").push_bind_unseparated(value);
                wrote = true;
            }
            if let Some(value) = &self.memory_limit_mb {
                sep.push("memory_limit_mb=").push_bind_unseparated(value);
                wrote = true;
            }
            if let Some(value) = &self.cpu_limit {
                sep.push("cpu_limit=").push_bind_unseparated(value);
                wrote = true;
            }
            if let Some(value) = self.public_exposure {
                sep.push("public_exposure=").push_bind_unseparated(value);
                wrote = true;
            }
            if let Some(value) = self.auto_deploy {
                sep.push("auto_deploy=").push_bind_unseparated(value);
                wrote = true;
            }
            if !wrote {
                return None;
            }
            sep.push("updated_at=now()");
        }
        qb.push(" WHERE id=").push_bind(id);
        Some(qb)
    }
}

/// Validate a `Option<Option<String>>` command field: an absent outer option
/// leaves the column untouched, an inner `Some` is cleaned, and an inner
/// `None` clears the command.
fn clean_command_field(
    field: Option<Option<String>>,
) -> Result<Option<Option<String>>, &'static str> {
    match field {
        Some(Some(value)) => Ok(Some(clean_command(Some(value))?)),
        Some(None) => Ok(Some(None)),
        None => Ok(None),
    }
}

async fn load_app_for_update(
    db: &PgPool,
    id: Uuid,
    user_id: Uuid,
) -> Result<Option<PgRow>, sqlx::Error> {
    sqlx::query(
        "SELECT id, domain, public_exposure, repo_full_name, auto_deploy FROM apps WHERE id=$1 AND user_id=$2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await
}

pub async fn update_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateApp>,
) -> impl IntoResponse {
    let context = match request_context_or_response(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    let row = match load_app_for_update(&state.db, id, user_id).await {
        Ok(Some(row)) => row,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::warn!(
                error = %err,
                app_id = %id,
                user_id = %user_id,
                "failed to load app for update"
            );
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let old_domain = row.get::<String, _>("domain");
    let old_public_exposure = row.get::<bool, _>("public_exposure");
    let repo_full_name = row.get::<String, _>("repo_full_name");
    let old_auto_deploy = row.get::<bool, _>("auto_deploy");
    let domain_changed = body.domain.is_some();
    let mut update = AppUpdate::default();
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
        app_domain = domain.clone();
        update.domain = Some(domain);
    }
    let desired_public_exposure = body.public_exposure.unwrap_or(old_public_exposure);
    if desired_public_exposure {
        if let Err(err) = hostlet_public_cloudflare_host(&state, &app_domain) {
            return (StatusCode::BAD_REQUEST, err.to_string()).into_response();
        }
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
        update.health_path = Some(path);
    }
    if let Some(root_directory) = body.root_directory {
        let root_directory = clean_optional(Some(root_directory)).unwrap_or_else(|| ".".into());
        if !valid_root_directory(&root_directory) {
            return (
                StatusCode::BAD_REQUEST,
                "root directory cannot be absolute or contain ..",
            )
                .into_response();
        }
        update.root_directory = Some(root_directory);
    }
    if let Some(value) = body.runtime_kind.as_deref() {
        match clean_runtime_kind(Some(value)) {
            Ok(value) => update.runtime_kind = Some(value),
            Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
        }
    }
    if let Some(value) = body.hostlet_config_path.as_deref() {
        match clean_hostlet_config_path(Some(value)) {
            Ok(value) => update.hostlet_config_path = Some(value),
            Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
        }
    }
    if let Some(value) = body.runtime_config {
        if let Err(message) = clean_runtime_config(&value) {
            return (StatusCode::BAD_REQUEST, message).into_response();
        }
        update.runtime_config = Some(value);
    }
    if let Some(value) = body.packaging_strategy.as_deref() {
        match clean_packaging_strategy(Some(value)) {
            Ok(value) => update.packaging_strategy = Some(value),
            Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
        }
    }
    match clean_command_field(body.install_command) {
        Ok(value) => update.install_command = value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    }
    match clean_command_field(body.build_command) {
        Ok(value) => update.build_command = value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    }
    match clean_command_field(body.start_command) {
        Ok(value) => update.start_command = value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    }
    if let Some(container_port) = body.container_port {
        if !(1..=65_535).contains(&container_port) {
            return (StatusCode::BAD_REQUEST, "container port must be 1-65535").into_response();
        }
        update.container_port = Some(container_port);
    }
    if let Some(memory_limit_mb) = body.memory_limit_mb {
        if !valid_memory_limit(memory_limit_mb) {
            return (
                StatusCode::BAD_REQUEST,
                "memory limit must be between 64 and 262144 MB",
            )
                .into_response();
        }
        update.memory_limit_mb = Some(memory_limit_mb);
    }
    if let Some(cpu_limit) = body.cpu_limit {
        if !valid_cpu_limit(cpu_limit) {
            return (
                StatusCode::BAD_REQUEST,
                "CPU limit must be between 0.1 and 128",
            )
                .into_response();
        }
        update.cpu_limit = Some(cpu_limit);
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
    if body.public_exposure.is_some() {
        update.public_exposure = Some(desired_public_exposure);
    }
    update.auto_deploy = body.auto_deploy;
    update.env = body.env;
    if let Err(err) = update.persist(&state, id).await {
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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    #[tokio::test]
    async fn load_app_for_update_reports_database_errors() {
        let pool = PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(10))
            .connect_lazy("postgres://127.0.0.1:1/hostlet")
            .unwrap();

        let result = load_app_for_update(&pool, Uuid::nil(), Uuid::nil()).await;

        assert!(result.is_err());
    }
}
