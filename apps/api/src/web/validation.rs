use super::*;

mod serialization;

pub(in crate::web) use serialization::{app_json, health_json};

// Re-export the contract-level validators directly so callers in `crate::web`
// can use them by name without a hand-written passthrough wrapper per function.
pub(in crate::web) use hostlet_contracts::{
    app_slug, clean_command, clean_hostlet_config_path, clean_optional, clean_packaging_strategy,
    clean_runtime_config, clean_runtime_kind, domain_host, valid_app_name, valid_branch,
    valid_cpu_limit, valid_domain, valid_env_key, valid_health_path, valid_hostname,
    valid_memory_limit, valid_repo_full_name, valid_root_directory,
};

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

pub(in crate::web) fn validate_env_vars(env: &[EnvVar]) -> Result<(), &'static str> {
    hostlet_contracts::validate_env_pairs(env.iter().map(|ev| (ev.key.as_str(), ev.value.as_str())))
}
