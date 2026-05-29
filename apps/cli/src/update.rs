use super::*;

pub(crate) async fn cleanup(root: &Path, dry_run: bool, yes: bool) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    let env = read_env_file(&root.join(".env")).unwrap_or_default();
    let (api_url, token) = operator_api_and_token(&env)?;
    let client = http_client()?;
    let preview: Value = client
        .get(format!("{}/api/system/operator-cleanup", api_url))
        .header("x-hostlet-agent-token", &token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    println!("{}", serde_json::to_string_pretty(&preview)?);
    if dry_run {
        return Ok(());
    }
    if !yes
        && !Confirm::new()
            .with_prompt("Run cleanup now?")
            .default(false)
            .interact()?
    {
        bail!("cleanup canceled");
    }
    let result: Value = client
        .post(format!("{}/api/system/operator-cleanup", api_url))
        .header("x-hostlet-agent-token", &token)
        .json(&serde_json::json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub(crate) fn operator_api_and_token(env: &BTreeMap<String, String>) -> anyhow::Result<(String, String)> {
    let api_url = env
        .get("PUBLIC_API_URL")
        .cloned()
        .or_else(|| env.get("HOSTLET_API_URL").cloned())
        .unwrap_or_else(|| "http://127.0.0.1:8080".into())
        .trim_end_matches('/')
        .to_string();
    let token = env
        .get("LOCAL_AGENT_TOKEN")
        .cloned()
        .context("LOCAL_AGENT_TOKEN is required for operator cleanup")?;
    Ok((api_url, token))
}

pub(crate) async fn print_operator_status(client: &reqwest::Client, env: &BTreeMap<String, String>) {
    let (Some(api_url), Some(token)) = (env.get("PUBLIC_API_URL"), env.get("LOCAL_AGENT_TOKEN"))
    else {
        println!("App health summary        unavailable");
        return;
    };
    match client
        .get(format!(
            "{}/api/system/operator-status",
            api_url.trim_end_matches('/')
        ))
        .header("x-hostlet-agent-token", token)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let summary: Value = resp.json().await.unwrap_or(Value::Null);
            if let Some(health) = summary.get("health") {
                println!("App health summary        {health}");
            } else {
                println!("App health summary        unavailable");
            }
            if let Some(servers) = summary.get("servers") {
                println!("Server summary            {servers}");
            }
        }
        _ => println!("App health summary        unavailable"),
    }
}

pub(crate) async fn update_check() -> anyhow::Result<()> {
    let client = http_client()?;
    let release = latest_release(&client).await?;
    print_update_check(&release);
    Ok(())
}

pub(crate) async fn update(root: &Path, dry_run: bool, yes: bool, no_backup: bool) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    let client = http_client()?;
    let release = latest_release(&client).await?;
    print_update_check(&release);
    let current = env!("CARGO_PKG_VERSION");
    let latest = release.version.trim_start_matches('v');
    if let Some(minimum) = &release.minimum_supported_version {
        if version_is_newer(minimum, current) {
            bail!(
                "direct update from {current} to {latest} is not supported; install at least {minimum} first"
            );
        }
    }
    if !version_is_newer(current, latest) {
        println!("Hostlet is already up to date.");
        return Ok(());
    }
    update_preflight(root, &release)?;
    if dry_run {
        println!("Dry run complete. No files changed.");
        return Ok(());
    }
    if !yes
        && !Confirm::new()
            .with_prompt(format!("Update Hostlet from {current} to {latest}?"))
            .default(false)
            .interact()?
    {
        bail!("update canceled");
    }
    let backup_dir = if no_backup {
        None
    } else {
        Some(pre_update_backup(root)?)
    };
    let update_state = save_update_state(root)?;
    let asset = release
        .asset(LINUX_X64_ASSET)
        .context("latest release does not include hostlet-linux-x64")?;
    let checksum_asset = release
        .asset(&format!("{LINUX_X64_ASSET}.sha256"))
        .context("latest release does not include hostlet-linux-x64.sha256")?;
    let tmp_dir = root.join(".hostlet-update");
    fs::create_dir_all(&tmp_dir)?;
    let tmp_binary = tmp_dir.join(LINUX_X64_ASSET);
    download(&client, &asset.download_url, &tmp_binary).await?;
    let expected = checksum_from_asset(&client, checksum_asset).await?;
    let actual = sha256_file(&tmp_binary)?;
    if actual != expected {
        bail!("downloaded CLI checksum mismatch");
    }
    checkout_release_tag(root, &release)?;
    update_env_image_tag(root, &release.image_tag())?;
    let current_exe = std::env::current_exe().context("could not locate current hostlet binary")?;
    let previous = tmp_dir.join(format!("hostlet.previous.{}", timestamp_suffix()));
    fs::copy(&current_exe, &previous).with_context(|| "failed to save previous CLI binary")?;
    fs::copy(&tmp_binary, &current_exe).with_context(|| {
        format!(
            "failed to replace {}; try running the update with sudo",
            current_exe.display()
        )
    })?;
    #[cfg(unix)]
    fs::set_permissions(&current_exe, fs::Permissions::from_mode(0o755))?;
    compose_up(root, false, false)?;
    doctor(root).await?;
    println!("Updated Hostlet to {latest}.");
    if let Some(backup_dir) = backup_dir {
        println!("Pre-update backup: {}", backup_dir.display());
    }
    println!("Update rollback state: {}", update_state.display());
    println!("Previous CLI saved at {}", previous.display());
    Ok(())
}

