use super::super::*;
use super::request_context_or_response;

pub async fn create_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut body): Json<CreateApp>,
) -> impl IntoResponse {
    let context = match request_context_or_response(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
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
    let mut runtime_kind = match clean_runtime_kind(body.runtime_kind.as_deref()) {
        Ok(value) => value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let hostlet_config_path = match clean_hostlet_config_path(body.hostlet_config_path.as_deref()) {
        Ok(value) => value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let mut runtime_config = body.runtime_config.unwrap_or_else(|| serde_json::json!({}));
    if let Err(message) = clean_runtime_config(&runtime_config) {
        return (StatusCode::BAD_REQUEST, message).into_response();
    }
    let packaging_strategy = match clean_packaging_strategy(body.packaging_strategy.as_deref()) {
        Ok(value) => value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let server_id = match crate::server_capacity::select_app_runner(&state, body.server_id).await {
        Ok(server_id) => server_id,
        Err(err) => {
            tracing::warn!(error = %err, "failed to select app runner for app create");
            return (StatusCode::BAD_REQUEST, err.to_string()).into_response();
        }
    };
    let domain = if body.domain.trim().is_empty() {
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
    let public_exposure = body.public_exposure.unwrap_or(false);
    if public_exposure {
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
    // Resolve managed add-ons (Postgres/Redis chosen at create time) into a
    // generated multi-service Compose runtime + the env to persist. Generated
    // secrets land in the app's encrypted env; the generated compose references
    // them via `${VAR}` interpolation, so nothing secret is stored in
    // runtime_config. The added env vars are validated alongside the rest below.
    match hostlet_contracts::compose::resolve_managed_addons(
        &runtime_config,
        "web",
        body.container_port as u16,
        &health_path,
        || crate::crypto::random_token(32),
    ) {
        Ok(Some(resolved)) => {
            runtime_kind = "compose".to_string();
            runtime_config = resolved.runtime_config;
            for (key, value) in resolved.env {
                body.env.push(EnvVar { key, value });
            }
        }
        Ok(None) => {}
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
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
    let auto_deploy = body.auto_deploy.unwrap_or(false);
    if auto_deploy {
        if let Err(err) = github::ensure_repo_webhook(&state, user_id, repo_full_name).await {
            tracing::warn!(error = %err, repo = %repo_full_name, "failed to ensure GitHub webhook");
            return (
                StatusCode::BAD_GATEWAY,
                format!("GitHub webhook could not be configured: {err}"),
            )
                .into_response();
        }
    }
    let record = crate::apps::NewAppRecord {
        user_id,
        server_id,
        name: app_name.to_string(),
        repo_full_name: repo_full_name.to_string(),
        branch: branch.to_string(),
        container_port: body.container_port,
        health_path,
        domain: domain.clone(),
        runtime_kind,
        hostlet_config_path,
        runtime_config,
        packaging_strategy,
        root_directory,
        install_command,
        build_command,
        start_command,
        memory_limit_mb: body.memory_limit_mb,
        cpu_limit: body.cpu_limit,
        public_exposure,
        auto_deploy,
        // Self-hosted has no per-plan app cap; skip the advisory-lock recount.
        app_limit: None,
        env: body
            .env
            .into_iter()
            .map(|ev| crate::apps::AppEnvVarInput {
                key: ev.key,
                value: ev.value,
            })
            .collect(),
    };
    let app_id = match crate::apps::create_app_record(&state, record).await {
        Ok(id) => id,
        Err(crate::apps::CreateAppError::Insert) => return StatusCode::BAD_REQUEST.into_response(),
        Err(crate::apps::CreateAppError::Internal) => {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        // Effectively unreachable: this handler passes app_limit: None, so the
        // recount never runs. Kept consistent with the cloud handler's 402 so the
        // response degrades sensibly if a future self-hosted cap ever sets it.
        Err(crate::apps::CreateAppError::LimitReached) => {
            return StatusCode::PAYMENT_REQUIRED.into_response()
        }
    };
    if public_exposure {
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
            Err(err) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    format!(
                        "App was created (id {app_id}), but its first deployment could not be \
                         started: {err}. Open the app and start a deployment to retry."
                    ),
                )
                    .into_response()
            }
        }
    } else {
        None
    };
    Json(serde_json::json!({"id": app_id, "deploymentId": deployment_id})).into_response()
}
