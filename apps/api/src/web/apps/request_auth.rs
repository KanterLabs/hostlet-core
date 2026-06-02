//! Shared auth preamble for the mutating app handlers.

use crate::auth::{request_context, RequestContext};
use crate::state::AppState;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

/// Resolve the authenticated [`RequestContext`] for a mutating request,
/// mapping auth failures onto the wire-compatible HTTP responses that the
/// route handlers have always returned: a missing/expired session yields
/// `401 Unauthorized`, while any other auth error is surfaced verbatim as a
/// `402 Payment Required` body.
///
/// Returning `Err(Response)` lets callers short-circuit with a `match` arm
/// while keeping the historical status codes intact.
pub(in crate::web) async fn request_context_or_response(
    headers: &HeaderMap,
    state: &AppState,
) -> Result<RequestContext, Response> {
    match request_context(headers, state).await {
        Ok(context) => Ok(context),
        Err(err) if err.to_string() == "sign in required" => {
            Err(StatusCode::UNAUTHORIZED.into_response())
        }
        Err(err) => Err((StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response()),
    }
}
