use crate::{agent::locks, auth::request_context, deploy, state::AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

pub const USER_REQUESTED: &str = "user_requested";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuspensionChange {
    pub app_id: Uuid,
    pub suspended: bool,
    pub reasons: Vec<String>,
    pub job_id: Option<Uuid>,
}

struct RuntimeTarget {
    server_id: Uuid,
    deployment_id: Uuid,
    container_name: String,
    compose_project: Option<String>,
    container_port: i32,
    published_port: i32,
    health_path: String,
    domain: String,
}

pub async fn pause_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(app_id): Path<Uuid>,
) -> Response {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(err) if err.to_string() == "sign in required" => {
            return StatusCode::UNAUTHORIZED.into_response()
        }
        Err(err) => return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response(),
    };
    match set_reason(
        &state,
        app_id,
        Some(context.user_id),
        USER_REQUESTED,
        "owner",
    )
    .await
    {
        Ok(change) => Json(change).into_response(),
        Err(ChangeError::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(ChangeError::Database(err)) => {
            tracing::warn!(%app_id, error = %err, "failed to pause app");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn resume_app(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(app_id): Path<Uuid>,
) -> Response {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(err) if err.to_string() == "sign in required" => {
            return StatusCode::UNAUTHORIZED.into_response()
        }
        Err(err) => return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response(),
    };
    match clear_reason(
        &state,
        app_id,
        Some(context.user_id),
        USER_REQUESTED,
        "owner",
    )
    .await
    {
        Ok(change) => Json(change).into_response(),
        Err(ChangeError::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(ChangeError::Database(err)) => {
            tracing::warn!(%app_id, error = %err, "failed to resume app");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ChangeError {
    #[error("app not found")]
    NotFound,
    #[error(transparent)]
    Database(#[from] anyhow::Error),
}

pub async fn set_reason(
    state: &AppState,
    app_id: Uuid,
    owner_id: Option<Uuid>,
    reason: &str,
    actor: &str,
) -> Result<SuspensionChange, ChangeError> {
    change_reason(state, app_id, owner_id, reason, actor, true).await
}

pub async fn clear_reason(
    state: &AppState,
    app_id: Uuid,
    owner_id: Option<Uuid>,
    reason: &str,
    actor: &str,
) -> Result<SuspensionChange, ChangeError> {
    change_reason(state, app_id, owner_id, reason, actor, false).await
}

async fn change_reason(
    state: &AppState,
    app_id: Uuid,
    owner_id: Option<Uuid>,
    reason: &str,
    actor: &str,
    add: bool,
) -> Result<SuspensionChange, ChangeError> {
    if reason.trim().is_empty() {
        return Err(ChangeError::Database(anyhow::anyhow!(
            "suspension reason is required"
        )));
    }
    let mut tx = state.db.begin().await.map_err(anyhow::Error::from)?;
    let target = load_runtime_target(&mut tx, app_id, owner_id).await?;
    // `load_runtime_target` holds the app row. Acquire every lifecycle row
    // that this transition may mutate before touching jobs or deployments so
    // suspension and activation/cancellation share app -> deployment -> job.
    let lifecycle_rows = sqlx::query(
        "SELECT id,deployment_id
         FROM agent_jobs
         WHERE app_id=$1
           AND job_type IN ('deploy','rollback','build','release')
           AND status IN ('queued','claimed','running')
         ORDER BY id",
    )
    .bind(app_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;
    let mut deployment_ids = lifecycle_rows
        .iter()
        .filter_map(|row| row.get::<Option<Uuid>, _>("deployment_id"))
        .collect::<Vec<_>>();
    if let Some(target) = &target {
        deployment_ids.push(target.deployment_id);
    }
    deployment_ids.sort_unstable();
    deployment_ids.dedup();
    locks::deployments(&mut tx, &deployment_ids)
        .await
        .map_err(ChangeError::Database)?;
    let mut job_ids = lifecycle_rows
        .iter()
        .map(|row| row.get::<Uuid, _>("id"))
        .collect::<Vec<_>>();
    job_ids.sort_unstable();
    job_ids.dedup();
    locks::jobs(&mut tx, &job_ids)
        .await
        .map_err(ChangeError::Database)?;
    let before: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM app_suspensions WHERE app_id=$1")
            .bind(app_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(anyhow::Error::from)?;
    if add {
        sqlx::query(
            "INSERT INTO app_suspensions(app_id,reason,created_by)
             VALUES ($1,$2,$3)
             ON CONFLICT(app_id,reason) DO NOTHING",
        )
        .bind(app_id)
        .bind(reason)
        .bind(actor)
        .execute(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
        let cancelled = sqlx::query(
            "UPDATE agent_jobs
             SET status='cancelled',
                 failure_summary='App was paused before this deployment started.',
                 payload_json=payload_json - 'env' - 'github_token' - 'artifact_registry',
                 updated_at=now(),finished_at=now()
             WHERE app_id=$1
               AND job_type IN ('deploy','rollback','build','release')
               AND status='queued'
             RETURNING deployment_id",
        )
        .bind(app_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
        let deployment_ids = cancelled
            .into_iter()
            .filter_map(|row| row.get::<Option<Uuid>, _>("deployment_id"))
            .collect::<Vec<_>>();
        if !deployment_ids.is_empty() {
            sqlx::query(
                "UPDATE deployments
                 SET status='canceled',failure_code='app_paused',
                     failure_summary='App was paused before this deployment started.',
                     finished_at=now()
                 WHERE id=ANY($1) AND status=ANY($2)",
            )
            .bind(&deployment_ids)
            .bind(deploy::ACTIVE_DEPLOYMENT_STATUSES)
            .execute(&mut *tx)
            .await
            .map_err(anyhow::Error::from)?;
            sqlx::query(
                "UPDATE deployment_builds
                 SET status='canceled',failure_code='app_paused',
                     failure_summary='App was paused before this deployment started.',
                     finished_at=now(),updated_at=now()
                 WHERE deployment_id=ANY($1)
                   AND status NOT IN ('succeeded','failed','canceled')",
            )
            .bind(&deployment_ids)
            .execute(&mut *tx)
            .await
            .map_err(anyhow::Error::from)?;
            sqlx::query(
                "UPDATE apps
                 SET pending_deployment_id=NULL,updated_at=now()
                 WHERE pending_deployment_id=ANY($1)",
            )
            .bind(&deployment_ids)
            .execute(&mut *tx)
            .await
            .map_err(anyhow::Error::from)?;
        }
        sqlx::query(
            "UPDATE agent_jobs
             SET cancel_requested_at=COALESCE(cancel_requested_at,clock_timestamp()),updated_at=now()
             WHERE app_id=$1
               AND job_type IN ('deploy','rollback','build','release')
               AND status IN ('claimed','running')",
        )
        .bind(app_id)
        .execute(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    } else {
        sqlx::query("DELETE FROM app_suspensions WHERE app_id=$1 AND reason=$2")
            .bind(app_id)
            .bind(reason)
            .execute(&mut *tx)
            .await
            .map_err(anyhow::Error::from)?;
    }
    let reasons = suspension_reasons_in_tx(&mut tx, app_id).await?;
    let suspended = !reasons.is_empty();
    sqlx::query(
        "UPDATE apps
         SET suspended_at=(
           SELECT MIN(created_at) FROM app_suspensions WHERE app_id=$1
         ), updated_at=now()
         WHERE id=$1",
    )
    .bind(app_id)
    .execute(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;
    let transitioned = (add && before == 0 && suspended) || (!add && before > 0 && !suspended);
    let job_id = if transitioned {
        match target {
            Some(target) => Some(
                enqueue_transition_job(
                    &mut tx,
                    app_id,
                    &target,
                    if suspended {
                        "suspend_app"
                    } else {
                        "resume_app"
                    },
                )
                .await?,
            ),
            None => None,
        }
    } else {
        None
    };
    sqlx::query(
        "INSERT INTO audit_events
           (actor_type,actor_id,event_type,app_id,job_id,metadata_json)
         VALUES ($1,$2,$3,$4,$5,jsonb_build_object('reason',$6,'suspended',$7))",
    )
    .bind(if actor == "owner" {
        "owner"
    } else {
        "operator"
    })
    .bind(actor)
    .bind(if add {
        "app_suspension_added"
    } else {
        "app_suspension_removed"
    })
    .bind(app_id)
    .bind(job_id)
    .bind(reason)
    .bind(suspended)
    .execute(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;
    tx.commit().await.map_err(anyhow::Error::from)?;
    Ok(SuspensionChange {
        app_id,
        suspended,
        reasons,
        job_id,
    })
}

async fn load_runtime_target(
    tx: &mut Transaction<'_, Postgres>,
    app_id: Uuid,
    owner_id: Option<Uuid>,
) -> Result<Option<RuntimeTarget>, ChangeError> {
    let row = sqlx::query(
        "SELECT a.server_id,a.current_deployment_id,a.container_port,a.health_path,a.domain,
                d.container_name,d.compose_project,d.published_port
         FROM apps a
         LEFT JOIN deployments d ON d.id=a.current_deployment_id
         WHERE a.id=$1 AND ($2::uuid IS NULL OR a.user_id=$2)
         FOR UPDATE OF a",
    )
    .bind(app_id)
    .bind(owner_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(anyhow::Error::from)?;
    let Some(row) = row else {
        return Err(ChangeError::NotFound);
    };
    let (Some(deployment_id), Some(container_name), Some(published_port)) = (
        row.get::<Option<Uuid>, _>("current_deployment_id"),
        row.get::<Option<String>, _>("container_name"),
        row.get::<Option<i32>, _>("published_port"),
    ) else {
        return Ok(None);
    };
    Ok(Some(RuntimeTarget {
        server_id: row.get("server_id"),
        deployment_id,
        container_name,
        compose_project: row.get("compose_project"),
        container_port: row.get("container_port"),
        published_port,
        health_path: row.get("health_path"),
        domain: row.get("domain"),
    }))
}

async fn suspension_reasons_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    app_id: Uuid,
) -> Result<Vec<String>, ChangeError> {
    let rows = sqlx::query_scalar::<_, String>(
        "SELECT reason FROM app_suspensions WHERE app_id=$1 ORDER BY created_at,reason",
    )
    .bind(app_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(rows)
}

async fn enqueue_transition_job(
    tx: &mut Transaction<'_, Postgres>,
    app_id: Uuid,
    target: &RuntimeTarget,
    job_type: &str,
) -> Result<Uuid, ChangeError> {
    let payload = serde_json::json!({
        "type": job_type,
        "app_id": app_id,
        "deployment_id": target.deployment_id,
        "container_name": target.container_name,
        "compose_project": target.compose_project,
        "container_port": target.container_port,
        "published_port": target.published_port,
        "health_path": target.health_path,
        "domain": target.domain,
        "route_key": format!("app-{app_id}"),
    });
    deploy::insert_agent_job_in_transaction(
        tx,
        target.server_id,
        Some(app_id),
        Some(target.deployment_id),
        job_type,
        payload,
        20,
    )
    .await
    .map_err(ChangeError::Database)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn db_suspension_reasons_stack_and_owner_resume_is_scoped() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        let user_id: Uuid = sqlx::query_scalar(
            "INSERT INTO users (github_id,login)
             VALUES (9851,'suspension-test-user')
             ON CONFLICT (github_id) DO UPDATE SET login=EXCLUDED.login
             RETURNING id",
        )
        .fetch_one(&state.db)
        .await
        .unwrap();
        let app_id: Uuid = sqlx::query_scalar(
            "INSERT INTO apps
               (user_id,server_id,name,repo_full_name,branch,container_port,health_path,domain)
             VALUES ($1,$2,$3,'owner/repo','main',3000,'/',$4)
             RETURNING id",
        )
        .bind(user_id)
        .bind(state.local_server_id)
        .bind(format!("suspension-test-{}", Uuid::new_v4()))
        .bind(format!("suspension-{}.local", Uuid::new_v4()))
        .fetch_one(&state.db)
        .await
        .unwrap();

        set_reason(&state, app_id, None, "billing_inactive", "test")
            .await
            .unwrap();
        set_reason(&state, app_id, Some(user_id), USER_REQUESTED, "owner")
            .await
            .unwrap();
        let change = clear_reason(&state, app_id, Some(user_id), USER_REQUESTED, "owner")
            .await
            .unwrap();
        assert!(change.suspended);
        assert_eq!(change.reasons, vec!["billing_inactive"]);

        sqlx::query("DELETE FROM apps WHERE id=$1")
            .bind(app_id)
            .execute(&state.db)
            .await
            .unwrap();
    }
}
