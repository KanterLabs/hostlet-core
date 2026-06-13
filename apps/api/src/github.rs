mod access_token;
mod inference;

use crate::{
    auth::{current_user_id, request_context},
    crypto::verify_signature,
    deploy::create_and_send_deploy,
    state::AppState,
};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use hostlet_contracts::{parse_github_repo, valid_commit_sha};
use inference::{
    dockerfile_inspection, gitea_inspection, infer_dockerfile, infer_package_json, node_inspection,
    railpack_inspection, unknown_inspection,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct RepoInspectRequest {
    repo_url: Option<String>,
    repo_full_name: Option<String>,
    branch: Option<String>,
}

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

    let ciphertext = access_token::latest_ciphertext(&state, user_id).await;
    let Ok(Some(ciphertext)) = ciphertext else {
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

    let Ok(token) = state.crypto.decrypt(&ciphertext) else {
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
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let user_id = context.user_id;
    let ciphertext = access_token::latest_ciphertext(&state, user_id).await;
    let Ok(Some(ciphertext)) = ciphertext else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Ok(token) = state.crypto.decrypt(&ciphertext) else {
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
            Err(err) => {
                tracing::warn!(error = %err, "GitHub repository request failed");
                (
                    StatusCode::BAD_GATEWAY,
                    "GitHub repositories could not be loaded",
                )
                    .into_response()
            }
        },
        Err(err) => {
            tracing::warn!(error = %err, "GitHub repository request could not be sent");
            (
                StatusCode::BAD_GATEWAY,
                "GitHub repositories could not be loaded",
            )
                .into_response()
        }
    }
}

pub async fn repo_inspect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RepoInspectRequest>,
) -> impl IntoResponse {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let user_id = context.user_id;
    let repo = body
        .repo_full_name
        .as_deref()
        .or(body.repo_url.as_deref())
        .and_then(parse_github_repo);
    let Some(repo) = repo else {
        return (
            StatusCode::BAD_REQUEST,
            "repo must be a GitHub owner/repo or URL",
        )
            .into_response();
    };
    let token = github_access_token_for_user(&state, user_id).await;
    match inspect_repo(&state, &repo, body.branch.as_deref(), token.as_deref()).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, repo, "GitHub repository inspection failed");
            let (status, body) = repo_inspect_failure(github_error_http_status(&err));
            (status, body).into_response()
        }
    }
}

/// Walks the error chain to find the first `reqwest::Error` that carries an HTTP
/// status code, returning it as a raw `u16`.  Self-contained inside `github.rs`
/// so that cloud's fork of this file can adopt it independently.
fn github_error_http_status(err: &anyhow::Error) -> Option<u16> {
    for cause in err.chain() {
        if let Some(reqwest_err) = cause.downcast_ref::<reqwest::Error>() {
            if let Some(status) = reqwest_err.status() {
                return Some(status.as_u16());
            }
        }
    }
    None
}

/// Maps an optional HTTP status from a failed `inspect_repo` call to an
/// actionable response status + user-facing message.  Pure function; unit-tested.
fn repo_inspect_failure(status: Option<u16>) -> (StatusCode, String) {
    match status {
        Some(404) => (
            StatusCode::NOT_FOUND,
            "GitHub repository was not found, or your GitHub connection cannot access it. \
             Check the owner/repo name or reconnect GitHub."
                .into(),
        ),
        Some(s @ 401) | Some(s @ 403) => (
            StatusCode::BAD_GATEWAY,
            format!(
                "GitHub rejected the repository inspection (HTTP {s}). \
                 Reconnect GitHub, or wait and retry if you are rate limited."
            ),
        ),
        Some(429) => (
            StatusCode::BAD_GATEWAY,
            "GitHub rate-limited the repository inspection. \
             Wait a minute and retry, or connect GitHub to raise the limit."
                .into(),
        ),
        _ => (
            StatusCode::BAD_GATEWAY,
            "GitHub repository could not be inspected".into(),
        ),
    }
}

async fn github_access_token_for_user(state: &AppState, user_id: Uuid) -> Option<String> {
    access_token::latest_decrypted(state, user_id).await
}

async fn inspect_repo(
    state: &AppState,
    repo: &str,
    requested_branch: Option<&str>,
    token: Option<&str>,
) -> anyhow::Result<Value> {
    let mut repo_request = state
        .http
        .get(format!("https://api.github.com/repos/{repo}"));
    if let Some(token) = token {
        repo_request = repo_request.bearer_auth(token);
    }
    let repo_meta: Value = repo_request
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "Hostlet")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let default_branch = repo_meta
        .get("default_branch")
        .and_then(|v| v.as_str())
        .unwrap_or("main");
    let branch = requested_branch
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_branch);
    if repo.eq_ignore_ascii_case("go-gitea/gitea") {
        return Ok(gitea_inspection(repo, branch, default_branch));
    }

    let dockerfile = github_file_text(state, repo, branch, "Dockerfile", token).await?;

    if let Some(package_text) = github_file_text(state, repo, branch, "package.json", token).await?
    {
        let inference = infer_package_json(
            &package_text,
            github_file_text(state, repo, branch, "bun.lock", token)
                .await?
                .is_some()
                || github_file_text(state, repo, branch, "bun.lockb", token)
                    .await?
                    .is_some(),
            github_file_text(state, repo, branch, "pnpm-lock.yaml", token)
                .await?
                .is_some(),
            github_file_text(state, repo, branch, "yarn.lock", token)
                .await?
                .is_some(),
        );
        return Ok(node_inspection(
            repo,
            branch,
            default_branch,
            inference,
            dockerfile.is_some(),
        ));
    }

    if let Some(contents) = dockerfile {
        let inference = infer_dockerfile(&contents);
        return Ok(dockerfile_inspection(
            repo,
            branch,
            default_branch,
            inference,
        ));
    }

    for (path, language) in [
        ("requirements.txt", "Python"),
        ("pyproject.toml", "Python"),
        ("go.mod", "Go"),
        ("Cargo.toml", "Rust"),
        ("index.html", "static"),
    ] {
        if github_file_text(state, repo, branch, path, token)
            .await?
            .is_some()
        {
            return Ok(railpack_inspection(repo, branch, default_branch, language));
        }
    }

    Ok(unknown_inspection(repo, branch, default_branch))
}

