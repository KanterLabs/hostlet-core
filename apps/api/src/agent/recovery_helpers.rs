use super::*;

/// Shared lock acquisition for recovery transitions. Rows are sorted before
/// locking, and the table order is app -> deployment -> job.
pub(crate) async fn lock_lifecycle_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    mut app_ids: Vec<Uuid>,
    mut deployment_ids: Vec<Uuid>,
    mut job_ids: Vec<Uuid>,
) -> anyhow::Result<()> {
    app_ids.sort_unstable();
    app_ids.dedup();
    deployment_ids.sort_unstable();
    deployment_ids.dedup();
    job_ids.sort_unstable();
    job_ids.dedup();
    crate::agent::locks::apps(tx, &app_ids).await?;
    crate::agent::locks::deployments(tx, &deployment_ids).await?;
    crate::agent::locks::jobs(tx, &job_ids).await?;
    Ok(())
}

pub(crate) async fn terminalize_expired_cancellations(state: &AppState) -> anyhow::Result<u64> {
    let mut tx = state.db.begin().await?;
    let candidates = sqlx::query(
        "SELECT j.id,j.app_id,j.deployment_id,COALESCE(j.app_id,d.app_id) AS lifecycle_app_id
         FROM agent_jobs j
         LEFT JOIN deployments d ON d.id=j.deployment_id
         WHERE j.status IN ('claimed','running')
           AND j.cancel_requested_at IS NOT NULL
           AND (j.lease_expires_at IS NULL OR j.lease_expires_at <= clock_timestamp())
         ORDER BY j.id",
    )
    .fetch_all(&mut *tx)
    .await?;
    let job_ids = candidates
        .iter()
        .map(|row| row.get::<Uuid, _>("id"))
        .collect::<Vec<_>>();
    if job_ids.is_empty() {
        tx.commit().await?;
        return Ok(0);
    }
    lock_lifecycle_rows(
        &mut tx,
        candidates
            .iter()
            .filter_map(|row| row.get::<Option<Uuid>, _>("lifecycle_app_id"))
            .collect(),
        candidates
            .iter()
            .filter_map(|row| row.get::<Option<Uuid>, _>("deployment_id"))
            .collect(),
        job_ids.clone(),
    )
    .await?;
    let rows = sqlx::query(
        "UPDATE agent_jobs
         SET status='cancelled',
             failure_summary=COALESCE(failure_summary,
               'Job cancellation was requested before its lease expired.'),
             last_error=COALESCE(last_error,
               'Job cancellation was requested before its lease expired.'),
             payload_json=payload_json-'env'-'github_token'-'artifact_registry',
             claimed_by=NULL,claimed_at=NULL,claim_token=NULL,
             lease_expires_at=NULL,finished_at=clock_timestamp(),updated_at=clock_timestamp()
         WHERE id=ANY($1)
           AND status IN ('claimed','running')
           AND cancel_requested_at IS NOT NULL
           AND (lease_expires_at IS NULL OR lease_expires_at <= clock_timestamp())
         RETURNING deployment_id",
    )
    .bind(&job_ids)
    .fetch_all(&mut *tx)
    .await?;
    let deployment_ids = rows
        .iter()
        .filter_map(|row| row.get::<Option<Uuid>, _>("deployment_id"))
        .collect::<Vec<_>>();
    if !deployment_ids.is_empty() {
        sqlx::query(
            "UPDATE deployments
             SET status='canceled',failure_code=COALESCE(failure_code,'cancelled_by_owner'),
                 failure_summary=COALESCE(failure_summary,
                   'Job cancellation was requested before its lease expired.'),
                 finished_at=clock_timestamp()
             WHERE id=ANY($1) AND status=ANY($2)",
        )
        .bind(&deployment_ids)
        .bind(crate::deploy::ACTIVE_DEPLOYMENT_STATUSES)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE deployment_builds
             SET status='canceled',
                 failure_code=COALESCE(failure_code,'cancelled_by_owner'),
                 failure_summary=COALESCE(failure_summary,
                   'Job cancellation was requested before its lease expired.'),
                 finished_at=clock_timestamp(),updated_at=clock_timestamp()
             WHERE deployment_id=ANY($1)
               AND status NOT IN ('succeeded','failed','canceled')",
        )
        .bind(&deployment_ids)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE apps
             SET pending_deployment_id=NULL,updated_at=clock_timestamp()
             WHERE pending_deployment_id=ANY($1)",
        )
        .bind(&deployment_ids)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(rows.len() as u64)
}

