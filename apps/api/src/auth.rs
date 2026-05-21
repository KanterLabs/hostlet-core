use crate::{
    crypto::{constant_time_eq, random_token, sign, verify_signature},
    state::AppState,
};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use url::Url;
use uuid::Uuid;

const SESSION_COOKIE: &str = "hostlet_session";
const OAUTH_STATE_COOKIE: &str = "hostlet_oauth_state";
const OAUTH_WEB_ORIGIN_COOKIE: &str = "hostlet_oauth_web_origin";
const UNLOCK_COOKIE: &str = "hostlet_unlock";
const SESSION_TTL_DAYS: i64 = 14;
const OAUTH_STATE_TTL_MINUTES: i64 = 10;
const UNLOCK_TTL_HOURS: i64 = 12;
const CONTROL_PLANE_PASSWORD_KEY: &str = "control_plane_password_hash";
const OAUTH_STATE_KEY_PREFIX: &str = "oauth_state:";

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: String,
    state: String,
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
}

#[derive(Serialize, Deserialize)]
struct StoredOAuthState {
    web_origin: String,
    expires_at: i64,
}

pub async fn github_start(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if state.github_client_id.trim().is_empty() || state.github_client_secret.trim().is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "GitHub OAuth is not configured",
        )
            .into_response();
    }
    let csrf_state = random_token(48);
    let callback_base =
        request_api_origin(&headers).unwrap_or_else(|| state.public_api_url.clone());
    let callback_url = format!(
        "{}/auth/github/callback",
        callback_base.trim_end_matches('/')
    );
    let url = Url::parse_with_params(
        "https://github.com/login/oauth/authorize",
        &[
            ("client_id", state.github_client_id.as_str()),
            ("scope", "repo read:user"),
            ("redirect_uri", callback_url.as_str()),
            ("state", csrf_state.as_str()),
        ],
    )
    .expect("static GitHub OAuth URL is valid");
    let secure = cookie_secure(&callback_base);
    let web_origin = request_web_origin(&headers).unwrap_or_else(|| state.public_web_url.clone());
    if let Err(err) = store_oauth_state(&state, &csrf_state, &web_origin).await {
        tracing::error!(error = %err, "failed to store OAuth state");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let state_cookie = build_cookie(
        OAUTH_STATE_COOKIE,
        &signed_value(
            &state.session_secret,
            &csrf_state,
            Duration::minutes(OAUTH_STATE_TTL_MINUTES),
        ),
        Some(Duration::minutes(OAUTH_STATE_TTL_MINUTES)),
        secure,
        "/auth/github/callback",
    );
    let web_origin_cookie = build_cookie(
        OAUTH_WEB_ORIGIN_COOKIE,
        &signed_value(
            &state.session_secret,
            &web_origin,
            Duration::minutes(OAUTH_STATE_TTL_MINUTES),
        ),
        Some(Duration::minutes(OAUTH_STATE_TTL_MINUTES)),
        secure,
        "/auth/github/callback",
    );
    (
        [
            (header::SET_COOKIE, state_cookie),
            (header::SET_COOKIE, web_origin_cookie),
        ],
        Redirect::temporary(url.as_str()),
    )
        .into_response()
}

