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

/// Resolves an app's effective storage limit in bytes from its runtime config,
/// falling back to the self-hosted default.
pub(crate) fn volume_storage_limit_bytes(runtime_config: &serde_json::Value) -> i64 {
    runtime_config
        .pointer("/compose/volumeStorageLimitMb")
        .and_then(|value| value.as_i64())
        .filter(|mb| *mb > 0)
        .unwrap_or_else(default_volume_storage_limit_mb)
        .saturating_mul(1024 * 1024)
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
}
