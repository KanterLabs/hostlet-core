use super::super::*;

/// Builds the shared Cloudflare-status JSON envelope, varying only the
/// `configured`/`tokenValid`/`message` fields between the distinct outcomes.
fn cloudflare_status_envelope(
    state: &AppState,
    configured: bool,
    token_valid: serde_json::Value,
    message: impl Into<serde_json::Value>,
) -> Response {
    Json(serde_json::json!({
        "configured": configured,
        "tokenValid": token_valid,
        "baseDomain": state.base_domain.as_deref(),
        "domainPrefix": state.domain_prefix,
        "defaultDomainPattern": default_domain_pattern(state),
        "tunnelTargetConfigured": state.cloudflare_tunnel_target.is_some(),
        "message": message.into(),
    }))
    .into_response()
}

pub async fn cloudflare_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(_user_id) = current_user_id(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let configured = state.cloudflare_api_token.is_some()
        && state.cloudflare_zone_id.is_some()
        && state.cloudflare_tunnel_target.is_some()
        && state.base_domain.is_some();
    let Some(token) = state.cloudflare_api_token.as_ref() else {
        return cloudflare_status_envelope(
            &state,
            false,
            serde_json::Value::Null,
            "CLOUDFLARE_API_TOKEN is not set.",
        );
    };
    let Some(zone_id) = state.cloudflare_zone_id.as_ref() else {
        return cloudflare_status_envelope(
            &state,
            false,
            serde_json::Value::Null,
            "CLOUDFLARE_ZONE_ID is not set.",
        );
    };
    let resp = state
        .http
        .get(format!(
            "https://api.cloudflare.com/client/v4/zones/{zone_id}"
        ))
        .bearer_auth(token)
        .send()
        .await;
    match resp {
        Ok(resp) if resp.status().is_success() => cloudflare_status_envelope(
            &state,
            configured,
            serde_json::Value::Bool(true),
            "Cloudflare API token can access the configured zone.",
        ),
        Ok(resp) => cloudflare_status_envelope(
            &state,
            configured,
            serde_json::Value::Bool(false),
            format!(
                "Cloudflare zone check failed with status {}.",
                resp.status()
            ),
        ),
        Err(_) => cloudflare_status_envelope(
            &state,
            configured,
            serde_json::Value::Bool(false),
            "Could not reach Cloudflare from the API container.",
        ),
    }
}

/// Issues a Cloudflare DELETE for a single DNS record, treating an already-gone
/// record (404) as success so cleanup is idempotent.
async fn cloudflare_delete_record(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    record_id: &str,
) -> anyhow::Result<()> {
    let resp = client
        .delete(format!("{base}/{record_id}"))
        .bearer_auth(token)
        .send()
        .await?;
    if !resp.status().is_success() && resp.status() != StatusCode::NOT_FOUND {
        resp.error_for_status()?;
    }
    Ok(())
}

/// Lists the Cloudflare CNAME records currently registered for `host`.
async fn cloudflare_list_cname_records(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    host: &str,
) -> anyhow::Result<Vec<CloudflareRecord>> {
    let listed = client
        .get(base)
        .bearer_auth(token)
        .query(&[("type", "CNAME"), ("name", host)])
        .send()
        .await?
        .error_for_status()?
        .json::<CloudflareListResponse>()
        .await?;
    Ok(listed.result)
}

pub(in crate::web) async fn ensure_cloudflare_app_dns(
    state: &AppState,
    app_id: Uuid,
    domain: &str,
) -> anyhow::Result<()> {
    let host = hostlet_public_cloudflare_host(state, domain)?;
    let (Some(token), Some(zone_id), Some(target)) = (
        &state.cloudflare_api_token,
        &state.cloudflare_zone_id,
        &state.cloudflare_tunnel_target,
    ) else {
        anyhow::bail!("Cloudflare DNS is not configured");
    };

    let client = &state.http;
    let base = format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records");
    let existing = cloudflare_list_cname_records(client, &base, token, &host).await?;

    let owned = sqlx::query(
        "SELECT app_id, cloudflare_record_id
         FROM app_public_dns_records
         WHERE zone_id=$1 AND hostname=$2",
    )
    .bind(zone_id)
    .bind(&host)
    .fetch_optional(&state.db)
    .await?;

    let payload = CloudflareDnsRecord {
        record_type: "CNAME",
        name: &host,
        content: target,
        proxied: true,
    };

    if let Some(owner) = owned.as_ref() {
        let owner_app_id = owner.get::<Uuid, _>("app_id");
        if owner_app_id != app_id {
            anyhow::bail!("{host} is already managed by another Hostlet app");
        }
    }

    let record_id = if let Some(record) = existing.first() {
        if owned.is_none() {
            anyhow::bail!(
                "{host} already has a Cloudflare CNAME record not managed by this Hostlet app"
            );
        }
        client
            .patch(format!("{base}/{}", record.id))
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;
        record.id.clone()
    } else {
        client
            .post(&base)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await?
            .error_for_status()?
            .json::<CloudflareMutationResponse>()
            .await?
            .result
            .id
    };

    sqlx::query(
        "INSERT INTO app_public_dns_records (app_id, zone_id, hostname, cloudflare_record_id, target)
         VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (zone_id, hostname)
         DO UPDATE SET app_id=$1, cloudflare_record_id=$4, target=$5, updated_at=now()",
    )
    .bind(app_id)
    .bind(zone_id)
    .bind(&host)
    .bind(record_id)
    .bind(target)
    .execute(&state.db)
    .await?;
    Ok(())
}

pub(in crate::web) async fn delete_cloudflare_app_dns(
    state: &AppState,
    app_id: Uuid,
    domain: &str,
) -> anyhow::Result<()> {
    let Ok(host) = hostlet_public_cloudflare_host(state, domain) else {
        return Ok(());
    };
    let (Some(token), Some(zone_id)) = (&state.cloudflare_api_token, &state.cloudflare_zone_id)
    else {
        anyhow::bail!("Cloudflare DNS is not configured");
    };

    let client = &state.http;
    let base = format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records");
    let owned = sqlx::query(
        "SELECT cloudflare_record_id
         FROM app_public_dns_records
         WHERE app_id=$1 AND zone_id=$2 AND hostname=$3",
    )
    .bind(app_id)
    .bind(zone_id)
    .bind(&host)
    .fetch_optional(&state.db)
    .await?;

    // Primary path: a record this app owns in our DB. Delete it from Cloudflare
    // (tolerating an already-removed record) and drop the ownership row.
    if let Some(record) = owned {
        let record_id = record.get::<String, _>("cloudflare_record_id");
        cloudflare_delete_record(client, &base, token, &record_id).await?;
        sqlx::query(
            "DELETE FROM app_public_dns_records WHERE app_id=$1 AND zone_id=$2 AND hostname=$3",
        )
        .bind(app_id)
        .bind(zone_id)
        .bind(&host)
        .execute(&state.db)
        .await?;
        return Ok(());
    }

    // No owned record. Leave Cloudflare untouched.
    Ok(())
}

#[derive(Deserialize)]
struct CloudflareListResponse {
    result: Vec<CloudflareRecord>,
}

#[derive(Deserialize)]
struct CloudflareRecord {
    id: String,
}

#[derive(Deserialize)]
struct CloudflareMutationResponse {
    result: CloudflareRecord,
}

#[derive(Serialize)]
struct CloudflareDnsRecord<'a> {
    #[serde(rename = "type")]
    record_type: &'a str,
    name: &'a str,
    content: &'a str,
    proxied: bool,
}
