use super::*;

mod serialization;

pub(in crate::web) use serialization::{app_json, health_json};

// Re-export the contract-level validators directly so callers in `crate::web`
// can use them by name without a hand-written passthrough wrapper per function.
pub(in crate::web) use hostlet_contracts::{
    valid_branch, valid_domain, valid_health_path, valid_hostname, valid_repo_full_name,
};

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

pub(in crate::web) fn valid_app_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' '))
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

pub(in crate::web) fn hostlet_public_cloudflare_host(
    state: &AppState,
    domain: &str,
) -> anyhow::Result<String> {
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

pub(in crate::web) fn clean_packaging_strategy(
    value: Option<&str>,
) -> Result<String, &'static str> {
    let value = value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("auto");
    match value {
        "auto" | "dockerfile" | "generated" => Ok(value.to_string()),
        _ => Err("packaging strategy must be auto, dockerfile, or generated"),
    }
}

pub(in crate::web) fn clean_hostlet_config_path(
    value: Option<&str>,
) -> Result<String, &'static str> {
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