pub async fn github_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> impl IntoResponse {
    let cookie_state_valid = cookie_value(&headers, OAUTH_STATE_COOKIE)
        .and_then(|value| verify_signed_value(&state.session_secret, value))
        .is_some_and(|cookie_state| constant_time_eq(cookie_state.as_bytes(), q.state.as_bytes()));
    let stored_web_origin = consume_oauth_state(&state, &q.state).await.ok().flatten();
    if !cookie_state_valid && stored_web_origin.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let callback_base =
        request_api_origin(&headers).unwrap_or_else(|| state.public_api_url.clone());
    let callback_url = format!(
        "{}/auth/github/callback",
        callback_base.trim_end_matches('/')
    );
    match exchange_and_store(&state, q.code, &callback_url).await {
        Ok(user_id) => {
            let request_api_origin =
                request_api_origin(&headers).unwrap_or_else(|| state.public_api_url.clone());
            let secure = cookie_secure(&request_api_origin);
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
            let unlock = build_cookie(
                UNLOCK_COOKIE,
                &signed_value(
                    &state.session_secret,
                    "unlocked",
                    Duration::hours(UNLOCK_TTL_HOURS),
                ),
                Some(Duration::hours(UNLOCK_TTL_HOURS)),
                secure,
                "/",
            );
            let clear_oauth = expire_cookie(OAUTH_STATE_COOKIE, secure, "/auth/github/callback");
            let web_origin = stored_web_origin
                .or_else(|| {
                    cookie_value(&headers, OAUTH_WEB_ORIGIN_COOKIE)
                        .and_then(|value| verify_signed_value(&state.session_secret, value))
                })
                .unwrap_or_else(|| state.public_web_url.clone());
            let clear_web_origin =
                expire_cookie(OAUTH_WEB_ORIGIN_COOKIE, secure, "/auth/github/callback");
            (
                [
                    (header::SET_COOKIE, session),
                    (header::SET_COOKIE, unlock),
                    (header::SET_COOKIE, clear_oauth),
                    (header::SET_COOKIE, clear_web_origin),
                ],
                Redirect::temporary(&web_origin),
            )
                .into_response()
        }
        Err(err) => {
            let clear_oauth = expire_cookie(
                OAUTH_STATE_COOKIE,
                cookie_secure(&state.public_api_url),
                "/auth/github/callback",
            );
            (
                [(header::SET_COOKIE, clear_oauth)],
                (
                    StatusCode::BAD_REQUEST,
                    format!("GitHub login failed: {err}"),
                ),
            )
                .into_response()
        }
    }
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

pub async fn setup_status(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let setup_required = control_plane_password_hash(&state)
        .await
        .ok()
        .flatten()
        .is_none();
    Json(serde_json::json!({
        "setupRequired": setup_required,
        "unlocked": !setup_required && control_plane_unlocked(&headers, &state.session_secret)
    }))
}

pub async fn setup_password(
    State(state): State<AppState>,
    Json(body): Json<PasswordBody>,
) -> impl IntoResponse {
    if !valid_control_plane_password(&body.password) {
        return (
            StatusCode::BAD_REQUEST,
            "password must be at least 12 characters",
        )
            .into_response();
    }
    if control_plane_password_hash(&state)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        return (
            StatusCode::CONFLICT,
            "control plane password is already set",
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
        Ok(()) => unlock_response(&state).await,
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
    current_user_id_from_headers(headers, &state.session_secret)
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

async fn exchange_and_store(
    state: &AppState,
    code: String,
    redirect_uri: &str,
) -> anyhow::Result<Uuid> {
    let client = reqwest::Client::new();
    let token: GitHubToken = client.post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .json(&serde_json::json!({"client_id": state.github_client_id, "client_secret": state.github_client_secret, "code": code, "redirect_uri": redirect_uri}))
        .send().await?.error_for_status()?.json().await?;
    let gh_user: GitHubUser = client
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
    }
    let mut tx = state.db.begin().await?;
    let row = sqlx::query("INSERT INTO users (github_id, login, name, avatar_url) VALUES ($1,$2,$3,$4) ON CONFLICT (github_id) DO UPDATE SET login=EXCLUDED.login, name=EXCLUDED.name, avatar_url=EXCLUDED.avatar_url RETURNING id")
        .bind(gh_user.id).bind(&gh_user.login).bind(&gh_user.name).bind(&gh_user.avatar_url)
        .fetch_one(&mut *tx).await?;
    let user_id: Uuid = row.get("id");
    let encrypted = state.crypto.encrypt(&token.access_token)?;
    sqlx::query("INSERT INTO github_accounts (user_id, github_id, access_token_ciphertext, scopes) VALUES ($1,$2,$3,$4)")
        .bind(user_id).bind(gh_user.id).bind(encrypted).bind(token.scope.unwrap_or_default())
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(user_id)
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

fn request_api_origin(headers: &HeaderMap) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("http");
    Some(format!("{proto}://{host}"))
}

fn request_web_origin(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Url::parse(value).ok())
        .map(|url| {
            let mut origin = format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default());
            if let Some(port) = url.port() {
                origin.push_str(&format!(":{port}"));
            }
            origin
        })
        .or_else(|| {
            headers
                .get(header::ORIGIN)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
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
        return ([(header::SET_COOKIE, unlock)], StatusCode::NO_CONTENT).into_response();
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
    (
        [(header::SET_COOKIE, unlock), (header::SET_COOKIE, session)],
        StatusCode::NO_CONTENT,
    )
        .into_response()
}

async fn control_plane_password_hash(state: &AppState) -> anyhow::Result<Option<String>> {
    let row = sqlx::query("SELECT value FROM settings WHERE key=$1")
        .bind(CONTROL_PLANE_PASSWORD_KEY)
        .fetch_optional(&state.db)
        .await?;
    Ok(row.map(|r| r.get("value")))
}

async fn store_control_plane_password_hash(state: &AppState, hash: &str) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO settings (key,value) VALUES ($1,$2)
         ON CONFLICT (key) DO UPDATE SET value=EXCLUDED.value, updated_at=now()",
    )
    .bind(CONTROL_PLANE_PASSWORD_KEY)
    .bind(hash)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn store_oauth_state(
    state: &AppState,
    csrf_state: &str,
    web_origin: &str,
) -> anyhow::Result<()> {
    let stored = StoredOAuthState {
        web_origin: web_origin.to_string(),
        expires_at: (Utc::now() + Duration::minutes(OAUTH_STATE_TTL_MINUTES)).timestamp(),
    };
    sqlx::query(
        "INSERT INTO settings (key,value) VALUES ($1,$2)
         ON CONFLICT (key) DO UPDATE SET value=EXCLUDED.value, updated_at=now()",
    )
    .bind(format!("{OAUTH_STATE_KEY_PREFIX}{csrf_state}"))
    .bind(serde_json::to_string(&stored)?)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn consume_oauth_state(state: &AppState, csrf_state: &str) -> anyhow::Result<Option<String>> {
    let row = sqlx::query("DELETE FROM settings WHERE key=$1 RETURNING value")
        .bind(format!("{OAUTH_STATE_KEY_PREFIX}{csrf_state}"))
        .fetch_optional(&state.db)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let stored: StoredOAuthState = serde_json::from_str(row.get("value"))?;
    if Utc::now().timestamp() > stored.expires_at {
        return Ok(None);
    }
    Ok(Some(stored.web_origin))
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
}
