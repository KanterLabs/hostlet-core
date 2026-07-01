use crate::state::AppState;
use sqlx::Row;
use thiserror::Error;
use uuid::Uuid;

pub const BUILDER_CAPABILITY: &str = "builder";
pub const APP_RUNNER_CAPABILITY: &str = "app_runner";

/// SQL predicate (on table alias `a` over `apps`) selecting the apps that occupy
/// a runner slot: either currently live (`current_deployment_id` set) or with an
/// in-flight deploy job (a `deploy` `agent_jobs` row that is queued/claimed/
/// running). Placement here and the deploy-time re-check in `deploy.rs` count
/// with this same model so an app created but not yet deployed still reserves a
/// slot once its deploy is enqueued — otherwise N apps could be placed on, and
/// later all deploy onto, a server with room for one.
pub(crate) const APP_OCCUPIES_SLOT: &str = "(a.current_deployment_id IS NOT NULL \
     OR EXISTS (SELECT 1 FROM agent_jobs j \
                WHERE j.app_id = a.id \
                  AND j.job_type = 'deploy' \
                  AND j.status IN ('queued', 'claimed', 'running')))";

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
    kind: String,
    status: String,
    capabilities: Vec<String>,
    draining: bool,
    max_concurrent_apps: i32,
    active_apps: i64,
}

impl AppRunnerCandidate {
    fn from_row(row: sqlx::postgres::PgRow) -> Self {
        Self {
            id: row.get("id"),
            kind: row.get("kind"),
            status: row.get("status"),
            capabilities: row.get("capabilities"),
            draining: row.get("draining"),
            max_concurrent_apps: row.get("max_concurrent_apps"),
            active_apps: row.get("active_apps"),
        }
    }

