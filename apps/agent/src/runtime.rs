use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

mod pipeline;

use pipeline::deploy;

/// Single-job concurrency slot. Acquired only via `try_acquire`'s
/// compare_exchange and released only by the `Drop` guard, so the flag clears on
/// normal completion AND on panic (the unwind drops the guard inside the spawned
/// job task).
struct JobGuard(Arc<AtomicBool>);

impl Drop for JobGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn try_acquire(slot: &Arc<AtomicBool>) -> Option<JobGuard> {
    slot.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .ok()
        .map(|_| JobGuard(slot.clone()))
}

#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) api_url: String,
    pub(crate) http: reqwest::Client,
    pub(crate) server_id: Uuid,
    pub(crate) agent_token: String,
    pub(crate) job_signing_secret: String,
    pub(crate) workdir: PathBuf,
    pub(crate) local_mode: bool,
    pub(crate) health_host: String,
    pub(crate) local_router: Option<LocalRouter>,
}

#[derive(Clone)]
pub(crate) struct LocalRouter {
    pub(crate) snippets_dir: PathBuf,
    pub(crate) reload_command: Vec<String>,
}

#[derive(Debug)]
struct ReportedDeploymentFailure {
    message: String,
}

#[derive(Debug)]
struct CancelledJob;

impl std::fmt::Display for CancelledJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("deployment was cancelled before activation")
    }
}

impl std::error::Error for CancelledJob {}

#[derive(Debug)]
pub(crate) struct UnacknowledgedActivation(String);

impl std::fmt::Display for UnacknowledgedActivation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "activation acknowledgement is pending: {}", self.0)
    }
}

impl std::error::Error for UnacknowledgedActivation {}

pub(crate) fn unacknowledged_activation(error: impl std::fmt::Display) -> anyhow::Error {
    UnacknowledgedActivation(error.to_string()).into()
}

impl ReportedDeploymentFailure {
    fn new(message: String) -> Self {
        Self { message }
    }
}

impl std::fmt::Display for ReportedDeploymentFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ReportedDeploymentFailure {}

pub(crate) fn reported_deployment_failure(message: String) -> anyhow::Error {
    ReportedDeploymentFailure::new(message).into()
}

fn deployment_failure_message(err: &anyhow::Error) -> String {
    err.downcast_ref::<ReportedDeploymentFailure>()
        .map(|failure| failure.message.clone())
        .unwrap_or_else(|| format!("{err}"))
}

fn deployment_status_already_reported(err: &anyhow::Error) -> bool {
    err.downcast_ref::<ReportedDeploymentFailure>().is_some()
}

