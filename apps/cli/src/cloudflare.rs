use super::*;

/// Walks a `result` array on a Cloudflare API response and pulls a string
/// field off the first element, e.g. `result_first_str(&value, "id")`.
fn result_first_str<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value
        .get("result")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get(field))
        .and_then(Value::as_str)
}

/// Values resolved from the interactive Cloudflare prompts that are written
/// into the env map. Separating this from the prompting/IO keeps the
/// env-population step pure and easy to follow.
struct CloudflareConfig {
    domain: String,
    hostlet_host: String,
    app_prefix: String,
    token: String,
    zone_id: String,
    tunnel_target: String,
    tunnel_token: String,
}

impl CloudflareConfig {
    fn populate_env(&self, env: &mut BTreeMap<String, String>) {
        let web_url = format!("https://{}", self.hostlet_host);
        env.insert("PUBLIC_WEB_URL".into(), web_url.clone());
        env.insert("PUBLIC_API_URL".into(), web_url.clone());
        env.insert("PUBLIC_WEBHOOK_URL".into(), web_url.clone());
        env.insert(
            "HOSTLET_CONTROL_PLANE_HOST".into(),
            self.hostlet_host.clone(),
        );
        env.insert("HOSTLET_ALLOWED_WEB_ORIGINS".into(), web_url);
        env.insert("HOSTLET_BASE_DOMAIN".into(), self.domain.clone());
        env.insert("HOSTLET_DOMAIN_PREFIX".into(), self.app_prefix.clone());
        env.insert("CLOUDFLARE_API_TOKEN".into(), self.token.clone());
        env.insert("CLOUDFLARE_ZONE_ID".into(), self.zone_id.clone());
        env.insert(
            "CLOUDFLARE_TUNNEL_TARGET".into(),
            self.tunnel_target.clone(),
        );
        env.insert("CLOUDFLARE_TUNNEL_TOKEN".into(), self.tunnel_token.clone());
    }
}

pub(crate) async fn configure_cloudflare(
    theme: &ColorfulTheme,
    env: &mut BTreeMap<String, String>,
) -> anyhow::Result<()> {
    let config = prompt_cloudflare_config(theme).await?;
    config.populate_env(env);
    if Confirm::with_theme(theme)
        .with_prompt(format!(
            "Create/update DNS record for {}?",
            config.hostlet_host
        ))
        .default(true)
        .interact()?
    {
        upsert_cloudflare_cname(
            &http_client()?,
            &config.token,
            &config.zone_id,
            &config.hostlet_host,
            &config.tunnel_target,
        )
        .await?;
        println!("Cloudflare DNS ready for {}", config.hostlet_host);
    }
    Ok(())
}

async fn prompt_cloudflare_config(theme: &ColorfulTheme) -> anyhow::Result<CloudflareConfig> {
    let domain: String = Input::with_theme(theme)
        .with_prompt("Cloudflare zone/domain")
        .allow_empty(false)
        .interact_text()?;
    let hostlet_host: String = Input::with_theme(theme)
        .with_prompt("Hostlet UI/API hostname")
        .default(format!("hostlet.{domain}"))
        .interact_text()?;
    let app_prefix: String = Input::with_theme(theme)
        .with_prompt("Managed app hostname prefix")
        .default("hostlet-".into())
        .interact_text()?;
    let token = Password::with_theme(theme)
        .with_prompt("Cloudflare API token")
        .allow_empty_password(false)
        .interact()?;
    let client = http_client()?;
    let detected_zone = lookup_cloudflare_zone(&client, &token, &domain)
        .await
        .ok()
        .flatten();
    let zone_id: String = Input::with_theme(theme)
        .with_prompt("Cloudflare Zone ID")
        .default(detected_zone.unwrap_or_default())
        .allow_empty(false)
        .interact_text()?;
    let account_id: String = Input::with_theme(theme)
        .with_prompt("Cloudflare Account ID, for automatic tunnel setup")
        .allow_empty(true)
        .interact_text()?;
    let (tunnel_target, tunnel_token) = if account_id.trim().is_empty() {
        let tunnel_target: String = Input::with_theme(theme)
            .with_prompt("Cloudflare Tunnel target CNAME")
            .with_initial_text("<tunnel-id>.cfargotunnel.com")
            .interact_text()?;
        let tunnel_token = Password::with_theme(theme)
            .with_prompt("Cloudflare Tunnel token")
            .allow_empty_password(false)
            .interact()?;
        (tunnel_target, tunnel_token)
    } else {
        select_or_create_tunnel(&client, theme, &token, account_id.trim(), &domain).await?
    };

    Ok(CloudflareConfig {
        domain,
        hostlet_host,
        app_prefix,
        token,
        zone_id,
        tunnel_target,
        tunnel_token,
    })
}

