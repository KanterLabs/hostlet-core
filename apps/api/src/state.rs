use crate::crypto::{hash_token, Crypto};
use crate::rate_limit::RateLimiter;
use anyhow::{bail, Context};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio::sync::{broadcast, mpsc, RwLock};
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub crypto: Crypto,
    pub http: reqwest::Client,
    pub github_client_id: String,
    pub github_webhook_secret: String,
    pub public_webhook_url: String,
    pub public_api_url: String,
    pub public_web_url: String,
    pub allowed_web_origins: Vec<String>,
    pub base_domain: Option<String>,
    pub domain_prefix: String,
    pub cloudflare_api_token: Option<String>,
    pub cloudflare_zone_id: Option<String>,
    pub cloudflare_tunnel_target: Option<String>,
    pub job_signing_secret: String,
    pub session_secret: String,
    pub setup_token: Option<String>,
    pub allowed_github_logins: Option<HashSet<String>>,
    pub update_checks_enabled: bool,
    pub agents: Arc<RwLock<HashMap<Uuid, AgentConnection>>>,
    pub rate_limiter: Arc<RateLimiter>,
    pub logs: broadcast::Sender<LogEvent>,
}

#[derive(Clone)]
pub struct AgentConnection {
    pub connection_id: Uuid,
    pub sender: mpsc::Sender<serde_json::Value>,
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
        let http = http_client()?;
        let local_agent_token = secret_from_env("LOCAL_AGENT_TOKEN", allow_insecure_dev_defaults)?;
        let job_signing_secret =
            secret_from_env("JOB_SIGNING_SECRET", allow_insecure_dev_defaults)?;
        let allowed_github_logins = allowed_github_logins();
        if !allow_insecure_dev_defaults && allowed_github_logins.is_none() {
            bail!("HOSTLET_ALLOWED_GITHUB_LOGINS is required in secure mode");
        }
        let public_api_url =
            std::env::var("PUBLIC_API_URL").unwrap_or_else(|_| "http://localhost:8080".into());
        let public_webhook_url = std::env::var("PUBLIC_WEBHOOK_URL")
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| public_api_url.trim_end_matches('/').to_string());
        let public_web_url =
            std::env::var("PUBLIC_WEB_URL").unwrap_or_else(|_| "http://localhost:3000".into());
        let allowed_web_origins =
            allowed_web_origins(&public_web_url, allow_insecure_dev_defaults)?;
        let setup_token = nonempty_env("HOSTLET_SETUP_TOKEN");
        if !allow_insecure_dev_defaults && setup_token.is_none() {
            bail!("HOSTLET_SETUP_TOKEN is required in secure mode for first-run setup");
        }
        seed_local_server(&db, &crypto, &local_agent_token, &job_signing_secret).await?;
        let (logs, _) = broadcast::channel(1024);
        Ok(Self {
            db,
            crypto,
            http,
            github_client_id: std::env::var("GITHUB_CLIENT_ID").unwrap_or_default(),
            github_webhook_secret: secret_from_env(
                "GITHUB_WEBHOOK_SECRET",
                allow_insecure_dev_defaults,
            )?,
            public_webhook_url,
            public_api_url,
            public_web_url,
            allowed_web_origins,
            base_domain: nonempty_env("HOSTLET_BASE_DOMAIN")
                .map(|domain| domain.trim_end_matches('.').to_ascii_lowercase()),
            domain_prefix: std::env::var("HOSTLET_DOMAIN_PREFIX")
                .unwrap_or_else(|_| "hostlet-".into())
                .to_ascii_lowercase(),
            cloudflare_api_token: nonempty_env("CLOUDFLARE_API_TOKEN"),
            cloudflare_zone_id: nonempty_env("CLOUDFLARE_ZONE_ID"),
            cloudflare_tunnel_target: nonempty_env("CLOUDFLARE_TUNNEL_TARGET"),
            job_signing_secret,
            session_secret: secret_from_env("SESSION_SECRET", allow_insecure_dev_defaults)?,
            setup_token,
            allowed_github_logins,
            update_checks_enabled: !matches!(
                std::env::var("HOSTLET_UPDATE_CHECKS")
                    .unwrap_or_else(|_| "true".into())
                    .to_ascii_lowercase()
                    .as_str(),
                "0" | "false" | "no" | "off"
            ),
            agents: Arc::new(RwLock::new(HashMap::new())),
            rate_limiter: Arc::new(RateLimiter::default()),
            logs,
        })
    }
}

