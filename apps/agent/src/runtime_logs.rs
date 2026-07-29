use futures_util::future::join_all;
use hostlet_contracts::{
    valid_container_name, RuntimeLogLine, RuntimeLogRequest, RuntimeLogResponse,
    RuntimeLogServiceError, RuntimeLogTarget, RUNTIME_LOG_MAX_BYTES, RUNTIME_LOG_MAX_LINES,
    RUNTIME_LOG_MAX_LINE_BYTES, RUNTIME_LOG_MAX_TARGETS,
};
use serde_json::{json, Value};
use std::{collections::VecDeque, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

const DOCKER_LOG_TIMEOUT: Duration = Duration::from_secs(2);
const RUNTIME_LOG_COLLECTION_TIMEOUT: Duration = Duration::from_secs(3);
const RUNTIME_LOG_LINE_BUDGET: usize = RUNTIME_LOG_MAX_BYTES - (8 * 1024);

struct ServiceLogs {
    lines: Vec<RuntimeLogLine>,
    truncated: bool,
    error: Option<RuntimeLogServiceError>,
}

/// Handles the one request type that is deliberately not a durable agent job.
/// Runtime logs are a read-only snapshot and are returned over the same
/// authenticated socket without being written to the agent workdir.
pub(crate) fn runtime_logs_request(text: &str) -> Option<RuntimeLogRequest> {
    let value = serde_json::from_str::<Value>(text).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("runtime_logs_request") {
        return None;
    }
    serde_json::from_value::<RuntimeLogRequest>(value).ok()
}

pub(crate) async fn runtime_logs_response(request: RuntimeLogRequest) -> Value {
    let timeout_response = unavailable_response(
        &request,
        "Runtime log collection timed out.",
        request.targets.len() > RUNTIME_LOG_MAX_TARGETS,
    );
    let response = tokio::time::timeout(
        RUNTIME_LOG_COLLECTION_TIMEOUT,
        collect_runtime_logs(request),
    )
    .await
    .unwrap_or(timeout_response);
    response_envelope(response)
}

pub(crate) fn runtime_logs_busy_response(request: &RuntimeLogRequest) -> Value {
    response_envelope(unavailable_response(
        request,
        "Another runtime log collection is already in progress.",
        true,
    ))
}

fn unavailable_response(
    request: &RuntimeLogRequest,
    message: &str,
    truncated: bool,
) -> RuntimeLogResponse {
    RuntimeLogResponse {
        request_id: request.request_id,
        deployment_id: request.deployment_id,
        truncated,
        lines: Vec::new(),
        unavailable_services: request
            .targets
            .iter()
            .take(RUNTIME_LOG_MAX_TARGETS)
            .map(|target| RuntimeLogServiceError {
                service: target.service.chars().take(64).collect(),
                message: message.into(),
            })
            .collect(),
    }
}

fn response_envelope(response: RuntimeLogResponse) -> Value {
    let mut value =
        serde_json::to_value(response).expect("runtime log response is always serializable");
    value
        .as_object_mut()
        .expect("runtime log response serializes as an object")
        .insert("type".into(), json!("runtime_logs_response"));
    value
}

async fn collect_runtime_logs(request: RuntimeLogRequest) -> RuntimeLogResponse {
    let mut truncated = request.targets.len() > RUNTIME_LOG_MAX_TARGETS;
    let targets = request
        .targets
        .into_iter()
        .take(RUNTIME_LOG_MAX_TARGETS)
        .collect::<Vec<_>>();
    let results = join_all(targets.into_iter().map(collect_service_logs)).await;
    let mut per_service_lines = Vec::new();
    let mut unavailable_services = Vec::new();
    for mut result in results {
        truncated |= result.truncated;
        result
            .lines
            .sort_by(|left, right| left.timestamp.cmp(&right.timestamp));
        if !result.lines.is_empty() {
            per_service_lines.push(result.lines);
        }
        if let Some(error) = result.error {
            unavailable_services.push(error);
        }
    }
    let (lines, selection_truncated) = fair_bounded_lines(per_service_lines);
    truncated |= selection_truncated;
    RuntimeLogResponse {
        request_id: request.request_id,
        deployment_id: request.deployment_id,
        truncated,
        lines,
        unavailable_services,
    }
}

async fn collect_service_logs(target: RuntimeLogTarget) -> ServiceLogs {
    if !valid_target(&target) {
        return unavailable(
            &target.service,
            "Runtime logs are unavailable for this service.",
        );
    }
    match run_docker_logs(&target).await {
        Ok((stdout, stderr, output_truncated)) => {
            let mut lines = parse_output(&target.service, "stdout", &stdout);
            lines.extend(parse_output(&target.service, "stderr", &stderr));
            ServiceLogs {
                lines,
                truncated: output_truncated,
                error: None,
            }
        }
        Err(()) => unavailable(
            &target.service,
            "Runtime logs are unavailable for this service.",
        ),
    }
}

fn valid_target(target: &RuntimeLogTarget) -> bool {
    valid_container_name(&target.container)
        && !target.service.is_empty()
        && target.service.len() <= 64
        && target.service.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn unavailable(service: &str, message: &str) -> ServiceLogs {
    ServiceLogs {
        lines: Vec::new(),
        truncated: false,
        error: Some(RuntimeLogServiceError {
            service: service.chars().take(64).collect(),
            message: message.into(),
        }),
    }
}

async fn run_docker_logs(target: &RuntimeLogTarget) -> Result<(Vec<u8>, Vec<u8>, bool), ()> {
    let tail = RUNTIME_LOG_MAX_LINES.to_string();
    let mut child = Command::new("docker");
    child
        .args(["logs", "--timestamps", "--tail", &tail, &target.container])
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = child.spawn().map_err(|_| ())?;
    let stdout = child.stdout.take().ok_or(())?;
    let stderr = child.stderr.take().ok_or(())?;
    let stdout_task = tokio::spawn(read_bounded(stdout));
    let stderr_task = tokio::spawn(read_bounded(stderr));

    let (status, timed_out) = match tokio::time::timeout(DOCKER_LOG_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => (Some(status), false),
        _ => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            (None, true)
        }
    };
    let (stdout, stdout_truncated) = stdout_task.await.map_err(|_| ())?.map_err(|_| ())?;
    let (stderr, stderr_truncated) = stderr_task.await.map_err(|_| ())?.map_err(|_| ())?;
    if status.is_some_and(|status| !status.success()) {
        return Err(());
    }
    if timed_out && stdout.is_empty() && stderr.is_empty() {
        return Err(());
    }
    Ok((
        stdout,
        stderr,
        timed_out || stdout_truncated || stderr_truncated,
    ))
}

async fn read_bounded(mut reader: impl AsyncRead + Unpin) -> std::io::Result<(Vec<u8>, bool)> {
    let mut body = VecDeque::with_capacity(RUNTIME_LOG_MAX_BYTES);
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        if read >= RUNTIME_LOG_MAX_BYTES {
            body.clear();
            body.extend(chunk[read - RUNTIME_LOG_MAX_BYTES..read].iter().copied());
            truncated = true;
            continue;
        }
        let overflow = body
            .len()
            .saturating_add(read)
            .saturating_sub(RUNTIME_LOG_MAX_BYTES);
        if overflow > 0 {
            body.drain(..overflow);
            truncated = true;
        }
        body.extend(chunk[..read].iter().copied());
    }
    Ok((body.into_iter().collect(), truncated))
}

