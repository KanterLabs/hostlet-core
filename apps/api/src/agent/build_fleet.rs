//! Operator-managed build pools and outbound-only builder enrollment.
//!
//! This module deliberately lives outside `web/servers.rs`: Hostlet Cloud
//! replaces that file at overlay time, while the build scheduler and protocol
//! are shared Core behavior.

use crate::{
    auth::current_user_id,
    crypto::{hash_token, random_token},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

const ENROLLMENT_LIFETIME_MINUTES: i64 = 15;
const DEFAULT_LOCAL_POOL_ID: &str = "00000000-0000-0000-0000-000000000010";

#[allow(clippy::result_large_err)]
fn owner(headers: &HeaderMap, state: &AppState) -> Result<Uuid, Response> {
    current_user_id(headers, state).ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())
}

fn valid_platform(value: &str) -> bool {
    matches!(value, "linux/amd64" | "linux/arm64")
}

fn normalized_platforms(values: Vec<String>) -> Result<Vec<String>, &'static str> {
    let mut values = values
        .into_iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    if values.is_empty() || values.iter().any(|value| !valid_platform(value)) {
        return Err("platforms must contain linux/amd64 or linux/arm64");
    }
    Ok(values)
}

fn pool_json(row: &sqlx::postgres::PgRow) -> Value {
    json!({
        "id": row.get::<Uuid, _>("id"),
        "name": row.get::<String, _>("name"),
        "provider": row.get::<String, _>("provider"),
        "enabled": row.get::<bool, _>("enabled"),
        "isDefault": row.get::<bool, _>("is_default"),
        "maxConcurrentBuilds": row.get::<i32, _>("max_concurrent_builds"),
        "supportedPlatforms": row.get::<Vec<String>, _>("supported_platforms"),
        "qualificationStatus": row.get::<String, _>("qualification_status"),
        "qualification": row.get::<Value, _>("qualification_json"),
        "config": row.get::<Value, _>("config_json"),
        "createdAt": row.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
        "updatedAt": row.get::<chrono::DateTime<chrono::Utc>, _>("updated_at"),
    })
}

