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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RouteKind {
    Single,
    Split,
}

#[derive(Debug, Eq, PartialEq)]
struct RouteVersion {
    deployment_id: Uuid,
    generation: i64,
    kind: RouteKind,
}

fn route_version(contents: &str) -> anyhow::Result<Option<RouteVersion>> {
    let mut deployment_id = None;
    let mut generation = None;
    let mut kind = None;
    for line in contents.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("# hostlet-deployment-id:") {
            if deployment_id.replace(value.trim()).is_some() {
                bail!("existing Hostlet route has duplicate deployment metadata");
            }
        }
        if let Some(value) = line.strip_prefix("# hostlet-route-generation:") {
            if generation.replace(value.trim()).is_some() {
                bail!("existing Hostlet route has duplicate generation metadata");
            }
        }
        if let Some(value) = line.strip_prefix("# hostlet-route-kind:") {
            if kind.replace(value.trim()).is_some() {
                bail!("existing Hostlet route has duplicate route-kind metadata");
            }
        }
    }
    match (deployment_id, generation, kind) {
        (None, None, None) => Ok(None),
        (Some(deployment_id), Some(generation), kind) => {
            let deployment_id = Uuid::parse_str(deployment_id)
                .context("existing Hostlet route has an invalid deployment id")?;
            let generation = generation
                .parse::<i64>()
                .ok()
                .filter(|generation| *generation >= 0)
                .context("existing Hostlet route has an invalid generation")?;
            let kind = match kind {
                Some("single") => RouteKind::Single,
                Some("split") => RouteKind::Split,
                Some(_) => bail!("existing Hostlet route has an invalid route kind"),
                None if route_looks_split(contents) => RouteKind::Split,
                None => RouteKind::Single,
            };
            Ok(Some(RouteVersion {
                deployment_id,
                generation,
                kind,
            }))
        }
        _ => bail!("existing Hostlet route has incomplete version metadata"),
    }
}

fn route_looks_split(contents: &str) -> bool {
    contents.lines().map(str::trim).any(|line| {
        line == "@hostletWebsocket header Connection *Upgrade*"
            || (line.starts_with('@') && line.ends_with("Websocket {"))
    })
}

/// Fence every write while the per-route lock is held. A newer activation may
/// replace an older route, and the current deployment may repair its own route,
/// but stale, conflicting, or unversioned writers cannot overwrite a versioned
/// route that another activation already installed.
pub(crate) fn ensure_route_write_is_current(
    previous: Option<&[u8]>,
    deployment_id: Uuid,
    generation: Option<i64>,
    requested_kind: RouteKind,
) -> anyhow::Result<()> {
    if generation.is_some_and(|generation| generation < 0) {
        bail!("route generation must be non-negative");
    }
    let Some(previous) = previous else {
        return Ok(());
    };
    let contents = std::str::from_utf8(previous).context("existing Hostlet route is not UTF-8")?;
    let Some(existing) = route_version(contents)? else {
        return Ok(());
    };
    let Some(generation) = generation else {
        bail!("an unversioned route write cannot replace a versioned route");
    };
    if generation < existing.generation {
        bail!(
            "stale route generation {generation} cannot replace generation {}",
            existing.generation
        );
    }
    if generation == existing.generation && deployment_id != existing.deployment_id {
        bail!("route generation is already owned by another deployment");
    }
    if generation == existing.generation
        && deployment_id == existing.deployment_id
        && existing.kind == RouteKind::Split
        && requested_kind == RouteKind::Single
    {
        bail!("a split route cannot be downgraded within the same deployment generation");
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    const CURRENT: Uuid = Uuid::from_u128(1);
    const OTHER: Uuid = Uuid::from_u128(2);

    fn versioned(deployment_id: Uuid, generation: i64) -> Vec<u8> {
        format!(
            "# hostlet-deployment-id: {deployment_id}\n# hostlet-route-generation: {generation}\n"
        )
        .into_bytes()
    }

    #[test]
    fn route_write_fence_allows_first_legacy_and_newer_writes() {
        assert!(ensure_route_write_is_current(None, CURRENT, Some(1), RouteKind::Single).is_ok());
        assert!(ensure_route_write_is_current(
            Some(b"legacy route"),
            CURRENT,
            Some(1),
            RouteKind::Single
        )
        .is_ok());
        assert!(ensure_route_write_is_current(
            Some(&versioned(CURRENT, 1)),
            OTHER,
            Some(2),
            RouteKind::Single
        )
        .is_ok());
    }

    #[test]
    fn route_write_fence_allows_current_deployment_repair() {
        assert!(ensure_route_write_is_current(
            Some(&versioned(CURRENT, 7)),
            CURRENT,
            Some(7),
            RouteKind::Single
        )
        .is_ok());
    }

    #[test]
    fn route_write_fence_rejects_stale_conflicting_and_unversioned_writes() {
        let current = versioned(CURRENT, 7);
        assert!(
            ensure_route_write_is_current(Some(&current), CURRENT, Some(6), RouteKind::Single)
                .is_err()
        );
        assert!(
            ensure_route_write_is_current(Some(&current), OTHER, Some(7), RouteKind::Single)
                .is_err()
        );
        assert!(
            ensure_route_write_is_current(Some(&current), CURRENT, None, RouteKind::Single)
                .is_err()
        );
    }

    #[test]
    fn route_write_fence_rejects_malformed_version_metadata() {
        assert!(ensure_route_write_is_current(
            Some(b"# hostlet-route-generation: 7\n"),
            CURRENT,
            Some(7),
            RouteKind::Single
        )
        .is_err());
        assert!(ensure_route_write_is_current(
            Some(b"# hostlet-deployment-id: nope\n# hostlet-route-generation: 7\n"),
            CURRENT,
            Some(7),
            RouteKind::Single
        )
        .is_err());
    }

    #[test]
    fn route_write_fence_rejects_same_generation_split_downgrade() {
        let split = format!(
            "# hostlet-deployment-id: {CURRENT}\n# hostlet-route-generation: 7\n@hostletWebsocket header Connection *Upgrade*\n"
        );
        assert!(ensure_route_write_is_current(
            Some(split.as_bytes()),
            CURRENT,
            Some(7),
            RouteKind::Single
        )
        .is_err());
        assert!(ensure_route_write_is_current(
            Some(split.as_bytes()),
            CURRENT,
            Some(7),
            RouteKind::Split
        )
        .is_ok());
    }
}
