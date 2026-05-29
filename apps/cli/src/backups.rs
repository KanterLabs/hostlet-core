use super::*;

pub(crate) fn backup(root: &Path, output: Option<PathBuf>, scheduled: bool) -> anyhow::Result<()> {
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

pub(crate) fn record_latest_backup_metadata(root: &Path) -> anyhow::Result<()> {
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