pub(crate) fn update_preflight(root: &Path, release: &ReleaseInfo) -> anyhow::Result<()> {
    check("Docker", command_ok("docker", &["version"]));
    check(
        "Docker Compose",
        command_ok("docker", &["compose", "version"]),
    );
    check(".env exists", root.join(".env").exists());
    check(
        "Hostlet release asset",
        release.asset(LINUX_X64_ASSET).is_some(),
    );
    check(
        "CLI checksum asset",
        release
            .asset(&format!("{LINUX_X64_ASSET}.sha256"))
            .is_some(),
    );
    ensure_repo_root(root)?;
    if !root.join(".env").exists() {
        bail!("missing .env; run hostlet init first");
    }
    let current = env!("CARGO_PKG_VERSION");
    if let Some(minimum) = &release.minimum_supported_version {
        if version_is_newer(minimum, current) {
            bail!(
                "latest release requires Hostlet {minimum} or newer before updating from {current}"
            );
        }
    }
    if release.asset(LINUX_X64_ASSET).is_none()
        || release
            .asset(&format!("{LINUX_X64_ASSET}.sha256"))
            .is_none()
    {
        bail!("latest release is missing required update assets");
    }
    if !release.has_release_images() {
        bail!("latest release is missing required Hostlet image metadata");
    }
    Ok(())
}

pub(crate) fn update_env_image_tag(root: &Path, image_tag: &str) -> anyhow::Result<()> {
    let env_path = root.join(".env");
    let mut env = read_env_file(&env_path)
        .with_context(|| format!("failed to read {}", env_path.display()))?;
    env.insert("HOSTLET_IMAGE_TAG".into(), image_tag.to_string());
    write_env_file(&env_path, &env)
}

pub(crate) fn checkout_release_tag(root: &Path, release: &ReleaseInfo) -> anyhow::Result<()> {
    if !root.join(".git").exists() {
        return Ok(());
    }
    let tag = release.image_tag();
    run_passthrough(
        root,
        "git",
        &["fetch".into(), "--tags".into(), "--force".into()],
    )?;
    run_passthrough(root, "git", &["checkout".into(), "--detach".into(), tag])
}

pub(crate) fn pre_update_backup(root: &Path) -> anyhow::Result<PathBuf> {
    let output = root
        .join("backups")
        .join(format!("pre-update-{}", timestamp_suffix()));
    backup(root, Some(output.clone()), false)?;
    if root.join(".env").exists() {
        fs::create_dir_all(&output)?;
        fs::copy(root.join(".env"), output.join(".env"))
            .with_context(|| "failed to copy .env into pre-update backup")?;
        set_secret_file_permissions(&output.join(".env"))?;
    }
    fs::write(
        output.join("hostlet-version.txt"),
        env!("CARGO_PKG_VERSION"),
    )?;
    Ok(output)
}

pub(crate) fn save_update_state(root: &Path) -> anyhow::Result<PathBuf> {
    let state_dir = root
        .join(".hostlet-update")
        .join(format!("state-{}", timestamp_suffix()));
    fs::create_dir_all(&state_dir)?;
    for relative in [
        ".env",
        "infra/docker-compose.yml",
        "infra/docker-compose.prod.yml",
    ] {
        let source = root.join(relative);
        if source.exists() {
            let target = state_dir.join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source, &target)
                .with_context(|| format!("failed to save {}", source.display()))?;
        }
    }
    let git_rev = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".into());
    fs::write(state_dir.join("git-revision.txt"), git_rev)?;
    fs::write(
        state_dir.join("hostlet-version.txt"),
        env!("CARGO_PKG_VERSION"),
    )?;
    Ok(state_dir)
}

