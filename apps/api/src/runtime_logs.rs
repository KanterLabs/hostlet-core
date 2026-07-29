//! Ephemeral, owner-requested runtime diagnostics.
//!
//! Runtime output is requested from the connected agent, bounded on both sides,
//! returned to the owner, and discarded. It is never inserted into deployment
//! logs or any other durable store.

use crate::{auth::request_context, state::AppState};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use hostlet_contracts::{
    valid_container_name, RuntimeLogLine, RuntimeLogRequest, RuntimeLogResponse,
    RuntimeLogServiceError, RuntimeLogTarget, RUNTIME_LOG_MAX_BYTES, RUNTIME_LOG_MAX_LINES,
    RUNTIME_LOG_MAX_LINE_BYTES, RUNTIME_LOG_MAX_TARGETS,
};
use serde::Serialize;
use sqlx::Row;
use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, MutexGuard, OnceLock},
    time::Duration,
};
use tokio::sync::oneshot;
use uuid::Uuid;

const AGENT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_PENDING_REQUESTS: usize = 128;
const OWNER_REQUESTS_PER_MINUTE: u32 = 12;

struct PendingRequest {
    server_id: Uuid,
    deployment_id: Uuid,
    sender: oneshot::Sender<RuntimeLogResponse>,
}

static PENDING_REQUESTS: OnceLock<Mutex<HashMap<Uuid, PendingRequest>>> = OnceLock::new();

fn pending_requests() -> &'static Mutex<HashMap<Uuid, PendingRequest>> {
    PENDING_REQUESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_pending_requests() -> MutexGuard<'static, HashMap<Uuid, PendingRequest>> {
    pending_requests()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Removes an abandoned request even when the HTTP future is canceled because
/// the client disconnected before the response timeout elapsed.
struct PendingRequestGuard(Uuid);

impl Drop for PendingRequestGuard {
    fn drop(&mut self) {
        lock_pending_requests().remove(&self.0);
    }
}

#[derive(Debug, Eq, PartialEq)]
enum RegisterPendingError {
    ServerBusy,
    Capacity,
}

fn register_pending_request(
    request_id: Uuid,
    server_id: Uuid,
    deployment_id: Uuid,
    sender: oneshot::Sender<RuntimeLogResponse>,
) -> Result<PendingRequestGuard, RegisterPendingError> {
    let mut pending = lock_pending_requests();
    if pending
        .values()
        .any(|request| request.server_id == server_id)
    {
        return Err(RegisterPendingError::ServerBusy);
    }
    if pending.len() >= MAX_PENDING_REQUESTS {
        return Err(RegisterPendingError::Capacity);
    }
    pending.insert(
        request_id,
        PendingRequest {
            server_id,
            deployment_id,
            sender,
        },
    );
    Ok(PendingRequestGuard(request_id))
}

