use super::*;

pub(in crate::agent) async fn authenticated_server_id(
    state: &AppState,
    headers: &HeaderMap,
) -> Option<Uuid> {
    let server_id = header_uuid(headers, "x-hostlet-server-id")?;
    let token = headers
        .get("x-hostlet-agent-token")
        .and_then(|v| v.to_str().ok())?;
    let row = sqlx::query("SELECT agent_token_hash FROM servers WHERE id=$1")
        .bind(server_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()?;
    let expected: Option<String> = row.get("agent_token_hash");
    expected
        .as_deref()
        .filter(|hash| verify_token(token, hash))
        .map(|_| server_id)
}
