use crate::state::AppState;
use async_trait::async_trait;
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

#[async_trait]
pub trait AccountPolicy: Send + Sync {
    async fn principal_allowed(
        &self,
        state: &AppState,
        principal: RequestPrincipal,
    ) -> anyhow::Result<()>;
}

#[async_trait]
pub trait AppPolicy: Send + Sync {
    async fn defaults_for_create(
        &self,
        state: &AppState,
        context: AppMutationContext,
        requested_name: &str,
        requested_server_id: Option<Uuid>,
    ) -> anyhow::Result<AppDefaults>;

    async fn validate_update(
        &self,
        state: &AppState,
        context: AppMutationContext,
    ) -> anyhow::Result<()>;
}

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
