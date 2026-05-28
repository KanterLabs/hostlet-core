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
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let user_id = context.user_id;
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
            (
                StatusCode::BAD_GATEWAY,
                "GitHub repository could not be inspected",
            )
                .into_response()
        }
    }
}

async fn github_access_token_for_user(state: &AppState, user_id: Uuid) -> Option<String> {
    let row = sqlx::query("SELECT access_token_ciphertext FROM github_accounts WHERE user_id=$1 ORDER BY updated_at DESC LIMIT 1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()?;
    state
        .crypto
        .decrypt(row.get::<String, _>("access_token_ciphertext").as_str())
        .ok()
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

    if let Some(contents) = github_file_text(state, repo, branch, "Dockerfile", token).await? {
        let inference = infer_dockerfile(&contents);
        return Ok(json!({
            "repoFullName": repo,
            "defaultBranch": default_branch,
            "branch": branch,
            "appName": repo.split('/').nth(1).unwrap_or("app"),
            "deployable": true,
            "runtimeKind": "single",
            "rootDirectory": ".",
            "containerPort": inference.port.unwrap_or(3000),
            "healthPath": "/",
            "hostletConfigPath": "hostlet.yml",
            "runtimeConfig": {},
            "packagingStrategy": "auto",
            "packagingOptions": ["auto", "dockerfile", "generated"],
            "recommendedPackagingStrategy": "auto",
            "env": inference.env,
            "warnings": inference.warnings,
            "summary": "Dockerfile detected. Hostlet inferred a single-container runtime.",
            "autoDeployAvailable": false
        }));
    }

    if let Some(package_text) = github_file_text(state, repo, branch, "package.json", token).await?
    {
        let inference = infer_package_json(
            &package_text,
            github_file_text(state, repo, branch, "pnpm-lock.yaml", token)
                .await?
                .is_some(),
            github_file_text(state, repo, branch, "yarn.lock", token)
                .await?
                .is_some(),
        );
        return Ok(json!({
            "repoFullName": repo,
            "defaultBranch": default_branch,
            "branch": branch,
            "appName": repo.split('/').nth(1).unwrap_or("app"),
            "deployable": true,
            "runtimeKind": "single",
            "rootDirectory": ".",
            "containerPort": 3000,
            "healthPath": "/",
            "hostletConfigPath": "hostlet.yml",
            "runtimeConfig": {},
            "packagingStrategy": "auto",
            "packagingOptions": ["auto", "generated"],
            "recommendedPackagingStrategy": "generated",
            "detectedFramework": inference.framework,
            "packageManager": inference.package_manager,
            "env": [],
            "warnings": ["Node app detected. Hostlet will infer install/build/start commands during deployment; set custom commands if the preview is incomplete."],
            "summary": format!("{} app detected. Hostlet will use optimized generated Docker with {}.", inference.framework, inference.package_manager),
            "autoDeployAvailable": false
        }));
    }

    Ok(json!({
        "repoFullName": repo,
        "defaultBranch": default_branch,
        "branch": branch,
        "appName": repo.split('/').nth(1).unwrap_or("app"),
        "deployable": false,
        "runtimeKind": "single",
        "rootDirectory": ".",
        "containerPort": 3000,
        "healthPath": "/",
        "hostletConfigPath": "hostlet.yml",
        "runtimeConfig": {},
        "packagingStrategy": "auto",
        "packagingOptions": ["auto"],
        "recommendedPackagingStrategy": "auto",
        "env": [],
        "warnings": ["No root Dockerfile or package.json was found. Add a Dockerfile, package.json, or Hostlet Compose manifest before deploying."],
        "summary": "Hostlet could not infer a runnable app shape.",
        "autoDeployAvailable": false
    }))
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

struct DockerfileInference {
    port: Option<i32>,
    env: Vec<Value>,
    warnings: Vec<String>,
}

struct PackageInference {
    framework: &'static str,
    package_manager: &'static str,
}

fn infer_package_json(
    contents: &str,
    has_pnpm_lock: bool,
    has_yarn_lock: bool,
) -> PackageInference {
    let package: Value = serde_json::from_str(contents).unwrap_or_else(|_| json!({}));
    let mut deps = std::collections::HashSet::new();
    for key in ["dependencies", "devDependencies"] {
        if let Some(map) = package.get(key).and_then(|value| value.as_object()) {
            deps.extend(map.keys().map(String::as_str));
        }
    }
    let framework = if deps.contains("next") {
        "Next.js"
    } else if deps.contains("astro") {
        "Astro"
    } else if deps.contains("nuxt") {
        "Nuxt"
    } else if deps.contains("@remix-run/node") || deps.contains("@remix-run/react") {
        "Remix"
    } else if deps.contains("@sveltejs/kit") {
        "SvelteKit"
    } else if deps.contains("vite") {
        "Vite"
    } else {
        "Node"
    };
    let package_manager = if has_pnpm_lock {
        "pnpm"
    } else if has_yarn_lock {
        "yarn"
    } else {
        "npm"
    };
    PackageInference {
        framework,
        package_manager,
    }
}

fn infer_dockerfile(contents: &str) -> DockerfileInference {
    let mut ports = Vec::new();
    let mut env = Vec::new();
    let mut warnings = vec![
        "Public Dockerfiles run arbitrary build steps on this machine. Review the upstream project before deploying.".to_string(),
    ];
    for line in contents.lines().map(str::trim) {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("EXPOSE ") {
            for token in line[7..].split_whitespace() {
                let port = token
                    .split('/')
                    .next()
                    .and_then(|part| part.parse::<i32>().ok());
                if let Some(port) = port {
                    ports.push(port);
                }
            }
        } else if upper.starts_with("ENV ") {
            for item in line[4..].split_whitespace() {
                let key = item.split('=').next().unwrap_or("").trim();
                if valid_env_prompt_key(key) {
                    env.push(json!({"key": key, "required": false, "value": "", "source": "Dockerfile ENV"}));
                }
            }
        } else if upper.starts_with("ARG ") {
            let key = line[4..].split('=').next().unwrap_or("").trim();
            if valid_env_prompt_key(key) {
                warnings.push(format!("Dockerfile declares build arg {key}; Hostlet does not prompt for build args yet."));
            }
        } else if upper.starts_with("VOLUME ") {
            warnings.push("Dockerfile declares volumes. Hostlet provides /data automatically; verify the app persists data where expected.".into());
        }
    }
    ports.sort_unstable();
    ports.dedup();
    let preferred = [3000, 8080, 8000, 80, 5000, 4000]
        .into_iter()
        .find(|port| ports.contains(port))
        .or_else(|| ports.iter().copied().find(|port| *port != 22));
    if ports.len() > 1 {
        warnings.push(format!(
            "Dockerfile exposes multiple ports ({ports:?}); Hostlet selected {}.",
            preferred.unwrap_or(3000)
        ));
    }
    DockerfileInference {
        port: preferred,
        env,
        warnings,
    }
}

fn gitea_inspection(repo: &str, branch: &str, default_branch: &str) -> Value {
    json!({
        "repoFullName": repo,
        "defaultBranch": default_branch,
        "branch": branch,
        "appName": "gitea",
        "deployable": true,
        "runtimeKind": "compose",
        "rootDirectory": ".",
        "containerPort": 3000,
        "healthPath": "/",
        "hostletConfigPath": "hostlet.yml",
        "runtimeConfig": {
            "generatedCompose": {
                "composeFile": "compose.generated.hostlet.yml",
                "webService": "server",
                "port": 3000,
                "healthPath": "/",
                "compose": "services:\n  server:\n    image: docker.gitea.com/gitea:latest-rootless\n    restart: unless-stopped\n    environment:\n      GITEA__server__DOMAIN: localhost\n      GITEA__server__HTTP_PORT: \"3000\"\n      GITEA__database__DB_TYPE: sqlite3\n    volumes:\n      - gitea-data:/var/lib/gitea\n      - gitea-config:/etc/gitea\nvolumes:\n  gitea-data:\n  gitea-config:\n"
            }
        },
        "packagingStrategy": "auto",
        "packagingOptions": ["auto"],
        "recommendedPackagingStrategy": "auto",
        "env": [],
        "warnings": ["Gitea SSH Git access is not exposed in Hostlet 0.3.9; use HTTPS Git through the web route.", "The generated Gitea default uses SQLite and named Docker volumes for the simplest self-hosted setup."],
        "summary": "Gitea detected. Hostlet will use the official rootless image with SQLite and persistent named volumes.",
        "autoDeployAvailable": false
    })
}

fn valid_env_prompt_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
}

fn parse_github_repo(input: &str) -> Option<String> {
    let trimmed = input.trim().trim_end_matches(".git");
    if let Some(caps) = trimmed
        .strip_prefix("git@github.com:")
        .and_then(parse_owner_repo)
    {
        return Some(caps);
    }
    if let Ok(url) = url::Url::parse(trimmed) {
        if url.host_str()? != "github.com" {
            return None;
        }
        return parse_owner_repo(url.path().trim_start_matches('/'));
    }
    parse_owner_repo(trimmed)
}

fn parse_owner_repo(value: &str) -> Option<String> {
    let mut parts = value.split('/').filter(|part| !part.is_empty());
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next().is_some() || !valid_repo_part(owner) || !valid_repo_part(repo) {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

fn valid_repo_part(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !value.starts_with('.')
        && !value.ends_with('.')
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
    use super::{
        gitea_inspection, infer_dockerfile, infer_package_json, parse_github_repo, valid_commit_sha,
    };

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
    fn dockerfile_inference_prefers_web_port_and_prompts_env() {
        let inference = infer_dockerfile(
            r#"
FROM alpine
ENV APP_SECRET=
ARG BUILD_TOKEN
EXPOSE 22 3000/tcp
VOLUME ["/data"]
"#,
        );
        assert_eq!(inference.port, Some(3000));
        assert!(inference.env.iter().any(|item| item["key"] == "APP_SECRET"));
        assert!(inference
            .warnings
            .iter()
            .any(|warning| warning.contains("multiple ports")));
        assert!(inference
            .warnings
            .iter()
            .any(|warning| warning.contains("BUILD_TOKEN")));
    }

    #[test]
    fn gitea_inspection_returns_generated_compose() {
        let value = gitea_inspection("go-gitea/gitea", "main", "main");
        assert_eq!(value["deployable"], true);
        assert_eq!(value["runtimeKind"], "compose");
        assert_eq!(
            value.pointer("/runtimeConfig/generatedCompose/webService"),
            Some(&serde_json::json!("server"))
        );
        assert!(value["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("SSH Git access")));
    }

    #[test]
    fn package_json_inference_detects_framework_and_package_manager() {
        let inference = infer_package_json(
            r#"{"dependencies":{"next":"16.0.0"},"devDependencies":{}}"#,
            true,
            false,
        );
        assert_eq!(inference.framework, "Next.js");
        assert_eq!(inference.package_manager, "pnpm");
    }
}