pub(crate) fn update_rollback(root: &Path) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    let update_dir = root.join(".hostlet-update");
    let mut previous = fs::read_dir(&update_dir)
        .with_context(|| format!("no update state found in {}", update_dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("hostlet.previous."))
        })
        .collect::<Vec<_>>();
    previous.sort();
    let Some(previous_binary) = previous.pop() else {
        bail!("no previous CLI binary found");
    };
    let current_exe = std::env::current_exe().context("could not locate current hostlet binary")?;
    fs::copy(&previous_binary, &current_exe).with_context(|| {
        format!(
            "failed to restore {}; try running rollback with sudo",
            current_exe.display()
        )
    })?;
    #[cfg(unix)]
    fs::set_permissions(&current_exe, fs::Permissions::from_mode(0o755))?;
    if let Some(state_dir) = latest_update_state(&update_dir) {
        restore_update_state(root, &state_dir)?;
        println!("Restored Compose state from {}", state_dir.display());
    }
    compose_up(root, false, false)?;
    println!(
        "Restored previous Hostlet CLI from {}",
        previous_binary.display()
    );
    println!("Database rollback is not automatic; use the pre-update backup if needed.");
    Ok(())
}

pub(crate) fn latest_update_state(update_dir: &Path) -> Option<PathBuf> {
    let mut states = fs::read_dir(update_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("state-"))
        })
        .collect::<Vec<_>>();
    states.sort();
    states.pop()
}

pub(crate) fn restore_update_state(root: &Path, state_dir: &Path) -> anyhow::Result<()> {
    for relative in [
        ".env",
        "infra/docker-compose.yml",
        "infra/docker-compose.prod.yml",
    ] {
        let source = state_dir.join(relative);
        if source.exists() {
            let target = root.join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source, &target)
                .with_context(|| format!("failed to restore {}", target.display()))?;
            if relative == ".env" {
                set_secret_file_permissions(&target)?;
            }
        }
    }
    Ok(())
}

pub(crate) struct ReleaseInfo {
    pub(crate) version: String,
    pub(crate) notes_url: String,
    pub(crate) released_at: Option<String>,
    pub(crate) minimum_supported_version: Option<String>,
    pub(crate) compose_migrations: bool,
    pub(crate) database_migrations: bool,
    pub(crate) assets: Vec<ReleaseAsset>,
    pub(crate) image_registry: Option<String>,
    pub(crate) image_tag: Option<String>,
    pub(crate) images: ReleaseImages,
}

pub(crate) struct ReleaseAsset {
    name: String,
    download_url: String,
}

#[derive(Default)]
pub(crate) struct ReleaseImages {
    api: Option<ReleaseImage>,
    pub(crate) web: Option<ReleaseImage>,
    pub(crate) agent: Option<ReleaseImage>,
}

pub(crate) struct ReleaseImage {
    pub(crate) reference: String,
    pub(crate) digest: Option<String>,
}

impl ReleaseInfo {
    fn asset(&self, name: &str) -> Option<&ReleaseAsset> {
        self.assets.iter().find(|asset| asset.name == name)
    }

    pub(crate) fn image_tag(&self) -> String {
        self.image_tag
            .clone()
            .unwrap_or_else(|| format!("v{}", self.version.trim_start_matches('v')))
    }

    pub(crate) fn has_release_images(&self) -> bool {
        self.images.api.is_some() && self.images.web.is_some() && self.images.agent.is_some()
    }
}

