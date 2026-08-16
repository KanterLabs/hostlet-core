use crate::{deploy::create_and_send_deploy, state::AppState};
use axum::http::StatusCode;
use hostlet_contracts::valid_commit_sha;
use serde_json::Value;
use sqlx::Row;
use std::time::Duration;
use uuid::Uuid;

const AUTO_DEPLOY_DISABLED_REASON: &str = "auto redeploy is disabled for this app";
pub(super) const WEBHOOK_CLAIM_LEASE_SECS: u64 = 10 * 60;
pub(super) const WEBHOOK_CLAIM_WORK_BUDGET_SECS: u64 = WEBHOOK_CLAIM_LEASE_SECS - 60;
const _: () = assert!(WEBHOOK_CLAIM_WORK_BUDGET_SECS < WEBHOOK_CLAIM_LEASE_SECS);

/// Test-only fault injection points for the webhook's durable boundaries.
/// Production always passes [`WebhookFailpoint::None`], so these branches are
/// inert outside the test binary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WebhookFailpoint {
    None,
    EventInsert,
    EventRead,
    EventLock,
    AppLookup,
    DeploymentCreate,
    AppOutcomeWrite,
    AppOutcomeUpdate,
    Finalization,
    Commit,
}

fn fail_webhook_at(failpoint: WebhookFailpoint, boundary: WebhookFailpoint) -> anyhow::Result<()> {
    if failpoint == boundary {
        anyhow::bail!("injected GitHub webhook failure at {boundary:?}");
    }
    Ok(())
}