fn parse_output(service: &str, stream: &str, output: &[u8]) -> Vec<RuntimeLogLine> {
    String::from_utf8_lossy(output)
        .lines()
        .map(|raw| {
            let (timestamp, line) = split_timestamp(raw);
            RuntimeLogLine {
                timestamp: timestamp.map(str::to_string),
                service: service.to_string(),
                stream: stream.to_string(),
                line: sanitize_line(line),
            }
        })
        .collect()
}

fn split_timestamp(raw: &str) -> (Option<&str>, &str) {
    match raw.split_once(' ') {
        Some((timestamp, line))
            if timestamp.len() <= 64
                && timestamp.contains('T')
                && (timestamp.ends_with('Z')
                    || timestamp
                        .rsplit_once(['+', '-'])
                        .is_some_and(|(_, offset)| offset.contains(':'))) =>
        {
            (Some(timestamp), line)
        }
        _ => (None, raw),
    }
}

fn sanitize_line(line: &str) -> String {
    const TRUNCATION_MARKER: &str = "...[truncated]";

    let line = line
        .chars()
        .filter(|character| !character.is_control() || *character == '\t')
        .collect::<String>();
    if line.len() <= RUNTIME_LOG_MAX_LINE_BYTES {
        return line;
    }
    let end = RUNTIME_LOG_MAX_LINE_BYTES.saturating_sub(TRUNCATION_MARKER.len());
    format!("{}{}", utf8_prefix(&line, end), TRUNCATION_MARKER)
}

