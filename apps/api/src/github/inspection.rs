//! Shared builders for `inspect_repo` result objects.
//!
//! The Dockerfile, package.json, and fallback branches of `inspect_repo` each
//! returned a hand-written JSON object that repeated ~15 identical fields
//! (`repoFullName`, `defaultBranch`, `branch`, `appName`, `runtimeKind`,
//! `rootDirectory`, `healthPath`, `hostletConfigPath`, `runtimeConfig`,
//! `packagingStrategy`, ...). [`InspectionBase`] builds that shared shell once
//! so the per-branch code only supplies what actually differs, removing the
//! copy-paste and the drift risk between the three literals. The emitted JSON
//! is identical to the previous inline objects.

use serde_json::{json, Map, Value};

/// The fields every `inspect_repo` result shares, plus the few that vary by a
/// fixed value across the three single-container branches.
pub(super) struct InspectionBase<'a> {
    pub repo: &'a str,
    pub default_branch: &'a str,
    pub branch: &'a str,
    pub deployable: bool,
    pub container_port: Value,
    pub packaging_options: Value,
    pub recommended_packaging_strategy: &'a str,
    pub env: Value,
    pub warnings: Value,
    pub summary: String,
}

impl InspectionBase<'_> {
    /// Materialize the common object. Callers then `insert` any branch-specific
    /// extra fields (e.g. `detectedFramework`) before returning it.
    pub(super) fn build(self) -> Map<String, Value> {
        let Self {
            repo,
            default_branch,
            branch,
            deployable,
            container_port,
            packaging_options,
            recommended_packaging_strategy,
            env,
            warnings,
            summary,
        } = self;
        let Value::Object(map) = json!({
            "repoFullName": repo,
            "defaultBranch": default_branch,
            "branch": branch,
            "appName": repo.split('/').nth(1).unwrap_or("app"),
            "deployable": deployable,
            "runtimeKind": "single",
            "rootDirectory": ".",
            "containerPort": container_port,
            "healthPath": "/",
            "hostletConfigPath": "hostlet.yml",
            "runtimeConfig": {},
            "packagingStrategy": "auto",
            "packagingOptions": packaging_options,
            "recommendedPackagingStrategy": recommended_packaging_strategy,
            "env": env,
            "warnings": warnings,
            "summary": summary,
            "autoDeployAvailable": false
        }) else {
            unreachable!("json! object literal is always an object")
        };
        map
    }
}
