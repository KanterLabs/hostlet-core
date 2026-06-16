use super::*;

pub(in crate::agent) fn valid_deployment_status(status: &str) -> bool {
    status.parse::<DeploymentStatus>().is_ok() && status != "canceled"
}

pub(in crate::agent) fn valid_agent_job_status(status: &str) -> bool {
    status.parse::<AgentJobStatus>().is_ok() && status != "canceled"
}

pub(in crate::agent) fn valid_health_status(status: &str) -> bool {
    status.parse::<RuntimeHealthStatus>().is_ok()
}

pub(in crate::agent) fn valid_reconcile_reason(reason: &str) -> bool {
    reason == "rebuild"
}

pub(in crate::agent) use hostlet_contracts::valid_container_name;

pub(in crate::agent) fn truncate_log_line(line: &str, max_bytes: usize) -> String {
    if line.len() <= max_bytes {
        return line.to_string();
    }
    let mut end = max_bytes;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...[truncated]", &line[..end])
}

pub(in crate::agent) fn header_uuid(headers: &HeaderMap, key: &str) -> Option<Uuid> {
    headers
        .get(key)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
}

pub(in crate::agent) fn connection_is_current(
    connection: &AgentConnection,
    connection_id: Uuid,
) -> bool {
    connection.connection_id == connection_id
}

#[cfg(test)]
mod reconcile_reason_tests {
    use super::*;

    #[test]
    fn rebuild_is_valid_reconcile_reason() {
        assert!(valid_reconcile_reason("rebuild"));
    }

    #[test]
    fn unknown_reasons_are_invalid() {
        assert!(!valid_reconcile_reason(""));
        assert!(!valid_reconcile_reason("unknown"));
        assert!(!valid_reconcile_reason("REBUILD"));
    }
}
