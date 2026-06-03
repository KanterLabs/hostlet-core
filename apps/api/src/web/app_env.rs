use super::apps::request_context_or_response;
use super::*;

/// Maximum byte length of a single env var value, matching the limit enforced
/// by [`validate_env_vars`] for app-level env definitions.
const MAX_ENV_VALUE_LEN: usize = 65_536;

pub async fn app_env_vars(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    // Read-only listing: like the other customer read endpoints, any auth
    // failure collapses to 401. The mutating handlers below instead use
    // `request_context_or_response`, which preserves the 402 surface.
    let context = match customer_context(&headers, &state).await {
        Ok(context) => context,
        Err(response) => return response,
    };
    let user_id = context.user_id;
    if !app_belongs_to_user(&state, id, user_id).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    match sqlx::query("SELECT key FROM app_env_vars WHERE app_id=$1 ORDER BY key ASC")
        .bind(id)
        .fetch_all(&state.db)
        .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| serde_json::json!({"key": row.get::<String, _>("key")}))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Authorize an env-var mutation: require an owner session, confirm the app
/// belongs to the caller, and validate the env key. On success the owning
/// `user_id` is returned; otherwise the wire-compatible error response is.
async fn authorize_env_mutation(
    state: &AppState,
    headers: &HeaderMap,
    app_id: Uuid,
    key: &str,
) -> Result<Uuid, Response> {
    let context = request_context_or_response(headers, state).await?;
    let user_id = context.user_id;
    if !app_belongs_to_user(state, app_id, user_id).await {
        return Err(StatusCode::NOT_FOUND.into_response());
    }
    if !valid_env_key(key) {
        return Err((StatusCode::BAD_REQUEST, "invalid env var key").into_response());
    }
    Ok(user_id)
}

/// Record an `app_env_var_*` audit event for an owner-initiated env mutation.
async fn record_env_audit(
    state: &AppState,
    user_id: Uuid,
    app_id: Uuid,
    event_type: &'static str,
    key: &str,
) {
    record_audit_event(
        state,
        AuditEventInput {
            actor_type: "owner",
            actor_id: Some(user_id.to_string()),
            event_type,
            app_id: Some(app_id),
            deployment_id: None,
            job_id: None,
            metadata: serde_json::json!({ "key": key }),
        },
    )
    .await;
}

pub async fn set_app_env_var(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, key)): Path<(Uuid, String)>,
    Json(body): Json<EnvValue>,
) -> impl IntoResponse {
    let user_id = match authorize_env_mutation(&state, &headers, id, &key).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    if body.value.len() > MAX_ENV_VALUE_LEN {
        return (StatusCode::BAD_REQUEST, "env var value is too large").into_response();
    }
    let Ok(enc) = state.crypto.encrypt(&body.value) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let res = sqlx::query(
        "INSERT INTO app_env_vars (app_id,key,value_ciphertext)
         VALUES ($1,$2,$3)
         ON CONFLICT (app_id,key) DO UPDATE SET value_ciphertext=EXCLUDED.value_ciphertext, updated_at=now()",
    )
    .bind(id)
    .bind(&key)
    .bind(enc)
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => {
            record_env_audit(&state, user_id, id, "app_env_var_changed", &key).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn delete_app_env_var(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, key)): Path<(Uuid, String)>,
) -> impl IntoResponse {
    let user_id = match authorize_env_mutation(&state, &headers, id, &key).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let res = sqlx::query("DELETE FROM app_env_vars WHERE app_id=$1 AND key=$2")
        .bind(id)
        .bind(&key)
        .execute(&state.db)
        .await;
    match res {
        Ok(_) => {
            record_env_audit(&state, user_id, id, "app_env_var_deleted", &key).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
