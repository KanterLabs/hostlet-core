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

/// How Hostlet's UI/API is reached, chosen interactively during `init`.
/// Ordering matches the `Select` items below; `LanOnly` is the default.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AccessMode {
    LanOnly,
    CloudflareTunnel,
}

impl AccessMode {
    fn from_index(index: usize) -> Self {
        match index {
            0 => Self::LanOnly,
            _ => Self::CloudflareTunnel,
        }
    }
}

fn configure_lan_env(
    theme: &ColorfulTheme,
    env: &mut BTreeMap<String, String>,
) -> anyhow::Result<()> {
    let host: String = Input::with_theme(theme)
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
    Ok(())
}

pub(crate) async fn init(root: &Path, force: bool) -> anyhow::Result<()> {
    require_interactive()?;
    ensure_repo_root(root)?;
    let env_path = root.join(".env");
    if env_path.exists() && !force {
        bail!(".env already exists. Run hostlet init --force to replace it after reviewing your backup.");
    }

    let theme = ColorfulTheme::default();
    println!("Hostlet init writes .env and generates local secrets.");

    let access_mode = AccessMode::from_index(
        Select::with_theme(&theme)
            .with_prompt("Hostlet UI/API access mode")
            .items(&["LAN only", "Cloudflare Tunnel for Hostlet UI/API"])
            .default(0)
            .interact()?,
    );

    let allowed_login: String = Input::with_theme(&theme)
        .with_prompt("Allowed GitHub username")
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
        AccessMode::LanOnly => configure_lan_env(&theme, &mut env)?,
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
