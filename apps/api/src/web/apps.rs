use super::*;

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
    let runtime_kind = match clean_runtime_kind(body.runtime_kind.as_deref()) {
        Ok(value) => value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
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
    if auto_deploy {
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
        .bind(body.memory_limit_mb)
        .bind(body.cpu_limit)
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
    let user_id = context.user_id;
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

pub(in crate::web) async fn finalize_delete_app_from_job(state: &AppState, job_id: Uuid) -> anyhow::Result<bool> {
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

pub(in crate::web) async fn app_belongs_to_user(state: &AppState, app_id: Uuid, user_id: Uuid) -> bool {
    matches!(
        sqlx::query("SELECT 1 FROM apps WHERE id=$1 AND user_id=$2")
            .bind(app_id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await,
        Ok(Some(_))
    )
}

pub(in crate::web) async fn app_domain_in_use(state: &AppState, domain: &str, except_app_id: Option<Uuid>) -> bool {
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

pub(in crate::web) async fn delete_created_app_row(state: &AppState, app_id: Uuid) {
    let _ = sqlx::query("DELETE FROM apps WHERE id=$1")
        .bind(app_id)
        .execute(&state.db)
        .await;
}

pub(in crate::web) async fn compensate_failed_app_update_dns(
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

pub(in crate::web) async fn mark_agent_job_failed(state: &AppState, job_id: Uuid, failure: &str) {
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

