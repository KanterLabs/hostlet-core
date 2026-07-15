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
use hostlet_contracts::compose::{detect_data_mount_path, with_data_mount_path};
use hostlet_contracts::{
    attach_topology_plan, detect_start_command, parse_github_repo, plan_repository_topology,
    valid_commit_sha, with_command_suggestion, RepoCommandFiles, RepositoryFile,
    RepositoryInventory, TopologyReadiness,
};
use inference::{
    compose_inspection, dockerfile_inspection, gitea_inspection, infer_addons_from_compose,
    infer_dockerfile, infer_package_json, infer_service_addons, manifest_dependency_tokens,
    node_inspection, package_json_dependencies, railpack_inspection, unknown_inspection,
    with_detected_services, DetectedServices,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

const GITHUB_INSPECTION_FILE_MAX_BYTES: u64 = 128 * 1024;
const GITHUB_INVENTORY_MAX_FILES: usize = 10_000;
const GITHUB_INVENTORY_MAX_RELEVANT_FILES: usize = 256;
const GITHUB_INVENTORY_MAX_CONTENT_BYTES: usize = 2 * 1024 * 1024;

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

    // An explicit `hostlet.yml` declaring a Compose runtime wins over the
    // package.json/Dockerfile detectors: the repo owner has stated the app is
    // multi-service. We preview the parsed service list and surface any
    // safe-subset violations the agent would reject at deploy time.
    if let Some(manifest_text) = github_file_text(state, repo, branch, "hostlet.yml", token).await?
    {
        if let Some(manifest) =
            hostlet_contracts::compose::HostletComposeManifest::parse_compose(&manifest_text)
        {
            let compose_file = manifest.compose_file();
            let (services, subset_warnings) =
                match github_file_text(state, repo, branch, compose_file, token).await? {
                    Some(compose_text) => (
                        hostlet_contracts::compose::parse_compose_services(
                            &compose_text,
                            &manifest.compose.web_service,
                        ),
                        hostlet_contracts::compose::compose_subset_warnings(
                            &compose_text,
                            &manifest.compose.web_service,
                        ),
                    ),
                    None => (
                        Vec::new(),
                        vec![format!(
                            "hostlet.yml references compose file {compose_file}, which was not found in the repository."
                        )],
                    ),
                };
            return Ok(compose_inspection(
                repo,
                branch,
                default_branch,
                "hostlet.yml",
                &manifest.compose,
                &services,
                &subset_warnings,
            ));
        }
    }

    let procfile = github_file_text(state, repo, branch, "Procfile", token).await?;
    let dockerfile = github_file_text(state, repo, branch, "Dockerfile", token).await?;
    let package_json = github_file_text(state, repo, branch, "package.json", token).await?;
    let railway_json = github_file_text(state, repo, branch, "railway.json", token).await?;
    let render_yaml = match github_file_text(state, repo, branch, "render.yaml", token).await? {
        Some(contents) => Some(("render.yaml", contents)),
        None => github_file_text(state, repo, branch, "render.yml", token)
            .await?
            .map(|contents| ("render.yml", contents)),
    };
    let command_suggestion = detect_start_command(RepoCommandFiles {
        procfile: procfile.as_deref(),
        package_json: package_json.as_deref(),
        railway_json: railway_json.as_deref(),
        render_yaml: render_yaml
            .as_ref()
            .map(|(source_file, contents)| (*source_file, contents.as_str())),
        dockerfile: dockerfile.as_deref(),
    });
    let railpack_config = github_file_text(state, repo, branch, "railpack.json", token).await?;

    // Auto-detected backing services come from two complementary signals: the
    // repo's dependency manifests (per language, below) and a bare compose file's
    // service images (one without a `hostlet.yml` — that explicit form already
    // returned above). Both map to the vetted managed catalog (the repo's own
    // images are never run on a shared host) and feed the same
    // `runtime_config.compose.addOns` the create handler resolves into a
    // generated multi-service stack.
    let (compose_addons, data_mount_path) =
        detect_compose_signals(state, repo, branch, token).await?;

    // When the repository owner has not supplied a runnable root process or an
    // explicit packaging file, inspect the whole bounded tree. This is the
    // zero-config path for workspace coordinators such as a pnpm monorepo whose
    // actual frontend/backend live below packages/. The same pure planner runs
    // again against the immutable checkout in the agent.
    if command_suggestion.is_none() && dockerfile.is_none() && railpack_config.is_none() {
        let inventory = github_repository_inventory(state, repo, branch, token).await?;
        let plan = plan_repository_topology(&inventory);
        if plan.readiness != TopologyReadiness::Unsupported {
            let base = if let Some(package_text) = package_json.as_deref() {
                let inference = infer_package_json(
                    package_text,
                    inventory_has(&inventory, "bun.lock") || inventory_has(&inventory, "bun.lockb"),
                    inventory_has(&inventory, "pnpm-lock.yaml"),
                    inventory_has(&inventory, "yarn.lock"),
                );
                node_inspection(
                    repo,
                    branch,
                    default_branch,
                    inference,
                    dockerfile.is_some(),
                )
            } else {
                let language = plan
                    .services
                    .first()
                    .map(|service| match service.provider.as_str() {
                        "python" => "Python",
                        "golang" => "Go",
                        "rust" => "Rust",
                        "staticfile" => "static",
                        _ => "generated",
                    })
                    .unwrap_or("generated");
                railpack_inspection(repo, branch, default_branch, language)
            };
            let mut detected = inventory_detected_services(&inventory);
            detected.merge(&compose_addons);
            let inspection = apply_data_mount_path(
                with_detected_services(base, &detected),
                data_mount_path.as_deref(),
            );
            return Ok(attach_topology_plan(inspection, &plan));
        }
    }

    if let Some(package_text) = package_json {
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
        let base = node_inspection(
            repo,
            branch,
            default_branch,
            inference,
            dockerfile.is_some(),
        );
        let mut detected = infer_service_addons(&package_json_dependencies(&package_text));
        detected.merge(&compose_addons);
        return Ok(with_command_suggestion(
            apply_data_mount_path(
                with_detected_services(base, &detected),
                data_mount_path.as_deref(),
            ),
            command_suggestion,
        ));
    }

    if let Some(contents) = dockerfile {
        let inference = infer_dockerfile(&contents);
        let base = dockerfile_inspection(repo, branch, default_branch, inference);
        return Ok(with_command_suggestion(
            apply_data_mount_path(
                with_detected_services(base, &compose_addons),
                data_mount_path.as_deref(),
            ),
            command_suggestion,
        ));
    }

    for (path, language, is_manifest) in [
        ("requirements.txt", "Python", true),
        ("pyproject.toml", "Python", true),
        ("go.mod", "Go", true),
        ("Cargo.toml", "Rust", true),
        ("index.html", "static", false),
    ] {
        if let Some(contents) = github_file_text(state, repo, branch, path, token).await? {
            let base = railpack_inspection(repo, branch, default_branch, language);
            let mut detected = if is_manifest {
                infer_service_addons(&manifest_dependency_tokens(&contents))
            } else {
                DetectedServices::default()
            };
            detected.merge(&compose_addons);
            return Ok(with_command_suggestion(
                apply_data_mount_path(
                    with_detected_services(base, &detected),
                    data_mount_path.as_deref(),
                ),
                command_suggestion,
            ));
        }
    }

    Ok(with_command_suggestion(
        unknown_inspection(repo, branch, default_branch),
        command_suggestion,
    ))
}

