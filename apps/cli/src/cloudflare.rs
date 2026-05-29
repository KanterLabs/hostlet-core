use super::*;

pub(crate) async fn configure_cloudflare(
    theme: &ColorfulTheme,
    env: &mut BTreeMap<String, String>,
) -> anyhow::Result<()> {
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

    env.insert("PUBLIC_WEB_URL".into(), format!("https://{hostlet_host}"));
    env.insert("PUBLIC_API_URL".into(), format!("https://{hostlet_host}"));
    env.insert(
        "PUBLIC_WEBHOOK_URL".into(),
        format!("https://{hostlet_host}"),
    );
    env.insert("HOSTLET_CONTROL_PLANE_HOST".into(), hostlet_host.clone());
    env.insert(
        "HOSTLET_ALLOWED_WEB_ORIGINS".into(),
        format!("https://{hostlet_host}"),
    );
    env.insert("HOSTLET_BASE_DOMAIN".into(), domain);
    env.insert("HOSTLET_DOMAIN_PREFIX".into(), app_prefix);
    env.insert("CLOUDFLARE_API_TOKEN".into(), token);
    env.insert("CLOUDFLARE_ZONE_ID".into(), zone_id);
    env.insert("CLOUDFLARE_TUNNEL_TARGET".into(), tunnel_target);
    env.insert("CLOUDFLARE_TUNNEL_TOKEN".into(), tunnel_token);
    if Confirm::with_theme(theme)
        .with_prompt(format!("Create/update DNS record for {hostlet_host}?"))
        .default(true)
        .interact()?
    {
        upsert_cloudflare_cname(
            &client,
            env.get("CLOUDFLARE_API_TOKEN").expect("token inserted"),
            env.get("CLOUDFLARE_ZONE_ID").expect("zone inserted"),
            &hostlet_host,
            env.get("CLOUDFLARE_TUNNEL_TARGET")
                .expect("tunnel target inserted"),
        )
        .await?;
        println!("Cloudflare DNS ready for {hostlet_host}");
    }
    Ok(())
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
    Ok(value
        .get("result")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .and_then(|zone| zone.get("id"))
        .and_then(|id| id.as_str())
        .map(str::to_string))
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
    let record_id = existing
        .get("result")
        .and_then(|v| v.as_array())
        .and_then(|items| items.first())
        .and_then(|item| item.get("id"))
        .and_then(|id| id.as_str());
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
            !tunnel
                .get("deleted_at")
                .map(|deleted_at| !deleted_at.is_null())
                .unwrap_or(false)
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

