use crate::crypto::{hash_token, Crypto};
use anyhow::{bail, Context};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};
use tokio::sync::{broadcast, mpsc, RwLock};
use uuid::Uuid;

const DEV_JOB_SIGNING_SECRET: &str = "dev-job-signing-secret-change-me";
const DEV_LOCAL_AGENT_TOKEN: &str = "dev-local-agent-token-change-me";
const DEV_GITHUB_WEBHOOK_SECRET: &str = "dev-webhook-secret";
const DEV_SESSION_SECRET: &str = "dev-session-secret-change-me-use-random-in-production";

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub crypto: Crypto,
    pub github_client_id: String,
    pub github_client_secret: String,
    pub github_webhook_secret: String,
    pub public_api_url: String,
    pub public_web_url: String,
    pub base_domain: Option<String>,
    pub domain_prefix: String,
    pub cloudflare_api_token: Option<String>,
    pub cloudflare_zone_id: Option<String>,
    pub cloudflare_tunnel_target: Option<String>,
    pub job_signing_secret: String,
    pub session_secret: String,
    pub allowed_github_logins: Option<HashSet<String>>,
    pub agents: Arc<RwLock<HashMap<Uuid, mpsc::Sender<serde_json::Value>>>>,
    pub logs: broadcast::Sender<LogEvent>,
}

#[derive(Clone, Debug)]
pub struct LogEvent {
    pub deployment_id: Uuid,
    pub stream: String,
    pub line: String,
}

impl AppState {
    pub async fn from_env() -> anyhow::Result<Self> {
        let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL is required")?;
        let allow_insecure_dev_defaults = bool_env("HOSTLET_ALLOW_INSECURE_DEV_DEFAULTS");
        let db = PgPoolOptions::new()
            .max_connections(10)
            .connect(&database_url)
            .await?;
        sqlx::migrate::Migrator::new(Path::new("apps/api/migrations"))
            .await?
            .run(&db)
            .await?;
        let crypto = Crypto::from_env(allow_insecure_dev_defaults)?;
        let local_agent_token = secret_from_env(
            "LOCAL_AGENT_TOKEN",
            DEV_LOCAL_AGENT_TOKEN,
            allow_insecure_dev_defaults,
        )?;
        let allowed_github_logins = allowed_github_logins();
        if !allow_insecure_dev_defaults && allowed_github_logins.is_none() {
            bail!("HOSTLET_ALLOWED_GITHUB_LOGINS is required in secure mode");
        }
        seed_local_server(&db, &local_agent_token).await?;
        let (logs, _) = broadcast::channel(1024);
        Ok(Self {
            db,
            crypto,
            github_client_id: std::env::var("GITHUB_CLIENT_ID").unwrap_or_default(),
            github_client_secret: std::env::var("GITHUB_CLIENT_SECRET").unwrap_or_default(),
            github_webhook_secret: secret_from_env(
                "GITHUB_WEBHOOK_SECRET",
                DEV_GITHUB_WEBHOOK_SECRET,
                allow_insecure_dev_defaults,
            )?,
            public_api_url: std::env::var("PUBLIC_API_URL")
                .unwrap_or_else(|_| "http://localhost:8080".into()),
            public_web_url: std::env::var("PUBLIC_WEB_URL")
                .unwrap_or_else(|_| "http://localhost:3000".into()),
            base_domain: nonempty_env("HOSTLET_BASE_DOMAIN"),
            domain_prefix: std::env::var("HOSTLET_DOMAIN_PREFIX")
                .unwrap_or_else(|_| "hostlet-".into()),
            cloudflare_api_token: nonempty_env("CLOUDFLARE_API_TOKEN"),
            cloudflare_zone_id: nonempty_env("CLOUDFLARE_ZONE_ID"),
            cloudflare_tunnel_target: nonempty_env("CLOUDFLARE_TUNNEL_TARGET"),
            job_signing_secret: secret_from_env(
                "JOB_SIGNING_SECRET",
                DEV_JOB_SIGNING_SECRET,
                allow_insecure_dev_defaults,
            )?,
            session_secret: secret_from_env(
                "SESSION_SECRET",
                DEV_SESSION_SECRET,
                allow_insecure_dev_defaults,
            )?,
            allowed_github_logins,
            agents: Arc::new(RwLock::new(HashMap::new())),
            logs,
        })
    }
}

async fn seed_local_server(db: &PgPool, local_agent_token: &str) -> anyhow::Result<()> {
    let local_server_id = std::env::var("LOCAL_SERVER_ID")
        .unwrap_or_else(|_| "00000000-0000-0000-0000-000000000001".into());
    sqlx::query(
        "INSERT INTO servers (id,user_id,name,public_ip,kind,agent_token_hash,status)
         VALUES ($1,NULL,'This machine','127.0.0.1','local',$2,'offline')
         ON CONFLICT (id) DO UPDATE SET agent_token_hash=EXCLUDED.agent_token_hash, kind='local', name='This machine'",
    )
    .bind(uuid::Uuid::parse_str(&local_server_id)?)
    .bind(hash_token(local_agent_token))
    .execute(db)
    .await?;
    Ok(())
}

fn secret_from_env(
    key: &str,
    dev_default: &str,
    allow_insecure_dev_defaults: bool,
) -> anyhow::Result<String> {
    let Some(value) = nonempty_env(key) else {
        if allow_insecure_dev_defaults {
            return Ok(dev_default.to_string());
        }
        bail!("{key} is required in secure mode");
    };
    if !allow_insecure_dev_defaults && (value == dev_default || value.len() < 32) {
        bail!("{key} must be a non-default value with at least 32 characters");
    }
    Ok(value)
}

fn nonempty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn bool_env(key: &str) -> bool {
    std::env::var(key)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn allowed_github_logins() -> Option<HashSet<String>> {
    let values = nonempty_env("HOSTLET_ALLOWED_GITHUB_LOGINS")?;
    let logins = values
        .split(',')
        .map(|login| login.trim().to_ascii_lowercase())
        .filter(|login| !login.is_empty())
        .collect::<HashSet<_>>();
    (!logins.is_empty()).then_some(logins)
}
