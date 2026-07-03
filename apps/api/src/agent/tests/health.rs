use super::*;
use crate::health_alerts::{HealthEventHooks, HealthTransitionEvent};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct CapturingHealthHooks {
    events: Mutex<Vec<HealthTransitionEvent>>,
}

impl CapturingHealthHooks {
    fn events(&self) -> Vec<HealthTransitionEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl HealthEventHooks for CapturingHealthHooks {
    fn handle_health_transition(&self, _state: AppState, event: HealthTransitionEvent) {
        self.events.lock().unwrap().push(event);
    }
}

#[tokio::test]
async fn db_health_down_hook_fires_once_per_unhealthy_transition() {
    let Some(state) = crate::state::db_test_state_from_env().await else {
        return;
    };
    let hooks = Arc::new(CapturingHealthHooks::default());
    let state = state.with_health_event_hooks(hooks.clone());
    reset_agent_db(&state).await;
    let user_id = insert_user(&state).await;
    let app_id = insert_app(&state, user_id).await;
    let deployment_id = insert_deployment(&state, app_id).await;

    send_health_status(&state, app_id, deployment_id, "degraded").await;
    send_health_status(&state, app_id, deployment_id, "unhealthy").await;
    send_health_status(&state, app_id, deployment_id, "unhealthy").await;
    send_health_status(&state, app_id, deployment_id, "healthy").await;
    send_health_status(&state, app_id, deployment_id, "unhealthy").await;

    let events = hooks.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].app_id, app_id);
    assert_eq!(events[0].deployment_id, Some(deployment_id));
    assert_eq!(events[0].status, "unhealthy");
    assert_eq!(events[0].previous_status.as_deref(), Some("degraded"));
    assert_eq!(events[1].previous_status.as_deref(), Some("healthy"));
}
