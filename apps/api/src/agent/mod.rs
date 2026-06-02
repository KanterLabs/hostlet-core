use crate::{
    crypto::{sign, verify_token},
    state::{AgentConnection, AppState},
};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use futures_util::{SinkExt, StreamExt};
use hostlet_contracts::{AgentJobStatus, DeploymentStatus, RuntimeHealthStatus};
use serde::Deserialize;
use sqlx::Row;
use tokio::sync::mpsc;
use uuid::Uuid;

mod auth;
mod messages;
mod routes;
mod socket;
mod validation;

pub use routes::{
    claim_job, complete_job, event, health_targets, recover_stale_agent_jobs, register, ws,
};

pub(in crate::agent) use auth::authenticated_server_id;
pub(in crate::agent) use messages::handle_agent_message;
pub(in crate::agent) use socket::handle_socket;
pub(in crate::agent) use validation::{
    connection_is_current, header_uuid, truncate_log_line, valid_agent_job_status,
    valid_container_name, valid_deployment_status, valid_health_status,
};

#[cfg(test)]
mod tests;