impl AppState {
    pub fn web_origin_allowed(&self, value: &str) -> bool {
        normalize_origin(value).as_deref().is_some_and(|origin| {
            self.allowed_web_origins
                .iter()
                .any(|allowed| allowed == origin)
        })
    }
}

async fn seed_local_server(
    db: &PgPool,
    crypto: &Crypto,
    local_agent_token: &str,
    job_signing_secret: &str,
) -> anyhow::Result<()> {
    let local_server_id = std::env::var("LOCAL_SERVER_ID")
        .unwrap_or_else(|_| "00000000-0000-0000-0000-000000000001".into());
    let public_ip = std::env::var("HOSTLET_PRIVATE_APP_HOST")
        .or_else(|_| std::env::var("LOCAL_SERVER_PUBLIC_IP"))
        .unwrap_or_else(|_| "127.0.0.1".into());
    sqlx::query(
        "INSERT INTO servers (id,user_id,name,public_ip,kind,agent_token_hash,job_signing_secret_ciphertext,status)
         VALUES ($1,NULL,'This machine',$2,'local',$3,$4,'offline')
         ON CONFLICT (id) DO UPDATE SET
           agent_token_hash=EXCLUDED.agent_token_hash,
           job_signing_secret_ciphertext=EXCLUDED.job_signing_secret_ciphertext,
           kind='local',
           name='This machine',
           public_ip=EXCLUDED.public_ip",
    )
    .bind(uuid::Uuid::parse_str(&local_server_id)?)
    .bind(public_ip)
    .bind(hash_token(local_agent_token))
    .bind(crypto.encrypt(job_signing_secret)?)
    .execute(db)
    .await?;
    Ok(())
}

fn secret_from_env(key: &str, allow_insecure_dev_defaults: bool) -> anyhow::Result<String> {
    let Some(value) = nonempty_env(key) else {
        bail!("{key} is required");
    };
    if !allow_insecure_dev_defaults && value.len() < 32 {
        bail!("{key} must be at least 32 characters");
    }
    Ok(value)
}

fn http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .user_agent("Hostlet")
        .build()
        .context("failed to build HTTP client")
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

fn allowed_web_origins(
    public_web_url: &str,
    allow_insecure_dev_defaults: bool,
) -> anyhow::Result<Vec<String>> {
    let mut origins = Vec::new();
    push_origin(&mut origins, public_web_url)?;
    if allow_insecure_dev_defaults {
        push_origin(&mut origins, "http://localhost:3000")?;
        push_origin(&mut origins, "http://127.0.0.1:3000")?;
    }
    if let Some(extra) = nonempty_env("HOSTLET_ALLOWED_WEB_ORIGINS") {
        for origin in extra
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
        {
            push_origin(&mut origins, origin)?;
        }
    }
    Ok(origins)
}

fn push_origin(origins: &mut Vec<String>, value: &str) -> anyhow::Result<()> {
    let origin = normalize_origin(value)
        .ok_or_else(|| anyhow::anyhow!("{value} is not a valid http(s) origin"))?;
    if !origins.iter().any(|existing| existing == &origin) {
        origins.push(origin);
    }
    Ok(())
}

pub fn normalize_origin(value: &str) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?;
    let mut origin = format!("{}://{}", url.scheme(), host);
    if let Some(port) = url.port() {
        origin.push_str(&format!(":{port}"));
    }
    Some(origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_origin_without_path() {
        assert_eq!(
            normalize_origin("http://10.0.0.194:3000/settings").as_deref(),
            Some("http://10.0.0.194:3000")
        );
    }

    #[test]
    fn rejects_non_http_origins() {
        assert!(normalize_origin("file:///tmp/index.html").is_none());
    }
}
