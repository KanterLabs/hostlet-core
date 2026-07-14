use super::*;

#[derive(Parser)]
#[command(name = "hostlet", version, about = "Hostlet setup and operations CLI")]
pub(crate) struct Cli {
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Verify that this host can run Hostlet without changing it.
    Preflight,
    /// Safely update an existing Hostlet installation.
    Configure,
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
pub(crate) enum UpdateCommand {
    Check,
    Rollback,
}

pub(crate) async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let root = cli.root.canonicalize().unwrap_or(cli.root);
    match cli.command {
        Commands::Init { force } => init(&root, force).await,
        Commands::Preflight => preflight(&root),
        Commands::Configure => configure(&root).await,
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

pub(crate) fn configure_lan_env(
    theme: &ColorfulTheme,
    env: &mut BTreeMap<String, String>,
) -> anyhow::Result<()> {
    let host: String = Input::with_theme(theme)
        .with_prompt("Hostlet LAN IP address")
        .default(
            env.get("HOSTLET_LAN_BIND_ADDR")
                .filter(|value| value.as_str() != "127.0.0.1")
                .cloned()
                .unwrap_or_else(|| "192.168.1.10".into()),
        )
        .interact_text()?;
    let _: std::net::Ipv4Addr = host
        .parse()
        .context("Hostlet LAN address must be an IPv4 address")?;
    let port: u16 = Input::with_theme(theme)
        .with_prompt("Hostlet LAN HTTP port")
        .default(
            env.get("HOSTLET_LAN_PORT")
                .and_then(|value| value.parse().ok())
                .unwrap_or(80),
        )
        .interact_text()?;
    let url_host = host.clone();
    let public_web_url = if port == 80 {
        format!("http://{url_host}")
    } else {
        format!("http://{url_host}:{port}")
    };
    env.insert(
        "HOSTLET_ACCESS_MODE".into(),
        AccessMode::Lan.as_env().into(),
    );
    env.insert("HOSTLET_CADDYFILE".into(), "./Caddyfile.lan".into());
    env.insert("HOSTLET_LAN_BIND_ADDR".into(), host.clone());
    env.insert("HOSTLET_LAN_PORT".into(), port.to_string());
    env.insert("PUBLIC_WEB_URL".into(), public_web_url.clone());
    env.insert("PUBLIC_API_URL".into(), public_web_url.clone());
    env.insert("PUBLIC_WEBHOOK_URL".into(), String::new());
    env.insert("HOSTLET_CONTROL_PLANE_HOST".into(), host);
    env.insert("HOSTLET_ALLOWED_WEB_ORIGINS".into(), public_web_url);
    Ok(())
}

pub(crate) async fn init(root: &Path, force: bool) -> anyhow::Result<()> {
    require_interactive()?;
    ensure_repo_root(root)?;
    preflight(root)?;
    let env_path = root.join(".env");
    if env_path.exists() {
        let _ = force;
        bail!(".env already exists. Run hostlet configure to preserve existing secrets and data.");
    }

    let theme = ColorfulTheme::default();
    println!("Hostlet init writes .env and generates local secrets.");

    let access_mode = match Select::with_theme(&theme)
        .with_prompt("Hostlet UI/API access mode")
        .items(&["LAN", "Cloudflare Tunnel for Hostlet UI/API"])
        .default(0)
        .interact()?
    {
        0 => AccessMode::Lan,
        _ => AccessMode::CloudflareTunnel,
    };

    let allowed_login: String = Input::with_theme(&theme)
        .with_prompt("Allowed GitHub username")
        .allow_empty(false)
        .interact_text()?;
    let github_client_id: String = Input::with_theme(&theme)
        .with_prompt("GitHub OAuth App Client ID (Device Flow enabled)")
        .allow_empty(false)
        .interact_text()?;

    let mut env = default_env();
    let release = latest_release(&http_client()?)
        .await
        .context("failed to fetch latest Hostlet release image metadata")?;
    if !release.has_release_image_digests() {
        bail!("latest Hostlet release is missing required image digests");
    }
    env.extend(release.image_env()?);
    env.insert("HOSTLET_IMAGE_TAG".into(), release.image_tag());
    env.insert("HOSTLET_ALLOWED_GITHUB_LOGINS".into(), allowed_login);
    env.insert("GITHUB_CLIENT_ID".into(), github_client_id);

    match access_mode {
        AccessMode::Lan => configure_lan_env(&theme, &mut env)?,
        AccessMode::CloudflareTunnel => configure_cloudflare(&theme, &mut env).await?,
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
        if access_mode == AccessMode::CloudflareTunnel {
            " --tunnel"
        } else {
            ""
        }
    );
    Ok(())
}

pub(crate) fn preflight(root: &Path) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    let linux = cfg!(target_os = "linux");
    let arch = matches!(std::env::consts::ARCH, "x86_64" | "aarch64");
    let docker = command_ok("docker", &["--version"]);
    let compose = command_ok("docker", &["compose", "version"]);
    let disk = disk_space_ok(root);
    check("Linux host", linux);
    check("CPU architecture", arch);
    check("Docker Engine", docker);
    check("Docker Compose v2", compose);
    check("Free disk space (>1 GiB)", disk);
    if !(linux && arch && docker && compose && disk) {
        bail!("preflight failed; install Docker Engine with Compose v2 and ensure at least 1 GiB is free, then rerun `hostlet preflight`");
    }
    Ok(())
}

pub(crate) async fn configure(root: &Path) -> anyhow::Result<()> {
    require_interactive()?;
    ensure_repo_root(root)?;
    preflight(root)?;
    let env_path = root.join(".env");
    let mut env =
        read_env_file(&env_path).context("Hostlet is not initialized; run hostlet init")?;
    let theme = ColorfulTheme::default();
    let current = access_mode(&env);
    let default_mode = usize::from(current == AccessMode::CloudflareTunnel);
    let selected = Select::with_theme(&theme)
        .with_prompt("Hostlet UI/API access mode")
        .items(&["LAN", "Cloudflare Tunnel"])
        .default(default_mode)
        .interact()?;
    let login: String = Input::with_theme(&theme)
        .with_prompt("Allowed GitHub username")
        .default(
            env.get("HOSTLET_ALLOWED_GITHUB_LOGINS")
                .cloned()
                .unwrap_or_default(),
        )
        .allow_empty(false)
        .interact_text()?;
    let client_id: String = Input::with_theme(&theme)
        .with_prompt("GitHub OAuth App Client ID (Device Flow enabled)")
        .default(env.get("GITHUB_CLIENT_ID").cloned().unwrap_or_default())
        .allow_empty(false)
        .interact_text()?;
    env.insert("HOSTLET_ALLOWED_GITHUB_LOGINS".into(), login);
    env.insert("GITHUB_CLIENT_ID".into(), client_id);
    let mode = if selected == 0 {
        AccessMode::Lan
    } else {
        AccessMode::CloudflareTunnel
    };
    match mode {
        AccessMode::Lan => configure_lan_env(&theme, &mut env)?,
        AccessMode::CloudflareTunnel => configure_cloudflare(&theme, &mut env).await?,
    }
    let candidate = root.join(".env.candidate");
    write_env_file(&candidate, &env)?;
    if !compose_config_with_env_ok(root, &candidate, mode == AccessMode::CloudflareTunnel) {
        let _ = fs::remove_file(&candidate);
        bail!("candidate configuration failed Docker Compose validation; existing .env was not changed");
    }
    let backup = root.join(format!(".env.backup.{}", timestamp_suffix()));
    fs::copy(&env_path, &backup)?;
    set_secret_file_permissions(&backup)?;
    fs::rename(&candidate, &env_path)?;
    set_secret_file_permissions(&env_path)?;
    println!("Configuration applied; backup: {}", backup.display());
    if Confirm::with_theme(&theme)
        .with_prompt("Restart Hostlet now?")
        .default(true)
        .interact()?
    {
        compose_up(root, false, false)?;
    }
    Ok(())
}

pub(crate) fn version() -> anyhow::Result<()> {
    println!("hostlet {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}

pub(crate) async fn status(root: &Path) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    let env = read_env_file(&root.join(".env")).unwrap_or_default();
    println!("Hostlet {}", env!("CARGO_PKG_VERSION"));
    check_docker_runtime();
    check("Compose services", any_compose_services_running(root));
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