#[derive(Debug)]
struct RuntimeTargetRow {
    service: String,
    container: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeLogSnapshot {
    deployment_id: Uuid,
    captured_at: String,
    truncated: bool,
    lines: Vec<RuntimeLogLine>,
    unavailable_services: Vec<RuntimeLogServiceError>,
}

/// Fetches a fresh tail from every current deployment service for the
/// authenticated app owner.
pub async fn get_app_runtime_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(app_id): Path<Uuid>,
) -> Response {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(err) if err.to_string() == "sign in required" => {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(err) => return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response(),
    };
    if !state.rate_limiter.check(
        format!("runtime-logs-owner:{}", context.user_id),
        OWNER_REQUESTS_PER_MINUTE,
        Duration::from_secs(60),
    ) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "Runtime logs were refreshed too often. Wait before trying again.",
        )
            .into_response();
    }

    let row = match sqlx::query(
        "SELECT a.server_id,d.id AS current_deployment_id,d.container_name
         FROM apps a
         LEFT JOIN deployments d
           ON d.id=a.current_deployment_id
          AND d.app_id=a.id
          AND d.server_id=a.server_id
         WHERE a.id=$1 AND a.user_id=$2",
    )
    .bind(app_id)
    .bind(context.user_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::warn!(error = %err, %app_id, "failed to load app for runtime logs");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let server_id = row.get::<Uuid, _>("server_id");
    let Some(deployment_id) = row.get::<Option<Uuid>, _>("current_deployment_id") else {
        return (
            StatusCode::CONFLICT,
            "Runtime logs are unavailable until the app has a current deployment.",
        )
            .into_response();
    };
    let deployment_container = row.get::<Option<String>, _>("container_name");

    let service_rows = match sqlx::query(
        "SELECT service_name,container_name
         FROM (
           SELECT DISTINCT ON (container_name)
                  service_name,container_name,role
           FROM deployment_services
           WHERE deployment_id=$1 AND container_name IS NOT NULL
           ORDER BY container_name,(role <> 'web'),service_name
         ) AS distinct_services
         ORDER BY (role <> 'web'),service_name
         LIMIT $2",
    )
    .bind(deployment_id)
    .bind((RUNTIME_LOG_MAX_TARGETS + 1) as i64)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(error = %err, %app_id, %deployment_id, "failed to load runtime log targets");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let rows = service_rows
        .into_iter()
        .filter_map(|row| {
            let container = row.get::<String, _>("container_name");
            valid_container_name(&container).then(|| RuntimeTargetRow {
                service: row.get::<String, _>("service_name"),
                container,
            })
        })
        .collect::<Vec<_>>();
    let (targets, targets_truncated) = runtime_log_targets(deployment_container, rows);
    if targets.is_empty() {
        return (
            StatusCode::CONFLICT,
            "The current deployment has no running containers to inspect.",
        )
            .into_response();
    }

    let agent_sender = {
        let agents = state.agents.read().await;
        agents
            .get(&server_id)
            .filter(|connection| !connection.sender.is_closed())
            .map(|connection| connection.sender.clone())
    };
    let Some(agent_sender) = agent_sender else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "The app's agent is offline. Runtime logs are temporarily unavailable.",
        )
            .into_response();
    };

    let request = RuntimeLogRequest {
        request_id: Uuid::new_v4(),
        deployment_id,
        targets,
    };
    let (sender, receiver) = oneshot::channel();
    let _pending_guard = match register_pending_request(
        request.request_id,
        server_id,
        deployment_id,
        sender,
    ) {
        Ok(guard) => guard,
        Err(RegisterPendingError::ServerBusy) => {
            return (
                    StatusCode::TOO_MANY_REQUESTS,
                    "Runtime logs are already being collected for an app on this server. Try again shortly.",
                )
                    .into_response();
        }
        Err(RegisterPendingError::Capacity) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Runtime diagnostics are busy. Try refreshing again.",
            )
                .into_response();
        }
    };
    let mut message = match serde_json::to_value(&request) {
        Ok(value) => value,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    message
        .as_object_mut()
        .expect("runtime log request serializes as an object")
        .insert("type".into(), serde_json::json!("runtime_logs_request"));
    if agent_sender.try_send(message).is_err() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "The app's agent is busy. Try refreshing runtime logs again.",
        )
            .into_response();
    }

    let response = match tokio::time::timeout(AGENT_RESPONSE_TIMEOUT, receiver).await {
        Ok(Ok(response)) => response,
        _ => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "The app's agent did not return runtime logs in time.",
            )
                .into_response();
        }
    };
    let mut snapshot = bounded_snapshot(response, targets_truncated);
    bound_snapshot_json(&mut snapshot);
    let mut response = Json(snapshot).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, private"),
    );
    response
}

/// Completes a pending owner request only when the authenticated agent and
/// deployment match the request. Unsolicited or late responses are ignored.
pub(crate) fn handle_agent_runtime_logs(server_id: Uuid, message: serde_json::Value) {
    let Ok(response) = serde_json::from_value::<RuntimeLogResponse>(message) else {
        return;
    };
    let sender = {
        let mut pending = lock_pending_requests();
        let matches = pending.get(&response.request_id).is_some_and(|request| {
            request.server_id == server_id && request.deployment_id == response.deployment_id
        });
        matches
            .then(|| pending.remove(&response.request_id))
            .flatten()
            .map(|request| request.sender)
    };
    if let Some(sender) = sender {
        let _ = sender.send(response);
    }
}

