use crate::{
    auth::{current_user_id, request_context, RequestContext},
    crypto::verify_token,
    deploy, github,
    state::AppState,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::HashSet;
use uuid::Uuid;

mod app_delete;
mod app_env;
mod apps;
mod audit;
mod cleanup;
mod dns;
mod health;
mod jobs;
mod servers;
mod system;
mod validation;

pub use app_delete::{delete_app, reconcile_completed_delete_jobs};
pub use app_env::{app_env_vars, delete_app_env_var, set_app_env_var};
pub use apps::{addons_catalog, create_app, get_app, list_apps, update_app};
pub use audit::audit_events;
pub use cleanup::{cleanup_preview, run_cleanup};
pub use dns::cloudflare::cloudflare_status;
pub use health::{
    app_health, app_health_events, app_resources, check_app_health_now, health_summary,
};
pub use jobs::{
    agent_job_status, cancel_agent_job, list_agent_jobs, restart_app_container, retry_agent_job,
};
pub use servers::{create_server, list_servers, server_install_command};
pub use system::{
    backup_metadata, operator_cleanup_preview, operator_run_cleanup, operator_status,
    refresh_update_check_if_stale, system_update_check, system_version,
};

pub(in crate::web) use app_delete::{
    app_belongs_to_user, app_domain_in_use, compensate_failed_app_update_dns,
    delete_created_app_row, finalize_delete_app_from_job,
};
pub(in crate::web) use cleanup::{cleanup_plan, run_cleanup_inner};
pub(in crate::web) use dns::cloudflare::{delete_cloudflare_app_dns, ensure_cloudflare_app_dns};
pub(in crate::web) use health::system_health_counts;
pub(in crate::web) use jobs::enqueue_interactive_agent_job;
pub(in crate::web) use system::domain_host;
pub(in crate::web) use validation::*;

/// Payload for creating a new app. Optional fields fall back to runtime/server
/// defaults when omitted.
#[derive(Deserialize)]
pub struct CreateApp {
    name: String,
    repo_full_name: String,
    branch: String,
    server_id: Option<Uuid>,
    /// Port the container listens on inside its network namespace.
    container_port: i32,
    health_path: String,
    domain: String,
    runtime_kind: Option<String>,
    hostlet_config_path: Option<String>,
    runtime_config: Option<serde_json::Value>,
    packaging_strategy: Option<String>,
    root_directory: Option<String>,
    install_command: Option<String>,
    build_command: Option<String>,
    start_command: Option<String>,
    /// Hard memory limit in mebibytes; `None` leaves the container unconstrained.
    memory_limit_mb: Option<i32>,
    /// Fractional CPU core allowance (e.g. `1.5` = one and a half cores).
    cpu_limit: Option<f64>,
    public_exposure: Option<bool>,
    auto_deploy: Option<bool>,
    deploy_after_create: Option<bool>,
    env: Vec<EnvVar>,
}

#[derive(Deserialize)]
pub struct EnvVar {
    key: String,
    value: String,
}

/// Partial update for an existing app. The outer `Option` distinguishes "field
/// not present in the request" (`None`, leave unchanged) from "field present".
/// For the command/limit fields the inner `Option` then distinguishes an
/// explicit clear-to-null (`Some(None)`) from a new value (`Some(Some(v))`).
#[derive(Deserialize)]
pub struct UpdateApp {
    domain: Option<String>,
    runtime_kind: Option<String>,
    hostlet_config_path: Option<String>,
    runtime_config: Option<serde_json::Value>,
    packaging_strategy: Option<String>,
    health_path: Option<String>,
    root_directory: Option<String>,
    install_command: Option<Option<String>>,
    build_command: Option<Option<String>>,
    start_command: Option<Option<String>>,
    container_port: Option<i32>,
    /// Hard memory limit in mebibytes; `Some(None)` clears the limit.
    memory_limit_mb: Option<Option<i32>>,
    /// Fractional CPU core allowance; `Some(None)` clears the limit.
    cpu_limit: Option<Option<f64>>,
    public_exposure: Option<bool>,
    auto_deploy: Option<bool>,
    env: Option<Vec<EnvVar>>,
}

#[derive(Deserialize)]
pub struct EnvValue {
    value: String,
}

async fn customer_context(
    headers: &HeaderMap,
    state: &AppState,
) -> Result<RequestContext, Response> {
    request_context(headers, state)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED.into_response())
}

struct AuditEventInput<'a> {
    actor_type: &'a str,
    actor_id: Option<String>,
    event_type: &'a str,
    app_id: Option<Uuid>,
    deployment_id: Option<Uuid>,
    job_id: Option<Uuid>,
    metadata: serde_json::Value,
}

async fn record_audit_event(state: &AppState, event: AuditEventInput<'_>) {
    let _ = sqlx::query(
        "INSERT INTO audit_events
           (actor_type,actor_id,event_type,app_id,deployment_id,job_id,metadata_json)
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(event.actor_type)
    .bind(event.actor_id)
    .bind(event.event_type)
    .bind(event.app_id)
    .bind(event.deployment_id)
    .bind(event.job_id)
    .bind(event.metadata)
    .execute(&state.db)
    .await;
}