/// Selects newest lines round-robin across services. Taking one line from each
/// service per round guarantees a quiet service remains represented even when
/// another service fills the global line or byte budget.
fn fair_bounded_lines(mut services: Vec<Vec<RuntimeLogLine>>) -> (Vec<RuntimeLogLine>, bool) {
    let original_line_count = services.iter().map(Vec::len).sum::<usize>();
    let mut selected = Vec::new();
    let mut selected_bytes = 0usize;
    let mut truncated = false;
    let mut remaining_services = services.iter().filter(|lines| !lines.is_empty()).count();
    // Give every service an equal share for its newest line before allowing any
    // service a second line. JSON escaping can nearly double tab-heavy content,
    // so shrink that first line to its share rather than silently dropping a
    // service near the end of the round.
    for lines in &mut services {
        let Some(line) = lines.pop() else {
            continue;
        };
        let share = RUNTIME_LOG_LINE_BUDGET
            .saturating_sub(selected_bytes)
            .checked_div(remaining_services.max(1))
            .unwrap_or_default();
        remaining_services = remaining_services.saturating_sub(1);
        match fit_runtime_line_to_budget(line, share) {
            Some((line, line_truncated)) => {
                selected_bytes += runtime_line_bytes(&line);
                truncated |= line_truncated;
                selected.push(line);
            }
            None => truncated = true,
        }
    }

    loop {
        let mut examined = false;
        for lines in &mut services {
            let Some(line) = lines.pop() else {
                continue;
            };
            examined = true;
            if selected.len() >= RUNTIME_LOG_MAX_LINES {
                truncated = true;
                continue;
            }
            let bytes = runtime_line_bytes(&line);
            if selected_bytes.saturating_add(bytes) > RUNTIME_LOG_LINE_BUDGET {
                truncated = true;
                continue;
            }
            selected_bytes += bytes;
            selected.push(line);
        }
        if !examined || selected.len() >= RUNTIME_LOG_MAX_LINES {
            break;
        }
    }
    truncated |= selected.len() < original_line_count;
    selected.sort_by(|left, right| left.timestamp.cmp(&right.timestamp));
    (selected, truncated)
}

fn runtime_line_bytes(line: &RuntimeLogLine) -> usize {
    serde_json::to_vec(line)
        .map(|line| line.len())
        .unwrap_or(RUNTIME_LOG_LINE_BUDGET)
}

fn fit_runtime_line_to_budget(
    mut line: RuntimeLogLine,
    max_bytes: usize,
) -> Option<(RuntimeLogLine, bool)> {
    const TRUNCATION_MARKER: &str = "...[truncated]";

    if runtime_line_bytes(&line) <= max_bytes {
        return Some((line, false));
    }
    let original = std::mem::take(&mut line.line);
    let mut keep_bytes = original.len();
    loop {
        let prefix = utf8_prefix(&original, keep_bytes);
        line.line = format!("{prefix}{TRUNCATION_MARKER}");
        let serialized = runtime_line_bytes(&line);
        if serialized <= max_bytes {
            return Some((line, true));
        }
        if prefix.is_empty() {
            return None;
        }
        let excess = serialized.saturating_sub(max_bytes);
        keep_bytes = prefix
            .len()
            .saturating_sub(excess.max(prefix.len() / 4).max(1));
    }
}