pub(super) fn webhook_processing_status(result: &anyhow::Result<()>) -> StatusCode {
    if result.is_ok() {
        StatusCode::ACCEPTED
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[derive(Debug)]
pub(super) struct WebhookEventRecord {
    id: Uuid,
    repo_full_name: String,
    event_type: String,
    payload: Value,
    pub(super) processing_token: Option<Uuid>,
}

pub(super) enum WebhookClaim {
    AlreadyProcessed,
    Claimed(WebhookEventRecord),
}

/// Claim one delivery with a short atomic update. The claim lease prevents a
/// crashed handler from blocking redelivery forever, while avoiding a held
/// PostgreSQL connection during deployment creation and fan-out work.
pub(super) async fn process_webhook(
    state: &AppState,
    delivery: &str,
    event: &str,
    repo: &str,
    payload: &Value,
) -> anyhow::Result<()> {
    process_webhook_with_failpoint(
        state,
        delivery,
        event,
        repo,
        payload,
        WebhookFailpoint::None,
    )
    .await
}

#[cfg(test)]
pub(super) async fn process_webhook_faulted(
    state: &AppState,
    delivery: &str,
    event: &str,
    repo: &str,
    payload: &Value,
    failpoint: WebhookFailpoint,
) -> anyhow::Result<()> {
    process_webhook_with_failpoint(state, delivery, event, repo, payload, failpoint).await
}

async fn process_webhook_with_failpoint(
    state: &AppState,
    delivery: &str,
    event: &str,
    repo: &str,
    payload: &Value,
    failpoint: WebhookFailpoint,
) -> anyhow::Result<()> {
    let claim = claim_webhook_event(state, delivery, event, repo, payload, failpoint).await?;
    let WebhookClaim::Claimed(event) = claim else {
        return Ok(());
    };
    let event_id = event.id;
    let processing_token = event
        .processing_token
        .ok_or_else(|| anyhow::anyhow!("GitHub webhook claim {event_id} has no ownership token"))?;
    // The lease is ten minutes. Cancel all fan-out work one minute earlier so
    // a live handler cannot continue issuing side effects after another
    // delivery has legitimately taken over an expired claim.
    let result = match tokio::time::timeout(
        Duration::from_secs(WEBHOOK_CLAIM_WORK_BUDGET_SECS),
        process_claimed_webhook(state, delivery, event, processing_token, failpoint),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!(
            "GitHub webhook processing exceeded its claim ownership budget"
        )),
    };
    if let Err(err) = &result {
        if let Err(release_error) = release_webhook_claim(state, event_id, processing_token).await {
            tracing::warn!(
                error = %release_error,
                delivery,
                "failed to release GitHub webhook processing lease"
            );
        }
        tracing::debug!(error = %err, delivery, "GitHub webhook claim will be retried");
    }
    result
}

pub(super) async fn claim_webhook_event(
    state: &AppState,
    delivery: &str,
    event: &str,
    repo: &str,
    payload: &Value,
    failpoint: WebhookFailpoint,
) -> anyhow::Result<WebhookClaim> {
    fail_webhook_at(failpoint, WebhookFailpoint::EventInsert)?;
    sqlx::query(
        "INSERT INTO webhook_events (github_delivery_id,repo_full_name,event_type,payload)
         VALUES ($1,$2,$3,$4)
         ON CONFLICT (github_delivery_id) DO NOTHING",
    )
    .bind(delivery)
    .bind(repo)
    .bind(event)
    .bind(payload)
    .execute(&state.db)
    .await?;

    let stored = sqlx::query(
        "SELECT id,repo_full_name,event_type,payload,processed
         FROM webhook_events
         WHERE github_delivery_id=$1",
    )
    .bind(delivery)
    .fetch_optional(&state.db)
    .await?;
    fail_webhook_at(failpoint, WebhookFailpoint::EventRead)?;
    let Some(stored) = stored else {
        anyhow::bail!("GitHub webhook delivery {delivery} was not recorded");
    };
    let record = WebhookEventRecord {
        id: stored.get("id"),
        repo_full_name: stored.get("repo_full_name"),
        event_type: stored.get("event_type"),
        payload: stored.get("payload"),
        processing_token: None,
    };
    let was_processed = stored.get::<bool, _>("processed");
    if was_processed && !webhook_event_needs_redrive(state, &record).await? {
        return Ok(WebhookClaim::AlreadyProcessed);
    }

    // The update is the claim/lease boundary. Concurrent deliveries either
    // observe the already-running lease and retry, or take over after ten
    // minutes without a heartbeat (a crashed handler cannot wedge a delivery).
    fail_webhook_at(failpoint, WebhookFailpoint::EventLock)?;
    let processing_token = Uuid::new_v4();
    let claimed = sqlx::query(
        "UPDATE webhook_events
         SET processed=false,processing=true,processing_started_at=clock_timestamp(),processing_token=$2
         WHERE id=$1
           AND (
             (processed=false AND
              (processing=false OR processing_started_at IS NULL OR
               processing_started_at < clock_timestamp() - ($3::bigint * interval '1 second')))
             OR (processed=true AND processing=false)
           )
         RETURNING id,repo_full_name,event_type,payload,processing_token",
    )
    .bind(record.id)
    .bind(processing_token)
    .bind(WEBHOOK_CLAIM_LEASE_SECS as i64)
    .fetch_optional(&state.db)
    .await?;
    let Some(claimed) = claimed else {
        anyhow::bail!("GitHub webhook delivery {delivery} is already processing");
    };
    Ok(WebhookClaim::Claimed(WebhookEventRecord {
        id: claimed.get("id"),
        repo_full_name: claimed.get("repo_full_name"),
        event_type: claimed.get("event_type"),
        payload: claimed.get("payload"),
        processing_token: Some(claimed.get("processing_token")),
    }))
}

async fn process_claimed_webhook(
    state: &AppState,
    delivery: &str,
    event: WebhookEventRecord,
    processing_token: Uuid,
    failpoint: WebhookFailpoint,
) -> anyhow::Result<()> {
    if event.event_type != "push" {
        finalize_webhook_event(
            state,
            event.id,
            delivery,
            None,
            None,
            Some("unsupported event type"),
            processing_token,
            failpoint,
        )
        .await?;
        return Ok(());
    }

    let branch = event
        .payload
        .get("ref")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim_start_matches("refs/heads/");
    let sha = event
        .payload
        .get("after")
        .and_then(Value::as_str)
        .unwrap_or("HEAD");
    if event
        .payload
        .get("deleted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        finalize_webhook_event(
            state,
            event.id,
            delivery,
            Some(branch),
            Some(sha),
            Some("branch was deleted"),
            processing_token,
            failpoint,
        )
        .await?;
        return Ok(());
    }
    if !valid_commit_sha(sha) {
        finalize_webhook_event(
            state,
            event.id,
            delivery,
            Some(branch),
            Some(sha),
            Some("push did not include a valid commit SHA"),
            processing_token,
            failpoint,
        )
        .await?;
        return Ok(());
    }

    fail_webhook_at(failpoint, WebhookFailpoint::AppLookup)?;
    let apps = sqlx::query(
        "SELECT id,user_id,auto_deploy FROM apps WHERE repo_full_name=$1 AND branch=$2",
    )
    .bind(&event.repo_full_name)
    .bind(branch)
    .fetch_all(&state.db)
    .await?;
    if apps.is_empty() {
        finalize_webhook_event(
            state,
            event.id,
            delivery,
            Some(branch),
            Some(sha),
            Some("no apps matched this repository and branch"),
            processing_token,
            failpoint,
        )
        .await?;
        return Ok(());
    }

    for app in apps {
        let app_id: Uuid = app.get("id");
        let user_id: Uuid = app.get("user_id");
        let auto_deploy = app.get::<bool, _>("auto_deploy");
        process_webhook_app(
            state,
            event.id,
            app_id,
            user_id,
            auto_deploy,
            &event.repo_full_name,
            branch,
            sha,
            processing_token,
            failpoint,
        )
        .await?;
    }

    finalize_webhook_event(
        state,
        event.id,
        delivery,
        Some(branch),
        Some(sha),
        None,
        processing_token,
        failpoint,
    )
    .await?;
    Ok(())
}

