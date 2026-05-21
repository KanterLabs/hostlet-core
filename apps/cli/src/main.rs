use anyhow::{bail, Context};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use clap::{Parser, Subcommand};
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Password, Select};
use rand::RngCore;
use serde_json::Value;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

#[derive(Parser)]
#[command(name = "hostlet", version, about = "Hostlet setup and operations CLI")]
struct Cli {
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Init {
        #[arg(long)]
        force: bool,
    },
    Doctor,
    Up {
        #[arg(long)]
        tunnel: bool,
        #[arg(long)]
        dev: bool,
    },
    Down {
        #[arg(long)]
        dev: bool,
    },
    Logs {
        services: Vec<String>,
        #[arg(long)]
        dev: bool,
    },
    Backup {
        output: Option<PathBuf>,
    },
    Restore {
        backup_dir: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let root = cli.root.canonicalize().unwrap_or(cli.root);
    match cli.command {
        Commands::Init { force } => init(&root, force).await,
        Commands::Doctor => doctor(&root).await,
        Commands::Up { tunnel, dev } => compose_up(&root, tunnel, dev),
        Commands::Down { dev } => compose_down(&root, dev),
        Commands::Logs { services, dev } => compose_logs(&root, dev, &services),
        Commands::Backup { output } => backup(&root, output),
        Commands::Restore { backup_dir } => restore(&root, &backup_dir),
    }
}

async fn init(root: &Path, force: bool) -> anyhow::Result<()> {
    require_interactive()?;
    ensure_repo_root(root)?;
    let env_path = root.join(".env");
    if env_path.exists() && !force {
        bail!(".env already exists. Run hostlet init --force to replace it after reviewing your backup.");
    }

    let theme = ColorfulTheme::default();
    println!("Hostlet init writes .env and generates local secrets.");

    let access_mode = Select::with_theme(&theme)
        .with_prompt("Hostlet UI/API access mode")
        .items(&["LAN only", "Cloudflare Tunnel for Hostlet UI/API"])
        .default(0)
        .interact()?;

    let allowed_login: String = Input::with_theme(&theme)
        .with_prompt("Allowed GitHub username")
        .interact_text()?;
    let github_client_id: String = Input::with_theme(&theme)
        .with_prompt("GitHub OAuth App Client ID (Device Flow enabled)")
        .allow_empty(false)
        .interact_text()?;

    let mut env = default_env();
    env.insert("HOSTLET_ALLOWED_GITHUB_LOGINS".into(), allowed_login);
    env.insert("GITHUB_CLIENT_ID".into(), github_client_id);

    if access_mode == 0 {
        let host: String = Input::with_theme(&theme)
            .with_prompt("Hostlet LAN host/IP")
            .default("localhost".into())
            .interact_text()?;
        let public_web_url = format!("http://{host}:3000");
        let public_api_url = format!("http://{host}:8080");
        env.insert("PUBLIC_WEB_URL".into(), public_web_url.clone());
        env.insert("PUBLIC_API_URL".into(), public_api_url);
        env.insert("HOSTLET_CONTROL_PLANE_HOST".into(), host);
        env.insert(
            "HOSTLET_ALLOWED_WEB_ORIGINS".into(),
            format!("{public_web_url},http://localhost:3000,http://127.0.0.1:3000"),
        );
    } else {
        configure_cloudflare(&theme, &mut env).await?;
    }

    if env_path.exists() {
        let backup = root.join(format!(".env.backup.{}", timestamp_suffix()));
        fs::copy(&env_path, &backup)
            .with_context(|| format!("failed to backup {}", env_path.display()))?;
        set_secret_file_permissions(&backup)?;
        println!("Existing .env backed up to {}", backup.display());
    }

    write_env_file(&env_path, &env)?;
    println!("Wrote {}", env_path.display());
    println!("Open Hostlet after start: {}", env["PUBLIC_WEB_URL"]);
    println!("First setup token: {}", env["HOSTLET_SETUP_TOKEN"]);
    println!(
        "Next: hostlet up{}",
        if access_mode == 1 { " --tunnel" } else { "" }
    );
    Ok(())
}

async fn configure_cloudflare(
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

async fn lookup_cloudflare_zone(
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

async fn upsert_cloudflare_cname(
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

async fn select_or_create_tunnel(
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
struct CloudflareTunnel {
    id: String,
    name: String,
}

async fn list_cloudflare_tunnels(
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

async fn create_cloudflare_tunnel(
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

async fn cloudflare_tunnel_token(
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

async fn doctor(root: &Path) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    let env_path = root.join(".env");
    let env = read_env_file(&env_path).unwrap_or_default();
    let client = http_client()?;
    check("Docker", command_ok("docker", &["version"]));
    check(
        "Docker Compose",
        command_ok("docker", &["compose", "version"]),
    );
    check(".env exists", env_path.exists());
    for key in [
        "PUBLIC_WEB_URL",
        "PUBLIC_API_URL",
        "ENCRYPTION_KEY",
        "SESSION_SECRET",
        "JOB_SIGNING_SECRET",
        "LOCAL_AGENT_TOKEN",
        "HOSTLET_SETUP_TOKEN",
        "HOSTLET_ALLOWED_GITHUB_LOGINS",
        "GITHUB_CLIENT_ID",
    ] {
        check(
            &format!("{key} set"),
            env.get(key).is_some_and(|v| !v.trim().is_empty()),
        );
    }
    check_url(&client, "Web", env.get("PUBLIC_WEB_URL")).await;
    check_url(
        &client,
        "API health",
        env.get("PUBLIC_API_URL")
            .map(|v| format!("{}/health", v.trim_end_matches('/')))
            .as_ref(),
    )
    .await;
    if let (Some(token), Some(zone_id)) = (
        env.get("CLOUDFLARE_API_TOKEN"),
        env.get("CLOUDFLARE_ZONE_ID"),
    ) {
        check(
            "Cloudflare zone access",
            cloudflare_zone_ok(&client, token, zone_id)
                .await
                .unwrap_or(false),
        );
    }
    Ok(())
}

async fn check_url(client: &reqwest::Client, label: &str, url: Option<&String>) {
    let Some(url) = url else {
        check(label, false);
        return;
    };
    let ok = client
        .get(url)
        .send()
        .await
        .map(|r| r.status().is_success() || r.status().is_redirection())
        .unwrap_or(false);
    check(label, ok);
}

async fn cloudflare_zone_ok(
    client: &reqwest::Client,
    token: &str,
    zone_id: &str,
) -> anyhow::Result<bool> {
    Ok(client
        .get(format!(
            "https://api.cloudflare.com/client/v4/zones/{zone_id}"
        ))
        .bearer_auth(token)
        .send()
        .await?
        .status()
        .is_success())
}

fn http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent("Hostlet CLI")
        .build()
        .context("failed to build HTTP client")
}

fn compose_up(root: &Path, tunnel: bool, dev: bool) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    let mut args = compose_args(dev);
    if tunnel && !dev {
        args.extend(["--profile".into(), "tunnel".into()]);
    }
    args.extend(["up".into(), "-d".into(), "--build".into()]);
    run_passthrough(root, "docker", &args)
}

fn compose_down(root: &Path, dev: bool) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    let mut args = compose_args(dev);
    args.push("down".into());
    run_passthrough(root, "docker", &args)
}

fn compose_logs(root: &Path, dev: bool, services: &[String]) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    let mut args = compose_args(dev);
    args.extend(["logs".into(), "-f".into()]);
    args.extend(services.iter().cloned());
    run_passthrough(root, "docker", &args)
}

fn backup(root: &Path, output: Option<PathBuf>) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    let mut args = vec![root.join("scripts/backup.sh").display().to_string()];
    if let Some(output) = output {
        args.push(output.display().to_string());
    }
    run_passthrough(root, "bash", &args)
}

fn restore(root: &Path, backup_dir: &Path) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    if !Confirm::new()
        .with_prompt("Restore replaces the current Hostlet database. Continue?")
        .default(false)
        .interact()?
    {
        bail!("restore canceled");
    }
    let script = root.join("scripts/restore.sh");
    let status = Command::new("bash")
        .current_dir(root)
        .env("HOSTLET_RESTORE_CONFIRM", "yes")
        .arg(script)
        .arg(backup_dir)
        .status()?;
    if !status.success() {
        bail!("restore failed with {status}");
    }
    Ok(())
}

fn compose_args(dev: bool) -> Vec<String> {
    vec![
        "compose".into(),
        "-f".into(),
        if dev {
            "infra/docker-compose.yml".into()
        } else {
            "infra/docker-compose.prod.yml".into()
        },
    ]
}

fn run_passthrough(root: &Path, bin: &str, args: &[String]) -> anyhow::Result<()> {
    let mut command = Command::new(bin);
    command
        .current_dir(root)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if std::env::var_os("DOCKER_GID").is_none() {
        command.env("DOCKER_GID", docker_gid());
    }
    let status = command
        .status()
        .with_context(|| format!("failed to run {bin}"))?;
    if !status.success() {
        bail!("{bin} failed with {status}");
    }
    Ok(())
}

fn command_ok(bin: &str, args: &[&str]) -> bool {
    Command::new(bin)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn check(label: &str, ok: bool) {
    println!(
        "{:<28} {}",
        label,
        if ok { "ok" } else { "needs attention" }
    );
}

fn default_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    let postgres_password = hex_secret(24);
    env.insert("POSTGRES_USER".into(), "hostlet".into());
    env.insert("POSTGRES_PASSWORD".into(), postgres_password.clone());
    env.insert("POSTGRES_DB".into(), "hostlet".into());
    env.insert("DOCKER_GID".into(), docker_gid());
    env.insert(
        "DATABASE_URL".into(),
        format!("postgres://hostlet:{postgres_password}@localhost:5432/hostlet"),
    );
    env.insert("BIND_ADDR".into(), "0.0.0.0:8080".into());
    env.insert("HOSTLET_BASE_DOMAIN".into(), String::new());
    env.insert("HOSTLET_DOMAIN_PREFIX".into(), "hostlet-".into());
    env.insert("HOSTLET_CONTROL_PLANE_HOST".into(), "localhost".into());
    env.insert("CLOUDFLARE_API_TOKEN".into(), String::new());
    env.insert("CLOUDFLARE_ZONE_ID".into(), String::new());
    env.insert("CLOUDFLARE_TUNNEL_TARGET".into(), String::new());
    env.insert("CLOUDFLARE_TUNNEL_TOKEN".into(), String::new());
    env.insert("HOSTLET_ALLOW_INSECURE_DEV_DEFAULTS".into(), "false".into());
    env.insert("HOSTLET_SETUP_TOKEN".into(), hex_secret(32));
    env.insert("ENCRYPTION_KEY".into(), base64_secret(32));
    env.insert("JOB_SIGNING_SECRET".into(), hex_secret(32));
    env.insert("SESSION_SECRET".into(), hex_secret(32));
    env.insert(
        "LOCAL_SERVER_ID".into(),
        "00000000-0000-0000-0000-000000000001".into(),
    );
    env.insert("LOCAL_AGENT_TOKEN".into(), hex_secret(32));
    env.insert("GITHUB_WEBHOOK_SECRET".into(), hex_secret(32));
    env
}

fn write_env_file(path: &Path, env: &BTreeMap<String, String>) -> anyhow::Result<()> {
    let mut out = String::new();
    for (key, value) in env {
        out.push_str(key);
        out.push('=');
        out.push_str(&quote_env(value));
        out.push('\n');
    }
    let mut file = secret_open_options()
        .open(path)
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.write_all(out.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    set_secret_file_permissions(path)?;
    Ok(())
}

fn secret_open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    options
}

fn set_secret_file_permissions(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to set secure permissions on {}", path.display()))?;
    Ok(())
}

fn read_env_file(path: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    for line in fs::read_to_string(path)?.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            env.insert(key.trim().to_string(), unquote_env(value.trim()));
        }
    }
    Ok(env)
}

