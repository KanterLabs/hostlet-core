use crate::{
    crypto::{constant_time_eq, hash_token, random_token, sign, verify_signature},
    github_app,
    state::{AppState, HostletMode},
};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

const SESSION_COOKIE: &str = "hostlet_session";
const CLOUD_SESSION_COOKIE: &str = "hostlet_cloud_session";
const OAUTH_STATE_COOKIE: &str = "hostlet_oauth_state";
const GITHUB_INSTALL_STATE_COOKIE: &str = "hostlet_install_state";
const UNLOCK_COOKIE: &str = "hostlet_unlock";
const SESSION_TTL_DAYS: i64 = 14;
const CLOUD_SESSION_TTL_DAYS: i64 = 30;
const UNLOCK_TTL_HOURS: i64 = 12;
const CONTROL_PLANE_PASSWORD_KEY: &str = "control_plane_password_hash";
const DEVICE_FLOW_KEY_PREFIX: &str = "github_device_flow:";

#[derive(Deserialize)]
pub struct DevicePollBody {
    flow_id: String,
}

#[derive(Deserialize)]
pub struct PasswordBody {
    password: String,
}

#[derive(Serialize, Deserialize)]
struct GitHubToken {
    access_token: String,
    scope: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct GitHubUser {
    id: i64,
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
    email: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct StoredDeviceFlow {
    device_code: String,
    web_origin: String,
    expires_at: i64,
    interval: i64,
}

struct AuthorizedGitHubUser {
    id: Uuid,
    login: String,
}

#[derive(Clone, Copy, Debug)]
pub struct RequestContext {
    pub user_id: Uuid,
    pub cloud_user_id: Option<Uuid>,
}

#[derive(Deserialize)]
pub struct OAuthCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
pub struct GitHubInstallCallbackQuery {
    installation_id: Option<i64>,
    state: Option<String>,
    setup_action: Option<String>,
}

pub async fn github_device_start(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if state.mode == HostletMode::Cloud {
        return (
            StatusCode::FORBIDDEN,
            "GitHub Device Flow is only available in self-hosted mode",
        )
            .into_response();
    }
    if state.github_client_id.trim().is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "GitHub device flow is not configured. Set GITHUB_CLIENT_ID and enable Device Flow on the GitHub OAuth App.",
        )
            .into_response();
    }
    if state.mode == HostletMode::SelfHosted {
        if control_plane_password_hash(&state)
            .await
            .ok()
            .flatten()
            .is_none()
        {
            return (StatusCode::PRECONDITION_REQUIRED, "setup is required").into_response();
        }
        if !control_plane_unlocked(&headers, &state.session_secret)
            && !has_existing_users(&state).await.unwrap_or(false)
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    let web_origin = request_web_origin(&headers)
        .filter(|origin| state.web_origin_allowed(origin))
        .unwrap_or_else(|| state.public_web_url.clone());

    let response = match state
        .http
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .form(&[
            ("client_id", state.github_client_id.as_str()),
            ("scope", "repo read:user admin:repo_hook"),
        ])
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(error = %err, "GitHub device code request failed");
            return (
                StatusCode::BAD_GATEWAY,
                "Could not reach GitHub device authorization endpoint",
            )
                .into_response();
        }
    };
    let payload = match response.json::<Value>().await {
        Ok(payload) => payload,
        Err(err) => {
            tracing::warn!(error = %err, "GitHub device code response was not JSON");
            return (
                StatusCode::BAD_GATEWAY,
                "GitHub returned an unexpected device flow response",
            )
                .into_response();
        }
    };
    if let Some(error) = payload.get("error").and_then(|value| value.as_str()) {
        let description = payload
            .get("error_description")
            .and_then(|value| value.as_str())
            .unwrap_or(error);
        return (
            StatusCode::BAD_GATEWAY,
            format!("GitHub device flow failed: {description}"),
        )
            .into_response();
    }

    let Some(device_code) = payload.get("device_code").and_then(|value| value.as_str()) else {
        return (
            StatusCode::BAD_GATEWAY,
            "GitHub did not return a device code",
        )
            .into_response();
    };
    let Some(user_code) = payload.get("user_code").and_then(|value| value.as_str()) else {
        return (StatusCode::BAD_GATEWAY, "GitHub did not return a user code").into_response();
    };
    let Some(verification_uri) = payload
        .get("verification_uri")
        .and_then(|value| value.as_str())
    else {
        return (
            StatusCode::BAD_GATEWAY,
            "GitHub did not return a verification URL",
        )
            .into_response();
    };
    let expires_in = payload
        .get("expires_in")
        .and_then(|value| value.as_i64())
        .unwrap_or(900)
        .max(60);
    let interval = payload
        .get("interval")
        .and_then(|value| value.as_i64())
        .unwrap_or(5)
        .max(5);
    let flow_id = random_token(32);
    let stored = StoredDeviceFlow {
        device_code: device_code.to_string(),
        web_origin,
        expires_at: (Utc::now() + Duration::seconds(expires_in)).timestamp(),
        interval,
    };
    if let Err(err) = store_device_flow(&state, &flow_id, &stored).await {
        tracing::error!(error = %err, "failed to store GitHub device flow");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    Json(serde_json::json!({
        "flowId": flow_id,
        "userCode": user_code,
        "verificationUri": verification_uri,
        "verificationUriComplete": payload.get("verification_uri_complete").and_then(|value| value.as_str()),
        "expiresIn": expires_in,
        "interval": interval,
    }))
    .into_response()
}

