use crate::state::AppState;
use sqlx::Row;
use thiserror::Error;
use uuid::Uuid;

pub const BUILDER_CAPABILITY: &str = "builder";
pub const APP_RUNNER_CAPABILITY: &str = "app_runner";

#[derive(Debug, Error)]
pub enum ServerSelectionError {
    #[error("server is not available")]
    NotFound,
    #[error("server cannot run apps")]
    NotAppRunner,
    #[error("server is draining")]
    Draining,
    #[error("server app capacity is full")]
    Full,
    #[error("no app runner has capacity")]
    NoCapacity,
    #[error("failed to select app runner")]
    Database(#[from] sqlx::Error),
}

#[derive(Debug)]
struct AppRunnerCandidate {
    id: Uuid,
    capabilities: Vec<String>,
    draining: bool,
    max_concurrent_apps: i32,
    active_apps: i64,
}

impl AppRunnerCandidate {
    fn from_row(row: sqlx::postgres::PgRow) -> Self {
        Self {
            id: row.get("id"),
            capabilities: row.get("capabilities"),
            draining: row.get("draining"),
            max_concurrent_apps: row.get("max_concurrent_apps"),
            active_apps: row.get("active_apps"),
        }
    }

    fn validate(self) -> Result<Uuid, ServerSelectionError> {
        if !self
            .capabilities
            .iter()
            .any(|capability| capability == APP_RUNNER_CAPABILITY)
        {
            return Err(ServerSelectionError::NotAppRunner);
        }
        if self.draining {
            return Err(ServerSelectionError::Draining);
        }
        if self.active_apps >= i64::from(self.max_concurrent_apps) {
            return Err(ServerSelectionError::Full);
        }
        Ok(self.id)
    }
}

pub async fn select_app_runner(
    state: &AppState,
    requested_server_id: Option<Uuid>,
) -> Result<Uuid, ServerSelectionError> {
    match requested_server_id {
        Some(server_id) => select_requested_app_runner(state, server_id).await,
        None => select_available_app_runner(state).await,
    }
}

async fn select_requested_app_runner(
    state: &AppState,
    server_id: Uuid,
) -> Result<Uuid, ServerSelectionError> {
    let row = sqlx::query(
        "SELECT s.id,
                s.capabilities,
                s.draining,
                s.max_concurrent_apps,
                COUNT(a.id)::bigint AS active_apps
         FROM servers s
         LEFT JOIN apps a
           ON a.server_id=s.id
          AND a.current_deployment_id IS NOT NULL
         WHERE s.id=$1
         GROUP BY s.id",
    )
    .bind(server_id)
    .fetch_optional(&state.db)
    .await?;

    row.map(AppRunnerCandidate::from_row)
        .ok_or(ServerSelectionError::NotFound)?
        .validate()
}

async fn select_available_app_runner(state: &AppState) -> Result<Uuid, ServerSelectionError> {
    let row = sqlx::query(
        "SELECT s.id,
                s.capabilities,
                s.draining,
                s.max_concurrent_apps,
                COUNT(a.id)::bigint AS active_apps
         FROM servers s
         LEFT JOIN apps a
           ON a.server_id=s.id
          AND a.current_deployment_id IS NOT NULL
         WHERE s.capabilities @> ARRAY[$1]::TEXT[]
           AND s.draining=false
         GROUP BY s.id
         HAVING COUNT(a.id)::bigint < s.max_concurrent_apps
         ORDER BY COUNT(a.id) ASC, s.created_at ASC, s.id ASC
         LIMIT 1",
    )
    .bind(APP_RUNNER_CAPABILITY)
    .fetch_optional(&state.db)
    .await?;

    row.map(AppRunnerCandidate::from_row)
        .map(|candidate| candidate.id)
        .ok_or(ServerSelectionError::NoCapacity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn db_select_app_runner_rejects_draining_server() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_capacity_db(&state).await;
        sqlx::query("UPDATE servers SET draining=true WHERE id=$1")
            .bind(state.local_server_id)
            .execute(&state.db)
            .await
            .unwrap();

        let err = select_app_runner(&state, Some(state.local_server_id))
            .await
            .unwrap_err();

        assert!(matches!(err, ServerSelectionError::Draining));
    }

    #[tokio::test]
    async fn db_select_app_runner_chooses_least_loaded_runner() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_capacity_db(&state).await;
        let user_id = insert_capacity_user(&state).await;
        let busy_server_id = state.local_server_id;
        let idle_server_id = insert_server(&state, user_id, "idle-runner").await;
        insert_active_app(&state, user_id, busy_server_id, "busy-app").await;

        let selected = select_app_runner(&state, None).await.unwrap();

        assert_eq!(selected, idle_server_id);
    }