pub(crate) async fn latest_release(client: &reqwest::Client) -> anyhow::Result<ReleaseInfo> {
    let value: Value = client
        .get(format!(
            "https://api.github.com/repos/{HOSTLET_REPO}/releases/latest"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let version = value
        .get("tag_name")
        .and_then(|v| v.as_str())
        .context("release did not include tag_name")?
        .to_string();
    let notes_url = value
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://github.com/ShaneKanterman04/Hostlet/releases/latest")
        .to_string();
    let assets = value
        .get("assets")
        .and_then(|v| v.as_array())
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|asset| {
            Some(ReleaseAsset {
                name: asset.get("name")?.as_str()?.to_string(),
                download_url: asset.get("browser_download_url")?.as_str()?.to_string(),
            })
        })
        .collect();
    let mut release = ReleaseInfo {
        version,
        notes_url,
        released_at: value
            .get("published_at")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        minimum_supported_version: None,
        compose_migrations: false,
        database_migrations: false,
        assets,
        image_registry: None,
        image_tag: None,
        images: ReleaseImages::default(),
    };
    if let Some(manifest_url) = release
        .asset("hostlet-release.json")
        .map(|asset| asset.download_url.clone())
    {
        apply_release_manifest(client, &mut release, &manifest_url).await?;
    }
    Ok(release)
}

pub(crate) async fn apply_release_manifest(
    client: &reqwest::Client,
    release: &mut ReleaseInfo,
    manifest_url: &str,
) -> anyhow::Result<()> {
    let value: Value = client
        .get(manifest_url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    apply_release_manifest_value(release, &value);
    Ok(())
}

pub(crate) fn apply_release_manifest_value(release: &mut ReleaseInfo, value: &Value) {
    if let Some(version) = value.get("version").and_then(|v| v.as_str()) {
        release.version = version.trim_start_matches('v').to_string();
    }
    if let Some(released_at) = value.get("released_at").and_then(|v| v.as_str()) {
        release.released_at = Some(released_at.to_string());
    }
    release.minimum_supported_version = value
        .get("minimum_supported_version")
        .and_then(|v| v.as_str())
        .map(|value| value.trim_start_matches('v').to_string());
    release.compose_migrations = value
        .get("compose_migrations")
        .and_then(|v| v.as_bool())
        .unwrap_or(release.compose_migrations);
    release.database_migrations = value
        .get("database_migrations")
        .and_then(|v| v.as_bool())
        .unwrap_or(release.database_migrations);
    if let Some(notes_url) = value.get("notes_url").and_then(|v| v.as_str()) {
        release.notes_url = notes_url.to_string();
    }
    release.image_registry = value
        .get("image_registry")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| release.image_registry.clone());
    release.image_tag = value
        .get("image_tag")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| release.image_tag.clone());
    if let Some(images) = value.get("images").and_then(|v| v.as_object()) {
        release.images.api = parse_release_image(images.get("api")).or(release.images.api.take());
        release.images.web = parse_release_image(images.get("web")).or(release.images.web.take());
        release.images.agent =
            parse_release_image(images.get("agent")).or(release.images.agent.take());
    }
}

pub(crate) fn parse_release_image(value: Option<&Value>) -> Option<ReleaseImage> {
    let value = value?;
    Some(ReleaseImage {
        reference: value.get("ref")?.as_str()?.to_string(),
        digest: value
            .get("digest")
            .and_then(|v| v.as_str())
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    })
}

pub(crate) fn print_update_check(release: &ReleaseInfo) {
    let current = env!("CARGO_PKG_VERSION");
    let latest = release.version.trim_start_matches('v');
    println!("Current version: {current}");
    println!("Latest version:  {latest}");
    if let Some(minimum) = &release.minimum_supported_version {
        println!("Minimum version: {minimum}");
    }
    if release.compose_migrations || release.database_migrations {
        println!(
            "Migrations:      compose={} database={}",
            release.compose_migrations, release.database_migrations
        );
    }
    if release.has_release_images() {
        println!("Image tag:       {}", release.image_tag());
        println!(
            "Images:          api={} web={} agent={}",
            release
                .images
                .api
                .as_ref()
                .map_or("missing", |image| image.reference.as_str()),
            release
                .images
                .web
                .as_ref()
                .map_or("missing", |image| image.reference.as_str()),
            release
                .images
                .agent
                .as_ref()
                .map_or("missing", |image| image.reference.as_str())
        );
        let signed_digests = [
            release.images.api.as_ref(),
            release.images.web.as_ref(),
            release.images.agent.as_ref(),
        ]
        .iter()
        .filter(|image| image.and_then(|image| image.digest.as_ref()).is_some())
        .count();
        println!("Image digests:   {signed_digests}/3 available");
    }
    println!(
        "Checksum signing: {}",
        if release.asset("hostlet-linux-x64.sha256.asc").is_some() {
            "available"
        } else {
            "unsigned checksum only"
        }
    );
    println!("Release notes:   {}", release.notes_url);
    println!(
        "Update:          {}",
        if version_is_newer(current, latest) {
            "available"
        } else {
            "not available"
        }
    );
}

pub(crate) fn version_is_newer(current: &str, latest: &str) -> bool {
    version_parts(latest) > version_parts(current)
}

pub(crate) fn version_parts(value: &str) -> (u64, u64, u64) {
    let mut parts = value
        .trim_start_matches('v')
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

pub(crate) async fn download(client: &reqwest::Client, url: &str, path: &Path) -> anyhow::Result<()> {
    let bytes = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    fs::write(path, &bytes).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub(crate) async fn checksum_from_asset(
    client: &reqwest::Client,
    asset: &ReleaseAsset,
) -> anyhow::Result<String> {
    let text = client
        .get(&asset.download_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    text.split_whitespace()
        .next()
        .map(str::to_string)
        .context("checksum asset was empty")
}

pub(crate) fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}