fn runtime_log_targets(
    deployment_container: Option<String>,
    rows: Vec<RuntimeTargetRow>,
) -> (Vec<RuntimeLogTarget>, bool) {
    let mut seen = HashSet::new();
    let mut targets = rows
        .into_iter()
        .filter(|row| seen.insert(row.container.clone()))
        .map(|row| RuntimeLogTarget {
            service: clean_short_text(&row.service, 64),
            container: row.container,
        })
        .collect::<Vec<_>>();
    if targets.is_empty() {
        if let Some(container) = deployment_container.filter(|name| valid_container_name(name)) {
            targets.push(RuntimeLogTarget {
                service: "web".into(),
                container,
            });
        }
    }
    let truncated = targets.len() > RUNTIME_LOG_MAX_TARGETS;
    targets.truncate(RUNTIME_LOG_MAX_TARGETS);
    (targets, truncated)
}

fn bounded_snapshot(response: RuntimeLogResponse, targets_truncated: bool) -> RuntimeLogSnapshot {
    let (selected_lines, lines_truncated) = fair_latest_lines(response.lines);
    let lines = selected_lines
        .into_iter()
        .map(|line| RuntimeLogLine {
            timestamp: line.timestamp.map(|value| clean_short_text(&value, 64)),
            service: clean_short_text(&line.service, 64),
            stream: if line.stream == "stderr" {
                "stderr".into()
            } else {
                "stdout".into()
            },
            line: clean_log_line(&line.line),
        })
        .collect::<Vec<_>>();
    let truncated = response.truncated || targets_truncated || lines_truncated;
    let unavailable_services = response
        .unavailable_services
        .into_iter()
        .take(RUNTIME_LOG_MAX_TARGETS)
        .map(|error| RuntimeLogServiceError {
            service: clean_short_text(&error.service, 64),
            message: clean_short_text(&error.message, 160),
        })
        .collect();
    RuntimeLogSnapshot {
        deployment_id: response.deployment_id,
        captured_at: chrono::Utc::now().to_rfc3339(),
        truncated,
        lines,
        unavailable_services,
    }
}

/// The external cap applies to the serialized response, not just raw log text.
fn bound_snapshot_json(snapshot: &mut RuntimeLogSnapshot) {
    if serde_json::to_vec(&*snapshot).is_ok_and(|body| body.len() <= RUNTIME_LOG_MAX_BYTES) {
        return;
    }
    let lines = std::mem::take(&mut snapshot.lines);
    let base_bytes = serde_json::to_vec(&*snapshot)
        .map(|body| body.len())
        .unwrap_or(RUNTIME_LOG_MAX_BYTES);
    let line_budget = RUNTIME_LOG_MAX_BYTES.saturating_sub(base_bytes);
    let original_line_count = lines.len();
    let mut service_indexes = HashMap::<String, usize>::new();
    let mut services = Vec::<Vec<(usize, RuntimeLogLine)>>::new();
    for (index, line) in lines.into_iter().enumerate() {
        let service_index = match service_indexes.get(&line.service) {
            Some(index) => *index,
            None => {
                let index = services.len();
                service_indexes.insert(line.service.clone(), index);
                services.push(Vec::new());
                index
            }
        };
        services[service_index].push((index, line));
    }

    let mut selected = Vec::<(usize, RuntimeLogLine)>::new();
    let mut used_bytes = 0usize;
    let mut remaining_services = services.iter().filter(|lines| !lines.is_empty()).count();
    // Reserve an equal share for each service's newest line before any noisy
    // service can consume a second line.
    for lines in &mut services {
        let Some((index, line)) = lines.pop() else {
            continue;
        };
        let comma = usize::from(!selected.is_empty());
        let share = line_budget
            .saturating_sub(used_bytes)
            .checked_div(remaining_services.max(1))
            .unwrap_or_default()
            .saturating_sub(comma);
        remaining_services = remaining_services.saturating_sub(1);
        if let Some((line, line_truncated)) = fit_line_to_json_budget(line, share) {
            used_bytes += serialized_line_bytes(&line) + comma;
            snapshot.truncated |= line_truncated;
            selected.push((index, line));
        } else {
            snapshot.truncated = true;
        }
    }

    loop {
        let mut examined = false;
        for lines in &mut services {
            let Some((index, line)) = lines.pop() else {
                continue;
            };
            examined = true;
            let comma = usize::from(!selected.is_empty());
            let bytes = serialized_line_bytes(&line) + comma;
            if used_bytes.saturating_add(bytes) <= line_budget {
                used_bytes += bytes;
                selected.push((index, line));
            } else {
                snapshot.truncated = true;
            }
        }
        if !examined {
            break;
        }
    }
    snapshot.truncated |= selected.len() < original_line_count;
    selected.sort_by_key(|(index, _)| *index);
    snapshot.lines = selected.into_iter().map(|(_, line)| line).collect();
}