pub(crate) async fn recover_expired_jobs(state: &AppState, fail: bool) -> anyhow::Result<u64> {
    let predicate = if fail {
        "j.status IN ('claimed','running')
         AND j.lease_expires_at < clock_timestamp()
         AND j.attempt >= j.max_attempts
         AND j.cancel_requested_at IS NULL"
    } else {
        super::routes::RETRYABLE_EXPIRED_JOBS_PREDICATE
    };
    let mut tx = state.db.begin().await?;
    let candidates = sqlx::query(&format!(
        "SELECT j.id,j.app_id,j.deployment_id,COALESCE(j.app_id,d.app_id) AS lifecycle_app_id
         FROM agent_jobs j
         LEFT JOIN deployments d ON d.id=j.deployment_id
         WHERE {predicate}
         ORDER BY j.id"
    ))
    .fetch_all(&mut *tx)
    .await?;
    let job_ids = candidates
        .iter()
        .map(|row| row.get::<Uuid, _>("id"))
        .collect::<Vec<_>>();
    if job_ids.is_empty() {
        tx.commit().await?;
        return Ok(0);
    }
    lock_lifecycle_rows(
        &mut tx,
        candidates
            .iter()
            .filter_map(|row| row.get::<Option<Uuid>, _>("lifecycle_app_id"))
            .collect(),
        candidates
            .iter()
            .filter_map(|row| row.get::<Option<Uuid>, _>("deployment_id"))
            .collect(),
        job_ids.clone(),
    )
    .await?;
    let updated = if fail {
        sqlx::query(&format!(
            "UPDATE agent_jobs j
             SET status='failed',
                 failure_summary=COALESCE(failure_summary, 'Agent job lease expired and retry limit was reached.'),
                 last_error=COALESCE(last_error, 'Agent job lease expired and retry limit was reached.'),
                 payload_json=payload_json - 'env' - 'github_token' - 'artifact_registry',
                 lease_expires_at=NULL,updated_at=clock_timestamp(),finished_at=clock_timestamp()
             WHERE j.id=ANY($1) AND {predicate}
             RETURNING j.deployment_id"
        ))
        .bind(&job_ids)
        .fetch_all(&mut *tx)
        .await?
    } else {
        sqlx::query(&format!(
            "UPDATE agent_jobs j
             {}
             WHERE j.id=ANY($1) AND {predicate}
             RETURNING j.deployment_id",
            super::routes::REQUEUE_JOB_SET_CLAUSE
        ))
        .bind(&job_ids)
        .fetch_all(&mut *tx)
        .await?
    };
    if fail {
        let mut deployment_ids = updated
            .iter()
            .filter_map(|row| row.get::<Option<Uuid>, _>("deployment_id"))
            .collect::<Vec<_>>();
        deployment_ids.sort_unstable();
        deployment_ids.dedup();
        if !deployment_ids.is_empty() {
            sqlx::query(
                "UPDATE deployments
                 SET status='failed',failure_code=COALESCE(failure_code,'execution_lease_exhausted'),
                     failure_summary=COALESCE(failure_summary,'Deployment recovery attempts were exhausted.'),
                     finished_at=clock_timestamp()
                 WHERE id=ANY($1) AND status=ANY($2)",
            )
            .bind(&deployment_ids)
            .bind(crate::deploy::ACTIVE_DEPLOYMENT_STATUSES)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE deployment_builds
                 SET status='failed',failure_code=COALESCE(failure_code,'execution_lease_exhausted'),
                     failure_summary=COALESCE(failure_summary,'Builder recovery attempts were exhausted.'),
                     finished_at=clock_timestamp(),updated_at=clock_timestamp()
                 WHERE deployment_id=ANY($1)
                   AND status NOT IN ('succeeded','failed','canceled')",
            )
            .bind(&deployment_ids)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE apps SET pending_deployment_id=NULL,updated_at=clock_timestamp()
                 WHERE pending_deployment_id=ANY($1)",
            )
            .bind(&deployment_ids)
            .execute(&mut *tx)
            .await?;
        }
    }
    let count = updated.len() as u64;
    tx.commit().await?;
    Ok(count)
}

pub(crate) async fn scrub_terminal_job_payload_secrets(state: &AppState) -> anyhow::Result<u64> {
    Ok(sqlx::query(
        "UPDATE agent_jobs
         SET payload_json = payload_json - 'env' - 'github_token' - 'artifact_registry'
         WHERE status IN ('success','failed','cancelled','expired')
           AND (jsonb_exists(payload_json,'env')
             OR jsonb_exists(payload_json,'github_token')
             OR jsonb_exists(payload_json,'artifact_registry'))",
    )
    .execute(&state.db)
    .await?
    .rows_affected())
}
