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
use serde_json::Value;
use sqlx::Row;

pub async fn status(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let oauth_configured =
        !state.github_client_id.trim().is_empty() && !state.github_client_secret.trim().is_empty();
    let webhook_configured = !state.github_webhook_secret.trim().is_empty()
        && state.github_webhook_secret != "dev-webhook-secret";

    let Some(user_id) = current_user_id(&headers, &state) else {
        return Json(serde_json::json!({
            "oauthConfigured": oauth_configured,
            "webhookConfigured": webhook_configured,
            "authenticated": false,
            "tokenValid": null,
            "login": null,
            "message": if oauth_configured { "GitHub OAuth is configured. Sign in to verify your account token." } else { "GitHub OAuth is missing GITHUB_CLIENT_ID or GITHUB_CLIENT_SECRET." }
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
            "message": "No GitHub token is stored for this session. Sign in with GitHub again."
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

    let res = reqwest::Client::new()
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
            "message": format!("GitHub token check failed with status {}. Sign in again.", resp.status())
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
    let res = reqwest::Client::new()
        .get("https://api.github.com/user/repos?per_page=100&sort=updated")
        .bearer_auth(token)
        .header("User-Agent", "Hostlet")
        .send()
        .await;
    match res {
        Ok(r) => match r.json::<Value>().await {
            Ok(v) => Json(v).into_response(),
            Err(_) => StatusCode::BAD_GATEWAY.into_response(),
        },
        Err(_) => StatusCode::BAD_GATEWAY.into_response(),
    }
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
    let _ = sqlx::query("INSERT INTO webhook_events (github_delivery_id,repo_full_name,event_type,payload) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING")
        .bind(delivery).bind(repo).bind(event).bind(&payload).execute(&state.db).await;
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
        let apps = sqlx::query("SELECT id,user_id FROM apps WHERE repo_full_name=$1 AND branch=$2")
            .bind(repo)
            .bind(branch)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();
        for app in apps {
            let _ = create_and_send_deploy(&state, app.get("user_id"), app.get("id"), sha).await;
        }
    }
    StatusCode::ACCEPTED.into_response()
}
