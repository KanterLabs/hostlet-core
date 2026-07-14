use super::*;

pub async fn recover_stale_deployments_and_cleanup(state: &AppState) -> anyhow::Result<u64> {
    let recovered = recover_stale_deployments(state).await?;
    crate::cleanup::auto_cleanup_sweep(state).await;
    Ok(recovered)
}

pub async fn recover_stale_deployments(state: &AppState) -> anyhow::Result<u64> {
    let result = sqlx::query("UPDATE deployments SET status='failed',failure_summary=COALESCE(failure_summary,'Deployment lost its durable execution job before completion.'),failure_code=COALESCE(failure_code,'missing_execution_job'),finished_at=now() WHERE status = ANY($1) AND NOT EXISTS (SELECT 1 FROM agent_jobs j WHERE j.deployment_id=deployments.id AND j.status IN ('queued','claimed','running'))")
        .bind(ACTIVE_DEPLOYMENT_STATUSES).execute(&state.db).await?;
    Ok(result.rows_affected())
}

pub(crate) async fn fail_deployment_row(state: &AppState, deployment_id: Uuid, summary: &str) {
    if let Err(err) = sqlx::query("UPDATE deployments SET status='failed',failure_summary=$2,finished_at=now() WHERE id=$1 AND status = ANY($3)")
        .bind(deployment_id).bind(summary).bind(ACTIVE_DEPLOYMENT_STATUSES).execute(&state.db).await {
        tracing::warn!(error = %err, %deployment_id, "failed to mark deployment row as failed during cleanup");
    }
}

pub(crate) async fn mark_deployment_running(state: &AppState, deployment_id: Uuid) {
    if let Err(err) =
        sqlx::query("UPDATE deployments SET status='running' WHERE id=$1 AND status='queued'")
            .bind(deployment_id)
            .execute(&state.db)
            .await
    {
        tracing::warn!(error = %err, %deployment_id, "failed to mark deployment running after enqueue");
    }
}

pub(crate) async fn ensure_no_active_deployment(
    state: &AppState,
    app_id: Uuid,
) -> anyhow::Result<()> {
    let active = sqlx::query(
        "SELECT id,status FROM deployments WHERE app_id=$1 AND status = ANY($2) LIMIT 1",
    )
    .bind(app_id)
    .bind(ACTIVE_DEPLOYMENT_STATUSES)
    .fetch_optional(&state.db)
    .await?;
    if let Some(row) = active {
        anyhow::bail!(
            "deployment {} is already {} for this app",
            row.get::<Uuid, _>("id"),
            row.get::<String, _>("status")
        );
    }
    Ok(())
}

pub(crate) fn is_active_deploy_unique_violation(err: &sqlx::Error) -> bool {
    let Some(db_err) = err.as_database_error() else {
        return false;
    };
    db_err.code().as_deref() == Some("23505")
        && db_err
            .message()
            .contains("idx_deployments_one_active_per_app")
}
