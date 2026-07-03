use super::*;

/// Environment overrides that steer `scripts/backup.sh` and
/// `scripts/restore.sh` at the same compose file + env file the normal CLI
/// commands resolve via [`compose_args`]. Without these, the scripts fall back
/// to their built-in `infra/docker-compose.yml` default and never load the
/// project `.env`, so a production backup/restore targets the wrong compose
/// file and misses the required `POSTGRES_*` values (the prod compose file
/// hard-requires `${POSTGRES_PASSWORD:?}`).
///
/// `dev` selects the dev compose file; production (the default for
/// backup/restore) uses `infra/docker-compose.prod.yml`.
pub(crate) fn script_compose_env(root: &Path, dev: bool) -> Vec<(String, String)> {
    let compose_file = if dev {
        "infra/docker-compose.yml"
    } else {
        "infra/docker-compose.prod.yml"
    };
    let mut env = vec![(
        "HOSTLET_COMPOSE_FILE".to_string(),
        root.join(compose_file).display().to_string(),
    )];
    // Mirror `compose_args`: only pass the env file when it exists so pre-init
    // flows do not point compose at a missing file.
    let env_file = root.join(".env");
    if env_file.is_file() {
        env.push((
            "HOSTLET_COMPOSE_ENV_FILE".to_string(),
            env_file.display().to_string(),
        ));
    }
    env
}

pub(crate) fn backup(root: &Path, output: Option<PathBuf>, scheduled: bool) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    let mut args = vec![root.join("scripts/backup.sh").display().to_string()];
    if let Some(output) = output {
        args.push(output.display().to_string());
    }
    let mut command = Command::new("bash");
    command
        .current_dir(root)
        .args(&args)
        .envs(script_compose_env(root, false));
    if scheduled {
        command.env("HOSTLET_BACKUP_SCHEDULED", "true");
    }
    let status = command.status()?;
    if !status.success() {
        bail!("backup failed with {status}");
    }
    // Metadata recording is best-effort: a missing database connection should
    // not fail an otherwise-successful backup, but surface the reason so the
    // failure is not silently invisible to the operator.
    if let Err(error) = record_latest_backup_metadata(root) {
        eprintln!("warning: failed to record latest backup metadata: {error}");
    }
    Ok(())
}

pub(crate) fn record_latest_backup_metadata(root: &Path) -> anyhow::Result<()> {
    let metadata_path = root.join("backups/latest.json");
    if !metadata_path.is_file() {
        return Ok(());
    }
    let metadata = fs::read_to_string(metadata_path)?;
    let env = read_env_file(&root.join(".env")).unwrap_or_default();
    let user = env
        .get("POSTGRES_USER")
        .cloned()
        .unwrap_or_else(|| "hostlet".into());
    let db = env
        .get("POSTGRES_DB")
        .cloned()
        .unwrap_or_else(|| "hostlet".into());
    // Pass the JSON payload as a psql variable and let psql quote it via the
    // :'metadata' substitution instead of hand-escaping single quotes into the
    // SQL text. Same upsert effect, no string-interpolation injection surface.
    let sql = "INSERT INTO settings (key,value,updated_at) VALUES ('latest_backup_metadata',:'metadata',now()) ON CONFLICT (key) DO UPDATE SET value=EXCLUDED.value, updated_at=now();";
    let mut args = compose_args(root, false);
    args.extend([
        "exec".into(),
        "-T".into(),
        "postgres".into(),
        "psql".into(),
        "-U".into(),
        user,
        "-d".into(),
        db,
        "-v".into(),
        format!("metadata={metadata}"),
        "-c".into(),
        sql.into(),
    ]);
    let status = Command::new("docker")
        .current_dir(root)
        .args(args)
        .status()
        .context("failed to run docker compose exec psql")?;
    if !status.success() {
        bail!("recording backup metadata failed with {status}");
    }
    Ok(())
}

pub(crate) fn restore(root: &Path, backup_dir: &Path) -> anyhow::Result<()> {
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
        .envs(script_compose_env(root, false))
        .arg(script)
        .arg(backup_dir)
        .status()?;
    if !status.success() {
        bail!("restore failed with {status}");
    }
    Ok(())
}

pub(crate) fn restore_preflight(root: &Path, backup_dir: &Path) -> anyhow::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prod_backup_command_carries_prod_compose_file_and_env_file() {
        let root = std::env::temp_dir().join(format!(
            "hostlet-cli-backup-scriptenv-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".env"), "POSTGRES_PASSWORD=secret\n").unwrap();

        // Backup/restore always run in production mode (dev=false).
        let env: std::collections::BTreeMap<String, String> =
            script_compose_env(&root, false).into_iter().collect();

        let expected_compose = root
            .join("infra/docker-compose.prod.yml")
            .display()
            .to_string();
        assert_eq!(env.get("HOSTLET_COMPOSE_FILE"), Some(&expected_compose));

        let expected_env_file = root.join(".env").display().to_string();
        assert_eq!(
            env.get("HOSTLET_COMPOSE_ENV_FILE"),
            Some(&expected_env_file)
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn script_compose_env_omits_env_file_when_missing() {
        let root = std::env::temp_dir().join(format!(
            "hostlet-cli-backup-scriptenv-absent-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let env: std::collections::BTreeMap<String, String> =
            script_compose_env(&root, false).into_iter().collect();

        assert!(env.contains_key("HOSTLET_COMPOSE_FILE"));
        assert!(
            !env.contains_key("HOSTLET_COMPOSE_ENV_FILE"),
            "env file override must be absent when .env does not exist"
        );

        let _ = fs::remove_dir_all(&root);
    }
}
