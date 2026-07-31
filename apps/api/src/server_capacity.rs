use crate::state::AppState;
use hostlet_contracts::HostResourceSnapshot;
use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use std::{collections::HashSet, time::Duration};
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
pub(crate) const APP_OCCUPIES_SLOT: &str =
    "((a.current_deployment_id IS NOT NULL AND a.suspended_at IS NULL) \
     OR EXISTS (SELECT 1 FROM agent_jobs j \
                WHERE j.app_id = a.id \
                  AND j.job_type IN ('deploy','rollback') \
                  AND j.status IN ('queued', 'claimed', 'running') \
                  AND j.payload_json->>'capacity_reserved'='true'))";

const CAPACITY_SCHEDULER_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CapacityDecision {
    Admitted,
    Waiting(&'static str),
}

#[derive(Clone, Copy, Debug, Default)]
struct RuntimeDemand {
    memory_mb: i64,
    services: i64,
}

impl std::ops::AddAssign for RuntimeDemand {
    fn add_assign(&mut self, rhs: Self) {
        self.memory_mb = self.memory_mb.saturating_add(rhs.memory_mb);
        self.services = self.services.saturating_add(rhs.services);
    }
}

struct ServerBudget {
    max_concurrent_apps: i32,
    max_concurrent_builds: i32,
    max_runtime_memory_mb: Option<i32>,
    max_running_services: Option<i32>,
    min_available_memory_mb: Option<i32>,
    min_disk_free_mb: Option<i32>,
    snapshot: Option<HostResourceSnapshot>,
    snapshot_fresh: bool,
}

struct CapacityApp {
    id: Uuid,
    current: bool,
    suspended: bool,
    demand: RuntimeDemand,
}

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

fn runtime_demand(runtime_config: &Value) -> RuntimeDemand {
    let explicit_memory = runtime_config
        .pointer("/capacity/reservedMemoryMb")
        .and_then(Value::as_i64)
        .filter(|value| (32..=65_536).contains(value));
    let explicit_services = runtime_config
        .pointer("/capacity/reservedServiceCount")
        .and_then(Value::as_i64)
        .filter(|value| (1..=64).contains(value));
    let add_ons = runtime_config
        .pointer("/compose/addOns")
        .and_then(Value::as_array)
        .map(|items| items.len() as i64)
        .unwrap_or(0);
    let generated_extra = i64::from(runtime_config.pointer("/generatedTopology").is_some());
    let inferred_services = 1_i64
        .saturating_add(add_ons)
        .saturating_add(generated_extra);
    RuntimeDemand {
        memory_mb: explicit_memory.unwrap_or_else(|| {
            128_i64.saturating_add(inferred_services.saturating_sub(1).saturating_mul(96))
        }),
        services: explicit_services.unwrap_or(inferred_services),
    }
}

fn server_snapshot(row: &sqlx::postgres::PgRow) -> Option<HostResourceSnapshot> {
    row.get::<Option<Value>, _>("resource_snapshot_json")
        .and_then(|value| serde_json::from_value(value).ok())
}

/// Lock one server and decide whether a deploy/rollback may become claimable.
/// The caller must persist `capacity_reserved=true` before committing the same
/// transaction; subsequent callers then include that candidate in their sums.
pub(crate) async fn capacity_decision_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    server_id: Uuid,
    app_id: Uuid,
    exclude_job_id: Option<Uuid>,
) -> anyhow::Result<CapacityDecision> {
    let server = sqlx::query(
        "SELECT max_concurrent_apps,max_concurrent_builds,
                max_runtime_memory_mb,max_running_services,
                min_available_memory_mb,min_disk_free_mb,
                resource_snapshot_json,
                resource_snapshot_at > now() - interval '60 seconds' AS snapshot_fresh
         FROM servers WHERE id=$1 FOR UPDATE",
    )
    .bind(server_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| anyhow::anyhow!("the app's assigned server is no longer available"))?;
    let budget = ServerBudget {
        max_concurrent_apps: server.get("max_concurrent_apps"),
        max_concurrent_builds: server.get("max_concurrent_builds"),
        max_runtime_memory_mb: server.get("max_runtime_memory_mb"),
        max_running_services: server.get("max_running_services"),
        min_available_memory_mb: server.get("min_available_memory_mb"),
        min_disk_free_mb: server.get("min_disk_free_mb"),
        snapshot: server_snapshot(&server),
        snapshot_fresh: server
            .get::<Option<bool>, _>("snapshot_fresh")
            .unwrap_or(false),
    };
    let app_rows = sqlx::query(
        "SELECT id,current_deployment_id IS NOT NULL AS current,
                suspended_at IS NOT NULL AS suspended,runtime_config
         FROM apps WHERE server_id=$1",
    )
    .bind(server_id)
    .fetch_all(&mut **tx)
    .await?;
    let apps = app_rows
        .into_iter()
        .map(|row| CapacityApp {
            id: row.get("id"),
            current: row.get("current"),
            suspended: row.get("suspended"),
            demand: runtime_demand(&row.get::<Value, _>("runtime_config")),
        })
        .collect::<Vec<_>>();
    let candidate = apps
        .iter()
        .find(|app| app.id == app_id)
        .ok_or_else(|| anyhow::anyhow!("app no longer exists"))?;
    let reserved_jobs = sqlx::query(
        "SELECT id,app_id
         FROM agent_jobs
         WHERE server_id=$1
           AND ($2::uuid IS NULL OR id <> $2)
           AND job_type IN ('deploy','rollback')
           AND status IN ('queued','claimed','running')
           AND payload_json->>'capacity_reserved'='true'",
    )
    .bind(server_id)
    .bind(exclude_job_id)
    .fetch_all(&mut **tx)
    .await?;
    let reserved_app_ids = reserved_jobs
        .iter()
        .filter_map(|row| row.get::<Option<Uuid>, _>("app_id"))
        .collect::<Vec<_>>();
    let mut occupied_apps = HashSet::new();
    let mut demand = RuntimeDemand::default();
    for app in &apps {
        if app.current && !app.suspended {
            occupied_apps.insert(app.id);
            demand += app.demand;
        }
        for _ in reserved_app_ids.iter().filter(|id| **id == app.id) {
            occupied_apps.insert(app.id);
            demand += app.demand;
        }
    }
    let candidate_already_occupies = occupied_apps.contains(&app_id);
    if !candidate_already_occupies
        && occupied_apps.len() >= usize::try_from(budget.max_concurrent_apps).unwrap_or(0)
    {
        return Ok(CapacityDecision::Waiting("app_slots"));
    }
    if reserved_jobs.len() >= usize::try_from(budget.max_concurrent_builds).unwrap_or(0) {
        return Ok(CapacityDecision::Waiting("build_slot"));
    }
    // The candidate always needs its own build/runtime reservation. A current
    // deployment remains in `demand`, deliberately reserving rollout overlap.
    demand += candidate.demand;
    if budget
        .max_runtime_memory_mb
        .is_some_and(|limit| demand.memory_mb > i64::from(limit))
    {
        return Ok(CapacityDecision::Waiting("runtime_memory"));
    }
    if budget
        .max_running_services
        .is_some_and(|limit| demand.services > i64::from(limit))
    {
        return Ok(CapacityDecision::Waiting("service_slots"));
    }
    let telemetry_required =
        budget.min_available_memory_mb.is_some() || budget.min_disk_free_mb.is_some();
    if telemetry_required && (!budget.snapshot_fresh || budget.snapshot.is_none()) {
        return Ok(CapacityDecision::Waiting("telemetry_stale"));
    }
    if let Some(snapshot) = budget.snapshot {
        if budget.min_available_memory_mb.is_some_and(|minimum| {
            snapshot.memory_available_mib
                < u64::try_from(i64::from(minimum).saturating_add(candidate.demand.memory_mb))
                    .unwrap_or(u64::MAX)
        }) {
            return Ok(CapacityDecision::Waiting("live_memory"));
        }
        if budget
            .min_disk_free_mb
            .is_some_and(|minimum| snapshot.disk_free_mib < u64::try_from(minimum).unwrap_or(0))
        {
            return Ok(CapacityDecision::Waiting("disk"));
        }
    }
    Ok(CapacityDecision::Admitted)
}