fn quote_env(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | ':' | '.' | '_' | '-' | ',' | '='))
    {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

fn unquote_env(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
        .replace("\\\"", "\"")
        .replace("\\\\", "\\")
}

fn base64_secret(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    STANDARD.encode(buf)
}

fn hex_secret(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    buf.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn docker_gid() -> String {
    #[cfg(unix)]
    {
        fs::metadata("/var/run/docker.sock")
            .map(|metadata| metadata.gid().to_string())
            .unwrap_or_else(|_| "998".into())
    }
    #[cfg(not(unix))]
    {
        "998".into()
    }
}

fn timestamp_suffix() -> String {
    chrono_like_timestamp()
}

fn chrono_like_timestamp() -> String {
    Command::new("date")
        .arg("-u")
        .arg("+%Y%m%dT%H%M%SZ")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "backup".into())
}

fn ensure_repo_root(root: &Path) -> anyhow::Result<()> {
    if !root.join("infra").join("docker-compose.yml").exists() || !root.join("Cargo.toml").exists()
    {
        bail!(
            "{} does not look like the Hostlet repository root",
            root.display()
        );
    }
    Ok(())
}

fn require_interactive() -> anyhow::Result<()> {
    if !io::stdin().is_terminal() {
        bail!("hostlet init requires an interactive terminal");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_quote_round_trips_spaces() {
        let value = "secret with spaces";
        assert_eq!(unquote_env(&quote_env(value)), value);
    }

    #[test]
    fn generated_encryption_key_is_base64_32_bytes() {
        let secret = base64_secret(32);
        assert_eq!(STANDARD.decode(secret).unwrap().len(), 32);
    }
}
