use serde_json::{json, Value};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub const BROWSER_SMOKE_JOB: &str = "browser_smoke";

/// Clear health state when a deployment becomes the app's active runtime.
///
/// Snapshots are keyed by app for the API's fast current-state reads, so an
/// activation must not leave the previous deployment's probe result attached
/// to the app while the new runtime is waiting for its first report. Browser
/// state is reset at the same boundary for the same reason. Historical health
/// events remain available and retain their deployment ids.
pub async fn reset_for_activation(
    tx: &mut Transaction<'_, Postgres>,
    app_id: Uuid,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM app_health_snapshots WHERE app_id=$1")
        .bind(app_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM app_browser_health WHERE app_id=$1")
        .bind(app_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub fn combined_status(http_status: &str, browser_status: Option<&str>) -> String {
    match http_status {
        "unhealthy" => "unhealthy".to_string(),
        "degraded" => "degraded".to_string(),
        "healthy" if matches!(browser_status, Some("pending" | "failed")) => "degraded".to_string(),
        other => other.to_string(),
    }
}

pub fn browser_json(
    status: Option<String>,
    checked_at: Option<chrono::DateTime<chrono::Utc>>,
    failure: Option<String>,
) -> Option<Value> {
    status.map(|status| {
        json!({
            "status": status,
            "checkedAt": checked_at,
            "failure": failure,
        })
    })
}

pub async fn mark_pending(
    db: &sqlx::PgPool,
    app_id: Uuid,
    deployment_id: Uuid,
) -> anyhow::Result<()> {
    let mut tx = db.begin().await?;
    let current_deployment_id = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT current_deployment_id
         FROM apps
         WHERE id=$1 AND public_exposure=true
         FOR UPDATE",
    )
    .bind(app_id)
    .fetch_optional(&mut *tx)
    .await?;
    if current_deployment_id != Some(Some(deployment_id)) {
        tx.commit().await?;
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO app_browser_health
           (app_id,deployment_id,status,failure,checked_at,updated_at)
         VALUES ($1,$2,'pending',NULL,NULL,now())
         ON CONFLICT (app_id) DO UPDATE SET
           deployment_id=EXCLUDED.deployment_id,status='pending',failure=NULL,
           checked_at=NULL,updated_at=now()
         WHERE app_browser_health.deployment_id=EXCLUDED.deployment_id",
    )
    .bind(app_id)
    .bind(deployment_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn record_job_result(
    tx: &mut Transaction<'_, Postgres>,
    job_type: &str,
    app_id: Option<Uuid>,
    deployment_id: Option<Uuid>,
    job_status: &str,
    failure: Option<&str>,
) -> anyhow::Result<()> {
    if job_type != BROWSER_SMOKE_JOB {
        return Ok(());
    }
    let (Some(app_id), Some(deployment_id)) = (app_id, deployment_id) else {
        return Ok(());
    };
    let status = if job_status == "success" {
        "ready"
    } else {
        "failed"
    };
    let failure = if status == "failed" {
        failure.or(Some("Browser check did not complete."))
    } else {
        None
    };
    let current_deployment_id = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT current_deployment_id FROM apps WHERE id=$1 FOR UPDATE",
    )
    .bind(app_id)
    .fetch_optional(&mut **tx)
    .await?;
    if current_deployment_id != Some(Some(deployment_id)) {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO app_browser_health
           (app_id,deployment_id,status,failure,checked_at,updated_at)
         VALUES ($1,$2,$3,$4,now(),now())
         ON CONFLICT (app_id) DO UPDATE SET
           deployment_id=EXCLUDED.deployment_id,status=EXCLUDED.status,
           failure=EXCLUDED.failure,checked_at=EXCLUDED.checked_at,updated_at=now()
         WHERE app_browser_health.deployment_id=EXCLUDED.deployment_id",
    )
    .bind(app_id)
    .bind(deployment_id)
    .bind(status)
    .bind(failure)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::combined_status;

    #[test]
    fn browser_failure_only_degrades_healthy_http_apps() {
        assert_eq!(combined_status("healthy", Some("failed")), "degraded");
        assert_eq!(combined_status("healthy", Some("pending")), "degraded");
        assert_eq!(combined_status("healthy", Some("ready")), "healthy");
        assert_eq!(combined_status("healthy", Some("skipped")), "healthy");
        assert_eq!(combined_status("unhealthy", Some("ready")), "unhealthy");
        assert_eq!(combined_status("degraded", Some("ready")), "degraded");
    }
}
