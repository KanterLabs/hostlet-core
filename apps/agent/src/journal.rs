use super::*;
use serde::{Deserialize, Serialize};

const JOURNAL_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct DeploymentJournal {
    version: u32,
    pub(crate) deployment_id: Uuid,
    pub(crate) job_id: Uuid,
    pub(crate) claim_token: Uuid,
    pub(crate) phase: String,
    pub(crate) app_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) route_generation: Option<i64>,
    #[serde(default)]
    pub(crate) rolled_back: bool,
    updated_at_unix_ms: u128,
}

fn journal_dir(cfg: &Config) -> PathBuf {
    cfg.workdir.join("deployment-journal")
}

fn journal_path(cfg: &Config, deployment_id: Uuid) -> PathBuf {
    journal_dir(cfg).join(format!("{deployment_id}.json"))
}

async fn persist(cfg: &Config, journal: &DeploymentJournal) -> anyhow::Result<()> {
    let dir = journal_dir(cfg);
    tokio::fs::create_dir_all(&dir).await?;
    let path = journal_path(cfg, journal.deployment_id);
    let tmp = dir.join(format!(".{}.{}.tmp", journal.deployment_id, Uuid::new_v4()));
    let body = serde_json::to_vec(journal)?;
    let mut options = tokio::fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options.open(&tmp).await?;
    tokio::io::AsyncWriteExt::write_all(&mut file, &body).await?;
    file.sync_all().await?;
    tokio::fs::rename(&tmp, &path).await?;
    // Syncing the directory makes the rename durable across a host power loss.
    let directory = tokio::fs::File::open(&dir).await?;
    directory.sync_all().await?;
    Ok(())
}

pub(crate) async fn start_deployment_journal(
    cfg: &Config,
    deployment_id: Uuid,
    job_id: Uuid,
    claim_token: Uuid,
    app_id: Option<Uuid>,
) -> anyhow::Result<()> {
    persist(
        cfg,
        &DeploymentJournal {
            version: JOURNAL_VERSION,
            deployment_id,
            job_id,
            claim_token,
            phase: "claimed".into(),
            app_id,
            route_generation: None,
            rolled_back: false,
            updated_at_unix_ms: unix_ms(),
        },
    )
    .await
}

pub(crate) async fn record_activation_generation(
    cfg: &Config,
    deployment_id: Uuid,
    generation: i64,
    rolled_back: bool,
) -> anyhow::Result<()> {
    let body = tokio::fs::read(journal_path(cfg, deployment_id)).await?;
    let mut journal: DeploymentJournal = serde_json::from_slice(&body)?;
    journal.phase = "activation_prepared".into();
    journal.route_generation = Some(generation);
    journal.rolled_back = rolled_back;
    journal.updated_at_unix_ms = unix_ms();
    persist(cfg, &journal).await
}

pub(crate) async fn record_deployment_phase(
    cfg: &Config,
    deployment_id: Uuid,
    phase: &str,
) -> anyhow::Result<()> {
    let path = journal_path(cfg, deployment_id);
    let body = match tokio::fs::read(&path).await {
        Ok(body) => body,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    let mut journal: DeploymentJournal = serde_json::from_slice(&body)?;
    journal.phase = phase.to_string();
    journal.updated_at_unix_ms = unix_ms();
    persist(cfg, &journal).await
}

pub(crate) async fn finish_deployment_journal(cfg: &Config, deployment_id: Uuid) {
    let path = journal_path(cfg, deployment_id);
    if let Err(err) = tokio::fs::remove_file(path).await {
        if err.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(%deployment_id, error = %err, "failed to remove deployment journal");
        }
    }
}

pub(crate) async fn log_recoverable_journals(cfg: &Config) -> anyhow::Result<()> {
    let dir = journal_dir(cfg);
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        if entry.path().extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        let Ok(body) = tokio::fs::read(entry.path()).await else {
            continue;
        };
        let Ok(journal) = serde_json::from_slice::<DeploymentJournal>(&body) else {
            continue;
        };
        tracing::warn!(
            deployment_id = %journal.deployment_id,
            job_id = %journal.job_id,
            phase = journal.phase,
            "found interrupted deployment journal; durable claim recovery will reconcile it"
        );
        if matches!(journal.phase.as_str(), "route_switched" | "committed") {
            if let Some(route_generation) = journal.route_generation {
                let response = cfg
                    .http
                    .post(format!(
                        "{}/api/agent/deployments/{}/commit-activation",
                        cfg.api_url, journal.deployment_id
                    ))
                    .header("x-hostlet-server-id", cfg.server_id.to_string())
                    .header("x-hostlet-agent-token", &cfg.agent_token)
                    .json(&hostlet_contracts::CommitActivationRequest {
                        job_id: journal.job_id,
                        claim_token: journal.claim_token,
                        route_generation,
                        local_url: None,
                        rolled_back: journal.rolled_back,
                    })
                    .send()
                    .await;
                if response.is_ok_and(|response| response.status().is_success()) {
                    finish_deployment_journal(cfg, journal.deployment_id).await;
                }
            }
        }
    }
    Ok(())
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_never_has_secret_fields() {
        let journal = DeploymentJournal {
            version: JOURNAL_VERSION,
            deployment_id: Uuid::nil(),
            job_id: Uuid::nil(),
            claim_token: Uuid::nil(),
            phase: "building".into(),
            app_id: None,
            route_generation: None,
            rolled_back: false,
            updated_at_unix_ms: 0,
        };
        let json = serde_json::to_string(&journal).unwrap();
        assert!(!json.contains("env"));
        assert!(!json.contains("github_token"));
    }
}