pub(crate) async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();
    let cfg = Config {
        api_url: env("HOSTLET_API_URL")?,
        http: http_client()?,
        server_id: env("HOSTLET_SERVER_ID")?.parse()?,
        agent_token: env("HOSTLET_AGENT_TOKEN")?,
        job_signing_secret: env("HOSTLET_JOB_SIGNING_SECRET")?,
        workdir: PathBuf::from(
            std::env::var("HOSTLET_WORKDIR").unwrap_or_else(|_| "/var/lib/hostlet".into()),
        ),
        local_mode: std::env::var("HOSTLET_LOCAL_MODE")
            .map(|v| v == "true")
            .unwrap_or(false),
        health_host: std::env::var("HOSTLET_HEALTH_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
        local_router: local_router_config()?,
    };
    tokio::fs::create_dir_all(&cfg.workdir).await?;
    log_recoverable_journals(&cfg).await?;
    log_docker_tooling().await;
    // The single-job slot outlives connect_loop so a spawned job keeps running
    // across a WS drop/reconnect: the reconnected loop cannot claim a second job
    // while the old one holds the slot.
    let job_slot = Arc::new(AtomicBool::new(false));
    loop {
        if let Err(err) = connect_loop(cfg.clone(), job_slot.clone()).await {
            tracing::warn!("agent disconnected: {err}");
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

pub(crate) async fn connect_loop(cfg: Config, job_slot: Arc<AtomicBool>) -> anyhow::Result<()> {
    let ws_url = cfg
        .api_url
        .replace("http://", "ws://")
        .replace("https://", "wss://")
        + "/ws/agent";
    let mut req = ws_url.into_client_request()?;
    req.headers_mut()
        .insert("x-hostlet-server-id", cfg.server_id.to_string().parse()?);
    req.headers_mut()
        .insert("x-hostlet-agent-token", cfg.agent_token.parse()?);
    let (mut ws, _) = connect_async(req).await?;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    let mut job_claim = tokio::time::interval(Duration::from_secs(3));
    let mut resource_stats = tokio::time::interval(Duration::from_secs(5));
    let mut storage_stats = tokio::time::interval(Duration::from_secs(60));
    let mut runtime_health = tokio::time::interval(runtime_health_interval());
    let mut health_counts: HashMap<Uuid, HealthCounts> = HashMap::new();
    loop {
        tokio::select! {
            _ = heartbeat.tick() => ws.send(Message::Text(json!({"type":"heartbeat"}).to_string())).await?,
            _ = job_claim.tick() => {
                // Job execution only ever happens inside a spawned task, so every
                // other select arm stays pollable during a multi-minute deploy.
                // When the slot is held no second claim is attempted this tick.
                if let Some(guard) = try_acquire(&job_slot) {
                    let cfg = cfg.clone();
                    tokio::spawn(async move {
                        let _guard = guard;
                        claim_and_run_job(&cfg).await;
                    });
                }
            }
            _ = resource_stats.tick() => publish_resource_stats(&cfg).await,
            _ = storage_stats.tick() => {
                // `docker system df -v` scans every volume's size, so measure off
                // the select loop to keep heartbeat/job-claim responsive.
                let cfg = cfg.clone();
                tokio::spawn(async move { publish_storage_stats(&cfg).await; });
            }
            _ = runtime_health.tick() => publish_runtime_health(&cfg, &mut health_counts).await,
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => handle_ws_text(&cfg, &job_slot, &text).await,
                    Some(Ok(Message::Ping(payload))) => ws.send(Message::Pong(payload)).await?,
                    Some(Ok(Message::Close(_))) | None => bail!("websocket closed"),
                    Some(Ok(_)) => continue,
                    Some(Err(err)) => bail!("websocket error: {err}"),
                }
            }
        }
    }
}

fn runtime_health_interval() -> Duration {
    let seconds = std::env::var("HOSTLET_RUNTIME_HEALTH_INTERVAL_SECONDS")
        .ok()
        .as_deref()
        .and_then(runtime_health_interval_seconds)
        .unwrap_or(60);
    Duration::from_secs(seconds)
}

fn runtime_health_interval_seconds(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed
        .parse::<u64>()
        .ok()
        .filter(|n| (1..=3600).contains(n))
}

pub(crate) async fn claim_and_run_job(cfg: &Config) {
    let response = cfg
        .http
        .post(format!("{}/api/agent/jobs/claim", cfg.api_url))
        .header("x-hostlet-server-id", cfg.server_id.to_string())
        .header("x-hostlet-agent-token", &cfg.agent_token)
        .json(&json!({
            "agent_id": cfg.server_id.to_string(),
            "protocol_version": hostlet_contracts::DEPLOYMENT_PROTOCOL_VERSION
        }))
        .send()
        .await;
    let Ok(response) = response else {
        return;
    };
    if !response.status().is_success() {
        return;
    }
    let Ok(value) = response.json::<Value>().await else {
        return;
    };
    let Some(job) = value.get("job").filter(|job| !job.is_null()) else {
        return;
    };
    let Some(payload) = job.get("payload").cloned() else {
        return;
    };
    let Some(signature) = job.get("signature").and_then(|v| v.as_str()) else {
        return;
    };
    let Ok(raw) = serde_json::to_vec(&payload) else {
        return;
    };
    if !verify_signature(&cfg.job_signing_secret, &raw, signature) {
        tracing::warn!("ignored claimed job with invalid signature");
        return;
    }
    let Some(job_id) = payload
        .get("job_id")
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok())
    else {
        return;
    };
    let Some(claim_token) = job
        .get("claimToken")
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok())
    else {
        tracing::warn!(%job_id, "claimed job did not include a claim token");
        return;
    };
    let deployment_id = payload
        .get("deployment_id")
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok());
    if let Some(deployment_id) = deployment_id {
        let app_id = payload
            .get("app_id")
            .and_then(|v| v.as_str())
            .and_then(|v| Uuid::parse_str(v).ok());
        if let Err(err) =
            start_deployment_journal(cfg, deployment_id, job_id, claim_token, app_id).await
        {
            tracing::warn!(%deployment_id, error = %err, "could not persist deployment journal");
            return;
        }
    }
    match run_claimed_job_with_lease(cfg.clone(), job_id, claim_token, payload.clone()).await {
        Ok(()) => {
            if complete_claimed_job(cfg, job_id, claim_token, "success", None).await {
                if let Some(deployment_id) = deployment_id {
                    finish_deployment_journal(cfg, deployment_id).await;
                }
            }
        }
        Err(err) => {
            if err.downcast_ref::<UnacknowledgedActivation>().is_some() {
                tracing::warn!(%job_id, error = %err, "leaving deployment claim recoverable until activation can be reconciled");
                return;
            }
            let message = deployment_failure_message(&err);
            let cancelled = err.downcast_ref::<CancelledJob>().is_some();
            if cancelled {
                if let Some(deployment_id) = payload
                    .get("deployment_id")
                    .and_then(|v| v.as_str())
                    .and_then(|v| Uuid::parse_str(v).ok())
                {
                    status(cfg, deployment_id, "canceled", Some(&message)).await;
                }
            } else if !deployment_status_already_reported(&err) {
                report_deployment_failure(cfg, &payload, &message).await;
            }
            if complete_claimed_job(
                cfg,
                job_id,
                claim_token,
                if cancelled { "cancelled" } else { "failed" },
                Some(&message),
            )
            .await
            {
                if let Some(deployment_id) = deployment_id {
                    finish_deployment_journal(cfg, deployment_id).await;
                }
            }
            tracing::warn!("claimed job failed: {message}");
        }
    }
}