pub fn spawn_capacity_scheduler_task(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(CAPACITY_SCHEDULER_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            if let Err(err) = admit_waiting_deployments(&state).await {
                tracing::warn!(error = %err, "capacity scheduler pass failed");
            }
        }
    });
}

pub async fn admit_waiting_deployments(state: &AppState) -> anyhow::Result<u64> {
    let waiting = sqlx::query(
        "SELECT id,server_id,app_id
         FROM agent_jobs
         WHERE status='queued'
           AND job_type IN ('deploy','rollback')
           AND payload_json->>'capacity_wait'='true'
         ORDER BY priority,created_at,id",
    )
    .fetch_all(&state.db)
    .await?;
    let mut admitted = 0;
    for row in waiting {
        let job_id: Uuid = row.get("id");
        let server_id: Uuid = row.get("server_id");
        let Some(app_id) = row.get::<Option<Uuid>, _>("app_id") else {
            continue;
        };
        let mut tx = state.db.begin().await?;
        let decision =
            capacity_decision_in_transaction(&mut tx, server_id, app_id, Some(job_id)).await?;
        if decision == CapacityDecision::Admitted {
            let changed = sqlx::query(
                "UPDATE agent_jobs
                 SET payload_json=(payload_json - 'capacity_wait' - 'capacity_wait_reason')
                                    || '{\"capacity_reserved\":true}'::jsonb,
                     available_at=now(),updated_at=now()
                 WHERE id=$1 AND status='queued'
                   AND payload_json->>'capacity_wait'='true'
                 RETURNING deployment_id",
            )
            .bind(job_id)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(changed) = changed {
                if let Some(deployment_id) = changed.get::<Option<Uuid>, _>("deployment_id") {
                    sqlx::query(
                        "UPDATE deployments SET status='running'
                         WHERE id=$1 AND status='queued'",
                    )
                    .bind(deployment_id)
                    .execute(&mut *tx)
                    .await?;
                }
                admitted += 1;
            }
        }
        tx.commit().await?;
    }
    Ok(admitted)
}