fn serialized_line_bytes(line: &RuntimeLogLine) -> usize {
    serde_json::to_vec(line)
        .map(|line| line.len())
        .unwrap_or(RUNTIME_LOG_MAX_BYTES)
}

fn fit_line_to_json_budget(
    mut line: RuntimeLogLine,
    max_bytes: usize,
) -> Option<(RuntimeLogLine, bool)> {
    let mut truncated = false;
    loop {
        let serialized = serialized_line_bytes(&line);
        if serialized <= max_bytes {
            return Some((line, truncated));
        }
        if line.line.is_empty() {
            return None;
        }
        let excess = serialized.saturating_sub(max_bytes);
        let target_len = line
            .line
            .len()
            .saturating_sub(excess.max(line.line.len() / 4).max(1));
        line.line = utf8_prefix(&line.line, target_len);
        truncated = true;
    }
}

fn utf8_prefix(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn fair_latest_lines(lines: Vec<RuntimeLogLine>) -> (Vec<RuntimeLogLine>, bool) {
    let original_line_count = lines.len();
    if original_line_count <= RUNTIME_LOG_MAX_LINES {
        return (lines, false);
    }

    let mut service_indexes = HashMap::<String, usize>::new();
    let mut services = Vec::<Vec<(usize, RuntimeLogLine)>>::new();
    for (index, line) in lines.into_iter().enumerate() {
        let service_index = match service_indexes.get(&line.service) {
            Some(index) => *index,
            None => {
                let index = services.len();
                service_indexes.insert(line.service.clone(), index);
                services.push(Vec::new());
                index
            }
        };
        services[service_index].push((index, line));
    }

    let mut selected = Vec::with_capacity(RUNTIME_LOG_MAX_LINES);
    while selected.len() < RUNTIME_LOG_MAX_LINES {
        let mut examined = false;
        for service in &mut services {
            let Some(line) = service.pop() else {
                continue;
            };
            examined = true;
            selected.push(line);
            if selected.len() == RUNTIME_LOG_MAX_LINES {
                break;
            }
        }
        if !examined {
            break;
        }
    }
    selected.sort_by_key(|(index, _)| *index);
    (selected.into_iter().map(|(_, line)| line).collect(), true)
}

fn clean_short_text(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(max_chars)
        .collect()
}

fn clean_log_line(value: &str) -> String {
    const TRUNCATION_MARKER: &str = "...[truncated]";

    let value = value
        .chars()
        .filter(|character| !character.is_control() || *character == '\t')
        .collect::<String>();
    if value.len() <= RUNTIME_LOG_MAX_LINE_BYTES {
        return value;
    }
    let mut end = RUNTIME_LOG_MAX_LINE_BYTES.saturating_sub(TRUNCATION_MARKER.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &value[..end], TRUNCATION_MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_prefer_per_service_containers_and_deduplicate() {
        let (targets, truncated) = runtime_log_targets(
            Some("hostlet-fallback".into()),
            vec![
                RuntimeTargetRow {
                    service: "web".into(),
                    container: "hostlet-web".into(),
                },
                RuntimeTargetRow {
                    service: "web-copy".into(),
                    container: "hostlet-web".into(),
                },
                RuntimeTargetRow {
                    service: "postgres".into(),
                    container: "hostlet-postgres".into(),
                },
            ],
        );

        assert!(!truncated);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].service, "web");
        assert_eq!(targets[1].service, "postgres");
    }

    #[test]
    fn single_service_falls_back_to_deployment_container() {
        let (targets, truncated) = runtime_log_targets(Some("hostlet-app-123".into()), Vec::new());

        assert!(!truncated);
        assert_eq!(
            targets,
            vec![RuntimeLogTarget {
                service: "web".into(),
                container: "hostlet-app-123".into(),
            }]
        );
    }

    #[test]
    fn arbitrary_non_hostlet_container_is_never_targeted() {
        let (targets, _) = runtime_log_targets(Some("postgres-production".into()), Vec::new());
        assert!(targets.is_empty());
    }

    #[test]
    fn clean_log_line_stays_inside_the_per_line_byte_limit() {
        let line = clean_log_line(&"é".repeat(RUNTIME_LOG_MAX_LINE_BYTES));

        assert!(line.ends_with("...[truncated]"));
        assert!(line.len() <= RUNTIME_LOG_MAX_LINE_BYTES);
    }

    #[test]
    fn response_is_bounded_as_serialized_json_and_keeps_latest_lines() {
        let response = RuntimeLogResponse {
            request_id: Uuid::new_v4(),
            deployment_id: Uuid::new_v4(),
            truncated: false,
            lines: (0..RUNTIME_LOG_MAX_LINES)
                .map(|index| RuntimeLogLine {
                    timestamp: Some(format!("2026-07-28T12:34:{index:02}Z")),
                    service: "web".into(),
                    stream: "stdout".into(),
                    line: format!("{index}:{}", "x".repeat(600)),
                })
                .collect(),
            unavailable_services: Vec::new(),
        };
        let mut snapshot = bounded_snapshot(response, false);
        bound_snapshot_json(&mut snapshot);
        let body = serde_json::to_vec(&snapshot).unwrap();

        assert!(body.len() <= RUNTIME_LOG_MAX_BYTES);
        assert!(snapshot.truncated);
        assert!(snapshot
            .lines
            .last()
            .is_some_and(|line| line.line.starts_with("499:")));
    }

    #[test]
    fn serialized_bound_retains_a_quiet_service() {
        let mut lines = vec![RuntimeLogLine {
            timestamp: Some("2026-07-28T11:00:00Z".into()),
            service: "quiet".into(),
            stream: "stdout".into(),
            line: "quiet-latest".into(),
        }];
        lines.extend(
            (0..(RUNTIME_LOG_MAX_LINES - 1)).map(|index| RuntimeLogLine {
                timestamp: Some(format!(
                    "2026-07-28T12:{:02}:{:02}Z",
                    index / 60,
                    index % 60
                )),
                service: "noisy".into(),
                stream: "stdout".into(),
                line: format!("{index}:{}", "\\\"".repeat(600)),
            }),
        );
        let mut snapshot = RuntimeLogSnapshot {
            deployment_id: Uuid::new_v4(),
            captured_at: "2026-07-28T13:00:00Z".into(),
            truncated: false,
            lines,
            unavailable_services: Vec::new(),
        };

        bound_snapshot_json(&mut snapshot);

        assert!(serde_json::to_vec(&snapshot).unwrap().len() <= RUNTIME_LOG_MAX_BYTES);
        assert!(snapshot.truncated);
        assert!(snapshot.lines.iter().any(|line| line.service == "quiet"));
        assert!(snapshot.lines.iter().any(|line| line.service == "noisy"));
    }

    #[test]
    fn oversized_agent_response_keeps_newest_lines() {
        let response = RuntimeLogResponse {
            request_id: Uuid::new_v4(),
            deployment_id: Uuid::new_v4(),
            truncated: false,
            lines: (0..(RUNTIME_LOG_MAX_LINES + 5))
                .map(|index| RuntimeLogLine {
                    timestamp: Some(format!(
                        "2026-07-28T12:{:02}:{:02}Z",
                        index / 60,
                        index % 60
                    )),
                    service: "web".into(),
                    stream: "stdout".into(),
                    line: format!("line-{index}"),
                })
                .collect(),
            unavailable_services: Vec::new(),
        };

        let snapshot = bounded_snapshot(response, false);

        assert!(snapshot.truncated);
        assert_eq!(snapshot.lines.len(), RUNTIME_LOG_MAX_LINES);
        assert_eq!(snapshot.lines.first().unwrap().line, "line-5");
        assert_eq!(
            snapshot.lines.last().unwrap().line,
            format!("line-{}", RUNTIME_LOG_MAX_LINES + 4)
        );
    }

    #[test]
    fn oversized_agent_response_keeps_a_quiet_service() {
        let mut lines = vec![RuntimeLogLine {
            timestamp: Some("2026-07-28T11:00:00Z".into()),
            service: "quiet".into(),
            stream: "stdout".into(),
            line: "quiet-latest".into(),
        }];
        lines.extend((0..RUNTIME_LOG_MAX_LINES).map(|index| RuntimeLogLine {
            timestamp: Some(format!(
                "2026-07-28T12:{:02}:{:02}Z",
                index / 60,
                index % 60
            )),
            service: "noisy".into(),
            stream: "stdout".into(),
            line: format!("noisy-{index}"),
        }));
        let response = RuntimeLogResponse {
            request_id: Uuid::new_v4(),
            deployment_id: Uuid::new_v4(),
            truncated: false,
            lines,
            unavailable_services: Vec::new(),
        };

        let snapshot = bounded_snapshot(response, false);

        assert!(snapshot.truncated);
        assert_eq!(snapshot.lines.len(), RUNTIME_LOG_MAX_LINES);
        assert!(snapshot.lines.iter().any(|line| line.service == "quiet"));
        assert_eq!(
            snapshot
                .lines
                .iter()
                .filter(|line| line.service == "noisy")
                .count(),
            RUNTIME_LOG_MAX_LINES - 1
        );
    }

    #[test]
    fn only_one_pending_runtime_log_request_is_allowed_per_server() {
        let server_id = Uuid::new_v4();
        let (first_sender, _first_receiver) = oneshot::channel();
        let first_guard =
            register_pending_request(Uuid::new_v4(), server_id, Uuid::new_v4(), first_sender)
                .unwrap();
        let (second_sender, _second_receiver) = oneshot::channel();

        let second =
            register_pending_request(Uuid::new_v4(), server_id, Uuid::new_v4(), second_sender);

        assert!(matches!(second, Err(RegisterPendingError::ServerBusy)));

        let (other_sender, _other_receiver) = oneshot::channel();
        let other_server =
            register_pending_request(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), other_sender);
        assert!(other_server.is_ok());

        drop(first_guard);

        let (retry_sender, _retry_receiver) = oneshot::channel();
        let retry =
            register_pending_request(Uuid::new_v4(), server_id, Uuid::new_v4(), retry_sender);
        assert!(retry.is_ok());
    }

    #[tokio::test]
    async fn response_must_match_the_pending_authenticated_agent() {
        let request_id = Uuid::new_v4();
        let deployment_id = Uuid::new_v4();
        let server_id = Uuid::new_v4();
        let (sender, mut receiver) = oneshot::channel();
        lock_pending_requests().insert(
            request_id,
            PendingRequest {
                server_id,
                deployment_id,
                sender,
            },
        );
        let message = serde_json::json!({
            "type": "runtime_logs_response",
            "requestId": request_id,
            "deploymentId": deployment_id,
            "truncated": false,
            "lines": [],
            "unavailableServices": []
        });

        handle_agent_runtime_logs(Uuid::new_v4(), message.clone());
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));

        handle_agent_runtime_logs(server_id, message);
        assert_eq!(receiver.await.unwrap().request_id, request_id);
    }
}
