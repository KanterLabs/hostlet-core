use crate::{
    auth::current_user_id, crypto::verify_signature, deploy::create_and_send_deploy,
    state::AppState,
};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

pub async fn status(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let oauth_configured = !state.github_client_id.trim().is_empty();
    let webhook_configured = !state.github_webhook_secret.trim().is_empty();

    let Some(user_id) = current_user_id(&headers, &state) else {
        return Json(serde_json::json!({
            "oauthConfigured": oauth_configured,
            "webhookConfigured": webhook_configured,
            "authenticated": false,
            "tokenValid": null,
            "login": null,
            "message": if oauth_configured { "GitHub Device Flow is configured. Connect GitHub to verify your account token." } else { "GitHub Device Flow is missing GITHUB_CLIENT_ID." }
        })).into_response();
    };

    let row = sqlx::query("SELECT access_token_ciphertext FROM github_accounts WHERE user_id=$1 ORDER BY updated_at DESC LIMIT 1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await;
    let Ok(Some(row)) = row else {
        return Json(serde_json::json!({
            "oauthConfigured": oauth_configured,
            "webhookConfigured": webhook_configured,
            "authenticated": true,
            "tokenValid": false,
            "login": null,
            "message": "No GitHub token is stored for this session. Connect GitHub again."
        }))
        .into_response();
    };

    let Ok(token) = state
        .crypto
        .decrypt(row.get::<String, _>("access_token_ciphertext").as_str())
    else {
        return Json(serde_json::json!({
            "oauthConfigured": oauth_configured,
            "webhookConfigured": webhook_configured,
            "authenticated": true,
            "tokenValid": false,
            "login": null,
            "message": "Stored GitHub token could not be decrypted. Check ENCRYPTION_KEY."
        }))
        .into_response();
    };

    let res = state
        .http
        .get("https://api.github.com/user")
        .bearer_auth(token)
        .header("User-Agent", "Hostlet")
        .send()
        .await;

    match res {
        Ok(resp) if resp.status().is_success() => {
            let user = resp.json::<Value>().await.unwrap_or_default();
            Json(serde_json::json!({
                "oauthConfigured": oauth_configured,
                "webhookConfigured": webhook_configured,
                "authenticated": true,
                "tokenValid": true,
                "login": user.get("login").and_then(|v| v.as_str()),
                "message": "GitHub is connected and the token is valid."
            })).into_response()
        }
        Ok(resp) => Json(serde_json::json!({
            "oauthConfigured": oauth_configured,
            "webhookConfigured": webhook_configured,
            "authenticated": true,
            "tokenValid": false,
            "login": null,
            "message": format!("GitHub token check failed with status {}. Connect GitHub again.", resp.status())
        })).into_response(),
        Err(_) => Json(serde_json::json!({
            "oauthConfigured": oauth_configured,
            "webhookConfigured": webhook_configured,
            "authenticated": true,
            "tokenValid": false,
            "login": null,
            "message": "Could not reach GitHub from the API container."
        })).into_response(),
    }
}

pub async fn repos(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let row = sqlx::query("SELECT access_token_ciphertext FROM github_accounts WHERE user_id=$1 ORDER BY updated_at DESC LIMIT 1").bind(user_id).fetch_optional(&state.db).await;
    let Ok(Some(row)) = row else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Ok(token) = state
        .crypto
        .decrypt(row.get::<String, _>("access_token_ciphertext").as_str())
    else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let res = state
        .http
        .get("https://api.github.com/user/repos?per_page=100&sort=updated")
        .bearer_auth(token)
        .header("User-Agent", "Hostlet")
        .send()
        .await;
    match res {
        Ok(r) => match r.error_for_status() {
            Ok(r) => match r.json::<Value>().await {
                Ok(Value::Array(items)) => Json(Value::Array(items)).into_response(),
                Ok(_) => (
                    StatusCode::BAD_GATEWAY,
                    "GitHub returned an unexpected repository payload",
                )
                    .into_response(),
                Err(_) => StatusCode::BAD_GATEWAY.into_response(),
            },
            Err(err) => (
                StatusCode::BAD_GATEWAY,
                format!("GitHub repository request failed: {err}"),
            )
                .into_response(),
        },
        Err(_) => StatusCode::BAD_GATEWAY.into_response(),
    }
}

pub async fn ensure_repo_webhook(
    state: &AppState,
    user_id: Uuid,
    repo_full_name: &str,
) -> anyhow::Result<()> {
    let token = github_access_token(state, user_id).await?;
    let webhook_url = format!(
        "{}/webhooks/github",
        state.public_webhook_url.trim_end_matches('/')
    );
    let hooks_url = format!("https://api.github.com/repos/{repo_full_name}/hooks");
    let hooks = state
        .http
        .get(&hooks_url)
        .bearer_auth(&token)
        .send()
        .await?
        .error_for_status()
        .map_err(|err| {
            anyhow::anyhow!(
                "could not list GitHub webhooks for {repo_full_name}; reconnect GitHub with repo hook permissions: {err}"
            )
        })?
        .json::<Value>()
        .await?;

    if let Some(existing) = hooks.as_array().and_then(|items| {
        items.iter().find(|hook| {
            hook.pointer("/config/url")
                .and_then(|value| value.as_str())
                .is_some_and(|url| url == webhook_url)
        })
    }) {
        let Some(id) = existing.get("id").and_then(|value| value.as_i64()) else {
            anyhow::bail!("GitHub returned a webhook without an id");
        };
        let patch_url = format!("{hooks_url}/{id}");
        state
            .http
            .patch(patch_url)
            .bearer_auth(&token)
            .json(&json!({
                "active": true,
                "events": ["push"],
                "config": {
                    "url": webhook_url,
                    "content_type": "json",
                    "secret": state.github_webhook_secret,
                },
            }))
            .send()
            .await?
            .error_for_status()
            .map_err(|err| {
                anyhow::anyhow!("could not update GitHub webhook for {repo_full_name}: {err}")
            })?;
        return Ok(());
    }

    state
        .http
        .post(hooks_url)
        .bearer_auth(&token)
        .json(&json!({
            "name": "web",
            "active": true,
            "events": ["push"],
            "config": {
                "url": webhook_url,
                "content_type": "json",
                "secret": state.github_webhook_secret,
            },
        }))
        .send()
        .await?
        .error_for_status()
        .map_err(|err| {
            anyhow::anyhow!("could not create GitHub webhook for {repo_full_name}: {err}")
        })?;
    Ok(())
}

async fn github_access_token(state: &AppState, user_id: Uuid) -> anyhow::Result<String> {
    let row = sqlx::query(
        "SELECT access_token_ciphertext FROM github_accounts WHERE user_id=$1 ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| anyhow::anyhow!("connect GitHub before enabling auto deploy"))?;
    state
        .crypto
        .decrypt(row.get::<String, _>("access_token_ciphertext").as_str())
        .map_err(|_| anyhow::anyhow!("stored GitHub token could not be decrypted"))
}

pub async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let sig = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !verify_signature(&state.github_webhook_secret, &body, sig) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let event = headers
        .get("x-github-event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let delivery = headers
        .get("x-github-delivery")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let repo = payload
        .pointer("/repository/full_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let inserted = sqlx::query(
        "INSERT INTO webhook_events (github_delivery_id,repo_full_name,event_type,payload)
         VALUES ($1,$2,$3,$4)
         ON CONFLICT DO NOTHING
         RETURNING id",
    )
    .bind(delivery)
    .bind(repo)
    .bind(event)
    .bind(&payload)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    let Some(inserted) = inserted else {
        return StatusCode::ACCEPTED.into_response();
    };
    let webhook_event_id: Uuid = inserted.get("id");
    if event == "push" {
        let branch = payload
            .get("ref")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim_start_matches("refs/heads/");
        let sha = payload
            .get("after")
            .and_then(|v| v.as_str())
            .unwrap_or("HEAD");
        if payload
            .get("deleted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let _ = sqlx::query("UPDATE webhook_events SET branch=$2, commit_sha=$3, ignored_reason='branch was deleted', processed=true, processed_at=now() WHERE github_delivery_id=$1")
                .bind(delivery)
                .bind(branch)
                .bind(sha)
                .execute(&state.db)
                .await;
            return StatusCode::ACCEPTED.into_response();
        }
        if !valid_commit_sha(sha) {
            let _ = sqlx::query("UPDATE webhook_events SET branch=$2, commit_sha=$3, ignored_reason='push did not include a valid commit SHA', processed=true, processed_at=now() WHERE github_delivery_id=$1")
                .bind(delivery)
                .bind(branch)
                .bind(sha)
                .execute(&state.db)
                .await;
            return StatusCode::ACCEPTED.into_response();
        }
        let apps = sqlx::query(
            "SELECT id,user_id,auto_deploy FROM apps WHERE repo_full_name=$1 AND branch=$2",
        )
        .bind(repo)
        .bind(branch)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
        if apps.is_empty() {
            let _ = sqlx::query("UPDATE webhook_events SET branch=$2, commit_sha=$3, ignored_reason='no apps matched this repository and branch', processed=true, processed_at=now() WHERE github_delivery_id=$1")
                .bind(delivery)
                .bind(branch)
                .bind(sha)
                .execute(&state.db)
                .await;
            return StatusCode::ACCEPTED.into_response();
        }
        for app in apps {
            let app_id: Uuid = app.get("id");
            if !app.get::<bool, _>("auto_deploy") {
                let _ = insert_webhook_app_event(
                    &state,
                    webhook_event_id,
                    app_id,
                    None,
                    repo,
                    branch,
                    sha,
                    "ignored",
                    Some("auto redeploy is disabled for this app"),
                )
                .await;
                continue;
            }
            match create_and_send_deploy(&state, app.get("user_id"), app_id, sha).await {
                Ok(deployment_id) => {
                    let _ = insert_webhook_app_event(
                        &state,
                        webhook_event_id,
                        app_id,
                        Some(deployment_id),
                        repo,
                        branch,
                        sha,
                        "deployed",
                        None,
                    )
                    .await;
                }
                Err(err) => {
                    let _ = insert_webhook_app_event(
                        &state,
                        webhook_event_id,
                        app_id,
                        None,
                        repo,
                        branch,
                        sha,
                        "ignored",
                        Some(&err.to_string()),
                    )
                    .await;
                }
            }
        }
        let _ = sqlx::query("UPDATE webhook_events SET branch=$2, commit_sha=$3, processed=true, processed_at=now() WHERE github_delivery_id=$1")
            .bind(delivery)
            .bind(branch)
            .bind(sha)
            .execute(&state.db)
            .await;
    } else {
        let _ = sqlx::query("UPDATE webhook_events SET ignored_reason='unsupported event type', processed=true, processed_at=now() WHERE github_delivery_id=$1")
            .bind(delivery)
            .execute(&state.db)
            .await;
    }
    StatusCode::ACCEPTED.into_response()
}

#[allow(clippy::too_many_arguments)]
async fn insert_webhook_app_event(
    state: &AppState,
    webhook_event_id: Uuid,
    app_id: Uuid,
    deployment_id: Option<Uuid>,
    repo: &str,
    branch: &str,
    sha: &str,
    status: &str,
    ignored_reason: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO webhook_app_events
         (webhook_event_id,app_id,deployment_id,repo_full_name,branch,commit_sha,status,ignored_reason)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(webhook_event_id)
    .bind(app_id)
    .bind(deployment_id)
    .bind(repo)
    .bind(branch)
    .bind(sha)
    .bind(status)
    .bind(ignored_reason)
    .execute(&state.db)
    .await?;
    Ok(())
}

fn valid_commit_sha(value: &str) -> bool {
    value.len() == 40
        && value.chars().all(|c| c.is_ascii_hexdigit())
        && !value.chars().all(|c| c == '0')
}

#[cfg(test)]
mod tests {
    use super::valid_commit_sha;

    #[test]
    fn rejects_branch_delete_zero_sha() {
        assert!(!valid_commit_sha(
            "0000000000000000000000000000000000000000"
        ));
    }

    #[test]
    fn accepts_normal_commit_sha() {
        assert!(valid_commit_sha("0123456789abcdef0123456789abcdef01234567"));
    }
}
