use crate::{agent, deploy, state::AppState, web};
use std::time::Duration;

const RUNTIME_RECOVERY_INTERVAL: Duration = Duration::from_secs(120);

pub async fn recover_startup_state(state: &AppState) -> anyhow::Result<()> {
    let recovered = deploy::recover_stale_deployments_and_cleanup(state).await?;
    if recovered > 0 {
        tracing::warn!(recovered, "marked stale deployments as failed");
    }
    let recovered_jobs = agent::recover_stale_agent_jobs(state).await?;
    if recovered_jobs > 0 {
        tracing::warn!(recovered_jobs, "reconciled stale agent jobs");
    }
    let finalized_deletes = web::reconcile_completed_delete_jobs(state).await?;
    if finalized_deletes > 0 {
        tracing::warn!(finalized_deletes, "finalized completed delete jobs");
    }
    Ok(())
}

pub fn spawn_runtime_recovery_task(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(RUNTIME_RECOVERY_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            recover_runtime_state(&state).await;
        }
    });
}

async fn recover_runtime_state(state: &AppState) {
    match deploy::recover_stale_deployments(state).await {
        Ok(recovered) if recovered > 0 => {
            tracing::warn!(recovered, "marked stale deployments as failed");
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(error = %err, "periodic stale-deployment recovery failed"),
    }
    match agent::recover_stale_agent_jobs(state).await {
        Ok(recovered) if recovered > 0 => {
            tracing::warn!(recovered, "reconciled stale agent jobs");
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(error = %err, "periodic stale-job recovery failed"),
    }
}
