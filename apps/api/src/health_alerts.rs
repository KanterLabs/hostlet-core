use crate::state::AppState;
use uuid::Uuid;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthTransitionEvent {
    pub app_id: Uuid,
    pub deployment_id: Option<Uuid>,
    pub container_name: Option<String>,
    pub status: String,
    pub previous_status: Option<String>,
    pub checked_url: Option<String>,
    pub http_status: Option<i32>,
    pub latency_ms: Option<i32>,
    pub failure_count: i32,
    pub success_count: i32,
    pub error: Option<String>,
}

pub trait HealthEventHooks: Send + Sync {
    fn handle_health_transition(&self, state: AppState, event: HealthTransitionEvent);
}

#[derive(Default)]
pub struct NoopHealthEventHooks;

impl HealthEventHooks for NoopHealthEventHooks {
    fn handle_health_transition(&self, _state: AppState, _event: HealthTransitionEvent) {}
}

pub(crate) fn is_health_down_status(status: &str) -> bool {
    status == "unhealthy"
}
