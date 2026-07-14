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
    token: String,
    zone_id: String,
    account_id: String,
    tunnel_id: String,
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
        env.insert(
            "HOSTLET_ACCESS_MODE".into(),
            AccessMode::CloudflareTunnel.as_env().into(),
        );
        env.insert("HOSTLET_CADDYFILE".into(), "./Caddyfile.tunnel".into());
        env.insert("CLOUDFLARE_API_TOKEN".into(), self.token.clone());
        env.insert("CLOUDFLARE_ZONE_ID".into(), self.zone_id.clone());
        env.insert("CLOUDFLARE_ACCOUNT_ID".into(), self.account_id.clone());
        env.insert("CLOUDFLARE_TUNNEL_ID".into(), self.tunnel_id.clone());
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
    let client = http_client()?;
    upsert_cloudflare_cname(
        &client,
        &config.token,
        &config.zone_id,
        &config.hostlet_host,
        &config.tunnel_target,
    )
    .await?;
    upsert_cloudflare_cname(
        &client,
        &config.token,
        &config.zone_id,
        &format!("*.{}", config.domain),
        &config.tunnel_target,
    )
    .await?;
    println!(
        "Cloudflare DNS ready for {} and *.{}",
        config.hostlet_host, config.domain
    );
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
    if !valid_hostname(&domain)
        || !valid_hostname(&hostlet_host)
        || !hostlet_host.ends_with(&format!(".{domain}"))
    {
        bail!("Cloudflare zone and Hostlet hostname must be valid DNS names in the same zone");
    }
    let token = Password::with_theme(theme)
        .with_prompt("Cloudflare API token")
        .allow_empty_password(false)
        .interact()?;
    let client = http_client()?;
    let zone = lookup_cloudflare_zone(&client, &token, &domain)
        .await?
        .context("Cloudflare token cannot access that zone")?;
    let name: String = Input::with_theme(theme)
        .with_prompt("Dedicated Hostlet tunnel name")
        .default(format!("hostlet-{}", domain.replace('.', "-")))
        .interact_text()?;
    let tunnel_id = create_cloudflare_tunnel(&client, &token, &zone.account_id, &name).await?;
    configure_cloudflare_tunnel(
        &client,
        &token,
        &zone.account_id,
        &tunnel_id,
        &hostlet_host,
        &domain,
    )
    .await?;
    let tunnel_token =
        cloudflare_tunnel_token(&client, &token, &zone.account_id, &tunnel_id).await?;
    let tunnel_target = format!("{tunnel_id}.cfargotunnel.com");

    Ok(CloudflareConfig {
        domain,
        hostlet_host,
        token,
        zone_id: zone.id,
        account_id: zone.account_id,
        tunnel_id,
        tunnel_target,
        tunnel_token,
    })
}

fn valid_hostname(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
}

pub(crate) struct CloudflareZone {
    id: String,
    account_id: String,
}

pub(crate) async fn lookup_cloudflare_zone(
    client: &reqwest::Client,
    token: &str,
    domain: &str,
) -> anyhow::Result<Option<CloudflareZone>> {
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
        .and_then(Value::as_array)
        .and_then(|v| v.first())
        .and_then(|zone| {
            Some(CloudflareZone {
                id: zone.get("id")?.as_str()?.to_string(),
                account_id: zone.get("account")?.get("id")?.as_str()?.to_string(),
            })
        }))
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

pub(crate) async fn configure_cloudflare_tunnel(
    client: &reqwest::Client,
    token: &str,
    account_id: &str,
    tunnel_id: &str,
    hostlet_host: &str,
    domain: &str,
) -> anyhow::Result<()> {
    client
        .put(format!("https://api.cloudflare.com/client/v4/accounts/{account_id}/cfd_tunnel/{tunnel_id}/configurations"))
        .bearer_auth(token)
        .json(&serde_json::json!({"config": {"ingress": [
            {"hostname": hostlet_host, "service": "http://127.0.0.1:18080"},
            {"hostname": format!("*.{domain}"), "service": "http://127.0.0.1:18080"},
            {"service": "http_status:404"}
        ]}}))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn populate_env_derives_public_urls_from_host() {
        let config = CloudflareConfig {
            domain: "example.test".into(),
            hostlet_host: "hostlet.example.test".into(),
            token: "cf-token".into(),
            zone_id: "zone-1".into(),
            account_id: "account-1".into(),
            tunnel_id: "abc".into(),
            tunnel_target: "abc.cfargotunnel.com".into(),
            tunnel_token: "tunnel-token".into(),
        };
        let mut env = BTreeMap::new();

        config.populate_env(&mut env);

        for key in ["PUBLIC_WEB_URL", "PUBLIC_API_URL", "PUBLIC_WEBHOOK_URL"] {
            assert_eq!(
                env.get(key).map(String::as_str),
                Some("https://hostlet.example.test"),
                "{key} should be the https host URL"
            );
        }
        assert_eq!(
            env.get("HOSTLET_ALLOWED_WEB_ORIGINS").map(String::as_str),
            Some("https://hostlet.example.test")
        );
        assert_eq!(
            env.get("HOSTLET_CONTROL_PLANE_HOST").map(String::as_str),
            Some("hostlet.example.test")
        );
        assert_eq!(
            env.get("HOSTLET_BASE_DOMAIN").map(String::as_str),
            Some("example.test")
        );
        assert_eq!(
            env.get("HOSTLET_ACCESS_MODE").map(String::as_str),
            Some("cloudflare_tunnel")
        );
        assert_eq!(
            env.get("CLOUDFLARE_ACCOUNT_ID").map(String::as_str),
            Some("account-1")
        );
        assert_eq!(
            env.get("CLOUDFLARE_TUNNEL_ID").map(String::as_str),
            Some("abc")
        );
        assert_eq!(
            env.get("CLOUDFLARE_API_TOKEN").map(String::as_str),
            Some("cf-token")
        );
        assert_eq!(
            env.get("CLOUDFLARE_ZONE_ID").map(String::as_str),
            Some("zone-1")
        );
        assert_eq!(
            env.get("CLOUDFLARE_TUNNEL_TARGET").map(String::as_str),
            Some("abc.cfargotunnel.com")
        );
        assert_eq!(
            env.get("CLOUDFLARE_TUNNEL_TOKEN").map(String::as_str),
            Some("tunnel-token")
        );
    }

    #[test]
    fn result_first_str_reads_field_off_first_element() {
        let value = serde_json::json!({
            "result": [ { "id": "first" }, { "id": "second" } ]
        });
        assert_eq!(result_first_str(&value, "id"), Some("first"));
    }

    #[test]
    fn result_first_str_handles_missing_empty_and_absent_field() {
        assert_eq!(result_first_str(&serde_json::json!({}), "id"), None);
        assert_eq!(
            result_first_str(&serde_json::json!({ "result": [] }), "id"),
            None
        );
        assert_eq!(
            result_first_str(&serde_json::json!({ "result": [ {} ] }), "id"),
            None
        );
    }
}
