struct SelectedBuildPool {
    id: Uuid,
    provider: String,
}

async fn selected_build_pool(
    state: &AppState,
    app_pool_id: Option<Uuid>,
) -> anyhow::Result<SelectedBuildPool> {
    let row = if let Some(id) = app_pool_id {
        sqlx::query("SELECT id,provider,enabled,qualification_status FROM build_pools WHERE id=$1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
    } else {
        sqlx::query(
            "SELECT id,provider,enabled,qualification_status FROM build_pools
             WHERE is_default ORDER BY created_at ASC LIMIT 1",
        )
        .fetch_optional(&state.db)
        .await?
    };
    let row = row.ok_or_else(|| anyhow::anyhow!("no default build pool is configured"))?;
    anyhow::ensure!(
        row.get::<bool, _>("enabled"),
        "selected build pool is disabled"
    );
    let provider = row.get::<String, _>("provider");
    if provider == "cloudflare" {
        anyhow::ensure!(
            row.get::<String, _>("qualification_status") == "passed",
            "Cloudflare build pool must pass qualification before production use"
        );
    }
    Ok(SelectedBuildPool {
        id: row.get("id"),
        provider,
    })
}

fn registry_host(url: &str) -> anyhow::Result<String> {
    let parsed = reqwest::Url::parse(url).context("artifact registry URL is invalid")?;
    anyhow::ensure!(
        parsed.scheme() == "https"
            || (parsed.scheme() == "http"
                && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1"))),
        "artifact registry must use HTTPS"
    );
    anyhow::ensure!(
        parsed.path().is_empty() || parsed.path() == "/",
        "artifact registry URL cannot contain a path"
    );
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("artifact registry URL is missing a host"))?;
    Ok(match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

fn required_registry_env(key: &str) -> anyhow::Result<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("{key} is required for remote build pools"))
}

fn build_registry_url(provider: &str) -> anyhow::Result<String> {
    if provider == "local" {
        if let Some(url) = std::env::var("HOSTLET_ARTIFACT_REGISTRY_LOCAL_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            return Ok(url);
        }
    }
    required_registry_env("HOSTLET_ARTIFACT_REGISTRY_URL")
}

async fn enqueue_build(
    state: &AppState,
    build_pool_id: Uuid,
    provider: &str,
    runner_server_id: Uuid,
    deployment_id: Uuid,
    app_id: Uuid,
    mut payload: serde_json::Value,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        matches!(provider, "local" | "vm"),
        "selected provider does not implement the universal build contract"
    );
    let registry_url = build_registry_url(provider)?;
    if provider == "vm" {
        anyhow::ensure!(
            registry_url.starts_with("https://"),
            "remote build pools require a builder-reachable HTTPS artifact registry URL"
        );
    }
    let registry_host = registry_host(&registry_url)?;
    let push_username = required_registry_env("HOSTLET_ARTIFACT_REGISTRY_PUSH_USERNAME")?;
    let push_password = required_registry_env("HOSTLET_ARTIFACT_REGISTRY_PUSH_PASSWORD")?;
    let _ = required_registry_env("HOSTLET_ARTIFACT_REGISTRY_PULL_USERNAME")?;
    let _ = required_registry_env("HOSTLET_ARTIFACT_REGISTRY_PULL_PASSWORD")?;
    let platform: String =
        sqlx::query_scalar("SELECT COALESCE(platforms[1],'linux/amd64') FROM servers WHERE id=$1")
            .bind(runner_server_id)
            .fetch_one(&state.db)
            .await?;
    let supported: bool = sqlx::query_scalar(
        "SELECT supported_platforms @> ARRAY[$2]::TEXT[] FROM build_pools WHERE id=$1",
    )
    .bind(build_pool_id)
    .bind(&platform)
    .fetch_one(&state.db)
    .await?;
    anyhow::ensure!(
        supported,
        "selected build pool does not support runner platform {platform}"
    );
    let waiting_reason =
        build_waiting_reason(state, build_pool_id, provider, runner_server_id, &platform).await?;
    let repository = format!("hostlet/apps/{app_id}/artifacts");
    let tag = format!("deployment-{deployment_id}");
    if let Some(object) = payload.as_object_mut() {
        object.insert("type".into(), json!("build"));
        object.insert("runner_server_id".into(), json!(runner_server_id));
        object.insert("required_platform".into(), json!(platform));
        object.insert(
            "artifact_registry".into(),
            json!({
                "url": registry_url,
                "host": registry_host,
                "repository": repository,
                "tag": tag,
                "pushUsername": push_username,
                "pushPassword": push_password,
            }),
        );
    }
    let mut build_spec = payload.clone();
    if let Some(object) = build_spec.as_object_mut() {
        object.remove("env");
        object.remove("github_token");
        object.remove("artifact_registry");
    }
    let mut tx = state.db.begin().await?;
    // Pause is a durable admission fence.  Recheck it while holding the app
    // row lock immediately before creating the build job so a pause racing the
    // deployment request cannot leave build work queued for a suspended app.
    let app_suspended: bool = sqlx::query_scalar(
        "SELECT suspended_at IS NOT NULL FROM apps WHERE id=$1 FOR UPDATE",
    )
    .bind(app_id)
    .fetch_one(&mut *tx)
    .await?;
    anyhow::ensure!(!app_suspended, "app is paused; build is not permitted");
    locks::deployment(&mut tx, deployment_id, app_id).await?;
    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO deployment_builds
           (deployment_id,build_pool_id,status,required_platform,build_spec_json,waiting_reason)
         VALUES ($1,$2,'queued',$3,$4,$5) RETURNING id",
    )
    .bind(deployment_id)
    .bind(build_pool_id)
    .bind(&platform)
    .bind(build_spec)
    .bind(waiting_reason)
    .fetch_one(&mut *tx)
    .await?;
    if let Some(object) = payload.as_object_mut() {
        object.insert("build_id".into(), json!(build_id));
    }
    sqlx::query("UPDATE deployments SET build_id=$1,status='queued_for_build' WHERE id=$2")
        .bind(build_id)
        .bind(deployment_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO agent_jobs
           (server_id,build_pool_id,app_id,deployment_id,job_type,status,payload_json,
            priority,protocol_version)
         VALUES ($1,$2,$3,$4,'build','queued',$5,10,6)",
    )
    .bind((provider == "local").then_some(runner_server_id))
    .bind(build_pool_id)
    .bind(app_id)
    .bind(deployment_id)
    .bind(payload)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn build_waiting_reason(
    state: &AppState,
    build_pool_id: Uuid,
    provider: &str,
    runner_server_id: Uuid,
    platform: &str,
) -> anyhow::Result<Option<String>> {
    if provider == "local" {
        let row = sqlx::query("SELECT status,agent_protocol_version FROM servers WHERE id=$1")
            .bind(runner_server_id)
            .fetch_one(&state.db)
            .await?;
        if row.get::<i32, _>("agent_protocol_version")
            < hostlet_contracts::DEPLOYMENT_PROTOCOL_VERSION
        {
            return Ok(Some("control_plane_agent_requires_upgrade".into()));
        }
        if row.get::<String, _>("status") != "online" {
            return Ok(Some("control_plane_builder_offline".into()));
        }
        return Ok(None);
    }
    let rows = sqlx::query(
        "SELECT status,draining,platforms,agent_protocol_version
         FROM servers
         WHERE build_pool_id=$1 AND capabilities @> ARRAY['builder']::TEXT[]
           AND status<>'revoked'",
    )
    .bind(build_pool_id)
    .fetch_all(&state.db)
    .await?;
    if rows.is_empty() {
        return Ok(Some("no_enrolled_builder".into()));
    }
    let compatible = rows
        .iter()
        .filter(|row| {
            row.get::<Vec<String>, _>("platforms")
                .iter()
                .any(|item| item == platform)
        })
        .collect::<Vec<_>>();
    if compatible.is_empty() {
        return Ok(Some("no_compatible_builder_platform".into()));
    }
    if compatible.iter().all(|row| {
        row.get::<i32, _>("agent_protocol_version") < hostlet_contracts::DEPLOYMENT_PROTOCOL_VERSION
    }) {
        return Ok(Some("builder_agents_require_upgrade".into()));
    }
    if compatible.iter().all(|row| row.get::<bool, _>("draining")) {
        return Ok(Some("compatible_builders_draining".into()));
    }
    if compatible
        .iter()
        .all(|row| row.get::<String, _>("status") != "online")
    {
        return Ok(Some("no_online_compatible_builder".into()));
    }
    Ok(None)
}