pub(crate) async fn run_claimed_job_with_lease(
    cfg: Config,
    job_id: Uuid,
    claim_token: Uuid,
    payload: Value,
) -> anyhow::Result<()> {
    heartbeat_job(&cfg, job_id, claim_token, "running").await?;
    let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
    let renew_cfg = cfg.clone();
    let renew = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        let mut failures = 0u8;
        loop {
            interval.tick().await;
            match heartbeat_job(&renew_cfg, job_id, claim_token, "running").await {
                Ok(cancel_requested) => {
                    failures = 0;
                    if cancel_requested {
                        let _ = cancel_tx.send(true);
                        break;
                    }
                }
                Err(err) => {
                    failures = failures.saturating_add(1);
                    tracing::warn!(%job_id, error = %err, failures, "deployment lease renewal failed");
                    // Stop before the five-minute lease can be reassigned. The
                    // next claim reconciles deterministic resources/journal.
                    if failures >= 4 {
                        let _ = cancel_tx.send(true);
                        break;
                    }
                }
            }
        }
    });
    let result = tokio::select! {
        result = handle_job(cfg, payload) => result,
        _ = async {
            while !*cancel_rx.borrow() {
                if cancel_rx.changed().await.is_err() {
                    break;
                }
            }
        } => Err(CancelledJob.into()),
    };
    renew.abort();
    result
}

async fn heartbeat_job(
    cfg: &Config,
    job_id: Uuid,
    claim_token: Uuid,
    phase: &str,
) -> anyhow::Result<bool> {
    let response = cfg
        .http
        .post(format!("{}/api/agent/jobs/{job_id}/heartbeat", cfg.api_url))
        .header("x-hostlet-server-id", cfg.server_id.to_string())
        .header("x-hostlet-agent-token", &cfg.agent_token)
        .json(&json!({"claimToken": claim_token, "phase": phase}))
        .send()
        .await?
        .error_for_status()?;
    Ok(response
        .json::<Value>()
        .await?
        .get("cancelRequested")
        .and_then(Value::as_bool)
        .unwrap_or(false))
}

pub(crate) async fn complete_claimed_job(
    cfg: &Config,
    id: Uuid,
    claim_token: Uuid,
    status: &str,
    failure: Option<&str>,
) -> bool {
    cfg.http
        .post(format!("{}/api/agent/jobs/{id}/complete", cfg.api_url))
        .header("x-hostlet-server-id", cfg.server_id.to_string())
        .header("x-hostlet-agent-token", &cfg.agent_token)
        .json(&json!({"status":status,"failure":failure,"claimToken":claim_token}))
        .send()
        .await
        .is_ok_and(|response| {
            response.status().is_success()
                || (status == "success" && response.status() == reqwest::StatusCode::NOT_FOUND)
        })
}