pub async fn github_device_poll(
    State(state): State<AppState>,
    Json(body): Json<DevicePollBody>,
) -> impl IntoResponse {
    if state.mode == HostletMode::Cloud {
        return (
            StatusCode::FORBIDDEN,
            "GitHub Device Flow is only available in self-hosted mode",
        )
            .into_response();
    }
    let flow_id = body.flow_id.trim();
    if flow_id.is_empty() {
        return (StatusCode::BAD_REQUEST, "flow_id is required").into_response();
    }
    let Some(mut flow) = load_device_flow(&state, flow_id).await.ok().flatten() else {
        return Json(serde_json::json!({
            "status": "expired",
            "message": "This GitHub device login expired. Start a new login.",
        }))
        .into_response();
    };
    if Utc::now().timestamp() > flow.expires_at {
        let _ = delete_device_flow(&state, flow_id).await;
        return Json(serde_json::json!({
            "status": "expired",
            "message": "This GitHub device login expired. Start a new login.",
        }))
        .into_response();
    }

    let response = match state
        .http
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&[
            ("client_id", state.github_client_id.as_str()),
            ("device_code", flow.device_code.as_str()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(error = %err, "GitHub device token poll failed");
            return (
                StatusCode::BAD_GATEWAY,
                "Could not reach GitHub device authorization endpoint",
            )
                .into_response();
        }
    };
    let payload = match response.json::<Value>().await {
        Ok(payload) => payload,
        Err(err) => {
            tracing::warn!(error = %err, "GitHub device token response was not JSON");
            return (
                StatusCode::BAD_GATEWAY,
                "GitHub returned an unexpected device authorization response",
            )
                .into_response();
        }
    };

    if let Some(access_token) = payload.get("access_token").and_then(|value| value.as_str()) {
        let token = GitHubToken {
            access_token: access_token.to_string(),
            scope: payload
                .get("scope")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        };
        match store_github_access_token(&state, token).await {
            Ok(user) => {
                let _ = delete_device_flow(&state, flow_id).await;
                let session = build_cookie(
                    SESSION_COOKIE,
                    &signed_value(
                        &state.session_secret,
                        &user.id.to_string(),
                        Duration::days(SESSION_TTL_DAYS),
                    ),
                    Some(Duration::days(SESSION_TTL_DAYS)),
                    cookie_secure(&state.public_api_url),
                    "/",
                );
                return with_cookies(
                    Json(serde_json::json!({
                        "status": "authorized",
                        "message": "GitHub connected.",
                        "login": user.login,
                        "redirectTo": flow.web_origin,
                    }))
                    .into_response(),
                    [session],
                );
            }
            Err(err) => {
                tracing::warn!(error = %err, "GitHub login failed");
                let _ = delete_device_flow(&state, flow_id).await;
                return (StatusCode::BAD_REQUEST, "GitHub login failed").into_response();
            }
        }
    }

    let error = payload
        .get("error")
        .and_then(|value| value.as_str())
        .unwrap_or("authorization_pending");
    match error {
        "authorization_pending" => Json(serde_json::json!({
            "status": "pending",
            "message": "Waiting for GitHub authorization.",
            "interval": flow.interval,
        }))
        .into_response(),
        "slow_down" => {
            flow.interval += 5;
            let _ = store_device_flow(&state, flow_id, &flow).await;
            Json(serde_json::json!({
                "status": "pending",
                "message": "GitHub asked Hostlet to slow down polling.",
                "interval": flow.interval,
            }))
            .into_response()
        }
        "expired_token" => {
            let _ = delete_device_flow(&state, flow_id).await;
            Json(serde_json::json!({
                "status": "expired",
                "message": "This GitHub device login expired. Start a new login.",
            }))
            .into_response()
        }
        "access_denied" => {
            let _ = delete_device_flow(&state, flow_id).await;
            Json(serde_json::json!({
                "status": "denied",
                "message": "GitHub authorization was cancelled.",
            }))
            .into_response()
        }
        _ => {
            let description = payload
                .get("error_description")
                .and_then(|value| value.as_str())
                .unwrap_or("GitHub device authorization failed.");
            let _ = delete_device_flow(&state, flow_id).await;
            (
                StatusCode::BAD_GATEWAY,
                format!("GitHub device authorization failed: {description}"),
            )
                .into_response()
        }
    }
}

pub async fn github_oauth_start(State(state): State<AppState>) -> impl IntoResponse {
    if state.mode != HostletMode::Cloud {
        return (
            StatusCode::FORBIDDEN,
            "GitHub OAuth login is only available in cloud mode",
        )
            .into_response();
    }
    let Some(client_id) = state.github_app_client_id.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "GitHub App OAuth is not configured",
        )
            .into_response();
    };
    let nonce = random_token(32);
    let redirect_uri = format!(
        "{}/auth/github/oauth/callback",
        state.public_api_url.trim_end_matches('/')
    );
    let url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope=read:user%20user:email%20read:org&state={}",
        url_encode(client_id),
        url_encode(&redirect_uri),
        url_encode(&nonce)
    );
    let cookie = build_cookie(
        OAUTH_STATE_COOKIE,
        &signed_value(&state.session_secret, &nonce, Duration::minutes(15)),
        Some(Duration::minutes(15)),
        cookie_secure(&state.public_api_url),
        "/",
    );
    with_cookies(Redirect::temporary(&url).into_response(), [cookie])
}

pub async fn github_oauth_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<OAuthCallbackQuery>,
) -> impl IntoResponse {
    if state.mode != HostletMode::Cloud {
        return Redirect::temporary("/login").into_response();
    }
    if let Some(error) = query.error.as_deref() {
        let description = query.error_description.as_deref().unwrap_or(error);
        return Redirect::temporary(&format!(
            "/login?error={}",
            url_encode(&format!("GitHub OAuth failed: {description}"))
        ))
        .into_response();
    }
    let Some(code) = query.code.as_deref().filter(|value| !value.is_empty()) else {
        return Redirect::temporary("/login?error=missing%20oauth%20code").into_response();
    };
    let Some(state_param) = query.state.as_deref().filter(|value| !value.is_empty()) else {
        return Redirect::temporary("/login?error=missing%20oauth%20state").into_response();
    };
    let Some(cookie_state) = cookie_value(&headers, OAUTH_STATE_COOKIE)
        .and_then(|value| verify_signed_value(&state.session_secret, value))
    else {
        return Redirect::temporary("/login?error=expired%20oauth%20state").into_response();
    };
    if !constant_time_eq(cookie_state.as_bytes(), state_param.as_bytes()) {
        return Redirect::temporary("/login?error=invalid%20oauth%20state").into_response();
    }

    let token = match exchange_github_oauth_code(&state, code).await {
        Ok(token) => token,
        Err(err) => {
            tracing::warn!(error = %err, "GitHub OAuth token exchange failed");
            return Redirect::temporary("/login?error=github%20token%20exchange%20failed")
                .into_response();
        }
    };
    let gh_user = match github_user_from_token(&state, &token.access_token).await {
        Ok(user) => user,
        Err(err) => {
            tracing::warn!(error = %err, "GitHub OAuth user lookup failed");
            return Redirect::temporary("/login?error=github%20user%20lookup%20failed")
                .into_response();
        }
    };
    match create_cloud_login_session(&state, &gh_user, &token).await {
        Ok((user_id, cloud_session_token)) => {
            let secure = cookie_secure(&state.public_api_url);
            let session = build_cookie(
                SESSION_COOKIE,
                &signed_value(
                    &state.session_secret,
                    &user_id.to_string(),
                    Duration::days(SESSION_TTL_DAYS),
                ),
                Some(Duration::days(SESSION_TTL_DAYS)),
                secure,
                "/",
            );
            let cloud_session = build_cookie(
                CLOUD_SESSION_COOKIE,
                &cloud_session_token,
                Some(Duration::days(CLOUD_SESSION_TTL_DAYS)),
                secure,
                "/",
            );
            let expired_state = expire_cookie(OAUTH_STATE_COOKIE, secure, "/");
            with_cookies(
                Redirect::temporary("/").into_response(),
                [session, cloud_session, expired_state],
            )
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to create cloud login session");
            Redirect::temporary("/login?error=cloud%20session%20failed").into_response()
        }
    }
}