/// Applies a detected data mount path to an inspection (no-op when none/invalid),
/// so the single-service managed volume mounts where the app declares it persists.
fn apply_data_mount_path(inspection: Value, path: Option<&str>) -> Value {
    match path {
        Some(path) => with_data_mount_path(inspection, path),
        None => inspection,
    }
}

/// Reads a repo's bare docker-compose file (one without a `hostlet.yml`) for two
/// single-service signals: the managed backing add-ons its service images map to,
/// and the container path it declares for persistent data (`./data:/app/data` →
/// `/app/data`). The repo's compose is never run on a shared host — only these
/// signals inform the generated stack + where the managed data volume mounts.
async fn detect_compose_signals(
    state: &AppState,
    repo: &str,
    branch: &str,
    token: Option<&str>,
) -> anyhow::Result<(DetectedServices, Option<String>)> {
    for path in ["compose.yaml", "docker-compose.yml"] {
        if let Some(compose_text) = github_file_text(state, repo, branch, path, token).await? {
            return Ok((
                infer_addons_from_compose(&compose_text),
                detect_data_mount_path(&compose_text),
            ));
        }
    }
    Ok((DetectedServices::default(), None))
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
    if value
        .get("size")
        .and_then(|value| value.as_u64())
        .is_some_and(|size| size > GITHUB_INSPECTION_FILE_MAX_BYTES)
    {
        return Ok(None);
    }
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

fn inventory_has(inventory: &RepositoryInventory, filename: &str) -> bool {
    inventory
        .files
        .iter()
        .any(|file| file.path == filename || file.path.ends_with(&format!("/{filename}")))
}

fn inventory_detected_services(inventory: &RepositoryInventory) -> DetectedServices {
    let mut detected = DetectedServices::default();
    for file in &inventory.files {
        let Some(contents) = file.contents.as_deref() else {
            continue;
        };
        let current = if file.path.ends_with("package.json") {
            infer_service_addons(&package_json_dependencies(contents))
        } else if matches!(
            file.path.rsplit('/').next(),
            Some("requirements.txt" | "pyproject.toml" | "go.mod" | "Cargo.toml")
        ) {
            infer_service_addons(&manifest_dependency_tokens(contents))
        } else {
            continue;
        };
        detected.merge(&current);
    }
    detected
}

/// Fetches the recursive tree once and downloads only small files that can
/// affect topology inference. Lockfiles are represented by path only: manager
/// detection needs their presence, while dependency resolution remains the
/// agent's responsibility and large lock contents never enter API memory.
async fn github_repository_inventory(
    state: &AppState,
    repo: &str,
    branch: &str,
    token: Option<&str>,
) -> anyhow::Result<RepositoryInventory> {
    let encoded_branch =
        url::form_urlencoded::byte_serialize(branch.as_bytes()).collect::<String>();
    let mut request = state
        .http
        .get(format!(
            "https://api.github.com/repos/{repo}/git/trees/{encoded_branch}?recursive=1"
        ))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "Hostlet");
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let tree: Value = request.send().await?.error_for_status()?.json().await?;
    let entries = tree
        .get("tree")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut relevant = entries
        .into_iter()
        .take(GITHUB_INVENTORY_MAX_FILES)
        .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("blob"))
        .filter_map(|entry| {
            let path = entry.get("path")?.as_str()?.to_string();
            let size = entry
                .get("size")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            topology_inventory_path(&path).then_some((path, size))
        })
        .take(GITHUB_INVENTORY_MAX_RELEVANT_FILES)
        .collect::<Vec<_>>();
    relevant.sort_by(|a, b| a.0.cmp(&b.0));

    let mut files = Vec::with_capacity(relevant.len());
    let mut content_bytes = 0usize;
    for (path, size) in relevant {
        let filename = path.rsplit('/').next().unwrap_or(&path);
        let is_lock = matches!(
            filename,
            "pnpm-lock.yaml" | "yarn.lock" | "package-lock.json" | "bun.lock" | "bun.lockb"
        );
        let contents = if is_lock
            || size > GITHUB_INSPECTION_FILE_MAX_BYTES
            || content_bytes + size as usize > GITHUB_INVENTORY_MAX_CONTENT_BYTES
        {
            None
        } else {
            let contents = github_file_text(state, repo, branch, &path, token).await?;
            content_bytes += contents.as_ref().map(String::len).unwrap_or_default();
            contents
        };
        files.push(RepositoryFile { path, contents });
    }
    Ok(RepositoryInventory { files })
}

fn topology_inventory_path(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    matches!(
        filename,
        "package.json"
            | "pnpm-workspace.yaml"
            | "pnpm-lock.yaml"
            | "package-lock.json"
            | "yarn.lock"
            | "bun.lock"
            | "bun.lockb"
            | "pyproject.toml"
            | "requirements.txt"
            | "go.mod"
            | "go.work"
            | "Cargo.toml"
            | "index.html"
            | "main.rs"
    ) || path.ends_with(".go")
        || matches!(
            path.rsplit('.').next(),
            Some("js" | "jsx" | "ts" | "tsx" | "vue" | "svelte")
        )
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
#[path = "github/tests.rs"]
mod tests;
