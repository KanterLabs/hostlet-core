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