pub(crate) async fn lookup_cloudflare_zone(
    client: &reqwest::Client,
    token: &str,
    domain: &str,
) -> anyhow::Result<Option<String>> {
    let value: Value = client
        .get("https://api.cloudflare.com/client/v4/zones")
        .bearer_auth(token)
        .query(&[("name", domain)])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(result_first_str(&value, "id").map(str::to_string))
}

pub(crate) async fn upsert_cloudflare_cname(
    client: &reqwest::Client,
    token: &str,
    zone_id: &str,
    host: &str,
    target: &str,
) -> anyhow::Result<()> {
    if host.trim().is_empty() || target.trim().is_empty() || !target.ends_with(".cfargotunnel.com")
    {
        bail!("Cloudflare tunnel target must look like <tunnel-id>.cfargotunnel.com");
    }
    let base = format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records");
    let existing: Value = client
        .get(&base)
        .bearer_auth(token)
        .query(&[("type", "CNAME"), ("name", host)])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let payload = serde_json::json!({
        "type": "CNAME",
        "name": host,
        "content": target,
        "proxied": true
    });
    let record_id = result_first_str(&existing, "id");
    let request = if let Some(record_id) = record_id {
        client.patch(format!("{base}/{record_id}"))
    } else {
        client.post(&base)
    };
    request
        .bearer_auth(token)
        .json(&payload)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

pub(crate) async fn select_or_create_tunnel(
    client: &reqwest::Client,
    theme: &ColorfulTheme,
    token: &str,
    account_id: &str,
    domain: &str,
) -> anyhow::Result<(String, String)> {
    let tunnels = list_cloudflare_tunnels(client, token, account_id)
        .await
        .unwrap_or_default();
    let create_label = "Create new Hostlet tunnel".to_string();
    let mut items = tunnels
        .iter()
        .map(|tunnel| format!("{} ({})", tunnel.name, tunnel.id))
        .collect::<Vec<_>>();
    items.push(create_label);
    let selected = Select::with_theme(theme)
        .with_prompt("Cloudflare Tunnel")
        .items(&items)
        .default(items.len().saturating_sub(1))
        .interact()?;
    let tunnel_id = if selected < tunnels.len() {
        tunnels[selected].id.clone()
    } else {
        let name: String = Input::with_theme(theme)
            .with_prompt("New tunnel name")
            .default(format!("hostlet-{}", domain.replace('.', "-")))
            .interact_text()?;
        create_cloudflare_tunnel(client, token, account_id, &name).await?
    };
    let tunnel_token = cloudflare_tunnel_token(client, token, account_id, &tunnel_id).await?;
    Ok((format!("{tunnel_id}.cfargotunnel.com"), tunnel_token))
}

#[derive(Clone)]
pub(crate) struct CloudflareTunnel {
    id: String,
    name: String,
}

pub(crate) async fn list_cloudflare_tunnels(
    client: &reqwest::Client,
    token: &str,
    account_id: &str,
) -> anyhow::Result<Vec<CloudflareTunnel>> {
    let value: Value = client
        .get(format!(
            "https://api.cloudflare.com/client/v4/accounts/{account_id}/cfd_tunnel"
        ))
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(value
        .get("result")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter(|tunnel| {
            // A tunnel is live when it has no non-null `deleted_at` timestamp.
            let deleted_at = tunnel.get("deleted_at");
            deleted_at.is_none() || deleted_at.is_some_and(Value::is_null)
        })
        .filter_map(|tunnel| {
            Some(CloudflareTunnel {
                id: tunnel.get("id")?.as_str()?.to_string(),
                name: tunnel
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unnamed")
                    .to_string(),
            })
        })
        .collect())
}

pub(crate) async fn create_cloudflare_tunnel(
    client: &reqwest::Client,
    token: &str,
    account_id: &str,
    name: &str,
) -> anyhow::Result<String> {
    let value: Value = client
        .post(format!(
            "https://api.cloudflare.com/client/v4/accounts/{account_id}/cfd_tunnel"
        ))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "name": name,
            "config_src": "cloudflare"
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    value
        .get("result")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .context("Cloudflare create tunnel response did not include a tunnel id")
}

pub(crate) async fn cloudflare_tunnel_token(
    client: &reqwest::Client,
    token: &str,
    account_id: &str,
    tunnel_id: &str,
) -> anyhow::Result<String> {
    let value: Value = client
        .get(format!(
            "https://api.cloudflare.com/client/v4/accounts/{account_id}/cfd_tunnel/{tunnel_id}/token"
        ))
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    value
        .get("result")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .context("Cloudflare tunnel token response did not include a token")
}
