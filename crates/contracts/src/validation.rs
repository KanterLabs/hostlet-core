//! Pure input validators that have no external dependencies beyond `std`.
//!
//! Extracted from `hostlet_api::web::validation` and `hostlet_api::web::system`
//! so that the Hostlet Cloud overlay can validate inputs without forking the web
//! layer. All items are `pub` so overlay callers can reach them.

use crate::{valid_env_key, valid_env_value, valid_root_directory};

/// Validates a free-text app name: non-empty, at most 80 chars, alphanumerics
/// plus `-`, `_`, and space only.
pub fn valid_app_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' '))
}

/// Validates the hard memory limit in mebibytes: `None` (unconstrained) is
/// valid; a `Some` value must be in the range 64 – 262 144 MiB.
pub fn valid_memory_limit(value: Option<i32>) -> bool {
    value.map(|v| (64..=262_144).contains(&v)).unwrap_or(true)
}

/// Validates the fractional CPU-core limit: `None` is valid; a `Some` value
/// must be finite and in the range 0.1 – 128.0.
pub fn valid_cpu_limit(value: Option<f64>) -> bool {
    value
        .map(|v| v.is_finite() && (0.1..=128.0).contains(&v))
        .unwrap_or(true)
}

/// Validates a JSON runtime-config blob: must be a JSON object and at most
/// 32 000 bytes when serialised.
pub fn clean_runtime_config(value: &serde_json::Value) -> Result<(), &'static str> {
    if !value.is_object() {
        return Err("runtime config must be an object");
    }
    if value.to_string().len() > 32_000 {
        return Err("runtime config is too large");
    }
    Ok(())
}

/// Normalises and validates the path to the Hostlet config file inside the
/// repo. `None` or an empty string falls back to `"hostlet.yml"`. The path
/// must pass `valid_root_directory` and end with `.yml` or `.yaml`.
pub fn clean_hostlet_config_path(value: Option<&str>) -> Result<String, &'static str> {
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

/// Extracts the host portion of a `host` or `host:port` value.
///
/// When a trailing `:port` is present and the port is a valid `u16`, the host
/// is returned; otherwise the full value is treated as the host. Returns
/// `None` only when `value` is empty.
pub fn domain_host(value: &str) -> Option<&str> {
    if let Some((host, port)) = value.rsplit_once(':') {
        if port.parse::<u16>().is_ok() {
            return Some(host);
        }
    }
    Some(value)
}

/// Validates a slice of `(key, value)` env-var pairs: each key must pass
/// [`valid_env_key`], each value must pass [`valid_env_value`], and keys must
/// be unique. The error strings match those previously returned by the
/// web-layer `validate_env_vars` helper so callers can compare them verbatim.
pub fn validate_env_pairs<'a>(
    pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<(), &'static str> {
    let mut keys = std::collections::HashSet::new();
    for (key, value) in pairs {
        if !valid_env_key(key) {
            return Err("invalid env var key");
        }
        if !valid_env_value(value) {
            return Err("env var value is too large");
        }
        if !keys.insert(key) {
            return Err("env var keys must be unique");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_app_name_accepts_alphanumeric_dash_underscore_space() {
        assert!(valid_app_name("My App-1_2"));
        assert!(!valid_app_name(""));
        assert!(!valid_app_name(&"a".repeat(81)));
        assert!(!valid_app_name("a/b"));
    }

    #[test]
    fn valid_memory_limit_range() {
        assert!(valid_memory_limit(None));
        assert!(valid_memory_limit(Some(64)));
        assert!(!valid_memory_limit(Some(63)));
        assert!(valid_memory_limit(Some(262_144)));
        assert!(!valid_memory_limit(Some(262_145)));
    }

    #[test]
    fn valid_cpu_limit_range() {
        assert!(valid_cpu_limit(None));
        assert!(valid_cpu_limit(Some(0.1)));
        assert!(!valid_cpu_limit(Some(0.05)));
        assert!(valid_cpu_limit(Some(128.0)));
        assert!(!valid_cpu_limit(Some(128.1)));
        assert!(!valid_cpu_limit(Some(f64::NAN)));
    }

    #[test]
    fn clean_runtime_config_validates_object_and_size() {
        assert!(clean_runtime_config(&serde_json::json!({})).is_ok());
        assert!(clean_runtime_config(&serde_json::json!([])).is_err());
        // Build a JSON object whose serialisation is > 32 000 bytes.
        let big_value = "x".repeat(33_000);
        let big = serde_json::json!({"k": big_value});
        assert!(clean_runtime_config(&big).is_err());
    }

    #[test]
    fn clean_hostlet_config_path_defaults_and_validates() {
        assert_eq!(clean_hostlet_config_path(None), Ok("hostlet.yml".into()));
        assert_eq!(
            clean_hostlet_config_path(Some("config.yaml")),
            Ok("config.yaml".into())
        );
        assert!(clean_hostlet_config_path(Some("../x.yml")).is_err());
        assert!(clean_hostlet_config_path(Some("x.txt")).is_err());
    }

    #[test]
    fn domain_host_strips_valid_port() {
        assert_eq!(domain_host("h.com:8080"), Some("h.com"));
        assert_eq!(domain_host("h.com"), Some("h.com"));
        // Non-numeric port → treat whole thing as the host.
        assert_eq!(domain_host("h.com:notaport"), Some("h.com:notaport"));
    }

    #[test]
    fn validate_env_pairs_errors() {
        assert!(validate_env_pairs(std::iter::empty()).is_ok());
        assert!(validate_env_pairs([("VALID_KEY", "value")]).is_ok());
        assert_eq!(
            validate_env_pairs([("a-b", "value")]),
            Err("invalid env var key")
        );
        assert_eq!(
            validate_env_pairs([("KEY", "v"), ("KEY", "w")]),
            Err("env var keys must be unique")
        );
    }
}
