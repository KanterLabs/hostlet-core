use super::*;
use health::ContainerState;

/// Observed actual state of a container during the reconciliation pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ContainerActual {
    Running,
    Stopped,
    Missing,
}

/// Action the reconciler should take for one app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReconcileDecision {
    /// Container is healthy; no action needed.
    Healthy,
    /// Container exists but is stopped; `docker start` it.
    StartStopped,
    /// Container is gone but the image is present; request a redeploy.
    Rebuild,
    /// Container AND image are gone; request a redeploy (will pull/build from source).
    RebuildImageGone,
}

/// Pure desired-vs-actual diff for ONE app.
///
/// `repair_in_flight` should be set when a repair was already requested or
/// attempted for the current failure streak, preventing duplicate enqueues.
pub(super) fn decide_reconcile(
    actual: ContainerActual,
    image_present: bool,
    repair_in_flight: bool,
) -> ReconcileDecision {
    if repair_in_flight {
        return ReconcileDecision::Healthy;
    }
    match actual {
        ContainerActual::Running => ReconcileDecision::Healthy,
        ContainerActual::Stopped => ReconcileDecision::StartStopped,
        ContainerActual::Missing if image_present => ReconcileDecision::Rebuild,
        ContainerActual::Missing => ReconcileDecision::RebuildImageGone,
    }
}

/// One observation per desired app, used by the list-level diff.
#[cfg(test)]
pub(super) struct ReconcileObservation {
    pub(super) app_id: Uuid,
    pub(super) actual: ContainerActual,
    pub(super) image_present: bool,
    pub(super) repair_in_flight: bool,
}

/// Compute the set of apps that need action from a slice of observations.
/// Apps whose decision is `Healthy` are dropped from the output.
#[cfg(test)]
pub(super) fn plan_reconcile_actions(
    obs: &[ReconcileObservation],
) -> Vec<(Uuid, ReconcileDecision)> {
    obs.iter()
        .filter_map(|o| {
            let decision = decide_reconcile(o.actual, o.image_present, o.repair_in_flight);
            if decision == ReconcileDecision::Healthy {
                None
            } else {
                Some((o.app_id, decision))
            }
        })
        .collect()
}

/// Map a `ContainerState` (from the health probe) to a `ContainerActual` for
/// the reconcile pass.  Returns `None` for transient states
/// (`Restarting` / `OomKilled`) where automatic intervention is not
/// appropriate — preserving the existing behaviour that OOM-killed and
/// restarting containers are NOT auto-started.
pub(super) fn container_actual_from_state(
    state: Option<&ContainerState>,
) -> Option<ContainerActual> {
    match state? {
        ContainerState::Running => Some(ContainerActual::Running),
        ContainerState::Stopped(_) => Some(ContainerActual::Stopped),
        ContainerState::Missing => Some(ContainerActual::Missing),
        ContainerState::Restarting(_) | ContainerState::OomKilled => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── decide_reconcile unit tests ───────────────────────────────────────────

    #[test]
    fn running_container_is_healthy() {
        assert_eq!(
            decide_reconcile(ContainerActual::Running, true, false),
            ReconcileDecision::Healthy
        );
    }

    #[test]
    fn stopped_container_triggers_start() {
        assert_eq!(
            decide_reconcile(ContainerActual::Stopped, true, false),
            ReconcileDecision::StartStopped
        );
    }

    #[test]
    fn missing_container_with_image_triggers_rebuild() {
        assert_eq!(
            decide_reconcile(ContainerActual::Missing, true, false),
            ReconcileDecision::Rebuild
        );
    }

    #[test]
    fn missing_container_without_image_triggers_rebuild_image_gone() {
        assert_eq!(
            decide_reconcile(ContainerActual::Missing, false, false),
            ReconcileDecision::RebuildImageGone
        );
    }

    #[test]
    fn repair_in_flight_suppresses_duplicate_for_missing_container() {
        assert_eq!(
            decide_reconcile(ContainerActual::Missing, true, true),
            ReconcileDecision::Healthy
        );
    }

    // ── plan_reconcile_actions regression tests ───────────────────────────────

    #[test]
    fn lost_app_yields_exactly_one_rebuild_action() {
        let app_id = Uuid::from_u128(1);
        let obs = [ReconcileObservation {
            app_id,
            actual: ContainerActual::Missing,
            image_present: true,
            repair_in_flight: false,
        }];
        let actions = plan_reconcile_actions(&obs);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0], (app_id, ReconcileDecision::Rebuild));
    }

    #[test]
    fn lost_app_with_repair_in_flight_yields_no_actions() {
        let app_id = Uuid::from_u128(2);
        let obs = [ReconcileObservation {
            app_id,
            actual: ContainerActual::Missing,
            image_present: true,
            repair_in_flight: true,
        }];
        let actions = plan_reconcile_actions(&obs);
        assert!(actions.is_empty());
    }

    // ── container_actual_from_state mapping ───────────────────────────────────

    #[test]
    fn running_state_maps_to_running() {
        assert_eq!(
            container_actual_from_state(Some(&ContainerState::Running)),
            Some(ContainerActual::Running)
        );
    }

    #[test]
    fn stopped_state_maps_to_stopped() {
        assert_eq!(
            container_actual_from_state(Some(&ContainerState::Stopped("0".into()))),
            Some(ContainerActual::Stopped)
        );
    }

    #[test]
    fn missing_state_maps_to_missing() {
        assert_eq!(
            container_actual_from_state(Some(&ContainerState::Missing)),
            Some(ContainerActual::Missing)
        );
    }

    #[test]
    fn restarting_state_maps_to_none() {
        assert_eq!(
            container_actual_from_state(Some(&ContainerState::Restarting("1".into()))),
            None
        );
    }

    #[test]
    fn oom_killed_state_maps_to_none() {
        assert_eq!(
            container_actual_from_state(Some(&ContainerState::OomKilled)),
            None
        );
    }

    #[test]
    fn no_state_maps_to_none() {
        assert_eq!(container_actual_from_state(None), None);
    }
}