pub async fn cloud_github_install_start(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if state.mode != HostletMode::Cloud {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(_cloud_user_id) = current_cloud_user_id(&headers, &state).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(slug) = state.github_app_slug.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "GITHUB_APP_SLUG is required for GitHub App installation",
        )
            .into_response();
    };
    let nonce = random_token(32);
    let cookie = build_cookie(
        GITHUB_INSTALL_STATE_COOKIE,
        &signed_value(&state.session_secret, &nonce, Duration::minutes(15)),
        Some(Duration::minutes(15)),
        cookie_secure(&state.public_api_url),
        "/",
    );
    let url = format!(
        "https://github.com/apps/{}/installations/new?state={}",
        url_encode(slug),
        url_encode(&nonce)
    );
    with_cookies(Redirect::temporary(&url).into_response(), [cookie])
}

pub async fn cloud_github_install_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<GitHubInstallCallbackQuery>,
) -> impl IntoResponse {
    if state.mode != HostletMode::Cloud {
        return Redirect::temporary("/").into_response();
    }
    let Some(cloud_user_id) = current_cloud_user_id(&headers, &state).await else {
        return Redirect::temporary("/login").into_response();
    };
    let Some(state_param) = query.state.as_deref().filter(|value| !value.is_empty()) else {
        return Redirect::temporary("/?error=missing%20installation%20state").into_response();
    };
    let Some(cookie_state) = cookie_value(&headers, GITHUB_INSTALL_STATE_COOKIE)
        .and_then(|value| verify_signed_value(&state.session_secret, value))
    else {
        return Redirect::temporary("/?error=expired%20installation%20state").into_response();
    };
    if !constant_time_eq(cookie_state.as_bytes(), state_param.as_bytes()) {
        return Redirect::temporary("/?error=invalid%20installation%20state").into_response();
    }
    let Some(installation_id) = query.installation_id else {
        return Redirect::temporary("/?error=missing%20installation").into_response();
    };
    if let Err(err) = upsert_cloud_installation(&state, cloud_user_id, installation_id).await {
        tracing::warn!(error = %err, installation_id, "failed to store GitHub App installation");
        return Redirect::temporary("/?error=installation%20save%20failed").into_response();
    }
    let _ = query.setup_action;
    let expired_state = expire_cookie(
        GITHUB_INSTALL_STATE_COOKIE,
        cookie_secure(&state.public_api_url),
        "/",
    );
    with_cookies(
        Redirect::temporary("/apps/new").into_response(),
        [expired_state],
    )
}

