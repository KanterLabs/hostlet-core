/// Environment-variable helpers that belong to the API binary but must NOT live
/// in any file hostlet-cloud overrides.
///
/// **Why this file exists:** cloud's overlay replaces whole files. Any helper
/// defined in `state.rs`, `lib.rs`, `main.rs`, etc. becomes an unmanaged fork
/// the moment cloud needs to customise those files. Placing shared, stable
/// helpers here is *intended* to keep them outside the override boundary.
///
/// **Current limitation:** hostlet-cloud currently re-inlines its own copies of
/// several helpers from this file rather than inheriting them through the
/// submodule. Changes made here are therefore NOT automatically reflected in
/// cloud until the corresponding cloud-side duplicates are removed and the
/// imports are redirected to core. The real fix is cloud-side; this file itself
/// is correct and should remain the canonical source.
///
/// Placement rule (mandatory): shared helpers must live here (or in another
/// file not listed in the cloud override set). See `AGENTS.md` for the full
/// override inventory.
use crate::crypto::nonempty_env;
use anyhow::Context;
use std::{path::PathBuf, time::Duration};

/// Build the shared reqwest HTTP client used by all outbound API calls.
///
/// Centralised here so connection/timeout/User-Agent policy is set once and
/// inherited by both core and cloud without duplicating the call.
pub(crate) fn http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .user_agent("Hostlet")
        .build()
        .context("failed to build HTTP client")
}

/// Read `key` from the environment and enforce a minimum length in production.
///
/// Returns an error when the variable is missing or empty. In secure mode
/// (`allow_insecure_dev_defaults == false`) also rejects values shorter than
/// 32 characters, which rules out obvious placeholder secrets.
pub(crate) fn secret_from_env(
    key: &str,
    allow_insecure_dev_defaults: bool,
) -> anyhow::Result<String> {
    let Some(value) = nonempty_env(key) else {
        anyhow::bail!("{key} is required");
    };
    if !allow_insecure_dev_defaults && value.len() < 32 {
        anyhow::bail!("{key} must be at least 32 characters");
    }
    Ok(value)
}

/// Return `true` when `key` is set to a truthy value (`1`, `true`, `yes` —
/// case-insensitive after trimming). Missing or empty → `false`.
pub(crate) fn bool_env(key: &str) -> bool {
    std::env::var(key)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false)
}

/// Normalise an HTTP/HTTPS URL string to its bare `scheme://host[:port]` origin.
///
/// Returns `None` for non-HTTP schemes or unparseable input. The normalised
/// form is used for CORS allow-list comparison and request-origin validation.
pub fn normalize_origin(value: &str) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?;
    let mut origin = format!("{}://{}", url.scheme(), host);
    if let Some(port) = url.port() {
        origin.push_str(&format!(":{port}"));
    }
    Some(origin)
}

/// Resolve the screenshot storage directory from `HOSTLET_SCREENSHOT_DIR`.
///
/// Defaults to `/var/lib/hostlet/screenshots` when the variable is absent.
pub(crate) fn screenshot_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("HOSTLET_SCREENSHOT_DIR")
            .unwrap_or_else(|_| "/var/lib/hostlet/screenshots".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_origin_without_path() {
        assert_eq!(
            normalize_origin("http://10.0.0.194:3000/settings").as_deref(),
            Some("http://10.0.0.194:3000")
        );
    }

    #[test]
    fn rejects_non_http_origins() {
        assert!(normalize_origin("file:///tmp/index.html").is_none());
    }

    #[test]
    fn bool_env_truthy_values() {
        std::env::set_var("__HOSTLET_TEST_BOOL_TRUE", "true");
        assert!(bool_env("__HOSTLET_TEST_BOOL_TRUE"));
        std::env::set_var("__HOSTLET_TEST_BOOL_TRUE", "TRUE");
        assert!(bool_env("__HOSTLET_TEST_BOOL_TRUE"));
        std::env::set_var("__HOSTLET_TEST_BOOL_TRUE", "1");
        assert!(bool_env("__HOSTLET_TEST_BOOL_TRUE"));
        std::env::set_var("__HOSTLET_TEST_BOOL_TRUE", "yes");
        assert!(bool_env("__HOSTLET_TEST_BOOL_TRUE"));
        std::env::remove_var("__HOSTLET_TEST_BOOL_TRUE");
    }

    #[test]
    fn bool_env_missing_is_false() {
        std::env::remove_var("__HOSTLET_TEST_BOOL_ABSENT");
        assert!(!bool_env("__HOSTLET_TEST_BOOL_ABSENT"));
    }
}
