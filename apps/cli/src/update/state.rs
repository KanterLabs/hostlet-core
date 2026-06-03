//! Update apply/rollback state: pre-update backups, the Compose/.env snapshot
//! saved before a binary swap, and restoring that snapshot on rollback.

use super::*;

/// Files snapshotted into (and restored from) the rollback state directory.
const STATE_FILES: [&str; 3] = [
    ".env",
    "infra/docker-compose.yml",
    "infra/docker-compose.prod.yml",
];

/// Returns the lexically-greatest entry in `dir` whose file name starts with
/// `prefix`. State and previous-binary directories are timestamp-suffixed, so
/// the greatest name is also the most recent.
fn latest_entry_with_prefix(dir: &Path, prefix: &str) -> Option<PathBuf> {
    let mut entries = fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix))
        })
        .collect::<Vec<_>>();
    entries.sort();
    entries.pop()
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
    for relative in STATE_FILES {
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
    let Some(previous_binary) = latest_entry_with_prefix(&update_dir, "hostlet.previous.") else {
        if fs::read_dir(&update_dir).is_err() {
            bail!("no update state found in {}", update_dir.display());
        }
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
    latest_entry_with_prefix(update_dir, "state-")
}

pub(crate) fn restore_update_state(root: &Path, state_dir: &Path) -> anyhow::Result<()> {
    for relative in STATE_FILES {
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
