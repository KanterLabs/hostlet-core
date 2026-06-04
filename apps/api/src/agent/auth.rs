use super::*;

pub(crate) async fn authenticated_server_id(state: &AppState, headers: &HeaderMap) -> Option<Uuid> {
    let server_id = header_uuid(headers, "x-hostlet-server-id")?;
    let token = headers
        .get("x-hostlet-agent-token")
        .and_then(|v| v.to_str().ok())?;
    // A DB error here is treated as "unauthenticated" so a transient outage can
    // never grant access, but we still surface it to logs as an operational fault
    // rather than swallowing it silently.
    let row = match sqlx::query("SELECT agent_token_hash FROM servers WHERE id=$1")
        .bind(server_id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(row) => row?,
        Err(err) => {
            tracing::warn!(error = %err, %server_id, "agent auth lookup failed");
            return None;
        }
    };
    let expected: Option<String> = row.get("agent_token_hash");
    expected
        .as_deref()
        .filter(|hash| verify_token(token, hash))
        .map(|_| server_id)
}
