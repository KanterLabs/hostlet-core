//! Per-app managed-volume storage limit resolution, shared by the app JSON
//! serializer (the quota meter) and the deploy gate (over-quota rejection).
//!
//! Usage itself is reported by the agent (`storage_stats` events) into
//! `app_storage_usage`; this module only resolves the *limit* an app is held to.

/// The self-hosted default per-app volume storage limit (MB), env-overridable
/// via `HOSTLET_DEFAULT_VOLUME_STORAGE_MB`. Hostlet Cloud overrides this per plan
/// by injecting `runtime_config.compose.volumeStorageLimitMb` at create time.
fn default_volume_storage_limit_mb() -> i64 {
    std::env::var("HOSTLET_DEFAULT_VOLUME_STORAGE_MB")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|mb| *mb > 0)
        .unwrap_or(hostlet_contracts::DEFAULT_VOLUME_STORAGE_LIMIT_MB)
}

/// Resolves an app's effective per-app storage limit in bytes from its runtime
/// config, falling back to the self-hosted default. Used by the deploy gate only
/// when no account-wide limit applies (see [`account_storage_limit_bytes`]).
pub(crate) fn volume_storage_limit_bytes(runtime_config: &serde_json::Value) -> i64 {
    runtime_config
        .pointer("/compose/volumeStorageLimitMb")
        .and_then(|value| value.as_i64())
        .filter(|mb| *mb > 0)
        .unwrap_or_else(default_volume_storage_limit_mb)
        .saturating_mul(1024 * 1024)
}

/// Resolves an app's account-wide (per-owner) storage limit in bytes, or `None`
/// when the app declares none. Hostlet Cloud injects
/// `runtime_config.compose.accountStorageLimitMb` per plan; when present it
/// supersedes the per-app limit, and the deploy gate holds the owner's *total*
/// footprint (image + volume across all their apps) to it instead. Self-hosted
/// apps set no such field and keep the per-app limit.
pub(crate) fn account_storage_limit_bytes(runtime_config: &serde_json::Value) -> Option<i64> {
    runtime_config
        .pointer("/compose/accountStorageLimitMb")
        .and_then(|value| value.as_i64())
        .filter(|mb| *mb > 0)
        .map(|mb| mb.saturating_mul(1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_prefers_runtime_config_over_default() {
        let rc = serde_json::json!({ "compose": { "volumeStorageLimitMb": 512 } });
        assert_eq!(volume_storage_limit_bytes(&rc), 512 * 1024 * 1024);
    }

    #[test]
    fn limit_falls_back_to_contract_default_when_absent_or_invalid() {
        let default = hostlet_contracts::DEFAULT_VOLUME_STORAGE_LIMIT_MB * 1024 * 1024;
        assert_eq!(volume_storage_limit_bytes(&serde_json::json!({})), default);
        assert_eq!(
            volume_storage_limit_bytes(
                &serde_json::json!({ "compose": { "volumeStorageLimitMb": 0 } })
            ),
            default
        );
    }

    #[test]
    fn account_limit_is_some_only_when_a_positive_cap_is_set() {
        let rc = serde_json::json!({ "compose": { "accountStorageLimitMb": 4096 } });
        assert_eq!(account_storage_limit_bytes(&rc), Some(4096 * 1024 * 1024));
        // Absent or non-positive => no account-wide limit (self-hosted path).
        assert_eq!(account_storage_limit_bytes(&serde_json::json!({})), None);
        assert_eq!(
            account_storage_limit_bytes(
                &serde_json::json!({ "compose": { "accountStorageLimitMb": 0 } })
            ),
            None
        );
    }
}
