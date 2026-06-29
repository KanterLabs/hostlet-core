use super::*;
use std::process::Output;

pub(crate) fn compose_args(root: &Path, dev: bool) -> Vec<String> {
    let mut args = vec!["compose".into()];
    // `hostlet init` writes the repo-root .env, but compose v2 only auto-loads
    // .env from the directory of the first -f file (infra/). Pass it explicitly;
    // skipped when missing so pre-init commands (doctor) still run.
    if root.join(".env").is_file() {
        args.extend(["--env-file".into(), ".env".into()]);
    }
    args.extend([
        "-f".into(),
        if dev {
            "infra/docker-compose.yml".into()
        } else {
            "infra/docker-compose.prod.yml".into()
        },
    ]);
    args
}

pub(crate) fn run_passthrough(root: &Path, bin: &str, args: &[String]) -> anyhow::Result<()> {
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

pub(crate) fn command_ok(bin: &str, args: &[&str]) -> bool {
    Command::new(bin)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Runs `docker compose <subcommand...>` in `root` with output suppressed and
/// reports whether the resulting output satisfies `accept`.
fn compose_status(
    root: &Path,
    dev: bool,
    subcommand: &[&str],
    accept: impl Fn(&Output) -> bool,
) -> bool {
    let mut args = compose_args(root, dev);
    args.extend(subcommand.iter().map(|arg| arg.to_string()));
    Command::new("docker")
        .current_dir(root)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .map(|output| accept(&output))
        .unwrap_or(false)
}

pub(crate) fn compose_config_ok(root: &Path, dev: bool) -> bool {
    compose_status(root, dev, &["config"], |output| output.status.success())
}

pub(crate) fn compose_services_running(root: &Path, dev: bool) -> bool {
    compose_status(root, dev, &["ps", "--status", "running", "-q"], |output| {
        output.status.success() && !output.stdout.is_empty()
    })
}

pub(crate) fn disk_space_ok(root: &Path) -> bool {
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

pub(crate) fn latest_backup(root: &Path) -> Option<PathBuf> {
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

pub(crate) fn latest_backup_age(root: &Path) -> Option<String> {
    let backup = latest_backup(root)?;
    let modified = backup.metadata().ok()?.modified().ok()?;
    let elapsed = modified.elapsed().ok()?;
    Some(format_duration(elapsed))
}

pub(crate) fn format_duration(duration: Duration) -> String {
    let hours = duration.as_secs() / 3600;
    if hours >= 48 {
        format!("{} days", hours / 24)
    } else if hours >= 1 {
        format!("{hours} hours")
    } else {
        format!("{} minutes", duration.as_secs() / 60)
    }
}

pub(crate) fn check(label: &str, ok: bool) {
    println!(
        "{:<28} {}",
        label,
        if ok { "ok" } else { "needs attention" }
    );
}

pub(crate) fn default_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    let mut set = |key: &str, value: String| {
        env.insert(key.to_string(), value);
    };

    // Database: the generated Postgres password is reused inside DATABASE_URL.
    let postgres_password = hex_secret(24);
    set("POSTGRES_USER", "hostlet".into());
    set("POSTGRES_PASSWORD", postgres_password.clone());
    set("POSTGRES_DB", "hostlet".into());
    set(
        "DATABASE_URL",
        format!("postgres://hostlet:{postgres_password}@localhost:5432/hostlet"),
    );

    // Runtime configuration (non-secret): wiring for Docker, image pinning and binding.
    set("DOCKER_GID", docker_gid());
    set(
        "HOSTLET_IMAGE_TAG",
        format!("v{}", env!("CARGO_PKG_VERSION")),
    );
    set("BIND_ADDR", "0.0.0.0:8080".into());
    set("HOSTLET_BASE_DOMAIN", String::new());
    set("HOSTLET_DOMAIN_PREFIX", "hostlet-".into());
    set("HOSTLET_CONTROL_PLANE_HOST", "localhost".into());
    set("HOSTLET_ALLOW_INSECURE_DEV_DEFAULTS", "false".into());
    set(
        "LOCAL_SERVER_ID",
        "00000000-0000-0000-0000-000000000001".into(),
    );

    // Cloudflare integration (operator-supplied; left blank by default).
    set("CLOUDFLARE_API_TOKEN", String::new());
    set("CLOUDFLARE_ZONE_ID", String::new());
    set("CLOUDFLARE_TUNNEL_TARGET", String::new());
    set("CLOUDFLARE_TUNNEL_TOKEN", String::new());

    // Secrets: generated per-install. ENCRYPTION_KEY needs raw 32-byte entropy
    // (base64), the rest are hex tokens.
    set("HOSTLET_SETUP_TOKEN", hex_secret(32));
    set("ENCRYPTION_KEY", base64_secret(32));
    set("JOB_SIGNING_SECRET", hex_secret(32));
    set("SESSION_SECRET", hex_secret(32));
    set("LOCAL_AGENT_TOKEN", hex_secret(32));
    set("GITHUB_WEBHOOK_SECRET", hex_secret(32));

    env
}

pub(crate) fn write_env_file(path: &Path, env: &BTreeMap<String, String>) -> anyhow::Result<()> {
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

pub(crate) fn secret_open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    options
}

pub(crate) fn set_secret_file_permissions(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to set secure permissions on {}", path.display()))?;
    Ok(())
}

pub(crate) fn read_env_file(path: &Path) -> anyhow::Result<BTreeMap<String, String>> {
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

pub(crate) fn quote_env(value: &str) -> String {
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

pub(crate) fn unquote_env(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
        .replace("\\\"", "\"")
        .replace("\\\\", "\\")
}

pub(crate) fn base64_secret(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    STANDARD.encode(buf)
}

pub(crate) fn hex_secret(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    buf.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn docker_gid() -> String {
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

/// Returns a UTC timestamp suffix (e.g. `20260601T143000Z`) for naming backup
/// and update-state directories. Shells out to `date -u` rather than pulling in
/// a datetime crate; falls back to the literal `backup` if that fails.
pub(crate) fn timestamp_suffix() -> String {
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

pub(crate) fn ensure_repo_root(root: &Path) -> anyhow::Result<()> {
    if !root.join("infra").join("docker-compose.yml").exists() || !root.join("Cargo.toml").exists()
    {
        bail!(
            "{} does not look like the Hostlet repository root",
            root.display()
        );
    }
    Ok(())
}

pub(crate) fn require_interactive() -> anyhow::Result<()> {
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
    fn compose_args_passes_root_env_file_when_present() {
        let root = std::env::temp_dir().join(format!(
            "hostlet-cli-test-envfile-present-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".env"), "POSTGRES_PASSWORD=secret\n").unwrap();

        let args = compose_args(&root, false);

        // --env-file .env must appear as a consecutive pair before the -f flag.
        let env_file_pos = args
            .windows(2)
            .position(|w| w[0] == "--env-file" && w[1] == ".env")
            .expect("--env-file .env pair not found");
        let f_pos = args.iter().position(|a| a == "-f").expect("-f not found");
        assert!(
            env_file_pos < f_pos,
            "--env-file must precede -f (positions: {env_file_pos} vs {f_pos})"
        );
        // Prod compose file selected when dev=false.
        assert!(args.contains(&"infra/docker-compose.prod.yml".to_string()));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn compose_args_omits_env_file_when_missing() {
        let root = std::env::temp_dir().join(format!(
            "hostlet-cli-test-envfile-absent-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let args_prod = compose_args(&root, false);
        let args_dev = compose_args(&root, true);

        assert!(
            !args_prod.contains(&"--env-file".to_string()),
            "--env-file must be absent when .env does not exist"
        );
        assert!(args_prod.contains(&"infra/docker-compose.prod.yml".to_string()));
        assert!(args_dev.contains(&"infra/docker-compose.yml".to_string()));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn env_quote_round_trips_spaces() {
        let value = "secret with spaces";
        assert_eq!(unquote_env(&quote_env(value)), value);
    }

    #[test]
    fn quote_env_leaves_url_safe_values_unquoted() {
        // Alphanumerics plus / : . _ - , = stay bare (see quote_env's allow list).
        let value = "ghcr.io/shanekanterman04/hostlet-api:v0.4.1";
        assert_eq!(quote_env(value), value);
    }

    #[test]
    fn quote_env_quotes_and_escapes_special_characters() {
        assert_eq!(quote_env("a b"), "\"a b\"");
        assert_eq!(quote_env("he said \"hi\""), "\"he said \\\"hi\\\"\"");
        assert_eq!(quote_env("back\\slash"), "\"back\\\\slash\"");
    }

    #[test]
    fn quote_env_and_unquote_env_treat_empty_as_empty() {
        assert_eq!(quote_env(""), "");
        assert_eq!(unquote_env(""), "");
    }

    #[test]
    fn env_quote_round_trips_quotes_and_backslashes() {
        for value in [
            "a b",
            "he said \"hi\"",
            "back\\slash",
            "trailing\\",
            "\\\"mixed\\\"",
        ] {
            assert_eq!(
                unquote_env(&quote_env(value)),
                value,
                "round trip failed for {value:?}"
            );
        }
    }

    #[test]
    fn format_duration_reports_minutes_below_one_hour() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0 minutes");
        assert_eq!(format_duration(Duration::from_secs(59)), "0 minutes");
        assert_eq!(format_duration(Duration::from_secs(60)), "1 minutes");
        assert_eq!(format_duration(Duration::from_secs(3599)), "59 minutes");
    }

    #[test]
    fn format_duration_reports_hours_then_days_at_boundaries() {
        assert_eq!(format_duration(Duration::from_secs(3600)), "1 hours");
        assert_eq!(format_duration(Duration::from_secs(47 * 3600)), "47 hours");
        // 48 h is the days threshold: hours/24 = 2.
        assert_eq!(format_duration(Duration::from_secs(48 * 3600)), "2 days");
        assert_eq!(format_duration(Duration::from_secs(72 * 3600)), "3 days");
    }

    #[test]
    fn generated_encryption_key_is_base64_32_bytes() {
        let secret = base64_secret(32);
        assert_eq!(STANDARD.decode(secret).unwrap().len(), 32);
    }

    #[test]
    fn release_manifest_parses_image_metadata() {
        let digest = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let manifest = serde_json::json!({
            "version": "v0.4.1",
            "image_registry": "ghcr.io/shanekanterman04",
            "image_tag": "v0.4.1",
            "images": {
                "api": {
                    "ref": "ghcr.io/shanekanterman04/hostlet-api:v0.4.1",
                    "digest": digest
                },
                "web": {
                    "ref": "ghcr.io/shanekanterman04/hostlet-web:v0.4.1",
                    "digest": digest
                },
                "agent": {
                    "ref": "ghcr.io/shanekanterman04/hostlet-agent:v0.4.1",
                    "digest": digest
                },
                "screenshotter": {
                    "ref": "ghcr.io/shanekanterman04/hostlet-screenshotter:v0.4.1",
                    "digest": digest
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
            Some(digest)
        );
        assert_eq!(
            release
                .images
                .screenshotter
                .as_ref()
                .unwrap()
                .digest
                .as_deref(),
            Some(digest)
        );
        assert!(release.has_release_image_digests());
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
    fn update_env_release_images_rewrites_existing_env_file() {
        let root = std::env::temp_dir().join(format!("hostlet-cli-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let env_path = root.join(".env");
        fs::write(
            &env_path,
            "POSTGRES_PASSWORD=secret\nHOSTLET_IMAGE_TAG=v0.4.0\nPUBLIC_API_URL=http://localhost:8080\n",
        )
        .unwrap();

        let digest = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let mut release = test_release();
        release.image_tag = Some("v0.4.1".into());
        release.images = ReleaseImages {
            api: Some(ReleaseImage {
                reference: "ghcr.io/example/hostlet-api:v0.4.1".into(),
                digest: Some(digest.into()),
            }),
            web: Some(ReleaseImage {
                reference: "ghcr.io/example/hostlet-web:v0.4.1".into(),
                digest: Some(digest.into()),
            }),
            agent: Some(ReleaseImage {
                reference: "ghcr.io/example/hostlet-agent:v0.4.1".into(),
                digest: Some(digest.into()),
            }),
            screenshotter: Some(ReleaseImage {
                reference: "ghcr.io/example/hostlet-screenshotter:v0.4.1".into(),
                digest: Some(digest.into()),
            }),
        };

        update_env_release_images(&root, &release).unwrap();
        let env = read_env_file(&env_path).unwrap();

        assert_eq!(
            env.get("HOSTLET_IMAGE_TAG").map(String::as_str),
            Some("v0.4.1")
        );
        assert_eq!(
            env.get("HOSTLET_API_IMAGE").map(String::as_str),
            Some("ghcr.io/example/hostlet-api@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(
            env.get("POSTGRES_PASSWORD").map(String::as_str),
            Some("secret")
        );
        let _ = fs::remove_dir_all(&root);
    }
}
