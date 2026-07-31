use super::*;
use std::sync::OnceLock;

static ROUTE_WRITE_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

pub(crate) fn route_write_lock() -> &'static tokio::sync::Mutex<()> {
    ROUTE_WRITE_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Build a unique same-directory temp path so atomic rename never crosses a
/// filesystem boundary. PID plus UUID prevents concurrent route writers from
/// sharing and clobbering an in-flight file.
pub(crate) fn route_temp_path(target: &Path) -> PathBuf {
    target.with_extension(format!(
        "caddy.tmp-{}-{}",
        std::process::id(),
        Uuid::new_v4()
    ))
}

pub(crate) async fn write_route_file(target: &Path, contents: &str) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    let tmp = route_temp_path(target);
    // `create_new` refuses to open a path that already exists, so even in the
    // astronomically unlikely event of a UUID collision we never truncate a
    // temp file a concurrent writer is still using.
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .await?;
    file.write_all(contents.as_bytes()).await?;
    file.flush().await?;
    drop(file);
    tokio::fs::rename(&tmp, target).await?;
    Ok(())
}

pub(crate) async fn restore_route_file(
    target: &Path,
    previous: Option<Vec<u8>>,
) -> anyhow::Result<()> {
    if let Some(contents) = previous {
        tokio::fs::write(target, contents).await?;
    } else {
        match tokio::fs::remove_file(target).await {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

pub(crate) async fn remove_local_caddy_route(
    router: &LocalRouter,
    app: &str,
) -> anyhow::Result<()> {
    let target = router.snippets_dir.join(format!("{app}.caddy"));
    match tokio::fs::remove_file(target).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

pub(crate) async fn ensure_no_conflicting_route(
    dir: &Path,
    target: &Path,
    domain: &str,
) -> anyhow::Result<()> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path == target || path.extension().and_then(|value| value.to_str()) != Some("caddy") {
            continue;
        }
        let Ok(contents) = tokio::fs::read_to_string(&path).await else {
            continue;
        };
        if route_domain(&contents).is_some_and(|existing| existing == domain) {
            bail!("another Hostlet route already uses domain {domain}");
        }
    }
    Ok(())
}

pub(crate) fn route_domain(contents: &str) -> Option<&str> {
    for line in contents.lines().map(str::trim) {
        if let Some(domain) = line.strip_prefix("# hostlet-domain:") {
            return Some(domain.trim());
        }
        if let Some((_, domain)) = line.split_once(" host ") {
            return Some(domain.trim());
        }
        if let Some(domain) = line.strip_suffix(" {") {
            return Some(domain.trim());
        }
    }
    None
}

pub(crate) async fn run_router_reload(
    cfg: &Config,
    deployment_id: Uuid,
    router: &LocalRouter,
) -> anyhow::Result<()> {
    let Some((bin, args)) = router.reload_command.split_first() else {
        return Ok(());
    };
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    run_log(cfg, deployment_id, bin, &args).await
}

pub(crate) async fn run_router_reload_quiet(router: &LocalRouter) -> anyhow::Result<()> {
    let Some((bin, args)) = router.reload_command.split_first() else {
        return Ok(());
    };
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    run_quiet(bin, &args).await
}
