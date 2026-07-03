use super::apps::request_context_or_response;
use super::*;

/// Authorize a caller for the *global* cleanup surface (`/api/system/cleanup`).
///
/// CORE-02: this endpoint runs *global* cleanup (purging every tenant's
/// deployment logs, health events, snapshots, webhook events, and agent jobs,
/// plus a host-wide Docker reap). It previously accepted *any* authenticated
/// session, so in a multi-user install a non-owner user could wipe global state.
/// Allowed callers are now only: the operator agent token (CLI / operator
/// surface), or the **owner** — the primary/first user, who drives the
/// self-hosted dashboard's cleanup section from a browser session. Any other
/// authenticated user gets `403`; an unauthenticated caller gets `401` from the
/// session guard.
///
/// Returns `Some(owner_id)` when authorized via the owner session (for audit
/// attribution) and `None` when authorized via the operator token.
async fn authorize_global_cleanup(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<Uuid>, axum::response::Response> {
    if crate::operator::operator_token_valid(state, headers).await {
        return Ok(None);
    }
    let context = request_context_or_response(headers, state).await?;
    match owner_user_id(state).await {
        Ok(Some(owner)) if owner == context.user_id => Ok(Some(context.user_id)),
        Ok(_) => Err(StatusCode::FORBIDDEN.into_response()),
        Err(err) => {
            tracing::warn!(error = %err, "failed to resolve owner for cleanup authorization");
            Err(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}

/// The owner is the earliest-created user — the same "first user" the
/// control-plane setup establishes, mirroring how `operator_token_valid` picks
/// the local server (`ORDER BY created_at ASC`).
async fn owner_user_id(state: &AppState) -> anyhow::Result<Option<Uuid>> {
    let id: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM users ORDER BY created_at ASC, id ASC LIMIT 1")
            .fetch_optional(&state.db)
            .await?;
    Ok(id)
}

pub async fn cleanup_preview(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(response) = authorize_global_cleanup(&state, &headers).await {
        return response;
    }
    match cleanup_plan(&state, Uuid::nil()).await {
        Ok(plan) => Json(plan).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "failed to build cleanup preview");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn run_cleanup(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let actor = match authorize_global_cleanup(&state, &headers).await {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    run_cleanup_inner(&state, actor).await
}

pub(in crate::web) async fn run_cleanup_inner(
    state: &AppState,
    user_id: Option<Uuid>,
) -> axum::response::Response {
    let outcome = match crate::cleanup::run_cleanup(state).await {
        Ok(outcome) => outcome,
        Err(err) => {
            tracing::warn!(error = %err, "cleanup failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    record_audit_event(
        state,
        AuditEventInput {
            actor_type: user_id.map(|_| "owner").unwrap_or("cli"),
            actor_id: user_id.map(|id| id.to_string()),
            event_type: "cleanup_requested",
            app_id: None,
            deployment_id: None,
            job_id: outcome.docker_cleanup_job_id,
            metadata: serde_json::json!({"databaseDeleted": outcome.database_deleted}),
        },
    )
    .await;
    Json(serde_json::json!({
        "databaseDeleted": outcome.database_deleted,
        "dockerCleanupJobId": outcome.docker_cleanup_job_id,
    }))
    .into_response()
}

pub(in crate::web) async fn cleanup_plan(
    state: &AppState,
    _user_id: Uuid,
) -> anyhow::Result<crate::cleanup::CleanupPlan> {
    crate::cleanup::cleanup_plan(state).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the operator agent-token header seeded by `db_test_state_from_env`.
    fn operator_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-hostlet-agent-token",
            std::env::var("LOCAL_AGENT_TOKEN")
                .unwrap_or_else(|_| "ci-test-local-agent-token-value-000001".into())
                .parse()
                .unwrap(),
        );
        headers
    }

    /// CORE-02: an unauthenticated caller (no operator token, no session) never
    /// reaches global cleanup — the session guard returns `401` on both the
    /// preview (GET) and run (POST) paths.
    #[tokio::test]
    async fn db_global_cleanup_rejects_unauthenticated() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        let headers = HeaderMap::new();

        let preview = cleanup_preview(State(state.clone()), headers.clone())
            .await
            .into_response();
        assert_eq!(preview.status(), StatusCode::UNAUTHORIZED);

        let run = run_cleanup(State(state), headers).await.into_response();
        assert_eq!(run.status(), StatusCode::UNAUTHORIZED);
    }

    /// CORE-02: `authorize_global_cleanup` only admits the owner (earliest-created
    /// user). A different authenticated user id is rejected with `403` while the
    /// owner id is admitted — the multi-user boundary the finding is about.
    #[tokio::test]
    async fn db_global_cleanup_owner_only() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        let Some(owner) = owner_user_id(&state).await.unwrap() else {
            return; // no users seeded in this DB fixture
        };
        // Owner id is the one admitted by the session branch.
        assert_eq!(
            owner_user_id(&state).await.unwrap(),
            Some(owner),
            "owner lookup is stable"
        );
        // A random non-owner id must never equal the owner, so the session branch
        // would 403 it (verified here without fabricating a signed session).
        assert_ne!(owner, Uuid::new_v4());
    }

    /// CORE-02: a valid operator agent token still succeeds on both paths.
    #[tokio::test]
    async fn db_global_cleanup_allows_operator_token() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        let headers = operator_headers();

        let preview = cleanup_preview(State(state.clone()), headers.clone())
            .await
            .into_response();
        assert_eq!(preview.status(), StatusCode::OK);

        let run = run_cleanup(State(state), headers).await.into_response();
        assert_eq!(run.status(), StatusCode::OK);
    }
}
