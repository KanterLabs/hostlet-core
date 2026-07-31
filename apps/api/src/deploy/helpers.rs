#[cfg(test)]
fn is_active_deployment_status(status: &str) -> bool {
    ACTIVE_DEPLOYMENT_STATUSES.contains(&status)
}

fn route_key(app_id: Uuid) -> String {
    format!("app-{app_id}")
}

fn rollback_supported_for_runtime(runtime_kind: &str) -> bool {
    matches!(runtime_kind, "single" | "compose")
}

/// Which storage quota scope was exceeded, determining the user-facing message.
#[derive(Debug, PartialEq)]
enum StorageScope {
    /// Account-wide cap: total footprint across all apps owned by the user.
    Account,
    /// Per-app cap: image + volumes for this one app.
    PerApp,
}

/// Pure: returns the over-quota error message when `used_bytes >= limit_bytes`,
/// `None` otherwise. No I/O; extracts the decision from deployment creation so
/// it can be unit-tested independently of the database.
fn storage_over_quota_error(
    used_bytes: i64,
    limit_bytes: i64,
    scope: StorageScope,
) -> Option<String> {
    if used_bytes < limit_bytes {
        return None;
    }
    let limit_mb = limit_bytes / (1024 * 1024);
    let used_mb = used_bytes / (1024 * 1024);
    let msg = match scope {
        StorageScope::Account => format!(
            "Your projects are over the {limit_mb} MB account storage limit \
             ({used_mb} MB used by their images + volumes). \
             Remove a project, shrink an image, or upgrade your plan before deploying."
        ),
        StorageScope::PerApp => format!(
            "This app is over its {limit_mb} MB storage limit \
             ({used_mb} MB used by its image + volumes). \
             Free space, shrink the image, or raise the limit before deploying."
        ),
    };
    Some(msg)
}