async fn github_file_text(
    state: &AppState,
    repo: &str,
    branch: &str,
    path: &str,
    token: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let url = format!(
        "https://api.github.com/repos/{repo}/contents/{path}?ref={}",
        url::form_urlencoded::byte_serialize(branch.as_bytes()).collect::<String>()
    );
    let mut request = state
        .http
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "Hostlet");
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let value: Value = response.error_for_status()?.json().await?;
    let Some(download_url) = value.get("download_url").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    Ok(Some(
        state
            .http
            .get(download_url)
            .header("User-Agent", "Hostlet")
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?,
    ))
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
    let ciphertext = access_token::latest_ciphertext(state, user_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("connect GitHub before enabling auto deploy"))?;
    state
        .crypto
        .decrypt(&ciphertext)
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
    let webhook_secret = &state.github_webhook_secret;
    if !verify_signature(webhook_secret, &body, sig) {
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
            mark_push_processed(&state, delivery, branch, sha, Some("branch was deleted")).await;
            return StatusCode::ACCEPTED.into_response();
        }
        if !valid_commit_sha(sha) {
            mark_push_processed(
                &state,
                delivery,
                branch,
                sha,
                Some("push did not include a valid commit SHA"),
            )
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
            mark_push_processed(
                &state,
                delivery,
                branch,
                sha,
                Some("no apps matched this repository and branch"),
            )
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
        mark_push_processed(&state, delivery, branch, sha, None).await;
    } else {
        let _ = sqlx::query("UPDATE webhook_events SET ignored_reason='unsupported event type', processed=true, processed_at=now() WHERE github_delivery_id=$1")
            .bind(delivery)
            .execute(&state.db)
            .await;
    }
    StatusCode::ACCEPTED.into_response()
}

/// Mark a `push` webhook event processed, recording the resolved branch/commit
/// and an optional reason it produced no deployments. Consolidates the four
/// previously-inline `UPDATE webhook_events SET ...` statements that differed
/// only by their `ignored_reason`.
async fn mark_push_processed(
    state: &AppState,
    delivery: &str,
    branch: &str,
    sha: &str,
    ignored_reason: Option<&str>,
) {
    let sql = if ignored_reason.is_some() {
        "UPDATE webhook_events SET branch=$2, commit_sha=$3, ignored_reason=$4, \
         processed=true, processed_at=now() WHERE github_delivery_id=$1"
    } else {
        "UPDATE webhook_events SET branch=$2, commit_sha=$3, \
         processed=true, processed_at=now() WHERE github_delivery_id=$1"
    };
    let mut query = sqlx::query(sql).bind(delivery).bind(branch).bind(sha);
    if let Some(reason) = ignored_reason {
        query = query.bind(reason);
    }
    let _ = query.execute(&state.db).await;
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

#[cfg(test)]
mod tests {
    use super::{repo_inspect_failure, StatusCode};
    use hostlet_contracts::{parse_github_repo, valid_commit_sha};

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

    #[test]
    fn parses_github_repo_inputs() {
        assert_eq!(
            parse_github_repo("https://github.com/go-gitea/gitea"),
            Some("go-gitea/gitea".into())
        );
        assert_eq!(
            parse_github_repo("git@github.com:owner/repo.git"),
            Some("owner/repo".into())
        );
        assert_eq!(parse_github_repo("owner/repo"), Some("owner/repo".into()));
        assert_eq!(parse_github_repo("https://example.com/owner/repo"), None);
    }

    #[test]
    fn repo_inspect_failure_404_gives_not_found_with_check_name_hint() {
        let (status, body) = repo_inspect_failure(Some(404));
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(
            body.contains("not found"),
            "body should mention 'not found': {body}"
        );
        assert!(
            body.contains("owner/repo") || body.contains("reconnect"),
            "body should hint at fix: {body}"
        );
    }

    #[test]
    fn repo_inspect_failure_401_gives_bad_gateway_with_reconnect_hint() {
        let (status, body) = repo_inspect_failure(Some(401));
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(body.contains("401"), "body should include status: {body}");
        assert!(
            body.contains("Reconnect"),
            "body should suggest reconnect: {body}"
        );
    }

    #[test]
    fn repo_inspect_failure_403_gives_bad_gateway() {
        let (status, body) = repo_inspect_failure(Some(403));
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(body.contains("403"), "body should include status: {body}");
    }

    #[test]
    fn repo_inspect_failure_429_gives_rate_limit_message() {
        let (status, body) = repo_inspect_failure(Some(429));
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(
            body.contains("rate-limited"),
            "body should mention rate limit: {body}"
        );
    }

    #[test]
    fn repo_inspect_failure_other_and_none_give_generic_bad_gateway() {
        for input in [None, Some(500u16), Some(503)] {
            let (status, body) = repo_inspect_failure(input);
            assert_eq!(status, StatusCode::BAD_GATEWAY);
            assert_eq!(
                body, "GitHub repository could not be inspected",
                "unexpected body for {input:?}"
            );
        }
    }
}