pub async fn list_build_pools(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(response) = owner(&headers, &state) {
        return response;
    }
    match sqlx::query(
        "SELECT id,name,provider,enabled,is_default,max_concurrent_builds,
                supported_platforms,qualification_status,qualification_json,
                config_json,created_at,updated_at
         FROM build_pools ORDER BY is_default DESC,created_at ASC",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => Json(rows.iter().map(pool_json).collect::<Vec<_>>()).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "failed to list build pools");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBuildPool {
    name: String,
    provider: String,
    #[serde(default = "default_one")]
    max_concurrent_builds: i32,
    #[serde(default)]
    supported_platforms: Vec<String>,
    #[serde(default)]
    config: Value,
}

fn default_one() -> i32 {
    1
}

pub async fn create_build_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateBuildPool>,
) -> impl IntoResponse {
    if let Err(response) = owner(&headers, &state) {
        return response;
    }
    let name = request.name.trim();
    if name.is_empty() || name.len() > 80 {
        return (StatusCode::BAD_REQUEST, "pool name must be 1-80 characters").into_response();
    }
    if !matches!(request.provider.as_str(), "vm" | "cloudflare") {
        return (StatusCode::BAD_REQUEST, "provider must be vm or cloudflare").into_response();
    }
    if !(1..=64).contains(&request.max_concurrent_builds) {
        return (StatusCode::BAD_REQUEST, "maxConcurrentBuilds must be 1-64").into_response();
    }
    let defaults = if request.provider == "cloudflare" {
        vec!["linux/amd64".to_string()]
    } else {
        vec!["linux/amd64".to_string(), "linux/arm64".to_string()]
    };
    let platforms = match normalized_platforms(if request.supported_platforms.is_empty() {
        defaults
    } else {
        request.supported_platforms
    }) {
        Ok(value) => value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    if request.provider == "cloudflare" && platforms.iter().any(|value| value != "linux/amd64") {
        return (
            StatusCode::BAD_REQUEST,
            "Cloudflare builder pools support linux/amd64 only",
        )
            .into_response();
    }
    let qualification = if request.provider == "cloudflare" {
        "pending"
    } else {
        "not_required"
    };
    match sqlx::query(
        "INSERT INTO build_pools
           (name,provider,max_concurrent_builds,supported_platforms,config_json,qualification_status)
         VALUES ($1,$2,$3,$4,$5,$6)
         RETURNING id,name,provider,enabled,is_default,max_concurrent_builds,
                   supported_platforms,qualification_status,qualification_json,
                   config_json,created_at,updated_at",
    )
    .bind(name)
    .bind(&request.provider)
    .bind(request.max_concurrent_builds)
    .bind(platforms)
    .bind(request.config)
    .bind(qualification)
    .fetch_one(&state.db)
    .await
    {
        Ok(row) => (StatusCode::CREATED, Json(pool_json(&row))).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "failed to create build pool");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateBuildPool {
    name: Option<String>,
    enabled: Option<bool>,
    is_default: Option<bool>,
    max_concurrent_builds: Option<i32>,
}

pub async fn update_build_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateBuildPool>,
) -> impl IntoResponse {
    if let Err(response) = owner(&headers, &state) {
        return response;
    }
    if request
        .name
        .as_deref()
        .is_some_and(|name| name.trim().is_empty() || name.trim().len() > 80)
    {
        return (StatusCode::BAD_REQUEST, "pool name must be 1-80 characters").into_response();
    }
    if request
        .max_concurrent_builds
        .is_some_and(|value| !(1..=64).contains(&value))
    {
        return (StatusCode::BAD_REQUEST, "maxConcurrentBuilds must be 1-64").into_response();
    }
    let mut tx = match state.db.begin().await {
        Ok(value) => value,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if request.is_default == Some(true)
        && sqlx::query("UPDATE build_pools SET is_default=false,updated_at=now() WHERE is_default")
            .execute(&mut *tx)
            .await
            .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let row = sqlx::query(
        "UPDATE build_pools SET
           name=COALESCE($2,name),enabled=COALESCE($3,enabled),
           is_default=COALESCE($4,is_default),
           max_concurrent_builds=COALESCE($5,max_concurrent_builds),updated_at=now()
         WHERE id=$1
         RETURNING id,name,provider,enabled,is_default,max_concurrent_builds,
                   supported_platforms,qualification_status,qualification_json,
                   config_json,created_at,updated_at",
    )
    .bind(id)
    .bind(request.name.map(|name| name.trim().to_string()))
    .bind(request.enabled)
    .bind(request.is_default)
    .bind(request.max_concurrent_builds)
    .fetch_optional(&mut *tx)
    .await;
    match row {
        Ok(Some(row)) => match tx.commit().await {
            Ok(()) => Json(pool_json(&row)).into_response(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "failed to update build pool");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn delete_build_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(response) = owner(&headers, &state) {
        return response;
    }
    if id.to_string() == DEFAULT_LOCAL_POOL_ID {
        return (
            StatusCode::CONFLICT,
            "the local build pool cannot be deleted",
        )
            .into_response();
    }
    let dependencies: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM servers WHERE build_pool_id=$1)
             OR EXISTS(SELECT 1 FROM apps WHERE build_pool_id=$1)
             OR EXISTS(SELECT 1 FROM deployment_builds WHERE build_pool_id=$1)",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(true);
    if dependencies {
        return (
            StatusCode::CONFLICT,
            "drain/remove builders and app overrides before deleting this pool",
        )
            .into_response();
    }
    match sqlx::query("DELETE FROM build_pools WHERE id=$1 AND NOT is_default")
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(result) if result.rows_affected() == 1 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn list_builders(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(response) = owner(&headers, &state) {
        return response;
    }
    match sqlx::query(
        "SELECT s.id,s.name,s.kind,s.status,s.last_seen_at,s.draining,
                s.max_concurrent_builds,s.platforms,s.resource_snapshot_json,
                s.resource_snapshot_at,s.build_pool_id,s.agent_protocol_version,
                p.name AS pool_name
         FROM servers s LEFT JOIN build_pools p ON p.id=s.build_pool_id
         WHERE s.capabilities @> ARRAY['builder']::TEXT[]
         ORDER BY s.kind='local' DESC,s.created_at ASC",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| {
                    json!({
                        "id": row.get::<Uuid, _>("id"),
                        "name": row.get::<String, _>("name"),
                        "kind": row.get::<String, _>("kind"),
                        "status": row.get::<String, _>("status"),
                        "lastSeenAt": row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("last_seen_at"),
                        "draining": row.get::<bool, _>("draining"),
                        "maxConcurrentBuilds": row.get::<i32, _>("max_concurrent_builds"),
                        "platforms": row.get::<Vec<String>, _>("platforms"),
                        "resourceSnapshot": row.get::<Option<Value>, _>("resource_snapshot_json"),
                        "resourceSnapshotAt": row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("resource_snapshot_at"),
                        "buildPoolId": row.get::<Option<Uuid>, _>("build_pool_id"),
                        "buildPoolName": row.get::<Option<String>, _>("pool_name"),
                        "agentProtocolVersion": row.get::<i32, _>("agent_protocol_version"),
                        "universalBuilds": row.get::<i32, _>("agent_protocol_version")
                            >= hostlet_contracts::DEPLOYMENT_PROTOCOL_VERSION,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateEnrollment {
    #[serde(default = "default_one")]
    max_concurrent_builds: i32,
}

pub async fn create_builder_enrollment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    request: Option<Json<CreateEnrollment>>,
) -> impl IntoResponse {
    let owner_user_id = match owner(&headers, &state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let slots = request
        .map(|Json(value)| value.max_concurrent_builds)
        .unwrap_or(1);
    if !(1..=16).contains(&slots) {
        return (StatusCode::BAD_REQUEST, "maxConcurrentBuilds must be 1-16").into_response();
    }
    let provider: Option<String> =
        sqlx::query_scalar("SELECT provider FROM build_pools WHERE id=$1 AND enabled=true")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    if provider.as_deref() != Some("vm") {
        return (
            StatusCode::CONFLICT,
            "enrollment tokens are available only for enabled VM pools",
        )
            .into_response();
    }
    let token = random_token(48);
    if sqlx::query(
        "INSERT INTO builder_enrollment_tokens(build_pool_id,owner_user_id,token_hash,expires_at)
         VALUES ($1,$2,$3,now() + interval '15 minutes')",
    )
    .bind(id)
    .bind(owner_user_id)
    .bind(hash_token(&token))
    .execute(&state.db)
    .await
    .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let api = state.public_api_url.trim_end_matches('/');
    let command = format!(
        "curl -fsSL {api}/install-builder.sh | sudo sh -s -- --api {api} --token {token} --slots {slots}"
    );
    Json(json!({
        "token": token,
        "expiresInSeconds": ENROLLMENT_LIFETIME_MINUTES * 60,
        "installCommand": command,
    }))
    .into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterBuilder {
    enrollment_token: String,
    name: String,
    platforms: Vec<String>,
    #[serde(default = "default_one")]
    max_concurrent_builds: i32,
}

pub async fn register_builder(
    State(state): State<AppState>,
    Json(request): Json<RegisterBuilder>,
) -> impl IntoResponse {
    let name = request.name.trim();
    if name.is_empty() || name.len() > 80 || !(1..=16).contains(&request.max_concurrent_builds) {
        return (
            StatusCode::BAD_REQUEST,
            "invalid builder name or concurrency",
        )
            .into_response();
    }
    let platforms = match normalized_platforms(request.platforms) {
        Ok(value) => value,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let agent_token = random_token(64);
    let signing_secret = random_token(64);
    let mut tx = match state.db.begin().await {
        Ok(value) => value,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let enrollment = sqlx::query(
        "SELECT id,build_pool_id,owner_user_id FROM builder_enrollment_tokens
         WHERE token_hash=$1 AND consumed_at IS NULL AND expires_at>now()
         FOR UPDATE",
    )
    .bind(hash_token(&request.enrollment_token))
    .fetch_optional(&mut *tx)
    .await;
    let Ok(Some(enrollment)) = enrollment else {
        return (
            StatusCode::UNAUTHORIZED,
            "invalid or expired enrollment token",
        )
            .into_response();
    };
    let enrollment_id = enrollment.get::<Uuid, _>("id");
    let pool_id = enrollment.get::<Uuid, _>("build_pool_id");
    let owner_user_id = enrollment.get::<Uuid, _>("owner_user_id");
    let signing_ciphertext = match state.crypto.encrypt(&signing_secret) {
        Ok(value) => value,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let row = sqlx::query(
        "INSERT INTO servers
           (user_id,name,kind,status,agent_token_hash,job_signing_secret_ciphertext,
            capabilities,draining,max_concurrent_apps,max_concurrent_builds,
            build_pool_id,platforms)
         VALUES
           ($1,$2,'builder_vm','offline',$3,$4,ARRAY['builder']::TEXT[],false,1,$5,$6,$7)
         RETURNING id",
    )
    .bind(owner_user_id)
    .bind(name)
    .bind(hash_token(&agent_token))
    .bind(signing_ciphertext)
    .bind(request.max_concurrent_builds)
    .bind(pool_id)
    .bind(platforms)
    .fetch_one(&mut *tx)
    .await;
    let Ok(row) = row else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if sqlx::query("UPDATE builder_enrollment_tokens SET consumed_at=now() WHERE id=$1")
        .bind(enrollment_id)
        .execute(&mut *tx)
        .await
        .is_err()
        || tx.commit().await.is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    Json(json!({
        "serverId": row.get::<Uuid, _>("id"),
        "buildPoolId": pool_id,
        "agentToken": agent_token,
        "jobSigningSecret": signing_secret,
        "apiUrl": state.public_api_url,
        "agentImage": std::env::var("HOSTLET_AGENT_IMAGE").ok(),
        "registryUrl": std::env::var("HOSTLET_ARTIFACT_REGISTRY_URL").ok(),
    }))
    .into_response()
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateBuilder {
    name: Option<String>,
    draining: Option<bool>,
    max_concurrent_builds: Option<i32>,
}

pub async fn update_builder(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateBuilder>,
) -> impl IntoResponse {
    if let Err(response) = owner(&headers, &state) {
        return response;
    }
    if request
        .max_concurrent_builds
        .is_some_and(|value| !(1..=16).contains(&value))
    {
        return (StatusCode::BAD_REQUEST, "maxConcurrentBuilds must be 1-16").into_response();
    }
    let result = sqlx::query(
        "UPDATE servers SET name=COALESCE($2,name),draining=COALESCE($3,draining),
                max_concurrent_builds=COALESCE($4,max_concurrent_builds)
         WHERE id=$1 AND capabilities @> ARRAY['builder']::TEXT[]
         RETURNING id",
    )
    .bind(id)
    .bind(request.name.map(|name| name.trim().to_string()))
    .bind(request.draining)
    .bind(request.max_concurrent_builds)
    .fetch_optional(&state.db)
    .await;
    match result {
        Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn revoke_builder(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(response) = owner(&headers, &state) {
        return response;
    }
    let local_id = state.local_server_id;
    if id == local_id {
        return (StatusCode::CONFLICT, "the local builder cannot be revoked").into_response();
    }
    match sqlx::query(
        "UPDATE servers SET agent_token_hash=NULL,status='revoked',draining=true
         WHERE id=$1 AND kind='builder_vm'",
    )
    .bind(id)
    .execute(&state.db)
    .await
    {
        Ok(result) if result.rows_affected() == 1 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetAppBuildPool {
    build_pool_id: Option<Uuid>,
}

pub async fn set_app_build_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<SetAppBuildPool>,
) -> impl IntoResponse {
    let user_id = match owner(&headers, &state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Some(pool_id) = request.build_pool_id {
        let enabled: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM build_pools WHERE id=$1 AND enabled=true)",
        )
        .bind(pool_id)
        .fetch_one(&state.db)
        .await
        .unwrap_or(false);
        if !enabled {
            return (StatusCode::BAD_REQUEST, "build pool is not enabled").into_response();
        }
    }
    match sqlx::query(
        "UPDATE apps SET build_pool_id=$1,updated_at=now() WHERE id=$2 AND user_id=$3",
    )
    .bind(request.build_pool_id)
    .bind(id)
    .bind(user_id)
    .execute(&state.db)
    .await
    {
        Ok(result) if result.rows_affected() == 1 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn registry_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(response) = owner(&headers, &state) {
        return response;
    }
    let Some(url) = std::env::var("HOSTLET_ARTIFACT_REGISTRY_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Json(json!({"configured": false, "healthy": false})).into_response();
    };
    let health_url =
        std::env::var("HOSTLET_ARTIFACT_REGISTRY_INTERNAL_URL").unwrap_or_else(|_| url.clone());
    let endpoint = format!("{}/v2/", health_url.trim_end_matches('/'));
    let result = state.http.get(&endpoint).send().await;
    let healthy = result.as_ref().is_ok_and(|response| {
        response.status().is_success() || response.status() == StatusCode::UNAUTHORIZED
    });
    Json(json!({
        "configured": true,
        "healthy": healthy,
        "url": url,
        "status": result.ok().map(|response| response.status().as_u16()),
    }))
    .into_response()
}

/// Cloudflare Workers Builds deploy Workers and Cloudflare Containers images;
/// they do not currently expose the generic privileged BuildKit contract that
/// Hostlet needs to build arbitrary repositories and push their OCI manifests
/// to a caller-selected registry. Keep the provider visible and measurable,
/// but fail closed until an external dispatcher proves the full contract.
pub async fn qualify_build_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(response) = owner(&headers, &state) {
        return response;
    }
    let result = sqlx::query(
        "UPDATE build_pools
         SET qualification_status=CASE WHEN provider='cloudflare' THEN 'failed' ELSE 'not_required' END,
             qualification_json=CASE WHEN provider='cloudflare' THEN $2 ELSE '{}'::jsonb END,
             updated_at=now()
         WHERE id=$1
         RETURNING provider,qualification_status,qualification_json",
    )
    .bind(id)
    .bind(json!({
        "checkedAt": chrono::Utc::now(),
        "reason": "Cloudflare does not expose a generic remote Docker/BuildKit job that returns an OCI digest to Hostlet",
        "required": [
            "clone an exact Git commit",
            "run Dockerfile, Railpack, Compose, and generated-topology builds",
            "mirror every service image to the configured OCI registry",
            "publish a Hostlet v2 OCI release bundle for the runner platform",
            "stream logs and accept cancellation"
        ],
        "status": "experimental_not_dispatchable"
    }))
    .fetch_optional(&state.db)
    .await;
    match result {
        Ok(Some(row)) => Json(json!({
            "provider": row.get::<String, _>("provider"),
            "qualificationStatus": row.get::<String, _>("qualification_status"),
            "qualification": row.get::<Value, _>("qualification_json"),
        }))
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platforms_are_normalized_and_restricted() {
        assert_eq!(
            normalized_platforms(vec!["linux/AMD64".into(), "linux/amd64".into()]).unwrap(),
            vec!["linux/amd64"]
        );
        assert!(normalized_platforms(vec!["windows/amd64".into()]).is_err());
        assert!(normalized_platforms(Vec::new()).is_err());
    }
}
