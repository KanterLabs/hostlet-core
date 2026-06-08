use super::apps::request_context_or_response;
use super::*;

pub async fn cleanup_preview(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user_id = match request_context_or_response(&headers, &state).await {
        Ok(context) => context.user_id,
        Err(response) => return response,
    };
    match cleanup_plan(&state, user_id).await {
        Ok(plan) => Json(plan).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "failed to build cleanup preview");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn run_cleanup(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let context = match request_context_or_response(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    run_cleanup_inner(&state, Some(context.user_id)).await
}

pub(in crate::web) async fn run_cleanup_inner(
    state: &AppState,
    user_id: Option<Uuid>,
) -> axum::response::Response {
    let plan = match cleanup_plan(state, user_id.unwrap_or_else(Uuid::nil)).await {
        Ok(plan) => plan,
        Err(err) => {
            tracing::warn!(error = %err, "failed to build cleanup plan");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let db_deleted = match apply_database_cleanup(state).await {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(error = %err, "database cleanup failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let job_id = if let Some(server_id) = plan.local_server_id {
        match deploy::enqueue_agent_job(
            state,
            server_id,
            None,
            None,
            "docker_cleanup",
            serde_json::json!({
                "type": "docker_cleanup",
                "keep_containers": plan.keep_containers,
                "keep_images": plan.keep_images,
                "dry_run": false,
            }),
            50,
        )
        .await
        {
            Ok(job_id) => Some(job_id),
            Err(err) => {
                tracing::warn!(error = %err, "failed to enqueue Docker cleanup job");
                None
            }
        }
    } else {
        None
    };
    record_audit_event(
        state,
        AuditEventInput {
            actor_type: user_id.map(|_| "owner").unwrap_or("cli"),
            actor_id: user_id.map(|id| id.to_string()),
            event_type: "cleanup_requested",
            app_id: None,
            deployment_id: None,
            job_id,
            metadata: serde_json::json!({"databaseDeleted": db_deleted}),
        },
    )
    .await;
    Json(serde_json::json!({
        "databaseDeleted": db_deleted,
        "dockerCleanupJobId": job_id,
    }))
    .into_response()
}

#[derive(Serialize)]
pub(in crate::web) struct CleanupPlan {
    retention: CleanupRetention,
    database: CleanupDatabasePreview,
    docker: CleanupDockerPreview,
    #[serde(skip_serializing)]
    local_server_id: Option<Uuid>,
    #[serde(skip_serializing)]
    keep_containers: Vec<String>,
    #[serde(skip_serializing)]
    keep_images: Vec<String>,
}

#[derive(Serialize)]
struct CleanupRetention {
    deployment_log_days: i64,
    deployments_per_app: i64,
    health_event_days: i64,
    health_events_per_app: i64,
    resource_snapshot_days: i64,
    resource_snapshots_per_app: i64,
    webhook_event_days: i64,
    completed_agent_job_days: i64,
    failed_agent_job_days: i64,
}

#[derive(Serialize)]
struct CleanupDatabasePreview {
    deployment_logs: i64,
    health_events: i64,
    resource_snapshots: i64,
    webhook_events: i64,
    completed_agent_jobs: i64,
    failed_agent_jobs: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CleanupDockerPreview {
    keep_containers: usize,
    keep_images: usize,
    stale_deployment_containers: i64,
    job_will_run: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CleanupDatabaseDeleted {
    deployment_logs: u64,
    health_events: u64,
    resource_snapshots: u64,
    webhook_events: u64,
    completed_agent_jobs: u64,
    failed_agent_jobs: u64,
}

const RETENTION: CleanupRetention = CleanupRetention {
    deployment_log_days: 30,
    deployments_per_app: 20,
    health_event_days: 7,
    health_events_per_app: 500,
    resource_snapshot_days: 7,
    resource_snapshots_per_app: 1000,
    webhook_event_days: 14,
    completed_agent_job_days: 30,
    failed_agent_job_days: 90,
};

const LIVE_DOCKER_DEPLOYMENT_STATUSES: &[&str] = &["success", "rolled_back"];

pub(in crate::web) async fn cleanup_plan(
    state: &AppState,
    _user_id: Uuid,
) -> anyhow::Result<CleanupPlan> {
    let database = CleanupDatabasePreview {
        deployment_logs: cleanup_count(state, &cleanup_deployment_logs_sql()).await?,
        health_events: cleanup_count(state, &HEALTH_EVENTS_RULE.count_sql()).await?,
        resource_snapshots: cleanup_count(state, &RESOURCE_SNAPSHOTS_RULE.count_sql()).await?,
        webhook_events: cleanup_count(state, &WEBHOOK_EVENTS_RULE.count_sql()).await?,
        completed_agent_jobs: cleanup_count(state, &COMPLETED_AGENT_JOBS_RULE.count_sql()).await?,
        failed_agent_jobs: cleanup_count(state, &FAILED_AGENT_JOBS_RULE.count_sql()).await?,
    };
    let active_statuses = deployment_status_strings(deploy::ACTIVE_DEPLOYMENT_STATUSES);
    let protected_statuses = docker_cleanup_keep_statuses();
    let keep_rows = sqlx::query(
        r#"
        WITH candidates AS (
          SELECT d.id,
                 d.container_name,
                 d.image_tag,
                 d.status,
                 row_number() OVER (
                   PARTITION BY d.app_id
                   ORDER BY
                     CASE WHEN a.current_deployment_id=d.id THEN 0 ELSE 1 END,
                     d.finished_at DESC NULLS LAST,
                     d.created_at DESC
                 ) AS rn
          FROM deployments d
          JOIN apps a ON a.id=d.app_id
          WHERE d.status = ANY($1)
        )
        SELECT container_name,image_tag
        FROM candidates
        WHERE status = ANY($2) OR rn <= 2
        "#,
    )
    .bind(&protected_statuses)
    .bind(&active_statuses)
    .fetch_all(&state.db)
    .await?;
    let mut keep_containers = keep_rows
        .iter()
        .filter_map(|row| row.get::<Option<String>, _>("container_name"))
        .collect::<Vec<_>>();
    keep_containers.sort();
    keep_containers.dedup();
    let mut keep_images = keep_rows
        .iter()
        .filter_map(|row| row.get::<Option<String>, _>("image_tag"))
        .collect::<Vec<_>>();
    keep_images.sort();
    keep_images.dedup();
    let stale_deployment_containers = sqlx::query_scalar::<_, i64>(
        r#"
        WITH candidates AS (
          SELECT d.id,
                 d.status,
                 row_number() OVER (
                   PARTITION BY d.app_id
                   ORDER BY
                     CASE WHEN a.current_deployment_id=d.id THEN 0 ELSE 1 END,
                     d.finished_at DESC NULLS LAST,
                     d.created_at DESC
                 ) AS rn
          FROM deployments d
          JOIN apps a ON a.id=d.app_id
          WHERE d.status = ANY($1)
        ),
        protected AS (
          SELECT id
          FROM candidates
          WHERE status = ANY($2) OR rn <= 2
        )
        SELECT count(*)::bigint
        FROM deployments d
        WHERE d.container_name IS NOT NULL
          AND NOT EXISTS (SELECT 1 FROM protected p WHERE p.id=d.id)
        "#,
    )
    .bind(&protected_statuses)
    .bind(&active_statuses)
    .fetch_one(&state.db)
    .await?;
    let local_server_id =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM servers WHERE kind='local' LIMIT 1")
            .fetch_optional(&state.db)
            .await?;
    Ok(CleanupPlan {
        retention: RETENTION,
        database,
        docker: CleanupDockerPreview {
            keep_containers: keep_containers.len(),
            keep_images: keep_images.len(),
            stale_deployment_containers,
            job_will_run: local_server_id.is_some(),
        },
        local_server_id,
        keep_containers,
        keep_images,
    })
}

fn deployment_status_strings(statuses: &[&str]) -> Vec<String> {
    statuses
        .iter()
        .map(|status| (*status).to_string())
        .collect()
}

fn docker_cleanup_keep_statuses() -> Vec<String> {
    let mut statuses = deployment_status_strings(deploy::ACTIVE_DEPLOYMENT_STATUSES);
    statuses.extend(deployment_status_strings(LIVE_DOCKER_DEPLOYMENT_STATUSES));
    statuses
}

async fn cleanup_count(state: &AppState, sql: &str) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(sql).fetch_one(&state.db).await?)
}

async fn cleanup_delete(state: &AppState, sql: &str) -> anyhow::Result<u64> {
    Ok(sqlx::query(sql).execute(&state.db).await?.rows_affected())
}

async fn apply_database_cleanup(state: &AppState) -> anyhow::Result<CleanupDatabaseDeleted> {
    Ok(CleanupDatabaseDeleted {
        deployment_logs: cleanup_delete(state, &delete_deployment_logs_sql()).await?,
        health_events: cleanup_delete(state, &HEALTH_EVENTS_RULE.delete_sql()).await?,
        resource_snapshots: cleanup_delete(state, &RESOURCE_SNAPSHOTS_RULE.delete_sql()).await?,
        webhook_events: cleanup_delete(state, &WEBHOOK_EVENTS_RULE.delete_sql()).await?,
        completed_agent_jobs: cleanup_delete(state, &COMPLETED_AGENT_JOBS_RULE.delete_sql())
            .await?,
        failed_agent_jobs: cleanup_delete(state, &FAILED_AGENT_JOBS_RULE.delete_sql()).await?,
    })
}

/// A retention rule expressed as a single SQL predicate that selects the rows
/// to purge from one table. The matching count and delete statements are then
/// derived from this one definition so a rule change only edits one place.
struct RetentionRule {
    /// `FROM`-clause target including its alias, e.g. `app_health_events e`.
    from: &'static str,
    /// The shared `WHERE` body identifying the rows to remove.
    predicate: &'static str,
}

impl RetentionRule {
    /// `SELECT count(*)::bigint FROM <from> WHERE <predicate>`.
    fn count_sql(&self) -> String {
        format!(
            "SELECT count(*)::bigint\nFROM {}\nWHERE {}\n",
            self.from, self.predicate
        )
    }

    /// `DELETE FROM <from> WHERE <predicate>`.
    fn delete_sql(&self) -> String {
        format!("DELETE FROM {}\nWHERE {}\n", self.from, self.predicate)
    }
}

/// Deployment-log retention cannot share one `from`/`predicate` because the
/// count uses a `JOIN` while the delete uses `USING`; the shared `WHERE` body
/// still lives in one place.
const DEPLOYMENT_LOGS_PREDICATE: &str = r#"l.created_at < now() - interval '30 days'
  AND d.id NOT IN (
    SELECT id FROM (
      SELECT id,row_number() OVER (PARTITION BY app_id ORDER BY created_at DESC) AS rn
      FROM deployments
    ) ranked WHERE rn <= 20
  )
  AND NOT EXISTS (
    SELECT 1 FROM agent_jobs j
    WHERE j.deployment_id=d.id AND j.status IN ('queued','claimed','running')
  )"#;

fn cleanup_deployment_logs_sql() -> String {
    format!(
        "SELECT count(*)::bigint\nFROM deployment_logs l\nJOIN deployments d ON d.id=l.deployment_id\nWHERE {DEPLOYMENT_LOGS_PREDICATE}\n"
    )
}

fn delete_deployment_logs_sql() -> String {
    format!(
        "DELETE FROM deployment_logs l\nUSING deployments d\nWHERE d.id=l.deployment_id\n  AND {DEPLOYMENT_LOGS_PREDICATE}\n"
    )
}

const HEALTH_EVENTS_RULE: RetentionRule = RetentionRule {
    from: "app_health_events e",
    predicate: r#"e.created_at < now() - interval '7 days'
   OR e.id IN (
      SELECT id FROM (
        SELECT id,row_number() OVER (PARTITION BY app_id ORDER BY created_at DESC) AS rn
        FROM app_health_events
      ) ranked WHERE rn > 500
   )"#,
};

const RESOURCE_SNAPSHOTS_RULE: RetentionRule = RetentionRule {
    from: "app_resource_snapshots s",
    predicate: r#"s.sampled_at < now() - interval '7 days'
  AND NOT EXISTS (
    SELECT 1 FROM deployments d
    JOIN apps a ON a.current_deployment_id=d.id
    WHERE d.container_name=s.container_name
  )"#,
};

const WEBHOOK_EVENTS_RULE: RetentionRule = RetentionRule {
    from: "webhook_events e",
    predicate: "e.created_at < now() - interval '14 days'",
};

const COMPLETED_AGENT_JOBS_RULE: RetentionRule = RetentionRule {
    from: "agent_jobs j",
    predicate: r#"j.status IN ('success','cancelled')
  AND COALESCE(j.finished_at,j.updated_at,j.created_at) < now() - interval '30 days'"#,
};

const FAILED_AGENT_JOBS_RULE: RetentionRule = RetentionRule {
    from: "agent_jobs j",
    predicate: r#"j.status IN ('failed','expired')
  AND COALESCE(j.finished_at,j.updated_at,j.created_at) < now() - interval '90 days'"#,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn db_cleanup_plan_marks_failed_deployment_containers_stale() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_cleanup_db(&state).await;
        let user_id = insert_cleanup_user(&state).await;
        let app_id = insert_cleanup_app(&state, user_id).await;
        let current_deployment = insert_cleanup_deployment(
            &state,
            app_id,
            "success",
            "hostlet/app-current:latest",
            "hostlet-app-current",
        )
        .await;
        insert_cleanup_deployment(
            &state,
            app_id,
            "health_checking",
            "hostlet/app-active:latest",
            "hostlet-app-active",
        )
        .await;
        insert_cleanup_deployment(
            &state,
            app_id,
            "failed",
            "hostlet/app-failed:latest",
            "hostlet-app-failed",
        )
        .await;
        sqlx::query("UPDATE apps SET current_deployment_id=$1 WHERE id=$2")
            .bind(current_deployment)
            .bind(app_id)
            .execute(&state.db)
            .await
            .unwrap();

        let plan = cleanup_plan(&state, user_id).await.unwrap();

        assert_eq!(plan.docker.stale_deployment_containers, 1);
        assert_eq!(
            plan.keep_containers,
            vec!["hostlet-app-active", "hostlet-app-current"]
        );
        assert_eq!(
            plan.keep_images,
            vec!["hostlet/app-active:latest", "hostlet/app-current:latest"]
        );
        assert!(!plan.keep_containers.contains(&"hostlet-app-failed".into()));
        assert!(!plan
            .keep_images
            .contains(&"hostlet/app-failed:latest".into()));
    }

    async fn reset_cleanup_db(state: &AppState) {
        sqlx::query(
            "TRUNCATE deployment_logs, app_health_events, app_health_snapshots, app_resource_snapshots, agent_jobs,
             deployments, app_env_vars, apps, users CASCADE",
        )
        .execute(&state.db)
        .await
        .unwrap();
    }

    async fn insert_cleanup_user(state: &AppState) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO users (github_id, login) VALUES ($1,'cleanup-user') RETURNING id",
        )
        .bind(9_090_001_i64)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    async fn insert_cleanup_app(state: &AppState, user_id: Uuid) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO apps
               (user_id,server_id,name,repo_full_name,branch,container_port,health_path,domain,runtime_kind,root_directory,public_exposure,auto_deploy)
             VALUES ($1,$2,'cleanup-app','hostlet-ci/node-hello','main',3000,'/health','cleanup.example.test','single','.',true,false)
             RETURNING id",
        )
        .bind(user_id)
        .bind(state.local_server_id)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    async fn insert_cleanup_deployment(
        state: &AppState,
        app_id: Uuid,
        status: &str,
        image_tag: &str,
        container_name: &str,
    ) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO deployments
               (app_id,server_id,status,commit_sha,image_tag,container_name,started_at,finished_at,runtime_kind)
             VALUES ($1,$2,$3,'HEAD',$4,$5,now(),now(),'single')
             RETURNING id",
        )
        .bind(app_id)
        .bind(state.local_server_id)
        .bind(status)
        .bind(image_tag)
        .bind(container_name)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }
}
