use super::*;

pub(crate) fn compose_up(root: &Path, tunnel: bool, dev: bool) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    let mut args = compose_args(dev);
    if tunnel && !dev {
        args.extend(["--profile".into(), "tunnel".into()]);
    }
    if dev {
        args.extend(["up".into(), "-d".into(), "--build".into()]);
        return run_passthrough(root, "docker", &args);
    }

    let mut pull_args = args.clone();
    pull_args.push("pull".into());
    run_passthrough(root, "docker", &pull_args)?;

    args.extend(["up".into(), "-d".into(), "--no-build".into()]);
    run_passthrough(root, "docker", &args)
}

pub(crate) fn compose_down(root: &Path, dev: bool) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    let mut args = compose_args(dev);
    args.push("down".into());
    run_passthrough(root, "docker", &args)
}

pub(crate) fn compose_logs(root: &Path, dev: bool, services: &[String]) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    let mut args = compose_args(dev);
    args.extend(["logs".into(), "-f".into()]);
    args.extend(services.iter().cloned());
    run_passthrough(root, "docker", &args)
}