    #[tokio::test]
    async fn db_select_app_runner_rejects_full_requested_server() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_capacity_db(&state).await;
        let user_id = insert_capacity_user(&state).await;
        sqlx::query("UPDATE servers SET max_concurrent_apps=1 WHERE id=$1")
            .bind(state.local_server_id)
            .execute(&state.db)
            .await
            .unwrap();
        insert_active_app(&state, user_id, state.local_server_id, "full-app").await;

        let err = select_app_runner(&state, Some(state.local_server_id))
            .await
            .unwrap_err();

        assert!(matches!(err, ServerSelectionError::Full));
    }

    async fn reset_capacity_db(state: &AppState) {
        sqlx::query(
            "TRUNCATE app_screenshots, app_resource_snapshots, agent_jobs, deployments, app_env_vars, apps CASCADE",
        )
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query("DELETE FROM users WHERE github_id BETWEEN 9700 AND 9799")
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query(
            "DELETE FROM servers
             WHERE id <> $1
               AND name LIKE 'capacity-%'",
        )
        .bind(state.local_server_id)
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE servers
             SET capabilities=ARRAY['builder','app_runner']::TEXT[],
                 draining=false,
                 max_concurrent_apps=8,
                 max_concurrent_builds=1
             WHERE id=$1",
        )
        .bind(state.local_server_id)
        .execute(&state.db)
        .await
        .unwrap();
    }

    async fn insert_capacity_user(state: &AppState) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO users (github_id, login)
             VALUES (9701, 'capacity-user')
             ON CONFLICT (github_id) DO UPDATE SET login=EXCLUDED.login
             RETURNING id",
        )
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    async fn insert_server(state: &AppState, user_id: Uuid, name: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO servers
               (user_id,name,kind,status,capabilities,draining,max_concurrent_apps,max_concurrent_builds)
             VALUES ($1,$2,'local','online',ARRAY['app_runner']::TEXT[],false,8,1)
             RETURNING id",
        )
        .bind(user_id)
        .bind(format!("capacity-{name}"))
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    async fn insert_active_app(state: &AppState, user_id: Uuid, server_id: Uuid, name: &str) {
        let app_id: Uuid = sqlx::query_scalar(
            "INSERT INTO apps
               (user_id,server_id,name,repo_full_name,branch,container_port,health_path,domain,runtime_kind,root_directory,public_exposure,auto_deploy,current_deployment_id)
             VALUES ($1,$2,$3,'owner/repo','main',3000,'/',$4,'container','.',false,false,'00000000-0000-0000-0000-000000000001')
             RETURNING id",
        )
        .bind(user_id)
        .bind(server_id)
        .bind(name)
        .bind(format!("{name}.example.test"))
        .fetch_one(&state.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO deployments
               (id,app_id,server_id,status,commit_sha,started_at,finished_at,runtime_kind)
             VALUES ('00000000-0000-0000-0000-000000000001',$1,$2,'success','abc',now(),now(),'container')",
        )
        .bind(app_id)
        .bind(server_id)
        .execute(&state.db)
        .await
        .unwrap();
    }
}
