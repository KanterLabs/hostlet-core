use crate::state::AppState;
use async_trait::async_trait;
use sqlx::Row;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestPrincipal {
    pub user_id: Uuid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppMutationContext {
    pub user_id: Uuid,
    pub app_id: Option<Uuid>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppDefaults {
    pub server_id: Uuid,
    pub domain: String,
    pub memory_limit_mb: Option<i32>,
    pub cpu_limit: Option<f64>,
    pub public_exposure: bool,
    pub auto_deploy: bool,
}

/// Authorizes whether a given principal is permitted to act at all.
///
/// Implementations gate access for the whole request (e.g. account-level
/// suspension or allowlist checks) and should return `Err` to reject the
/// request before any handler-specific logic runs. The self-hosted overlay
/// allows every principal; cloud overlays may enforce tenancy/quota rules.
#[async_trait]
pub trait AccountPolicy: Send + Sync {
    async fn principal_allowed(
        &self,
        state: &AppState,
        principal: RequestPrincipal,
    ) -> anyhow::Result<()>;
}

/// Resolves and validates per-app mutation rules.
///
/// This is the extension seam that lets overlays inject hosting-tier defaults
/// and guardrails around app create/update without forking the handlers.
#[async_trait]
pub trait AppPolicy: Send + Sync {
    /// Resolves the effective [`AppDefaults`] for a create request.
    ///
    /// Called once while handling an app-create request, before the row is
    /// inserted, with the caller's requested (possibly absent) name and server.
    /// Implementations must return a fully-resolved set of defaults (server,
    /// domain, limits, exposure flags) or `Err` if the request is disallowed.
    async fn defaults_for_create(
        &self,
        state: &AppState,
        context: AppMutationContext,
        requested_name: &str,
        requested_server_id: Option<Uuid>,
    ) -> anyhow::Result<AppDefaults>;

    /// Enforces invariants on an app-update request.
    ///
    /// Called while handling an app-update request, before persisting changes,
    /// to confirm the principal in `context` may mutate the target app. Returns
    /// `Err` to reject the update; an `Ok` result must mean the update is
    /// permitted under the active policy.
    async fn validate_update(
        &self,
        state: &AppState,
        context: AppMutationContext,
    ) -> anyhow::Result<()>;
}

/// Supplies the credentials used to pull a repository during deploy.
#[async_trait]
pub trait RepositoryAccessProvider: Send + Sync {
    async fn token_for_deploy(
        &self,
        state: &AppState,
        user_id: Uuid,
        repo_full_name: &str,
    ) -> anyhow::Result<Option<String>>;
}

#[derive(Default)]
pub struct SelfHostedAccountPolicy;

#[async_trait]
impl AccountPolicy for SelfHostedAccountPolicy {
    async fn principal_allowed(
        &self,
        _state: &AppState,
        _principal: RequestPrincipal,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Default repository access for self-hosted deploys: the user's stored GitHub
/// OAuth token (absent for users who never connected GitHub, e.g. deploying a
/// public repo). A lookup/decrypt failure yields `None` — the clone falls back
/// to unauthenticated rather than failing the deploy — preserving the prior
/// best-effort behavior. Cloud overlays this with an installation-token provider.
#[derive(Default)]
pub struct SelfHostedRepositoryAccessProvider;

#[async_trait]
impl RepositoryAccessProvider for SelfHostedRepositoryAccessProvider {
    async fn token_for_deploy(
        &self,
        state: &AppState,
        user_id: Uuid,
        _repo_full_name: &str,
    ) -> anyhow::Result<Option<String>> {
        Ok(latest_github_oauth_token(state, user_id)
            .await
            .ok()
            .flatten())
    }
}

/// Fetch and decrypt the most-recently-updated stored GitHub OAuth token for a
/// user, or `Ok(None)` if they have no connected GitHub account.
async fn latest_github_oauth_token(
    state: &AppState,
    user_id: Uuid,
) -> anyhow::Result<Option<String>> {
    let row = sqlx::query(
        "SELECT access_token_ciphertext
         FROM github_accounts
         WHERE user_id=$1
         ORDER BY updated_at DESC
         LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;
    row.map(|row| {
        state
            .crypto
            .decrypt(row.get::<String, _>("access_token_ciphertext").as_str())
    })
    .transpose()
}
