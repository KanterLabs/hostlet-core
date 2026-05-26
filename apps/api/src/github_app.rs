use crate::state::{AppState, HostletMode};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

#[derive(Serialize)]
struct GitHubAppClaims {
    iat: i64,
    exp: i64,
    iss: String,
}

#[derive(Deserialize)]
struct InstallationTokenResponse {
    token: String,
}

#[derive(Debug, Deserialize)]
pub struct GitHubInstallationInfo {
    pub account: Option<GitHubInstallationAccount>,
    pub permissions: Option<Value>,
    pub repository_selection: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubInstallationAccount {
    pub login: String,
    #[serde(rename = "type")]
    pub account_type: String,
}

pub async fn fetch_installation_info(
    state: &AppState,
    installation_id: i64,
) -> anyhow::Result<GitHubInstallationInfo> {
    let jwt = github_app_jwt(state)?;
    state
        .http
        .get(format!(
            "https://api.github.com/app/installations/{installation_id}"
        ))
        .bearer_auth(jwt)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "Hostlet")
        .send()
        .await?
        .error_for_status()?
        .json::<GitHubInstallationInfo>()
        .await
        .map_err(Into::into)
}

pub async fn repositories_for_user(state: &AppState, app_user_id: Uuid) -> anyhow::Result<Value> {
    let token = installation_token_for_app_user(state, app_user_id, None)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Install the Hostlet GitHub App before importing repos"))?;
    state
        .http
        .get("https://api.github.com/installation/repositories?per_page=100")
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "Hostlet")
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await
        .map_err(Into::into)
}

pub async fn installation_token_for_app_user(
    state: &AppState,
    app_user_id: Uuid,
    repo_full_name: Option<&str>,
) -> anyhow::Result<Option<String>> {
    if state.mode != HostletMode::Cloud {
        return Ok(None);
    }
    let installations = sqlx::query(
        "SELECT cgi.installation_id
         FROM cloud_github_installations cgi
         JOIN cloud_users cu ON cu.id=cgi.cloud_user_id
         JOIN users u ON u.github_id=cu.github_id
         WHERE u.id=$1 AND cgi.suspended_at IS NULL
         ORDER BY cgi.updated_at DESC",
    )
    .bind(app_user_id)
    .fetch_all(&state.db)
    .await?;

    for row in installations {
        let installation_id: i64 = row.get("installation_id");
        let token = create_installation_token(state, installation_id).await?;
        let Some(repo) = repo_full_name else {
            return Ok(Some(token));
        };
        if installation_can_access_repo(state, &token, repo).await? {
            return Ok(Some(token));
        }
    }

    if repo_full_name.is_some() {
        anyhow::bail!("The Hostlet GitHub App is not installed for this repository");
    }
    Ok(None)
}

async fn create_installation_token(
    state: &AppState,
    installation_id: i64,
) -> anyhow::Result<String> {
    let jwt = github_app_jwt(state)?;
    let response = state
        .http
        .post(format!(
            "https://api.github.com/app/installations/{installation_id}/access_tokens"
        ))
        .bearer_auth(jwt)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "Hostlet")
        .send()
        .await?
        .error_for_status()?
        .json::<InstallationTokenResponse>()
        .await?;
    Ok(response.token)
}

async fn installation_can_access_repo(
    state: &AppState,
    token: &str,
    repo_full_name: &str,
) -> anyhow::Result<bool> {
    let response = state
        .http
        .get(format!("https://api.github.com/repos/{repo_full_name}"))
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "Hostlet")
        .send()
        .await?;
    if response.status().is_success() {
        return Ok(true);
    }
    if response.status().as_u16() == 403 || response.status().as_u16() == 404 {
        return Ok(false);
    }
    response.error_for_status()?;
    Ok(false)
}

fn github_app_jwt(state: &AppState) -> anyhow::Result<String> {
    let app_id = state
        .github_app_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("GITHUB_APP_ID is missing"))?;
    let private_key = state
        .github_app_private_key_pem
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("GITHUB_APP_PRIVATE_KEY_PEM is missing"))?;
    let now = Utc::now();
    let claims = GitHubAppClaims {
        iat: (now - Duration::seconds(60)).timestamp(),
        exp: (now + Duration::minutes(9)).timestamp(),
        iss: app_id.to_string(),
    };
    let mut header = Header::new(Algorithm::RS256);
    header.typ = Some("JWT".to_string());
    encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(private_key.as_bytes())?,
    )
    .map_err(Into::into)
}

pub fn missing_cloud_github_app_config(state: &AppState) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if state.github_app_id.is_none() {
        missing.push("GITHUB_APP_ID");
    }
    if state.github_app_client_id.is_none() {
        missing.push("GITHUB_APP_CLIENT_ID");
    }
    if state.github_app_client_secret.is_none() {
        missing.push("GITHUB_APP_CLIENT_SECRET");
    }
    if state.github_app_private_key_pem.is_none() {
        missing.push("GITHUB_APP_PRIVATE_KEY_PEM");
    }
    if state.github_app_slug.is_none() {
        missing.push("GITHUB_APP_SLUG");
    }
    missing
}

pub fn installation_info_defaults(info: GitHubInstallationInfo) -> (String, String, Value, String) {
    let account_login = info
        .account
        .as_ref()
        .map(|account| account.login.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let account_type = info
        .account
        .as_ref()
        .map(|account| account.account_type.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let permissions = info.permissions.unwrap_or_else(|| json!({}));
    let repository_selection = info
        .repository_selection
        .unwrap_or_else(|| "selected".to_string());
    (
        account_login,
        account_type,
        permissions,
        repository_selection,
    )
}