pub(crate) async fn handle_ws_text(cfg: &Config, job_slot: &Arc<AtomicBool>, text: &str) {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        tracing::warn!("ignored invalid websocket JSON from API");
        return;
    };
    if value.get("type").and_then(|v| v.as_str()) != Some("job") {
        return;
    }
    let Some(payload) = value.get("payload").cloned() else {
        tracing::warn!("ignored job without payload");
        return;
    };
    let Some(signature) = value.get("signature").and_then(|v| v.as_str()) else {
        tracing::warn!("ignored job without signature");
        return;
    };
    let Ok(raw) = serde_json::to_vec(&payload) else {
        tracing::warn!("ignored job with unserializable payload");
        return;
    };
    if !verify_signature(&cfg.job_signing_secret, &raw, signature) {
        tracing::warn!("ignored job with invalid signature");
        return;
    }
    // WS-pushed jobs share the claim path's single-job slot so a push arriving
    // mid-claim cannot run concurrently with a spawned claim task. Dropping when
    // busy cannot strand work: core's API never sends type:"job" WS frames
    // (socket.rs's send_task is the only producer into agent sockets and apps/api
    // has none), and WS push is best-effort — durable work arrives through the
    // claim/lease/requeue path.
    let Some(guard) = try_acquire(job_slot) else {
        tracing::warn!("dropped websocket job while another job is running");
        return;
    };
    let job_id = payload
        .get("job_id")
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok());
    let cfg = cfg.clone();
    tokio::spawn(async move {
        let _guard = guard;
        match handle_job(cfg.clone(), payload.clone()).await {
            Ok(()) => {
                if let Some(job_id) = job_id {
                    job_status(&cfg, job_id, "success", None).await;
                }
            }
            Err(err) => {
                let message = deployment_failure_message(&err);
                if !deployment_status_already_reported(&err) {
                    report_deployment_failure(&cfg, &payload, &message).await;
                }
                if let Some(job_id) = job_id {
                    job_status(&cfg, job_id, "failed", Some(&message)).await;
                }
                tracing::warn!("job failed: {message}");
            }
        }
    });
}

/// Reports a failed job back to the API as a `failed` deployment status (when the
/// payload carries a deployment id), mirroring the stderr log and status update
/// shared by the claim and websocket job paths.
async fn report_deployment_failure(cfg: &Config, payload: &Value, message: &str) {
    let Some(deployment_id) = payload
        .get("deployment_id")
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok())
    else {
        return;
    };
    log(cfg, deployment_id, "stderr", message).await;
    status(cfg, deployment_id, "failed", Some(message)).await;
}

pub(crate) async fn handle_job(cfg: Config, payload: Value) -> anyhow::Result<()> {
    match payload.get("type").and_then(|v| v.as_str()) {
        Some("deploy") => deploy(cfg, payload).await,
        Some("rollback") => rollback(cfg, payload).await,
        Some("delete_app") => delete_app(cfg, payload).await,
        Some("health_check") => {
            health_check_job(&cfg, &payload).await;
            Ok(())
        }
        Some("capture_screenshot") => capture_screenshot_job(&cfg, &payload).await,
        Some("restart_container") => {
            restart_container_job(&cfg, &payload).await?;
            Ok(())
        }
        Some("stop_container") => stop_container_job(&payload).await,
        Some("docker_cleanup") => docker_cleanup_job(&payload).await,
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_health_interval_seconds_accepts_valid_range() {
        assert_eq!(runtime_health_interval_seconds("1"), Some(1));
        assert_eq!(runtime_health_interval_seconds("60"), Some(60));
        assert_eq!(runtime_health_interval_seconds(" 3600 "), Some(3600));
    }

    #[test]
    fn runtime_health_interval_seconds_rejects_invalid_values() {
        assert_eq!(runtime_health_interval_seconds("0"), None);
        assert_eq!(runtime_health_interval_seconds("3601"), None);
        assert_eq!(runtime_health_interval_seconds(""), None);
        assert_eq!(runtime_health_interval_seconds("fast"), None);
    }

    #[test]
    fn try_acquire_rejects_a_second_claim_while_held() {
        let slot = Arc::new(AtomicBool::new(false));

        let held = try_acquire(&slot).expect("first acquisition succeeds");
        assert!(try_acquire(&slot).is_none());

        drop(held);
    }

    #[test]
    fn dropping_the_guard_frees_the_slot() {
        let slot = Arc::new(AtomicBool::new(false));

        let held = try_acquire(&slot).expect("first acquisition succeeds");
        drop(held);

        assert!(try_acquire(&slot).is_some());
    }

    #[test]
    fn sequential_acquisitions_both_succeed() {
        let slot = Arc::new(AtomicBool::new(false));

        let first = try_acquire(&slot).expect("first acquisition succeeds");
        drop(first);
        let second = try_acquire(&slot).expect("second acquisition succeeds");
        drop(second);
    }

    #[tokio::test]
    async fn slot_frees_when_the_holding_task_panics() {
        let slot = Arc::new(AtomicBool::new(false));
        let guard = try_acquire(&slot).expect("acquisition succeeds");

        let handle = tokio::spawn(async move {
            let _guard = guard;
            panic!("job task panicked while holding the slot");
        });

        assert!(handle.await.is_err());
        assert!(
            try_acquire(&slot).is_some(),
            "the slot is free after the panicking task unwinds"
        );
    }
}
