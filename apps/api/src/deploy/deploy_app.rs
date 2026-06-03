//! Typed view of the `apps` columns a deploy job needs.
//!
//! Centralizing the SELECT column list and the row -> payload field mapping
//! here keeps the deploy payload shape in one cohesive place instead of being
//! spread across a long sequence of `app.get::<T,_>("col")` calls inline in the
//! handler, where the column list silently duplicates the one in
//! `apps::create_app_record`'s INSERT and is easy to drift out of sync.

use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

/// The exact `apps` columns a deploy job reads, in one place so the SELECT and
/// the payload mapping cannot drift independently.
pub(super) const DEPLOY_APP_COLUMNS: &str = "id,server_id,name,repo_full_name,branch,\
     container_port,health_path,domain,runtime_kind,hostlet_config_path,runtime_config,\
     packaging_strategy,root_directory,install_command,build_command,start_command,\
     memory_limit_mb,cpu_limit";

/// A single `apps` row decoded into the fields a deploy job payload is built
/// from. Reading every column once here (rather than repeatedly off the raw
/// row) makes the column<->field mapping explicit and reviewable.
pub(super) struct DeployApp {
    pub server_id: Uuid,
    pub name: String,
    pub repo_full_name: String,
    pub branch: String,
    pub container_port: i32,
    pub health_path: String,
    pub domain: String,
    pub runtime_kind: String,
    pub hostlet_config_path: String,
    pub runtime_config: Value,
    pub packaging_strategy: String,
    pub root_directory: String,
    pub install_command: Option<String>,
    pub build_command: Option<String>,
    pub start_command: Option<String>,
    pub memory_limit_mb: Option<i32>,
    pub cpu_limit: Option<f64>,
}

impl DeployApp {
    pub(super) fn from_row(row: &sqlx::postgres::PgRow) -> Self {
        Self {
            server_id: row.get("server_id"),
            name: row.get("name"),
            repo_full_name: row.get("repo_full_name"),
            branch: row.get("branch"),
            container_port: row.get("container_port"),
            health_path: row.get("health_path"),
            domain: row.get("domain"),
            runtime_kind: row.get("runtime_kind"),
            hostlet_config_path: row.get("hostlet_config_path"),
            runtime_config: row.get("runtime_config"),
            packaging_strategy: row.get("packaging_strategy"),
            root_directory: row.get("root_directory"),
            install_command: row.get("install_command"),
            build_command: row.get("build_command"),
            start_command: row.get("start_command"),
            memory_limit_mb: row.get("memory_limit_mb"),
            cpu_limit: row.get("cpu_limit"),
        }
    }

    /// Build the agent deploy-job payload. The JSON shape is byte-for-byte the
    /// same as the previous inline `json!` block.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn deploy_payload(
        &self,
        deployment_id: Uuid,
        app_id: Uuid,
        route_key: String,
        commit_sha: &str,
        env: serde_json::Map<String, Value>,
        github_token: Option<String>,
    ) -> Value {
        json!({
            "type": "deploy", "deployment_id": deployment_id, "app_id": app_id,
            "route_key": route_key,
            "app_name": self.name, "repo": self.repo_full_name,
            "branch": self.branch, "commit_sha": commit_sha,
            "container_port": self.container_port, "health_path": self.health_path,
            "domain": self.domain, "env": env,
            "runtime_kind": self.runtime_kind,
            "hostlet_config_path": self.hostlet_config_path,
            "runtime_config": self.runtime_config,
            "packaging_strategy": self.packaging_strategy,
            "root_directory": self.root_directory,
            "install_command": self.install_command,
            "build_command": self.build_command,
            "start_command": self.start_command,
            "memory_limit_mb": self.memory_limit_mb,
            "cpu_limit": self.cpu_limit,
            "github_token": github_token
        })
    }
}
