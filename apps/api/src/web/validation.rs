use super::*;
pub(in crate::web) fn clean_runtime_config(value: &serde_json::Value) -> Result<(), &'static str> {
    if !value.is_object() {
        return Err("runtime config must be an object");
    }
    if value.to_string().len() > 32_000 {
        return Err("runtime config is too large");
    }
    Ok(())
}

pub(in crate::web) fn app_slug(value: &str) -> String {
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

pub(in crate::web) fn app_json(r: sqlx::postgres::PgRow) -> serde_json::Value {
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

pub(in crate::web) fn valid_app_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' '))
}

pub(in crate::web) fn valid_repo_full_name(value: &str) -> bool {
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

pub(in crate::web) fn valid_branch(value: &str) -> bool {
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

pub(in crate::web) fn valid_domain(value: &str) -> bool {
    let Some((host, port)) = value.rsplit_once(':') else {
        return valid_hostname(value);
    };
    valid_hostname(host) && !port.is_empty() && port.parse::<u16>().is_ok()
}

pub(in crate::web) fn valid_hostname(value: &str) -> bool {
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

pub(in crate::web) fn valid_health_path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 256
        && !value.chars().any(|c| c.is_control() || c == '\\')
}

pub(in crate::web) fn valid_root_directory(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 256
        && !value.starts_with('/')
        && !value.starts_with('\\')
        && !value.split('/').any(|part| part == "..")
        && !value.chars().any(|c| c.is_control() || c == '\\')
}

pub(in crate::web) fn valid_memory_limit(value: Option<i32>) -> bool {
    value.map(|v| (64..=262_144).contains(&v)).unwrap_or(true)
}

pub(in crate::web) fn valid_cpu_limit(value: Option<f64>) -> bool {
    value
        .map(|v| v.is_finite() && (0.1..=128.0).contains(&v))
        .unwrap_or(true)
}

pub(in crate::web) fn default_domain_pattern(state: &AppState) -> Option<String> {
    state
        .base_domain
        .as_ref()
        .map(|base_domain| format!("{{app}}.{base_domain}"))
}

pub(in crate::web) fn hostlet_public_cloudflare_host(state: &AppState, domain: &str) -> anyhow::Result<String> {
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

pub(in crate::web) fn hostlet_legacy_prefixed_host(state: &AppState, host: &str) -> bool {
    let Some(base_domain) = state.base_domain.as_ref() else {
        return false;
    };
    host.strip_suffix(&format!(".{base_domain}"))
        .is_some_and(|label| label.starts_with(&state.domain_prefix) && !label.contains('.'))
}

pub(in crate::web) fn reserved_public_domain_label(label: &str) -> bool {
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

pub(in crate::web) fn health_json(row: sqlx::postgres::PgRow) -> serde_json::Value {
    serde_json::json!({
        "appId": row.get::<Uuid, _>("id"),
        "deploymentId": row.get::<Option<Uuid>, _>("deployment_id"),
        "containerName": row.get::<Option<String>, _>("container_name"),
        "status": row.get::<String, _>("status"),
        "checkedUrl": row.get::<Option<String>, _>("checked_url"),
        "httpStatus": row.get::<Option<i32>, _>("http_status"),
        "latencyMs": row.get::<Option<i32>, _>("latency_ms"),
        "failureCount": row.get::<i32, _>("failure_count"),
        "successCount": row.get::<i32, _>("success_count"),
        "lastError": row.get::<Option<String>, _>("last_error"),
        "lastCheckedAt": row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("last_checked_at"),
        "lastHealthyAt": row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("last_healthy_at"),
        "updatedAt": row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("updated_at"),
    })
}

pub(in crate::web) fn valid_env_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

pub(in crate::web) fn validate_env_vars(env: &[EnvVar]) -> Result<(), &'static str> {
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

pub(in crate::web) fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub(in crate::web) fn clean_command(value: Option<String>) -> Result<Option<String>, &'static str> {
    let Some(value) = clean_optional(value) else {
        return Ok(None);
    };
    if value.len() > 500 || value.chars().any(|c| matches!(c, '\n' | '\r' | '\0')) {
        return Err("commands cannot contain newlines, NUL bytes, or more than 500 characters");
    }
    Ok(Some(value))
}

pub(in crate::web) fn clean_runtime_kind(value: Option<&str>) -> Result<String, &'static str> {
    let value = value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("single");
    match value {
        "single" | "compose" => Ok(value.to_string()),
        _ => Err("runtime kind must be single or compose"),
    }
}

pub(in crate::web) fn clean_packaging_strategy(value: Option<&str>) -> Result<String, &'static str> {
    let value = value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("auto");
    match value {
        "auto" | "dockerfile" | "generated" => Ok(value.to_string()),
        _ => Err("packaging strategy must be auto, dockerfile, or generated"),
    }
}

pub(in crate::web) fn clean_hostlet_config_path(value: Option<&str>) -> Result<String, &'static str> {
    let value = value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("hostlet.yml");
    if valid_root_directory(value) && (value.ends_with(".yml") || value.ends_with(".yaml")) {
        Ok(value.to_string())
    } else {
        Err("Hostlet config path must be a relative .yml or .yaml file")
    }
}
