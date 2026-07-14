use super::*;

mod release;
mod state;

pub(crate) use release::*;
pub(crate) use state::*;

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

pub(crate) fn operator_api_and_token(
    env: &BTreeMap<String, String>,
) -> anyhow::Result<(String, String)> {
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

pub(crate) async fn print_operator_status(
    client: &reqwest::Client,
    env: &BTreeMap<String, String>,
) {
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

pub(crate) async fn update(
    root: &Path,
    dry_run: bool,
    yes: bool,
    no_backup: bool,
) -> anyhow::Result<()> {
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
    if !confirm_update(current, latest, yes)? {
        bail!("update canceled");
    }

    let backup_dir = if no_backup {
        None
    } else {
        Some(pre_update_backup(root)?)
    };
    let update_state = save_update_state(root)?;

    let tmp_dir = root.join(".hostlet-update");
    fs::create_dir_all(&tmp_dir)?;
    let tmp_binary = download_verified_cli(&client, &release, &tmp_dir).await?;

    checkout_release_tag(root, &release)?;
    update_env_release_images(root, &release)?;
    let previous = swap_cli_binary(&tmp_dir, &tmp_binary)?;

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

/// Prompts the operator to confirm the update. Returns `Ok(true)` to proceed.
/// When `yes` is set, the prompt is skipped and the update is confirmed.
fn confirm_update(current: &str, latest: &str, yes: bool) -> anyhow::Result<bool> {
    if yes {
        return Ok(true);
    }
    Ok(Confirm::new()
        .with_prompt(format!("Update Hostlet from {current} to {latest}?"))
        .default(false)
        .interact()?)
}

/// Downloads the new CLI binary into `tmp_dir` and verifies it against the
/// release's published SHA-256 checksum. Returns the path to the verified
/// binary in `tmp_dir`.
async fn download_verified_cli(
    client: &reqwest::Client,
    release: &ReleaseInfo,
    tmp_dir: &Path,
) -> anyhow::Result<PathBuf> {
    let asset_name = linux_asset()?;
    let asset = release
        .asset(asset_name)
        .with_context(|| format!("latest release does not include {asset_name}"))?;
    let checksum_asset = release
        .asset(&format!("{asset_name}.sha256"))
        .with_context(|| format!("latest release does not include {asset_name}.sha256"))?;
    let tmp_binary = tmp_dir.join(asset_name);
    download(client, &asset.download_url, &tmp_binary).await?;
    let expected = checksum_from_asset(client, checksum_asset).await?;
    let actual = sha256_file(&tmp_binary)?;
    if actual != expected {
        bail!("downloaded CLI checksum mismatch");
    }
    Ok(tmp_binary)
}

/// Replaces the running CLI binary with `tmp_binary`, first copying the current
/// executable into `tmp_dir` for rollback. Returns the saved previous-binary
/// path.
fn swap_cli_binary(tmp_dir: &Path, tmp_binary: &Path) -> anyhow::Result<PathBuf> {
    let current_exe = std::env::current_exe().context("could not locate current hostlet binary")?;
    let previous = tmp_dir.join(format!("hostlet.previous.{}", timestamp_suffix()));
    fs::copy(&current_exe, &previous).with_context(|| "failed to save previous CLI binary")?;
    fs::copy(tmp_binary, &current_exe).with_context(|| {
        format!(
            "failed to replace {}; try running the update with sudo",
            current_exe.display()
        )
    })?;
    #[cfg(unix)]
    fs::set_permissions(&current_exe, fs::Permissions::from_mode(0o755))?;
    Ok(previous)
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
        release.asset(linux_asset()?).is_some(),
    );
    check(
        "CLI checksum asset",
        release
            .asset(&format!("{}.sha256", linux_asset()?))
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
    let asset_name = linux_asset()?;
    if release.asset(asset_name).is_none()
        || release.asset(&format!("{asset_name}.sha256")).is_none()
    {
        bail!("latest release is missing required update assets");
    }
    if !release.has_release_images() {
        bail!("latest release is missing required Hostlet image metadata");
    }
    if !release.has_release_image_digests() {
        bail!("latest release is missing required Hostlet image digests");
    }
    Ok(())
}

pub(crate) fn update_env_release_images(root: &Path, release: &ReleaseInfo) -> anyhow::Result<()> {
    let env_path = root.join(".env");
    let mut env = read_env_file(&env_path)
        .with_context(|| format!("failed to read {}", env_path.display()))?;
    for (key, value) in release.image_env()? {
        env.insert(key, value);
    }
    env.insert("HOSTLET_IMAGE_TAG".into(), release.image_tag());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn env_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn operator_api_and_token_prefers_public_api_url_and_trims_slashes() {
        let env = env_map(&[
            ("PUBLIC_API_URL", "https://hostlet.example.test/"),
            ("HOSTLET_API_URL", "http://ignored.test"),
            ("LOCAL_AGENT_TOKEN", "tok-123"),
        ]);

        let (api_url, token) = operator_api_and_token(&env).unwrap();

        assert_eq!(api_url, "https://hostlet.example.test");
        assert_eq!(token, "tok-123");
    }

    #[test]
    fn operator_api_and_token_falls_back_to_hostlet_api_url() {
        let env = env_map(&[
            ("HOSTLET_API_URL", "http://10.0.0.1:8080///"),
            ("LOCAL_AGENT_TOKEN", "tok"),
        ]);

        let (api_url, _) = operator_api_and_token(&env).unwrap();

        assert_eq!(api_url, "http://10.0.0.1:8080");
    }

    #[test]
    fn operator_api_and_token_defaults_to_loopback_when_unset() {
        let env = env_map(&[("LOCAL_AGENT_TOKEN", "tok")]);

        let (api_url, _) = operator_api_and_token(&env).unwrap();

        assert_eq!(api_url, "http://127.0.0.1:8080");
    }

    #[test]
    fn operator_api_and_token_requires_local_agent_token() {
        let env = env_map(&[("PUBLIC_API_URL", "https://hostlet.example.test")]);

        assert!(operator_api_and_token(&env).is_err());
    }
}