pub async fn configure_local_server_capacity_from_env(state: &AppState) -> anyhow::Result<()> {
    let values = [
        positive_env_i32("HOSTLET_MAX_RUNTIME_MEMORY_MB"),
        positive_env_i32("HOSTLET_MAX_RUNNING_SERVICES"),
        nonnegative_env_i32("HOSTLET_MIN_AVAILABLE_MEMORY_MB"),
        nonnegative_env_i32("HOSTLET_MIN_DISK_FREE_MB"),
    ];
    if values.iter().all(Option::is_none) {
        return Ok(());
    }
    sqlx::query(
        "UPDATE servers
         SET max_runtime_memory_mb=COALESCE($2,max_runtime_memory_mb),
             max_running_services=COALESCE($3,max_running_services),
             min_available_memory_mb=COALESCE($4,min_available_memory_mb),
             min_disk_free_mb=COALESCE($5,min_disk_free_mb)
         WHERE id=$1",
    )
    .bind(state.local_server_id)
    .bind(values[0])
    .bind(values[1])
    .bind(values[2])
    .bind(values[3])
    .execute(&state.db)
    .await?;
    Ok(())
}

fn positive_env_i32(key: &str) -> Option<i32> {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<i32>().ok())
        .filter(|value| *value > 0)
}

fn nonnegative_env_i32(key: &str) -> Option<i32> {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<i32>().ok())
        .filter(|value| *value >= 0)
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
#[cfg(test)]
async fn ensure_server_has_capacity(
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
             SET kind='local',
                 status='online',
                 capabilities=ARRAY['builder','app_runner']::TEXT[],
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
