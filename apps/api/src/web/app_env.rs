use super::*;

pub async fn app_env_vars(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
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

pub async fn set_app_env_var(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, key)): Path<(Uuid, String)>,
    Json(body): Json<EnvValue>,
) -> impl IntoResponse {
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(err) if err.to_string() == "sign in required" => {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(err) => return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response(),
    };
    let user_id = context.user_id;
    if !app_belongs_to_user(&state, id, user_id).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !valid_env_key(&key) {
        return (StatusCode::BAD_REQUEST, "invalid env var key").into_response();
    }
    if body.value.len() > 65_536 {
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
            record_audit_event(
                &state,
                AuditEventInput {
                    actor_type: "owner",
                    actor_id: Some(user_id.to_string()),
                    event_type: "app_env_var_changed",
                    app_id: Some(id),
                    deployment_id: None,
                    job_id: None,
                    metadata: serde_json::json!({"key": key}),
                },
            )
            .await;
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
    let context = match request_context(&headers, &state).await {
        Ok(context) => context,
        Err(err) if err.to_string() == "sign in required" => {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Err(err) => return (StatusCode::PAYMENT_REQUIRED, err.to_string()).into_response(),
    };
    let user_id = context.user_id;
    if !app_belongs_to_user(&state, id, user_id).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !valid_env_key(&key) {
        return (StatusCode::BAD_REQUEST, "invalid env var key").into_response();
    }
    let res = sqlx::query("DELETE FROM app_env_vars WHERE app_id=$1 AND key=$2")
        .bind(id)
        .bind(&key)
        .execute(&state.db)
        .await;
    match res {
        Ok(_) => {
            record_audit_event(
                &state,
                AuditEventInput {
                    actor_type: "owner",
                    actor_id: Some(user_id.to_string()),
                    event_type: "app_env_var_deleted",
                    app_id: Some(id),
                    deployment_id: None,
                    job_id: None,
                    metadata: serde_json::json!({"key": key}),
                },
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

