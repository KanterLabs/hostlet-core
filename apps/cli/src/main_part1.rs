use anyhow::{bail, Context};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use clap::{Parser, Subcommand};
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Password, Select};
use rand::RngCore;
use serde_json::Value;
use sha2::{Digest, Sha256};
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

const HOSTLET_REPO: &str = "ShaneKanterman04/Hostlet";
const LINUX_X64_ASSET: &str = "hostlet-linux-x64";

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
        #[arg(long)]
        scheduled: bool,
        output: Option<PathBuf>,
    },
    Restore {
        backup_dir: PathBuf,
    },
    Version,
    Status,
    Cleanup {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
    },
    Update {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        no_backup: bool,
        #[command(subcommand)]
        command: Option<UpdateCommand>,
    },
}

#[derive(Subcommand)]
enum UpdateCommand {
    Check,
    Rollback,
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
        Commands::Backup { scheduled, output } => backup(&root, output, scheduled),
        Commands::Restore { backup_dir } => restore(&root, &backup_dir),
        Commands::Version => version(),
        Commands::Status => status(&root).await,
        Commands::Cleanup { dry_run, yes } => cleanup(&root, dry_run, yes).await,
        Commands::Update {
            dry_run,
            yes,
            no_backup,
            command,
        } => match command {
            Some(UpdateCommand::Check) => update_check().await,
            Some(UpdateCommand::Rollback) => update_rollback(&root),
            None => update(&root, dry_run, yes, no_backup).await,
        },
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
    println!("Hostlet {}", env!("CARGO_PKG_VERSION"));
    check("Docker", command_ok("docker", &["version"]));
    check(
        "Docker Compose",
        command_ok("docker", &["compose", "version"]),
    );
    check(
        "Compose config",
        compose_config_ok(root, false) || compose_config_ok(root, true),
    );
    check(
        "Compose services",
        compose_services_running(root, false) || compose_services_running(root, true),
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
    print_operator_status(&client, &env).await;
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
    check("Disk space", disk_space_ok(root));
    check("Recent backup", latest_backup(root).is_some());
    match latest_release(&client).await {
        Ok(release) => check(
            "Hostlet update",
            !version_is_newer(
                env!("CARGO_PKG_VERSION"),
                release.version.trim_start_matches('v'),
            ),
        ),
        Err(_) => println!("Hostlet update             unknown"),
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
    if dev {
        args.extend(["up".into(), "-d".into(), "--build".into()]);
        return run_passthrough(root, "docker", &args);
    }

    let mut pull_args = args.clone();
    pull_args.push("pull".into());
    run_passthrough(root, "docker", &pull_args)?;

    args.extend(["up".into(), "-d".into(), "--no-build".into()]);
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

fn backup(root: &Path, output: Option<PathBuf>, scheduled: bool) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    let mut args = vec![root.join("scripts/backup.sh").display().to_string()];
    if let Some(output) = output {
        args.push(output.display().to_string());
    }
    let mut command = Command::new("bash");
    command.current_dir(root).args(&args);
    if scheduled {
        command.env("HOSTLET_BACKUP_SCHEDULED", "true");
    }
    let status = command.status()?;
    if !status.success() {
        bail!("backup failed with {status}");
    }
    record_latest_backup_metadata(root).ok();
    Ok(())
}

fn record_latest_backup_metadata(root: &Path) -> anyhow::Result<()> {
    let metadata_path = root.join("backups/latest.json");
    if !metadata_path.is_file() {
        return Ok(());
    }
    let metadata = fs::read_to_string(metadata_path)?;
    let sql = format!(
        "INSERT INTO settings (key,value,updated_at) VALUES ('latest_backup_metadata','{}',now()) ON CONFLICT (key) DO UPDATE SET value=EXCLUDED.value, updated_at=now();",
        metadata.replace('\'', "''")
    );
    let env = read_env_file(&root.join(".env")).unwrap_or_default();
    let user = env
        .get("POSTGRES_USER")
        .cloned()
        .unwrap_or_else(|| "hostlet".into());
    let db = env
        .get("POSTGRES_DB")
        .cloned()
        .unwrap_or_else(|| "hostlet".into());
    let mut args = compose_args(false);
    args.extend([
        "exec".into(),
        "-T".into(),
        "postgres".into(),
        "psql".into(),
        "-U".into(),
        user,
        "-d".into(),
        db,
        "-c".into(),
        sql,
    ]);
    let _ = Command::new("docker").current_dir(root).args(args).status();
    Ok(())
}

fn restore(root: &Path, backup_dir: &Path) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    restore_preflight(root, backup_dir)?;
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

fn restore_preflight(root: &Path, backup_dir: &Path) -> anyhow::Result<()> {
    if !backup_dir.is_dir() {
        bail!("backup directory does not exist: {}", backup_dir.display());
    }
    if !backup_dir.join("postgres.sql").is_file() {
        bail!("backup is missing postgres.sql");
    }
    if !root.join(".env").is_file() {
        bail!("restore requires .env with the original Hostlet secrets");
    }
    if !command_ok("docker", &["version"]) {
        bail!("Docker is not available");
    }
    if !command_ok("docker", &["compose", "version"]) {
        bail!("Docker Compose is not available");
    }
    if !disk_space_ok(root) {
        bail!("less than 1 GiB free; refusing restore");
    }
    Ok(())
}

fn version() -> anyhow::Result<()> {
    println!("hostlet {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}

async fn status(root: &Path) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    let env = read_env_file(&root.join(".env")).unwrap_or_default();
    println!("Hostlet {}", env!("CARGO_PKG_VERSION"));
    check("Docker", command_ok("docker", &["version"]));
    check(
        "Docker Compose",
        command_ok("docker", &["compose", "version"]),
    );
    check(
        "Compose services",
        compose_services_running(root, false) || compose_services_running(root, true),
    );
    if let Some(backup) = latest_backup(root) {
        println!("Latest backup              {}", backup.display());
        if let Some(age) = latest_backup_age(root) {
            println!("Latest backup age          {age}");
        }
    } else {
        println!("Latest backup              none");
    }
    let client = http_client()?;
    check_url(
        &client,
        "API health",
        env.get("PUBLIC_API_URL")
            .map(|value| format!("{}/health", value.trim_end_matches('/')))
            .as_ref(),
    )
    .await;
    print_operator_status(&client, &env).await;
    match latest_release(&client).await {
        Ok(release) => {
            let latest = release.version.trim_start_matches('v');
            let current = env!("CARGO_PKG_VERSION");
            check("Update available", version_is_newer(current, latest));
        }
        Err(_) => println!("Update available          unknown"),
    }
    Ok(())
}