async fn webhook_event_needs_redrive(
    state: &AppState,
    event: &WebhookEventRecord,
) -> anyhow::Result<bool> {
    if event.event_type != "push" {
        return Ok(false);
    }
    let branch = event
        .payload
        .get("ref")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim_start_matches("refs/heads/");
    let sha = event
        .payload
        .get("after")
        .and_then(Value::as_str)
        .unwrap_or("HEAD");
    if event
        .payload
        .get("deleted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || !valid_commit_sha(sha)
    {
        return Ok(false);
    }
    let apps = sqlx::query("SELECT id,auto_deploy FROM apps WHERE repo_full_name=$1 AND branch=$2")
        .bind(&event.repo_full_name)
        .bind(branch)
        .fetch_all(&state.db)
        .await?;
    for app in apps {
        let app_id: Uuid = app.get("id");
        let outcome = sqlx::query(
            "SELECT deployment_id,status,ignored_reason
             FROM webhook_app_events
             WHERE webhook_event_id=$1 AND app_id=$2
             LIMIT 1",
        )
        .bind(event.id)
        .bind(app_id)
        .fetch_optional(&state.db)
        .await?;
        let Some(outcome) = outcome else {
            return Ok(true);
        };
        let deployment_id = outcome.get::<Option<Uuid>, _>("deployment_id");
        let status = outcome.get::<String, _>("status");
        let ignored_reason = outcome.get::<Option<String>, _>("ignored_reason");
        if !app.get::<bool, _>("auto_deploy") {
            if status == "ignored" && ignored_reason.as_deref() == Some(AUTO_DEPLOY_DISABLED_REASON)
            {
                continue;
            }
            return Ok(true);
        }
        if status == "ignored" && ignored_reason.as_deref() == Some(AUTO_DEPLOY_DISABLED_REASON) {
            continue;
        }
        if status == "deployed"
            && deployment_id.is_some()
            && deployment_is_viable(state, deployment_id.unwrap()).await?
        {
            continue;
        }
        return Ok(true);
    }
    Ok(false)
}

pub(super) async fn release_webhook_claim(
    state: &AppState,
    event_id: Uuid,
    processing_token: Uuid,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE webhook_events
         SET processing=false,processing_started_at=NULL,processing_token=NULL
         WHERE id=$1 AND processed=false AND processing_token=$2",
    )
    .bind(event_id)
    .bind(processing_token)
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Process one matched app while preserving idempotency across retries. An
/// existing terminal outcome is reused; otherwise a matching active/successful
/// deployment is linked before a new deployment is requested.
#[allow(clippy::too_many_arguments)]
pub(super) async fn process_webhook_app(
    state: &AppState,
    webhook_event_id: Uuid,
    app_id: Uuid,
    user_id: Uuid,
    auto_deploy: bool,
    repo: &str,
    branch: &str,
    sha: &str,
    processing_token: Uuid,
    failpoint: WebhookFailpoint,
) -> anyhow::Result<()> {
    ensure_webhook_claim_owner(state, webhook_event_id, processing_token).await?;
    let previous = sqlx::query(
        "SELECT id,deployment_id,status,ignored_reason
         FROM webhook_app_events
         WHERE webhook_event_id=$1 AND app_id=$2
         ORDER BY created_at DESC,id DESC
         LIMIT 1",
    )
    .bind(webhook_event_id)
    .bind(app_id)
    .fetch_optional(&state.db)
    .await?;
    if let Some(row) = &previous {
        let status = row.get::<String, _>("status");
        let deployment_id = row.get::<Option<Uuid>, _>("deployment_id");
        let ignored_reason = row.get::<Option<String>, _>("ignored_reason");
        if webhook_app_event_is_terminal(&status, deployment_id, ignored_reason.as_deref()) {
            let viable = if status == "deployed" {
                match deployment_id {
                    Some(deployment_id) => deployment_is_viable(state, deployment_id).await?,
                    None => false,
                }
            } else {
                true
            };
            if viable {
                return Ok(());
            }
        }
    }

    if !auto_deploy {
        fail_webhook_at(
            failpoint,
            if previous.is_some() {
                WebhookFailpoint::AppOutcomeUpdate
            } else {
                WebhookFailpoint::AppOutcomeWrite
            },
        )?;
        insert_webhook_app_event(
            state,
            previous.as_ref().map(|row| row.get("id")),
            webhook_event_id,
            app_id,
            None,
            repo,
            branch,
            sha,
            processing_token,
            "ignored",
            Some(AUTO_DEPLOY_DISABLED_REASON),
        )
        .await?;
        return Ok(());
    }

    let deployment_id = match existing_webhook_deployment(state, app_id, sha).await? {
        Some(id) => id,
        None => {
            anyhow::ensure!(
                renew_webhook_claim(state, webhook_event_id, processing_token).await?,
                "GitHub webhook claim for event {webhook_event_id} expired before deployment creation"
            );
            fail_webhook_at(failpoint, WebhookFailpoint::DeploymentCreate)?;
            match create_and_send_deploy(state, user_id, app_id, sha).await {
                Ok(id) => id,
                Err(create_error) => {
                    // A concurrent redelivery can win the active-deployment
                    // insert between the lookup above and create_and_send_deploy.
                    // Re-checking after the error turns that race into an
                    // idempotent link while preserving genuine failures for retry.
                    if let Some(id) = existing_webhook_deployment(state, app_id, sha).await? {
                        id
                    } else {
                        return Err(create_error);
                    }
                }
            }
        }
    };
    fail_webhook_at(
        failpoint,
        if previous.is_some() {
            WebhookFailpoint::AppOutcomeUpdate
        } else {
            WebhookFailpoint::AppOutcomeWrite
        },
    )?;
    ensure_webhook_claim_owner(state, webhook_event_id, processing_token).await?;
    insert_webhook_app_event(
        state,
        previous.as_ref().map(|row| row.get("id")),
        webhook_event_id,
        app_id,
        Some(deployment_id),
        repo,
        branch,
        sha,
        processing_token,
        "deployed",
        None,
    )
    .await?;
    Ok(())
}

async fn ensure_webhook_claim_owner(
    state: &AppState,
    webhook_event_id: Uuid,
    processing_token: Uuid,
) -> anyhow::Result<()> {
    let owner: bool = sqlx::query_scalar(
        "SELECT EXISTS(
           SELECT 1 FROM webhook_events
           WHERE id=$1 AND processed=false AND processing=true AND processing_token=$2
         )",
    )
    .bind(webhook_event_id)
    .bind(processing_token)
    .fetch_one(&state.db)
    .await?;
    anyhow::ensure!(
        owner,
        "GitHub webhook claim for event {webhook_event_id} is no longer owned"
    );
    Ok(())
}

