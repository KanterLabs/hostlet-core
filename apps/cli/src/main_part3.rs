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

fn compose_config_ok(root: &Path, dev: bool) -> bool {
    let mut args = compose_args(dev);
    args.push("config".into());
    Command::new("docker")
        .current_dir(root)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn compose_services_running(root: &Path, dev: bool) -> bool {
    let mut args = compose_args(dev);
    args.extend([
        "ps".into(),
        "--status".into(),
        "running".into(),
        "-q".into(),
    ]);
    Command::new("docker")
        .current_dir(root)
        .args(args)
        .output()
        .map(|output| output.status.success() && !output.stdout.is_empty())
        .unwrap_or(false)
}

fn disk_space_ok(root: &Path) -> bool {
    Command::new("df")
        .arg("-Pk")
        .arg(root)
        .output()
        .ok()
        .and_then(|output| {
            if !output.status.success() {
                return None;
            }
            let stdout = String::from_utf8(output.stdout).ok()?;
            let line = stdout.lines().nth(1)?;
            let available_kb = line.split_whitespace().nth(3)?.parse::<u64>().ok()?;
            Some(available_kb > 1024 * 1024)
        })
        .unwrap_or(false)
}

fn latest_backup(root: &Path) -> Option<PathBuf> {
    let backup_dir = root.join("backups");
    let mut entries = fs::read_dir(backup_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|(modified, _)| *modified);
    entries.pop().map(|(_, path)| path)
}

fn latest_backup_age(root: &Path) -> Option<String> {
    let backup = latest_backup(root)?;
    let modified = backup.metadata().ok()?.modified().ok()?;
    let elapsed = modified.elapsed().ok()?;
    Some(format_duration(elapsed))
}

fn format_duration(duration: Duration) -> String {
    let hours = duration.as_secs() / 3600;
    if hours >= 48 {
        format!("{} days", hours / 24)
    } else if hours >= 1 {
        format!("{hours} hours")
    } else {
        format!("{} minutes", duration.as_secs() / 60)
    }
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
        "HOSTLET_IMAGE_TAG".into(),
        format!("v{}", env!("CARGO_PKG_VERSION")),
    );
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

    fn test_release() -> ReleaseInfo {
        ReleaseInfo {
            version: "0.4.0".into(),
            notes_url: "https://example.test/release".into(),
            released_at: None,
            minimum_supported_version: None,
            compose_migrations: false,
            database_migrations: false,
            assets: Vec::new(),
            image_registry: None,
            image_tag: None,
            images: ReleaseImages::default(),
        }
    }

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

    #[test]
    fn version_comparison_handles_patch_versions() {
        assert!(version_is_newer("0.2.0", "0.2.1"));
        assert!(version_is_newer("0.1.9", "0.2.0"));
        assert!(!version_is_newer("0.2.0", "0.2.0"));
        assert!(!version_is_newer("0.2.1", "0.2.0"));
    }

    #[test]
    fn release_manifest_parses_image_metadata() {
        let manifest = serde_json::json!({
            "version": "v0.4.1",
            "image_registry": "ghcr.io/shanekanterman04",
            "image_tag": "v0.4.1",
            "images": {
                "api": {
                    "ref": "ghcr.io/shanekanterman04/hostlet-api:v0.4.1",
                    "digest": "sha256:api"
                },
                "web": {
                    "ref": "ghcr.io/shanekanterman04/hostlet-web:v0.4.1",
                    "digest": "sha256:web"
                },
                "agent": {
                    "ref": "ghcr.io/shanekanterman04/hostlet-agent:v0.4.1",
                    "digest": "sha256:agent"
                }
            }
        });
        let mut release = test_release();

        apply_release_manifest_value(&mut release, &manifest);

        assert_eq!(release.version, "0.4.1");
        assert_eq!(release.image_tag(), "v0.4.1");
        assert!(release.has_release_images());
        assert_eq!(
            release.images.web.as_ref().unwrap().reference,
            "ghcr.io/shanekanterman04/hostlet-web:v0.4.1"
        );
        assert_eq!(
            release.images.agent.as_ref().unwrap().digest.as_deref(),
            Some("sha256:agent")
        );
    }

    #[test]
    fn default_env_pins_current_release_image_tag() {
        let env = default_env();
        assert_eq!(
            env.get("HOSTLET_IMAGE_TAG").map(String::as_str),
            Some(concat!("v", env!("CARGO_PKG_VERSION")))
        );
    }

    #[test]
    fn update_env_image_tag_rewrites_existing_env_file() {
        let root = std::env::temp_dir().join(format!("hostlet-cli-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let env_path = root.join(".env");
        fs::write(
            &env_path,
            "POSTGRES_PASSWORD=secret\nHOSTLET_IMAGE_TAG=v0.4.0\nPUBLIC_API_URL=http://localhost:8080\n",
        )
        .unwrap();

        update_env_image_tag(&root, "v0.4.1").unwrap();
        let env = read_env_file(&env_path).unwrap();

        assert_eq!(
            env.get("HOSTLET_IMAGE_TAG").map(String::as_str),
            Some("v0.4.1")
        );
        assert_eq!(
            env.get("POSTGRES_PASSWORD").map(String::as_str),
            Some("secret")
        );
        let _ = fs::remove_dir_all(&root);
    }
}