pub async fn me(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let Some(user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let row = sqlx::query("SELECT id, login, name, avatar_url FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await;
    match row {
        Ok(Some(r)) => Json(serde_json::json!({"id": r.get::<Uuid,_>("id"), "login": r.get::<String,_>("login"), "name": r.get::<Option<String>,_>("name"), "avatarUrl": r.get::<Option<String>,_>("avatar_url")})).into_response(),
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

pub async fn session_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if state.mode == HostletMode::Cloud {
        let Some(cloud_user_id) = current_cloud_user_id(&headers, &state).await else {
            return Json(serde_json::json!({
                "mode": state.mode.as_str(),
                "authenticated": false,
                "user": null,
                "cloud": {
                    "billingActive": false,
                    "githubInstalled": false,
                    "nextStep": "login"
                }
            }))
            .into_response();
        };
        let user = sqlx::query(
            "SELECT id, login, name, email, avatar_url FROM cloud_users WHERE id=$1 AND status='active'",
        )
        .bind(cloud_user_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
        let subscription = cloud_active_subscription_for_cloud_user(&state, cloud_user_id)
            .await
            .ok()
            .flatten();
        let billing_active = subscription.is_some();
        let github_installed = cloud_github_installed(&state, cloud_user_id)
            .await
            .unwrap_or(false);
        let next_step = if !github_installed {
            "install_github"
        } else if !billing_active {
            "billing"
        } else {
            "ready"
        };
        return Json(serde_json::json!({
            "mode": state.mode.as_str(),
            "authenticated": true,
            "user": user.map(|row| serde_json::json!({
                "id": row.get::<Uuid, _>("id"),
                "login": row.get::<String, _>("login"),
                "name": row.get::<Option<String>, _>("name"),
                "email": row.get::<Option<String>, _>("email"),
                "avatarUrl": row.get::<Option<String>, _>("avatar_url")
            })),
            "cloud": {
                "billingActive": billing_active,
                "githubInstalled": github_installed,
                "nextStep": next_step,
                "planCode": subscription.as_ref().map(|row| row.get::<String, _>("plan_code")),
                "subscriptionStatus": subscription.as_ref().map(|row| row.get::<String, _>("status"))
            }
        }))
        .into_response();
    }
    let authenticated = current_user_id(&headers, &state).is_some();
    Json(serde_json::json!({
        "mode": state.mode.as_str(),
        "authenticated": authenticated,
        "user": null,
        "cloud": null
    }))
    .into_response()
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if state.mode == HostletMode::Cloud {
        if let Some(token) = cookie_value(&headers, CLOUD_SESSION_COOKIE) {
            let _ = sqlx::query(
                "UPDATE cloud_sessions SET revoked_at=now() WHERE token_hash=$1 AND revoked_at IS NULL",
            )
            .bind(hash_token(token))
            .execute(&state.db)
            .await;
        }
    }
    let secure = cookie_secure(&state.public_api_url);
    with_cookies(
        StatusCode::NO_CONTENT.into_response(),
        [
            expire_cookie(SESSION_COOKIE, secure, "/"),
            expire_cookie(CLOUD_SESSION_COOKIE, secure, "/"),
            expire_cookie(UNLOCK_COOKIE, secure, "/"),
        ],
    )
}

pub async fn setup_status(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if state.mode == HostletMode::Cloud {
        return Json(serde_json::json!({
            "mode": state.mode.as_str(),
            "setupRequired": false,
            "unlocked": true
        }));
    }
    let setup_required = control_plane_password_hash(&state)
        .await
        .ok()
        .flatten()
        .is_none();
    Json(serde_json::json!({
        "mode": state.mode.as_str(),
        "setupRequired": setup_required,
        "unlocked": !setup_required && control_plane_unlocked(&headers, &state.session_secret)
    }))
}

pub async fn setup_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PasswordBody>,
) -> impl IntoResponse {
    if state.mode == HostletMode::Cloud {
        return (
            StatusCode::FORBIDDEN,
            "control-plane setup is only available in self-hosted mode",
        )
            .into_response();
    }
    if let Some(expected) = &state.setup_token {
        let provided = headers
            .get("x-hostlet-setup-token")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    if !valid_control_plane_password(&body.password) {
        return (
            StatusCode::BAD_REQUEST,
            "password must be at least 12 characters",
        )
            .into_response();
    }
    let hash = match hash_password(&body.password) {
        Ok(hash) => hash,
        Err(err) => {
            tracing::error!(error = %err, "failed to hash control plane password");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    match store_control_plane_password_hash(&state, &hash).await {
        Ok(true) => unlock_response(&state).await,
        Ok(false) => (
            StatusCode::CONFLICT,
            "control plane password is already set",
        )
            .into_response(),
        Err(err) => {
            tracing::error!(error = %err, "failed to store control plane password");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn unlock(
    State(state): State<AppState>,
    Json(body): Json<PasswordBody>,
) -> impl IntoResponse {
    if state.mode == HostletMode::Cloud {
        return (
            StatusCode::FORBIDDEN,
            "control-plane unlock is only available in self-hosted mode",
        )
            .into_response();
    }
    let Some(hash) = control_plane_password_hash(&state).await.ok().flatten() else {
        return (StatusCode::PRECONDITION_REQUIRED, "setup is required").into_response();
    };
    match verify_password(&hash, &body.password) {
        Ok(true) => unlock_response(&state).await,
        Ok(false) => StatusCode::UNAUTHORIZED.into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "control plane password verification failed");
            StatusCode::UNAUTHORIZED.into_response()
        }
    }
}

pub fn current_user_id(headers: &HeaderMap, state: &AppState) -> Option<Uuid> {
    match state.mode {
        HostletMode::Cloud => current_user_id_from_headers(headers, &state.session_secret),
        HostletMode::SelfHosted => control_plane_unlocked(headers, &state.session_secret)
            .then(|| current_user_id_from_headers(headers, &state.session_secret))
            .flatten(),
    }
}

fn current_user_id_from_headers(headers: &HeaderMap, session_secret: &str) -> Option<Uuid> {
    let value = cookie_value(headers, SESSION_COOKIE)?;
    let user_id = verify_signed_value(session_secret, value)?;
    Uuid::parse_str(&user_id).ok()
}

fn control_plane_unlocked(headers: &HeaderMap, session_secret: &str) -> bool {
    cookie_value(headers, UNLOCK_COOKIE)
        .and_then(|value| verify_signed_value(session_secret, value))
        .is_some_and(|value| value == "unlocked")
}

pub async fn request_context(
    headers: &HeaderMap,
    state: &AppState,
) -> anyhow::Result<RequestContext> {
    let Some(user_id) = current_user_id(headers, state) else {
        anyhow::bail!("sign in required");
    };
    if state.mode != HostletMode::Cloud {
        return Ok(RequestContext {
            user_id,
            cloud_user_id: None,
        });
    }
    let token = cookie_value(headers, CLOUD_SESSION_COOKIE)
        .ok_or_else(|| anyhow::anyhow!("Hostlet Cloud session is required"))?;
    let cloud_user_id = cloud_user_id_for_app_user_session(state, user_id, token)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Hostlet Cloud session is required"))?;
    Ok(RequestContext {
        user_id,
        cloud_user_id: Some(cloud_user_id),
    })
}

async fn store_github_access_token(
    state: &AppState,
    token: GitHubToken,
) -> anyhow::Result<AuthorizedGitHubUser> {
    let gh_user: GitHubUser = state
        .http
        .get("https://api.github.com/user")
        .bearer_auth(&token.access_token)
        .header("User-Agent", "Hostlet")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if let Some(allowed_logins) = &state.allowed_github_logins {
        if !allowed_logins.contains(&gh_user.login.to_ascii_lowercase()) {
            anyhow::bail!("GitHub account is not allowed to access this Hostlet instance");
        }
    } else if has_existing_users(state).await? && !github_user_exists(state, gh_user.id).await? {
        anyhow::bail!("GitHub account is not registered on this Hostlet instance");
    }
    let mut tx = state.db.begin().await?;
    let row = sqlx::query("INSERT INTO users (github_id, login, name, avatar_url) VALUES ($1,$2,$3,$4) ON CONFLICT (github_id) DO UPDATE SET login=EXCLUDED.login, name=EXCLUDED.name, avatar_url=EXCLUDED.avatar_url RETURNING id")
        .bind(gh_user.id).bind(&gh_user.login).bind(&gh_user.name).bind(&gh_user.avatar_url)
        .fetch_one(&mut *tx).await?;
    let user_id: Uuid = row.get("id");
    let encrypted = state.crypto.encrypt(&token.access_token)?;
    sqlx::query(
        "INSERT INTO github_accounts (user_id, github_id, access_token_ciphertext, scopes)
         VALUES ($1,$2,$3,$4)
         ON CONFLICT (user_id, github_id)
         DO UPDATE SET access_token_ciphertext=EXCLUDED.access_token_ciphertext,
                       scopes=EXCLUDED.scopes,
                       updated_at=now()",
    )
    .bind(user_id)
    .bind(gh_user.id)
    .bind(encrypted)
    .bind(token.scope.unwrap_or_default())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(AuthorizedGitHubUser {
        id: user_id,
        login: gh_user.login,
    })
}

async fn exchange_github_oauth_code(state: &AppState, code: &str) -> anyhow::Result<GitHubToken> {
    let client_id = state
        .github_app_client_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("GITHUB_APP_CLIENT_ID is missing"))?;
    let client_secret = state
        .github_app_client_secret
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("GITHUB_APP_CLIENT_SECRET is missing"))?;
    let redirect_uri = format!(
        "{}/auth/github/oauth/callback",
        state.public_api_url.trim_end_matches('/')
    );
    let payload = state
        .http
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    if let Some(error) = payload.get("error").and_then(|value| value.as_str()) {
        let description = payload
            .get("error_description")
            .and_then(|value| value.as_str())
            .unwrap_or(error);
        anyhow::bail!("{description}");
    }
    let access_token = payload
        .get("access_token")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("GitHub did not return an access token"))?;
    Ok(GitHubToken {
        access_token: access_token.to_string(),
        scope: payload
            .get("scope")
            .and_then(|value| value.as_str())
            .map(str::to_string),
    })
}

async fn github_user_from_token(
    state: &AppState,
    access_token: &str,
) -> anyhow::Result<GitHubUser> {
    state
        .http
        .get("https://api.github.com/user")
        .bearer_auth(access_token)
        .header("User-Agent", "Hostlet")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .map_err(Into::into)
}

async fn create_cloud_login_session(
    state: &AppState,
    gh_user: &GitHubUser,
    token: &GitHubToken,
) -> anyhow::Result<(Uuid, String)> {
    let mut tx = state.db.begin().await?;
    let app_user_row = sqlx::query(
        "INSERT INTO users (github_id, login, name, avatar_url)
         VALUES ($1,$2,$3,$4)
         ON CONFLICT (github_id) DO UPDATE SET
           login=EXCLUDED.login,
           name=EXCLUDED.name,
           avatar_url=EXCLUDED.avatar_url
         RETURNING id",
    )
    .bind(gh_user.id)
    .bind(&gh_user.login)
    .bind(&gh_user.name)
    .bind(&gh_user.avatar_url)
    .fetch_one(&mut *tx)
    .await?;
    let app_user_id: Uuid = app_user_row.get("id");

    let cloud_user_row = sqlx::query(
        "INSERT INTO cloud_users (github_id, login, name, email, avatar_url, status)
         VALUES ($1,$2,$3,$4,$5,'active')
         ON CONFLICT (github_id) DO UPDATE SET
           login=EXCLUDED.login,
           name=EXCLUDED.name,
           email=EXCLUDED.email,
           avatar_url=EXCLUDED.avatar_url,
           updated_at=now()
         RETURNING id",
    )
    .bind(gh_user.id)
    .bind(&gh_user.login)
    .bind(&gh_user.name)
    .bind(&gh_user.email)
    .bind(&gh_user.avatar_url)
    .fetch_one(&mut *tx)
    .await?;
    let cloud_user_id: Uuid = cloud_user_row.get("id");

    let encrypted = state.crypto.encrypt(&token.access_token)?;
    sqlx::query(
        "INSERT INTO github_accounts (user_id, github_id, access_token_ciphertext, scopes)
         VALUES ($1,$2,$3,$4)
         ON CONFLICT (user_id, github_id)
         DO UPDATE SET access_token_ciphertext=EXCLUDED.access_token_ciphertext,
                       scopes=EXCLUDED.scopes,
                       updated_at=now()",
    )
    .bind(app_user_id)
    .bind(gh_user.id)
    .bind(encrypted)
    .bind(token.scope.clone().unwrap_or_default())
    .execute(&mut *tx)
    .await?;

    let cloud_session_token = random_token(48);
    sqlx::query(
        "INSERT INTO cloud_sessions (cloud_user_id, token_hash, expires_at)
         VALUES ($1,$2,now() + make_interval(days => $3))",
    )
    .bind(cloud_user_id)
    .bind(hash_token(&cloud_session_token))
    .bind(CLOUD_SESSION_TTL_DAYS as i32)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok((app_user_id, cloud_session_token))
}

pub async fn current_cloud_user_id(headers: &HeaderMap, state: &AppState) -> Option<Uuid> {
    if state.mode != HostletMode::Cloud {
        return None;
    }
    let token = cookie_value(headers, CLOUD_SESSION_COOKIE)?;
    let token_hash = hash_token(token);
    sqlx::query_scalar::<_, Uuid>(
        "SELECT cloud_user_id
         FROM cloud_sessions
         WHERE token_hash=$1 AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(token_hash)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
}

async fn cloud_user_id_for_app_user_session(
    state: &AppState,
    app_user_id: Uuid,
    token: &str,
) -> anyhow::Result<Option<Uuid>> {
    let token_hash = hash_token(token);
    sqlx::query_scalar::<_, Uuid>(
        "SELECT cu.id
         FROM cloud_sessions cs
         JOIN cloud_users cu ON cu.id=cs.cloud_user_id
         JOIN users u ON u.github_id=cu.github_id
         WHERE cs.token_hash=$1
           AND cs.revoked_at IS NULL
           AND cs.expires_at > now()
           AND u.id=$2
           AND cu.status='active'",
    )
    .bind(token_hash)
    .bind(app_user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(Into::into)
}

pub async fn cloud_compute_allowed_for_user(
    state: &AppState,
    app_user_id: Uuid,
) -> anyhow::Result<()> {
    if state.mode != HostletMode::Cloud {
        return Ok(());
    }
    let row = sqlx::query(
        "SELECT cu.id
         FROM cloud_users cu
         JOIN users u ON u.github_id=cu.github_id
         WHERE u.id=$1 AND cu.status='active'",
    )
    .bind(app_user_id)
    .fetch_optional(&state.db)
    .await?;
    let Some(row) = row else {
        anyhow::bail!("Hostlet Cloud account is required");
    };
    let cloud_user_id: Uuid = row.get("id");
    cloud_compute_gate(
        cloud_github_installed(state, cloud_user_id).await?,
        cloud_billing_active_for_cloud_user(state, cloud_user_id).await?,
    )
}

pub async fn cloud_compute_allowed_for_context(
    state: &AppState,
    context: RequestContext,
) -> anyhow::Result<()> {
    if state.mode != HostletMode::Cloud {
        return Ok(());
    }
    let Some(cloud_user_id) = context.cloud_user_id else {
        anyhow::bail!("Hostlet Cloud session is required");
    };
    cloud_compute_gate(
        cloud_github_installed(state, cloud_user_id).await?,
        cloud_billing_active_for_cloud_user(state, cloud_user_id).await?,
    )
}

pub async fn cloud_request_ready(headers: &HeaderMap, state: &AppState) -> anyhow::Result<Uuid> {
    let context = request_context(headers, state).await?;
    cloud_compute_allowed_for_context(state, context).await?;
    Ok(context.user_id)
}

fn cloud_compute_gate(github_installed: bool, billing_active: bool) -> anyhow::Result<()> {
    if !github_installed {
        anyhow::bail!("Install the Hostlet GitHub App before deploying");
    }
    if !billing_active {
        anyhow::bail!("An active Hostlet Cloud subscription is required before deploying");
    }
    Ok(())
}

async fn cloud_github_installed(state: &AppState, cloud_user_id: Uuid) -> anyhow::Result<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM cloud_github_installations
         WHERE cloud_user_id=$1 AND suspended_at IS NULL",
    )
    .bind(cloud_user_id)
    .fetch_one(&state.db)
    .await?;
    Ok(count > 0)
}

async fn cloud_billing_active_for_cloud_user(
    state: &AppState,
    cloud_user_id: Uuid,
) -> anyhow::Result<bool> {
    Ok(
        cloud_active_subscription_for_cloud_user(state, cloud_user_id)
            .await?
            .is_some(),
    )
}

async fn cloud_active_subscription_for_cloud_user(
    state: &AppState,
    cloud_user_id: Uuid,
) -> anyhow::Result<Option<sqlx::postgres::PgRow>> {
    Ok(sqlx::query(
        "SELECT plan_code, status
         FROM cloud_subscriptions
         WHERE cloud_user_id=$1
           AND status IN ('active','trialing')
           AND (current_period_end IS NULL OR current_period_end > now())
         ORDER BY created_at DESC
         LIMIT 1",
    )
    .bind(cloud_user_id)
    .fetch_optional(&state.db)
    .await?)
}

async fn upsert_cloud_installation(
    state: &AppState,
    cloud_user_id: Uuid,
    installation_id: i64,
) -> anyhow::Result<()> {
    let info = github_app::fetch_installation_info(state, installation_id).await?;
    let (account_login, account_type, permissions, repository_selection) =
        github_app::installation_info_defaults(info);
    ensure_installation_owner(
        state,
        cloud_user_id,
        installation_id,
        &account_login,
        &account_type,
    )
    .await?;
    sqlx::query(
        "INSERT INTO cloud_github_installations
           (cloud_user_id, installation_id, account_login, account_type, permissions_json, repository_selection)
         VALUES ($1,$2,$3,$4,$5,$6)
         ON CONFLICT (installation_id) DO UPDATE SET
           account_login=EXCLUDED.account_login,
           account_type=EXCLUDED.account_type,
           permissions_json=EXCLUDED.permissions_json,
           repository_selection=EXCLUDED.repository_selection,
           updated_at=now(),
           suspended_at=NULL",
    )
    .bind(cloud_user_id)
    .bind(installation_id)
    .bind(account_login)
    .bind(account_type)
    .bind(permissions)
    .bind(repository_selection)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn ensure_installation_owner(
    state: &AppState,
    cloud_user_id: Uuid,
    installation_id: i64,
    account_login: &str,
    account_type: &str,
) -> anyhow::Result<()> {
    let row = sqlx::query("SELECT login FROM cloud_users WHERE id=$1 AND status='active'")
        .bind(cloud_user_id)
        .fetch_one(&state.db)
        .await?;
    let cloud_login = row.get::<String, _>("login");
    let existing_owner: Option<Uuid> = sqlx::query_scalar(
        "SELECT cloud_user_id FROM cloud_github_installations WHERE installation_id=$1",
    )
    .bind(installation_id)
    .fetch_optional(&state.db)
    .await?;
    if existing_owner.is_some_and(|owner| owner != cloud_user_id) {
        anyhow::bail!("GitHub App installation is already linked to another Hostlet Cloud account");
    }
    if github_installation_is_user(account_type) {
        if github_user_installation_matches_login(account_login, &cloud_login) {
            return Ok(());
        }
        anyhow::bail!("GitHub App user installation does not belong to the signed-in account");
    }
    if github_installation_is_org(account_type) {
        if github_org_admin_for_cloud_user(state, cloud_user_id, account_login).await? {
            return Ok(());
        }
        anyhow::bail!("GitHub organization admin access is required to link this installation");
    }
    anyhow::bail!("unsupported GitHub App installation account type")
}

fn github_installation_is_user(account_type: &str) -> bool {
    account_type.eq_ignore_ascii_case("User")
}

fn github_installation_is_org(account_type: &str) -> bool {
    account_type.eq_ignore_ascii_case("Organization")
}

fn github_user_installation_matches_login(account_login: &str, cloud_login: &str) -> bool {
    account_login.eq_ignore_ascii_case(cloud_login)
}

async fn github_org_admin_for_cloud_user(
    state: &AppState,
    cloud_user_id: Uuid,
    org: &str,
) -> anyhow::Result<bool> {
    let token_ciphertext: Option<String> = sqlx::query_scalar(
        "SELECT ga.access_token_ciphertext
         FROM github_accounts ga
         JOIN users u ON u.id=ga.user_id
         JOIN cloud_users cu ON cu.github_id=u.github_id
         WHERE cu.id=$1
         ORDER BY ga.updated_at DESC
         LIMIT 1",
    )
    .bind(cloud_user_id)
    .fetch_optional(&state.db)
    .await?;
    let Some(token_ciphertext) = token_ciphertext else {
        return Ok(false);
    };
    let token = state.crypto.decrypt(&token_ciphertext)?;
    let response = state
        .http
        .get(format!(
            "https://api.github.com/user/memberships/orgs/{org}"
        ))
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "Hostlet")
        .send()
        .await?;
    if response.status().as_u16() == 403 || response.status().as_u16() == 404 {
        return Ok(false);
    }
    let value = response.error_for_status()?.json::<Value>().await?;
    let active = value
        .get("state")
        .and_then(|value| value.as_str())
        .is_some_and(|state| state == "active");
    let admin = value
        .get("role")
        .and_then(|value| value.as_str())
        .is_some_and(|role| matches!(role, "admin" | "owner"));
    Ok(active && admin)
}

fn signed_value(secret: &str, value: &str, ttl: Duration) -> String {
    let expires_at = (Utc::now() + ttl).timestamp();
    let payload = URL_SAFE_NO_PAD.encode(value.as_bytes());
    let data = format!("v2.{payload}.{expires_at}");
    let signature = sign(secret, data.as_bytes());
    format!("{data}.{signature}")
}

fn verify_signed_value(secret: &str, value: &str) -> Option<String> {
    if let Some(value) = value.strip_prefix("v2.") {
        return verify_signed_value_v2(secret, value);
    }
    verify_signed_value_v1(secret, value)
}

fn verify_signed_value_v2(secret: &str, value: &str) -> Option<String> {
    let mut parts = value.splitn(3, '.');
    let payload = parts.next()?;
    let expires_at = parts.next()?.parse::<i64>().ok()?;
    let signature = parts.next()?;
    if Utc::now().timestamp() > expires_at {
        return None;
    }
    let data = format!("v2.{payload}.{expires_at}");
    if !verify_signature(secret, data.as_bytes(), signature) {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    String::from_utf8(decoded).ok()
}

fn verify_signed_value_v1(secret: &str, value: &str) -> Option<String> {
    let mut parts = value.splitn(3, '.');
    let payload = parts.next()?;
    let expires_at = parts.next()?.parse::<i64>().ok()?;
    let signature = parts.next()?;
    if Utc::now().timestamp() > expires_at {
        return None;
    }
    let data = format!("{payload}.{expires_at}");
    verify_signature(secret, data.as_bytes(), signature).then(|| payload.to_string())
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie.split(';').find_map(|part| {
        let part = part.trim();
        part.strip_prefix(name)?.strip_prefix('=')
    })
}

fn build_cookie(
    name: &str,
    value: &str,
    max_age: Option<Duration>,
    secure: bool,
    path: &str,
) -> String {
    let mut cookie = format!("{name}={value}; HttpOnly; SameSite=Lax; Path={path}");
    if let Some(max_age) = max_age {
        cookie.push_str(&format!("; Max-Age={}", max_age.num_seconds()));
    }
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

fn expire_cookie(name: &str, secure: bool, path: &str) -> String {
    let mut cookie = format!("{name}=; HttpOnly; SameSite=Lax; Path={path}; Max-Age=0");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

fn cookie_secure(public_api_url: &str) -> bool {
    public_api_url.trim_start().starts_with("https://")
}

fn request_web_origin(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
        .and_then(crate::state::normalize_origin)
        .or_else(|| {
            headers
                .get(header::ORIGIN)
                .and_then(|value| value.to_str().ok())
                .and_then(crate::state::normalize_origin)
        })
}

fn url_encode(value: &str) -> String {
    use url::form_urlencoded::byte_serialize;
    byte_serialize(value.as_bytes()).collect()
}

async fn unlock_response(state: &AppState) -> axum::response::Response {
    let unlock = build_cookie(
        UNLOCK_COOKIE,
        &signed_value(
            &state.session_secret,
            "unlocked",
            Duration::hours(UNLOCK_TTL_HOURS),
        ),
        Some(Duration::hours(UNLOCK_TTL_HOURS)),
        cookie_secure(&state.public_api_url),
        "/",
    );
    let Some(user_id) = single_existing_user_id(state).await else {
        return with_cookies(StatusCode::NO_CONTENT.into_response(), [unlock]);
    };
    let session = build_cookie(
        SESSION_COOKIE,
        &signed_value(
            &state.session_secret,
            &user_id.to_string(),
            Duration::days(SESSION_TTL_DAYS),
        ),
        Some(Duration::days(SESSION_TTL_DAYS)),
        cookie_secure(&state.public_api_url),
        "/",
    );
    with_cookies(StatusCode::NO_CONTENT.into_response(), [unlock, session])
}

fn with_cookies<const N: usize>(mut response: Response, cookies: [String; N]) -> Response {
    for cookie in cookies {
        if let Ok(value) = cookie.parse() {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

async fn control_plane_password_hash(state: &AppState) -> anyhow::Result<Option<String>> {
    let row = sqlx::query("SELECT value FROM settings WHERE key=$1")
        .bind(CONTROL_PLANE_PASSWORD_KEY)
        .fetch_optional(&state.db)
        .await?;
    Ok(row.map(|r| r.get("value")))
}

async fn has_existing_users(state: &AppState) -> anyhow::Result<bool> {
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&state.db)
        .await?;
    Ok(count > 0)
}

async fn github_user_exists(state: &AppState, github_id: i64) -> anyhow::Result<bool> {
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE github_id=$1")
        .bind(github_id)
        .fetch_one(&state.db)
        .await?;
    Ok(count > 0)
}

async fn store_control_plane_password_hash(state: &AppState, hash: &str) -> anyhow::Result<bool> {
    let done = sqlx::query(
        "INSERT INTO settings (key,value) VALUES ($1,$2)
         ON CONFLICT (key) DO NOTHING",
    )
    .bind(CONTROL_PLANE_PASSWORD_KEY)
    .bind(hash)
    .execute(&state.db)
    .await?;
    Ok(done.rows_affected() == 1)
}

async fn store_device_flow(
    state: &AppState,
    flow_id: &str,
    flow: &StoredDeviceFlow,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO settings (key,value) VALUES ($1,$2)
         ON CONFLICT (key) DO UPDATE SET value=EXCLUDED.value, updated_at=now()",
    )
    .bind(format!("{DEVICE_FLOW_KEY_PREFIX}{flow_id}"))
    .bind(serde_json::to_string(flow)?)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn load_device_flow(
    state: &AppState,
    flow_id: &str,
) -> anyhow::Result<Option<StoredDeviceFlow>> {
    let row = sqlx::query("SELECT value FROM settings WHERE key=$1")
        .bind(format!("{DEVICE_FLOW_KEY_PREFIX}{flow_id}"))
        .fetch_optional(&state.db)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_str(
        row.get::<String, _>("value").as_str(),
    )?))
}

async fn delete_device_flow(state: &AppState, flow_id: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM settings WHERE key=$1")
        .bind(format!("{DEVICE_FLOW_KEY_PREFIX}{flow_id}"))
        .execute(&state.db)
        .await?;
    Ok(())
}

async fn single_existing_user_id(state: &AppState) -> Option<Uuid> {
    let rows = sqlx::query("SELECT id FROM users ORDER BY created_at ASC LIMIT 2")
        .fetch_all(&state.db)
        .await
        .ok()?;
    if rows.len() == 1 {
        Some(rows[0].get("id"))
    } else {
        None
    }
}

fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| anyhow::anyhow!("argon2 password hashing failed: {err}"))
}

fn verify_password(hash: &str, password: &str) -> anyhow::Result<bool> {
    let parsed = PasswordHash::new(hash)
        .map_err(|err| anyhow::anyhow!("stored password hash is invalid: {err}"))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

fn valid_control_plane_password(password: &str) -> bool {
    password.chars().count() >= 12 && !password.chars().any(|c| c.is_control())
}

#[cfg(test)]
pub(crate) fn test_cloud_cookie_header(
    session_secret: &str,
    user_id: Uuid,
    cloud_token: &str,
) -> String {
    let session = signed_value(session_secret, &user_id.to_string(), Duration::hours(1));
    format!("{SESSION_COOKIE}={session}; {CLOUD_SESSION_COOKIE}={cloud_token}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_session_rejects_plain_uuid() {
        let id = Uuid::new_v4();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{SESSION_COOKIE}={id}").parse().unwrap(),
        );
        assert!(
            current_user_id_from_headers(&headers, "session-secret-session-secret-123").is_none()
        );
    }

    #[test]
    fn signed_session_accepts_valid_cookie() {
        let id = Uuid::new_v4();
        let value = signed_value(
            "session-secret-session-secret-123",
            &id.to_string(),
            Duration::minutes(5),
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{SESSION_COOKIE}={value}").parse().unwrap(),
        );
        assert_eq!(
            current_user_id_from_headers(&headers, "session-secret-session-secret-123"),
            Some(id)
        );
    }

    #[test]
    fn github_installation_user_account_must_match_cloud_login() {
        assert!(github_installation_is_user("User"));
        assert!(github_user_installation_matches_login(
            "ShaneKanterman04",
            "shanekanterman04"
        ));
        assert!(!github_user_installation_matches_login(
            "someone-else",
            "shanekanterman04"
        ));
    }

    #[test]
    fn github_installation_org_account_requires_org_path() {
        assert!(github_installation_is_org("Organization"));
        assert!(!github_installation_is_org("User"));
        assert!(!github_installation_is_user("Organization"));
    }

    #[test]
    fn cloud_compute_gate_requires_github_install_and_active_billing() {
        let missing_github = cloud_compute_gate(false, true).unwrap_err().to_string();
        assert!(missing_github.contains("GitHub App"));

        let inactive_billing = cloud_compute_gate(true, false).unwrap_err().to_string();
        assert!(inactive_billing.contains("subscription"));

        assert!(cloud_compute_gate(true, true).is_ok());
    }

    #[tokio::test]
    async fn cloud_db_auth_gates_revoked_sessions_and_cross_user_isolation() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_cloud_db(&state).await;

        let user_a = insert_app_user(&state, 101, "alice").await;
        let user_b = insert_app_user(&state, 202, "bob").await;
        let cloud_a = insert_cloud_user(&state, 101, "alice").await;
        let cloud_b = insert_cloud_user(&state, 202, "bob").await;
        insert_cloud_session(&state, cloud_a, "token-a", false).await;
        insert_cloud_session(&state, cloud_b, "token-b", false).await;

        let context = request_context(&cloud_headers(&state, user_a, "token-a"), &state)
            .await
            .unwrap();
        assert_eq!(context.user_id, user_a);
        assert_eq!(context.cloud_user_id, Some(cloud_a));

        assert!(
            request_context(&cloud_headers(&state, user_a, "token-b"), &state)
                .await
                .is_err()
        );

        insert_cloud_session(&state, cloud_a, "revoked-token", true).await;
        assert!(
            request_context(&cloud_headers(&state, user_a, "revoked-token"), &state)
                .await
                .is_err()
        );

        let gated = cloud_compute_allowed_for_context(&state, context).await;
        assert!(gated.unwrap_err().to_string().contains("GitHub App"));

        sqlx::query(
            "INSERT INTO cloud_github_installations
               (cloud_user_id, installation_id, account_login, account_type, permissions_json, repository_selection)
             VALUES ($1,12345,'alice','User','{}'::jsonb,'selected')",
        )
        .bind(cloud_a)
        .execute(&state.db)
        .await
        .unwrap();
        let context = RequestContext {
            user_id: user_a,
            cloud_user_id: Some(cloud_a),
        };
        let gated = cloud_compute_allowed_for_context(&state, context).await;
        assert!(gated.unwrap_err().to_string().contains("subscription"));

        sqlx::query(
            "INSERT INTO cloud_subscriptions (cloud_user_id, stripe_subscription_id, plan_code, status)
             VALUES ($1,'sub_inactive','starter','canceled')",
        )
        .bind(cloud_a)
        .execute(&state.db)
        .await
        .unwrap();
        let context = RequestContext {
            user_id: user_a,
            cloud_user_id: Some(cloud_a),
        };
        assert!(cloud_compute_allowed_for_context(&state, context)
            .await
            .is_err());

        sqlx::query(
            "INSERT INTO cloud_subscriptions (cloud_user_id, stripe_subscription_id, plan_code, status)
             VALUES ($1,'sub_active','starter','active')",
        )
        .bind(cloud_a)
        .execute(&state.db)
        .await
        .unwrap();
        let context = RequestContext {
            user_id: user_a,
            cloud_user_id: Some(cloud_a),
        };
        assert!(cloud_compute_allowed_for_context(&state, context)
            .await
            .is_ok());

        assert!(
            request_context(&cloud_headers(&state, user_b, "token-a"), &state)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn cloud_db_github_install_callback_rejects_bad_state_before_network() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_cloud_db(&state).await;
        let user_id = insert_app_user(&state, 3030, "install-user").await;
        let cloud_user_id = insert_cloud_user(&state, 3030, "install-user").await;
        insert_cloud_session(&state, cloud_user_id, "install-token", false).await;

        let base_headers = cloud_headers(&state, user_id, "install-token");
        assert_redirect_contains(
            cloud_github_install_callback(
                State(state.clone()),
                base_headers.clone(),
                Query(GitHubInstallCallbackQuery {
                    installation_id: Some(1),
                    setup_action: None,
                    state: None,
                }),
            )
            .await
            .into_response(),
            "missing%20installation%20state",
        );

        assert_redirect_contains(
            cloud_github_install_callback(
                State(state.clone()),
                base_headers.clone(),
                Query(GitHubInstallCallbackQuery {
                    installation_id: Some(1),
                    setup_action: None,
                    state: Some("nonce".into()),
                }),
            )
            .await
            .into_response(),
            "expired%20installation%20state",
        );

        let mut mismatched_headers = base_headers.clone();
        mismatched_headers.insert(
            header::COOKIE,
            format!(
                "{}; {}={}",
                test_cloud_cookie_header(&state.session_secret, user_id, "install-token"),
                GITHUB_INSTALL_STATE_COOKIE,
                signed_value(&state.session_secret, "expected", Duration::minutes(15))
            )
            .parse()
            .unwrap(),
        );
        assert_redirect_contains(
            cloud_github_install_callback(
                State(state.clone()),
                mismatched_headers.clone(),
                Query(GitHubInstallCallbackQuery {
                    installation_id: Some(1),
                    setup_action: None,
                    state: Some("actual".into()),
                }),
            )
            .await
            .into_response(),
            "invalid%20installation%20state",
        );

        assert_redirect_contains(
            cloud_github_install_callback(
                State(state),
                mismatched_headers,
                Query(GitHubInstallCallbackQuery {
                    installation_id: None,
                    setup_action: None,
                    state: Some("expected".into()),
                }),
            )
            .await
            .into_response(),
            "missing%20installation",
        );
    }

    #[tokio::test]
    async fn cloud_db_installation_owner_rejects_cross_account_linking() {
        let Some(state) = crate::state::db_test_state_from_env().await else {
            return;
        };
        reset_cloud_db(&state).await;
        let cloud_a = insert_cloud_user(&state, 4040, "alice").await;
        let cloud_b = insert_cloud_user(&state, 5050, "bob").await;
        sqlx::query(
            "INSERT INTO cloud_github_installations
               (cloud_user_id, installation_id, account_login, account_type, permissions_json, repository_selection)
             VALUES ($1,67890,'alice','User','{}'::jsonb,'selected')",
        )
        .bind(cloud_a)
        .execute(&state.db)
        .await
        .unwrap();

        let duplicate = ensure_installation_owner(&state, cloud_b, 67890, "bob", "User").await;
        assert!(duplicate
            .unwrap_err()
            .to_string()
            .contains("already linked"));

        let mismatch = ensure_installation_owner(&state, cloud_b, 98765, "alice", "User").await;
        assert!(mismatch
            .unwrap_err()
            .to_string()
            .contains("does not belong"));
    }

    async fn reset_cloud_db(state: &AppState) {
        sqlx::query(
            "TRUNCATE cloud_webhook_events, cloud_usage_buckets, cloud_subscriptions,
             cloud_stripe_customers, cloud_github_installations, cloud_sessions,
             cloud_users, users CASCADE",
        )
        .execute(&state.db)
        .await
        .unwrap();
    }

    async fn insert_app_user(state: &AppState, github_id: i64, login: &str) -> Uuid {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO users (github_id, login) VALUES ($1,$2) RETURNING id",
        )
        .bind(github_id)
        .bind(login)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    async fn insert_cloud_user(state: &AppState, github_id: i64, login: &str) -> Uuid {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO cloud_users (github_id, login) VALUES ($1,$2) RETURNING id",
        )
        .bind(github_id)
        .bind(login)
        .fetch_one(&state.db)
        .await
        .unwrap()
    }

    async fn insert_cloud_session(
        state: &AppState,
        cloud_user_id: Uuid,
        token: &str,
        revoked: bool,
    ) {
        sqlx::query(
            "INSERT INTO cloud_sessions (cloud_user_id, token_hash, expires_at, revoked_at)
             VALUES ($1,$2,now() + interval '1 hour', CASE WHEN $3 THEN now() ELSE NULL END)",
        )
        .bind(cloud_user_id)
        .bind(hash_token(token))
        .bind(revoked)
        .execute(&state.db)
        .await
        .unwrap();
    }

    fn cloud_headers(state: &AppState, user_id: Uuid, cloud_token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            test_cloud_cookie_header(&state.session_secret, user_id, cloud_token)
                .parse()
                .unwrap(),
        );
        headers
    }

    fn assert_redirect_contains(response: Response, expected: &str) {
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(
            location.contains(expected),
            "redirect location {location:?} did not contain {expected:?}"
        );
    }
}