fn utf8_prefix(value: &str, max_bytes: usize) -> &str {
    let mut end = max_bytes.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn parses_docker_timestamps_and_preserves_service_and_stream() {
        let rows = parse_output(
            "api",
            "stderr",
            b"2026-07-28T12:34:56.123456789Z request failed\n",
        );
        assert_eq!(
            rows,
            vec![RuntimeLogLine {
                timestamp: Some("2026-07-28T12:34:56.123456789Z".into()),
                service: "api".into(),
                stream: "stderr".into(),
                line: "request failed".into(),
            }]
        );
    }

    #[test]
    fn invalid_timestamp_is_kept_as_log_text() {
        let rows = parse_output("web", "stdout", b"ordinary application output\n");
        assert_eq!(rows[0].timestamp, None);
        assert_eq!(rows[0].line, "ordinary application output");
    }

    #[test]
    fn truncation_marker_stays_inside_the_per_line_byte_limit() {
        let line = sanitize_line(&"é".repeat(RUNTIME_LOG_MAX_LINE_BYTES));

        assert!(line.ends_with("...[truncated]"));
        assert!(line.len() <= RUNTIME_LOG_MAX_LINE_BYTES);
    }

    #[test]
    fn bounds_the_global_tail_by_lines_and_bytes() {
        let rows = (0..(RUNTIME_LOG_MAX_LINES + 1))
            .map(|index| RuntimeLogLine {
                timestamp: Some(format!("2026-07-28T12:34:{index:02}Z")),
                service: "web".into(),
                stream: "stdout".into(),
                line: "x".repeat((RUNTIME_LOG_MAX_BYTES / RUNTIME_LOG_MAX_LINES) + 32),
            })
            .collect::<Vec<_>>();
        let (rows, truncated) = fair_bounded_lines(vec![rows]);

        assert!(truncated);
        assert!(rows.len() <= RUNTIME_LOG_MAX_LINES);
        assert!(rows.iter().map(runtime_line_bytes).sum::<usize>() <= RUNTIME_LOG_LINE_BUDGET);
    }

    #[test]
    fn fair_selection_keeps_quiet_services_represented() {
        let noisy = (0..(RUNTIME_LOG_MAX_LINES * 2))
            .map(|index| RuntimeLogLine {
                timestamp: Some(format!(
                    "2026-07-28T12:{:02}:{:02}Z",
                    index / 60,
                    index % 60
                )),
                service: "noisy".into(),
                stream: "stdout".into(),
                line: format!("noisy-{index}"),
            })
            .collect::<Vec<_>>();
        let quiet = vec![RuntimeLogLine {
            timestamp: Some("2026-07-28T11:00:00Z".into()),
            service: "quiet".into(),
            stream: "stdout".into(),
            line: "quiet-latest".into(),
        }];

        let (rows, truncated) = fair_bounded_lines(vec![noisy, quiet]);

        assert!(truncated);
        assert_eq!(rows.len(), RUNTIME_LOG_MAX_LINES);
        assert!(rows.iter().any(|line| line.service == "quiet"));
        assert_eq!(
            rows.iter().filter(|line| line.service == "noisy").count(),
            RUNTIME_LOG_MAX_LINES - 1
        );
    }

    #[test]
    fn json_expansion_cannot_erase_the_last_service() {
        let services = (0..RUNTIME_LOG_MAX_TARGETS)
            .map(|index| {
                vec![RuntimeLogLine {
                    timestamp: Some("2026-07-28T12:34:56Z".into()),
                    service: format!("service-{index}"),
                    stream: "stdout".into(),
                    line: "\t".repeat(RUNTIME_LOG_MAX_LINE_BYTES),
                }]
            })
            .collect::<Vec<_>>();

        let (rows, truncated) = fair_bounded_lines(services);

        assert!(truncated);
        assert_eq!(rows.len(), RUNTIME_LOG_MAX_TARGETS);
        assert!((0..RUNTIME_LOG_MAX_TARGETS).all(|index| rows
            .iter()
            .any(|line| line.service == format!("service-{index}"))));
        assert!(rows.iter().map(runtime_line_bytes).sum::<usize>() <= RUNTIME_LOG_LINE_BUDGET);
    }

    #[tokio::test]
    async fn request_and_response_wire_shape_contains_no_secret_fields() {
        let request = RuntimeLogRequest {
            request_id: Uuid::new_v4(),
            deployment_id: Uuid::new_v4(),
            targets: Vec::new(),
        };
        let text = serde_json::json!({
            "type": "runtime_logs_request",
            "requestId": request.request_id,
            "deploymentId": request.deployment_id,
            "targets": []
        })
        .to_string();
        let parsed = runtime_logs_request(&text).unwrap();
        let response = runtime_logs_response(parsed).await;
        let serialized = response.to_string();

        assert!(serialized.contains("runtime_logs_response"));
        assert!(!serialized.contains("\"env\""));
        assert!(!serialized.contains("secret"));
    }

    #[test]
    fn busy_response_preserves_correlation_and_exposes_no_target_container() {
        let request = RuntimeLogRequest {
            request_id: Uuid::new_v4(),
            deployment_id: Uuid::new_v4(),
            targets: vec![RuntimeLogTarget {
                service: "api".into(),
                container: "hostlet-private-container-name".into(),
            }],
        };

        let response = runtime_logs_busy_response(&request);
        let serialized = response.to_string();

        assert_eq!(response["requestId"], request.request_id.to_string());
        assert_eq!(response["deploymentId"], request.deployment_id.to_string());
        assert!(serialized.contains("already in progress"));
        assert!(!serialized.contains("hostlet-private-container-name"));
    }

    #[tokio::test]
    async fn bounded_reader_drains_and_keeps_the_newest_bytes() {
        let mut input = vec![b'a'; RUNTIME_LOG_MAX_BYTES];
        input.extend(vec![b'b'; 512]);

        let (body, truncated) = read_bounded(input.as_slice()).await.unwrap();

        assert!(truncated);
        assert_eq!(body.len(), RUNTIME_LOG_MAX_BYTES);
        assert!(body.ends_with(&vec![b'b'; 512]));
    }
}