    fn validate(self) -> Result<Uuid, ServerSelectionError> {
        if self.kind != "local" {
            return Err(ServerSelectionError::NotFound);
        }
        if self.status != "online" {
            return Err(ServerSelectionError::NotFound);
        }
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
    let row = sqlx::query(&format!(
        "SELECT s.id,
                s.kind,
                s.status,
                s.capabilities,
                s.draining,
                s.max_concurrent_apps,
                cap.active_apps
         FROM servers s
         CROSS JOIN LATERAL (
           SELECT COUNT(*)::bigint AS active_apps
           FROM apps a
           WHERE a.server_id=s.id AND {APP_OCCUPIES_SLOT}
         ) cap
         WHERE s.id=$1"
    ))
    .bind(server_id)
    .fetch_optional(&state.db)
    .await?;

    row.map(AppRunnerCandidate::from_row)
        .ok_or(ServerSelectionError::NotFound)?
        .validate()
}

async fn select_available_app_runner(state: &AppState) -> Result<Uuid, ServerSelectionError> {
    let row = sqlx::query(&format!(
        "SELECT s.id,
                s.kind,
                s.status,
                s.capabilities,
                s.draining,
                s.max_concurrent_apps,
                cap.active_apps
         FROM servers s
         CROSS JOIN LATERAL (
           SELECT COUNT(*)::bigint AS active_apps
           FROM apps a
           WHERE a.server_id=s.id AND {APP_OCCUPIES_SLOT}
         ) cap
         WHERE s.capabilities @> ARRAY[$1]::TEXT[]
           AND s.kind='local'
           AND s.status='online'
           AND s.draining=false
           AND cap.active_apps < s.max_concurrent_apps
         ORDER BY cap.active_apps ASC, s.created_at ASC, s.id ASC
         LIMIT 1"
    ))
    .bind(APP_RUNNER_CAPABILITY)
    .fetch_optional(&state.db)
    .await?;

    row.map(AppRunnerCandidate::from_row)
        .map(|candidate| candidate.id)
        .ok_or(ServerSelectionError::NoCapacity)
}

/// Deploy-time server-capacity gate.
///
/// [`select_app_runner`] reserves a slot at app-create time from the count of
/// apps already live on a server, so apps created before their first deploy don't
/// count and N of them can be assigned to a one-slot server. This closes that
/// gap: it locks the app's assigned server row and counts the *other* apps on it
/// that occupy a slot (live, or with an in-flight deploy job) under the exact
/// same model as placement ([`APP_OCCUPIES_SLOT`]). If that already fills
/// `max_concurrent_apps`, the deploy is refused, so a server can never run more
/// concurrent apps than its cap. The app being deployed is excluded from its own
/// count so any redeploy (including of the only app on a full server) still
/// works. The `FOR UPDATE` lock serializes the *capacity counts* of concurrent
/// deploys targeting the same server (the lock is held only for the duration of
/// this count). Note this fully closes the reported gap — creating N idle apps
/// and then deploying them can no longer exceed the cap — but it does not by
/// itself serialize the whole enqueue: an app first *occupies* a slot when its
/// `deploy` `agent_jobs` row is written, which happens shortly after this check
/// returns (outside the lock). Two truly simultaneous deploys of distinct idle
/// apps can therefore still transiently oversubscribe by up to the number of
/// in-flight enqueues; the per-app active-deployment unique index bounds
/// duplication for a single app. Reserving the slot inside this lock would
/// require threading the transaction through the enqueue path (tracked as a
/// follow-up); the current check is a strict, low-risk improvement over the
/// prior no-check behavior.
pub(crate) async fn ensure_server_has_capacity(
    state: &AppState,
    server_id: Uuid,
    app_id: Uuid,
) -> anyhow::Result<()> {
    let mut tx = state.db.begin().await?;
    let max_concurrent_apps: Option<i32> =
        sqlx::query_scalar("SELECT max_concurrent_apps FROM servers WHERE id=$1 FOR UPDATE")
            .bind(server_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(max_concurrent_apps) = max_concurrent_apps else {
        anyhow::bail!("the app's assigned server is no longer available");
    };
    let occupied: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*)::bigint FROM apps a \
         WHERE a.server_id=$1 AND a.id <> $2 AND {APP_OCCUPIES_SLOT}"
    ))
    .bind(server_id)
    .bind(app_id)
    .fetch_one(&mut *tx)
    .await?;
    if occupied >= i64::from(max_concurrent_apps) {
        // Drop `tx` (read-only) to release the `FOR UPDATE` lock before returning.
        anyhow::bail!(
            "the app's assigned server is at capacity ({max_concurrent_apps} concurrent apps); \
             free a slot or wait for a running deployment to finish before deploying"
        );
    }
    tx.commit().await?;
    Ok(())
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
    async fn db_select_app_runner_rejects_remote_or_offline_server() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_capacity_db(&state).await;
        let user_id = insert_capacity_user(&state).await;
        let remote_id =
            insert_server_with_kind_status(&state, user_id, "remote-runner", "remote", "online")
                .await;
        let offline_id =
            insert_server_with_kind_status(&state, user_id, "offline-runner", "local", "offline")
                .await;

        assert!(matches!(
            select_app_runner(&state, Some(remote_id))
                .await
                .unwrap_err(),
            ServerSelectionError::NotFound
        ));
        assert!(matches!(
            select_app_runner(&state, Some(offline_id))
                .await
                .unwrap_err(),
            ServerSelectionError::NotFound
        ));
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

    #[tokio::test]
    async fn db_select_app_runner_counts_in_flight_deploy_job_as_occupied() {
        // An app with a queued deploy job but no current deployment must still
        // occupy a slot, so a one-slot server reads as full on both the requested
        // and the auto-selected path. This keeps placement in step with the
        // deploy-time re-check in `ensure_server_has_capacity`.
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
        insert_inflight_app(&state, user_id, state.local_server_id, "inflight-app").await;

        let requested_err = select_app_runner(&state, Some(state.local_server_id))
            .await
            .unwrap_err();
        assert!(matches!(requested_err, ServerSelectionError::Full));

        let auto_err = select_app_runner(&state, None).await.unwrap_err();
        assert!(matches!(auto_err, ServerSelectionError::NoCapacity));
    }

    #[tokio::test]
    async fn db_ensure_server_has_capacity_blocks_extra_app_when_slot_reserved() {
        // N apps created back-to-back on a one-slot server, none with a current
        // deployment, must not all be able to deploy: once one reserves the slot
        // with a queued deploy job the others are refused at enqueue time.
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_capacity_db(&state).await;
        let user_id = insert_capacity_user(&state).await;
        let server_id = state.local_server_id;
        sqlx::query("UPDATE servers SET max_concurrent_apps=1 WHERE id=$1")
            .bind(server_id)
            .execute(&state.db)
            .await
            .unwrap();
        let app_b = insert_idle_app(&state, user_id, server_id, "cap-b").await;

        // Nothing in flight yet: the single free slot accepts a deploy.
        ensure_server_has_capacity(&state, server_id, app_b)
            .await
            .expect("a free one-slot server should accept a deploy");

        // Another app reserves the slot with a queued deploy job (still no current
        // deployment) — exactly the state that used to slip past capacity checks.
        let app_a = insert_inflight_app(&state, user_id, server_id, "cap-a").await;

        // The extra app is now refused: the reserved slot fills the server even
        // though no app has a current deployment.
        let err = ensure_server_has_capacity(&state, server_id, app_b)
            .await
            .expect_err("second app must be blocked when the slot is reserved");
        assert!(
            err.to_string().contains("at capacity"),
            "unexpected error: {err}"
        );

        // The reserving app is excluded from its own count, so it can still deploy
        // into the slot it holds (a redeploy must not be self-blocked).
        ensure_server_has_capacity(&state, server_id, app_a)
            .await
            .expect("an app must not be blocked by its own in-flight deploy job");
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
        insert_server_with_kind_status(state, user_id, name, "local", "online").await
    }

    async fn insert_server_with_kind_status(
        state: &AppState,
        user_id: Uuid,
        name: &str,
        kind: &str,
        status: &str,
    ) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO servers
               (user_id,name,kind,status,capabilities,draining,max_concurrent_apps,max_concurrent_builds)
             VALUES ($1,$2,$3,$4,ARRAY['app_runner']::TEXT[],false,8,1)
             RETURNING id",
        )
        .bind(user_id)
        .bind(format!("capacity-{name}"))
        .bind(kind)
        .bind(status)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    /// Inserts an app with no current deployment and no deploy job — the state an
    /// app sits in between creation and its first deploy.
    async fn insert_idle_app(state: &AppState, user_id: Uuid, server_id: Uuid, name: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO apps
               (user_id,server_id,name,repo_full_name,branch,container_port,health_path,domain,runtime_kind,root_directory,public_exposure,auto_deploy)
             VALUES ($1,$2,$3,'owner/repo','main',3000,'/',$4,'container','.',false,false)
             RETURNING id",
        )
        .bind(user_id)
        .bind(server_id)
        .bind(name)
        .bind(format!("{name}.example.test"))
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    /// Inserts an app with an in-flight (`queued`) deploy job but *no* current
    /// deployment — the state that used to slip past the capacity count.
    async fn insert_inflight_app(
        state: &AppState,
        user_id: Uuid,
        server_id: Uuid,
        name: &str,
    ) -> Uuid {
        let app_id = insert_idle_app(state, user_id, server_id, name).await;
        sqlx::query(
            "INSERT INTO agent_jobs (server_id,app_id,job_type,status,payload_json)
             VALUES ($1,$2,'deploy','queued','{}'::jsonb)",
        )
        .bind(server_id)
        .bind(app_id)
        .execute(&state.db)
        .await
        .unwrap();
        app_id
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