pub(super) async fn renew_webhook_claim(
    state: &AppState,
    webhook_event_id: Uuid,
    processing_token: Uuid,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        "UPDATE webhook_events
         SET processing_started_at=clock_timestamp()
         WHERE id=$1 AND processed=false AND processing=true AND processing_token=$2",
    )
    .bind(webhook_event_id)
    .bind(processing_token)
    .execute(&state.db)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub(super) fn webhook_app_event_is_terminal(
    status: &str,
    deployment_id: Option<Uuid>,
    ignored_reason: Option<&str>,
) -> bool {
    (status == "deployed" && deployment_id.is_some())
        || (status == "ignored" && ignored_reason == Some(AUTO_DEPLOY_DISABLED_REASON))
}

async fn existing_webhook_deployment(
    state: &AppState,
    app_id: Uuid,
    sha: &str,
) -> anyhow::Result<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id
         FROM deployments
         WHERE app_id=$1
           AND LOWER(commit_sha)=LOWER($2)
           AND (
             status='success'
             OR (
               status IN (
                 'queued','queued_for_build','running','building','publishing',
                 'queued_for_release','pulling','starting','health_checking',
                 'routing'
               )
               AND EXISTS (
                 SELECT 1 FROM agent_jobs
                 WHERE agent_jobs.deployment_id=deployments.id
                   AND agent_jobs.status IN ('queued','claimed','running')
               )
             )
           )
         ORDER BY created_at DESC,id DESC
         LIMIT 1",
    )
    .bind(app_id)
    .bind(sha)
    .fetch_optional(&state.db)
    .await?)
}

