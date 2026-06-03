//! Shared GitHub access-token retrieval.
//!
//! The "fetch the most recently updated stored token ciphertext for a user"
//! query was copy-pasted in four call sites (`status`, `repos`,
//! `repo_inspect`, and `ensure_repo_webhook`), each with a slightly different
//! error-handling shape. Centralizing the SELECT here removes that drift risk
//! while leaving each caller free to map a missing token / decrypt failure to
//! the HTTP or error shape it needs.

use crate::state::AppState;
use sqlx::Row;
use uuid::Uuid;

/// Fetch the most-recently-updated stored access-token ciphertext for `user_id`,
/// or `Ok(None)` if the user has no connected GitHub account.
pub(super) async fn latest_ciphertext(
    state: &AppState,
    user_id: Uuid,
) -> sqlx::Result<Option<String>> {
    let row = sqlx::query(
        "SELECT access_token_ciphertext FROM github_accounts WHERE user_id=$1 \
         ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(row.map(|row| row.get::<String, _>("access_token_ciphertext")))
}

/// Fetch and decrypt the latest token, returning `None` on any failure (missing
/// account, query error, or decrypt error). Used where the caller only cares
/// whether a usable token exists.
pub(super) async fn latest_decrypted(state: &AppState, user_id: Uuid) -> Option<String> {
    let ciphertext = latest_ciphertext(state, user_id).await.ok().flatten()?;
    state.crypto.decrypt(&ciphertext).ok()
}
