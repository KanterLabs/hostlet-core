//! Operator-token validation for protected system endpoints.
//!
//! Extracted from `crate::web::system` so that the Hostlet Cloud overlay
//! (which replaces `web/` wholesale) can call this helper directly instead
//! of forking it.

use crate::crypto::verify_token;
use crate::state::AppState;
use axum::http::HeaderMap;
use sqlx::Row;

/// Returns `true` when the request carries a valid operator agent token
/// (i.e. the token stored for the local server).
pub async fn operator_token_valid(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(token) = headers
        .get("x-hostlet-agent-token")
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let row = sqlx::query(
        "SELECT agent_token_hash FROM servers WHERE kind='local' ORDER BY created_at ASC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await;
    let Ok(Some(row)) = row else {
        return false;
    };
    let expected: Option<String> = row.get("agent_token_hash");
    expected
        .as_deref()
        .is_some_and(|hash| verify_token(token, hash))
}