async fn deployment_is_viable(state: &AppState, deployment_id: Uuid) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(
           SELECT 1 FROM deployments
           WHERE id=$1
             AND (
               status='success'
               OR (
                 status IN (
                   'queued','queued_for_build','running','building','publishing',
                   'queued_for_release','pulling','starting','health_checking',
                   'routing'
                 )
                 AND EXISTS (
                   SELECT 1 FROM agent_jobs
                   WHERE agent_jobs.deployment_id=deployments.id
                     AND agent_jobs.status IN ('queued','claimed','running')
                 )
               )
             )
         )",
    )
    .bind(deployment_id)
    .fetch_one(&state.db)
    .await?)
}

/// Mark a delivery processed, recording branch/commit and an optional reason.
/// A missing row is an error rather than a successful no-op: acknowledging that
/// response would permanently lose the GitHub delivery.
#[allow(clippy::too_many_arguments)]
pub(super) async fn finalize_webhook_event(
    state: &AppState,
    event_id: Uuid,
    delivery: &str,
    branch: Option<&str>,
    sha: Option<&str>,
    ignored_reason: Option<&str>,
    processing_token: Uuid,
    failpoint: WebhookFailpoint,
) -> anyhow::Result<()> {
    let mut tx = state.db.begin().await?;
    fail_webhook_at(failpoint, WebhookFailpoint::Finalization)?;
    let result = sqlx::query(
        "UPDATE webhook_events
         SET branch=$2, commit_sha=$3, ignored_reason=$4,
             processed=true, processing=false, processing_started_at=NULL,
             processing_token=NULL, processed_at=now()
         WHERE id=$1 AND github_delivery_id=$5 AND processed=false AND processing=true
           AND processing_token=$6",
    )
    .bind(event_id)
    .bind(branch)
    .bind(sha)
    .bind(ignored_reason)
    .bind(delivery)
    .bind(processing_token)
    .execute(&mut *tx)
    .await?;
    anyhow::ensure!(
        result.rows_affected() == 1,
        "GitHub webhook delivery {delivery} could not be finalized"
    );
    fail_webhook_at(failpoint, WebhookFailpoint::Commit)?;
    tx.commit().await?;
    Ok(())
}

/// Record a per-app outcome, updating a prior retryable row instead of adding
/// a second row. The delivery lease serializes normal redeliveries, and the
/// unique index preserves the invariant if a lease expires during fan-out.
#[allow(clippy::too_many_arguments)]
async fn insert_webhook_app_event(
    state: &AppState,
    existing_id: Option<Uuid>,
    webhook_event_id: Uuid,
    app_id: Uuid,
    deployment_id: Option<Uuid>,
    repo: &str,
    branch: &str,
    sha: &str,
    processing_token: Uuid,
    status: &str,
    ignored_reason: Option<&str>,
) -> anyhow::Result<()> {
    // Lock the event row for this short transaction. A lease takeover must
    // wait for this outcome write, then observe the token mismatch instead of
    // letting a stale owner overwrite the current owner's result.
    let mut tx = state.db.begin().await?;
    let owner = sqlx::query(
        "SELECT id FROM webhook_events
         WHERE id=$1 AND processed=false AND processing=true AND processing_token=$2
         FOR UPDATE",
    )
    .bind(webhook_event_id)
    .bind(processing_token)
    .fetch_optional(&mut *tx)
    .await?;
    anyhow::ensure!(
        owner.is_some(),
        "GitHub webhook claim for event {webhook_event_id} is no longer owned"
    );
    let result = sqlx::query(
        "INSERT INTO webhook_app_events
         (webhook_event_id,app_id,deployment_id,repo_full_name,branch,commit_sha,status,ignored_reason)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
         ON CONFLICT (webhook_event_id,app_id) DO UPDATE SET
           deployment_id=EXCLUDED.deployment_id,
           repo_full_name=EXCLUDED.repo_full_name,
           branch=EXCLUDED.branch,
           commit_sha=EXCLUDED.commit_sha,
           status=EXCLUDED.status,
           ignored_reason=EXCLUDED.ignored_reason",
    )
    .bind(webhook_event_id)
    .bind(app_id)
    .bind(deployment_id)
    .bind(repo)
    .bind(branch)
    .bind(sha)
    .bind(status)
    .bind(ignored_reason)
    .execute(&mut *tx)
    .await?;
    anyhow::ensure!(
        result.rows_affected() == 1,
        "webhook app outcome {:?} disappeared before recording",
        existing_id
    );
    tx.commit().await?;
    Ok(())
}
