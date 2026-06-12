//! Docker resource helpers: inspecting published ports and compose containers,
//! running commands in a working directory, and creating/removing the per-app
//! data volume and compose project resources.

use super::super::*;

pub(crate) async fn docker_published_port(
    container: &str,
    container_port: u16,
) -> anyhow::Result<u16> {
    let target = format!("{container_port}/tcp");
    let output = command_output(
        "docker",
        &["port", container, &target],
        Duration::from_secs(15),
    )
    .await
    .context("failed to inspect Docker published port")?;
    if !output.status.success() {
        bail!("could not inspect Docker published port");
    }
    let stdout =
        String::from_utf8(output.stdout).context("Docker port output was not valid UTF-8")?;
    stdout
        .lines()
        .filter_map(|line| line.rsplit(':').next())
        .filter_map(|port| port.trim().parse::<u16>().ok())
        .next()
        .context("Docker did not report a published port")
}

pub(crate) async fn compose_service_container(
    dir: &Path,
    project: &str,
    compose_file: &Path,
    override_file: &Path,
    service: &str,
) -> anyhow::Result<String> {
    let output = command_output_in_dir(
        dir,
        "docker",
        &[
            "compose",
            "-p",
            project,
            "-f",
            compose_file.to_str().unwrap(),
            "-f",
            override_file.to_str().unwrap(),
            "ps",
            "-q",
            service,
        ],
        Duration::from_secs(30),
    )
    .await?;
    if !output.status.success() {
        bail!("docker compose ps failed");
    }
    let id = String::from_utf8(output.stdout)?.trim().to_string();
    if id.is_empty() {
        bail!("compose web service did not create a container");
    }
    let name_output = command_output(
        "docker",
        &["inspect", "-f", "{{.Name}}", &id],
        Duration::from_secs(15),
    )
    .await?;
    if !name_output.status.success() {
        bail!("failed to inspect compose web container");
    }
    let name = String::from_utf8(name_output.stdout)?
        .trim()
        .trim_start_matches('/')
        .to_string();
    if !valid_container_name(&name) {
        bail!("compose web container name is not Hostlet-managed");
    }
    Ok(name)
}

pub(crate) async fn command_output_in_dir(
    dir: &Path,
    bin: &str,
    args: &[&str],
    timeout: Duration,
) -> anyhow::Result<Output> {
    let mut cmd = Command::new(bin);
    cmd.current_dir(dir).args(args).kill_on_drop(true);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(output) => output.with_context(|| format!("failed to start {bin}")),
        Err(_) => bail!("{bin} timed out after {} seconds", timeout.as_secs()),
    }
}

pub(crate) fn app_data_volume(app_id: Uuid) -> String {
    format!("hostlet-app-data-{app_id}")
}

pub(crate) async fn ensure_app_data_volume(
    cfg: &Config,
    deployment_id: Uuid,
    volume: &str,
) -> anyhow::Result<()> {
    run_log(cfg, deployment_id, "docker", &["volume", "create", volume]).await?;
    let volume_mount = format!("{volume}:/data");
    run_log(
        cfg,
        deployment_id,
        "docker",
        &[
            "run",
            "--rm",
            "-v",
            &volume_mount,
            "alpine:3.20",
            "sh",
            "-lc",
            "chmod 0777 /data",
        ],
    )
    .await
}

pub(crate) async fn remove_app_data_volume(app_id: Uuid) -> anyhow::Result<()> {
    let volume = app_data_volume(app_id);
    run_quiet_absent_ok(
        "docker",
        &["volume", "rm", "-f", &volume],
        &["No such volume"],
    )
    .await
}

pub(crate) async fn remove_compose_project_resources(project: &str) -> anyhow::Result<()> {
    if !valid_compose_project_name(project) {
        bail!("refusing to remove invalid compose project");
    }
    let containers = docker_names_by_label(
        "ps",
        &[
            "-a",
            "--filter",
            &format!("label=com.docker.compose.project={project}"),
        ],
        "{{.Names}}",
    )
    .await?;
    for container in containers {
        if valid_container_name(&container) {
            run_quiet_absent_ok("docker", &["rm", "-f", &container], &["No such container"])
                .await?;
        }
    }
    let volumes = docker_names_by_label(
        "volume",
        &[
            "ls",
            "--filter",
            &format!("label=com.docker.compose.project={project}"),
        ],
        "{{.Name}}",
    )
    .await?;
    for volume in volumes {
        if valid_compose_volume_name(&volume) {
            run_quiet_absent_ok(
                "docker",
                &["volume", "rm", "-f", &volume],
                &["No such volume"],
            )
            .await?;
        }
    }
    // Compose `up` always creates the project's default network; every deleted
    // or torn-down compose app would otherwise leak one bridge network until
    // Docker's address pool is exhausted. Containers are removed above first or
    // `network rm` fails with "has active endpoints".
    let networks = docker_names_by_label(
        "network",
        &[
            "ls",
            "--filter",
            &format!("label=com.docker.compose.project={project}"),
        ],
        "{{.Name}}",
    )
    .await?;
    for network in networks {
        if compose_network_belongs_to_project(&network, project) {
            run_quiet_absent_ok(
                "docker",
                &["network", "rm", &network],
                &["No such network", "not found"],
            )
            .await?;
        }
    }
    Ok(())
}

/// The label filter scopes the listing, but only names under the validated
/// project prefix may be removed (e.g. `<project>_default`).
fn compose_network_belongs_to_project(network: &str, project: &str) -> bool {
    network
        .strip_prefix(project)
        .is_some_and(|rest| rest.starts_with('_'))
}

pub(crate) async fn docker_names_by_label(
    cmd: &str,
    args: &[&str],
    format: &str,
) -> anyhow::Result<Vec<String>> {
    let mut full = vec![cmd];
    full.extend(args);
    full.push("--format");
    full.push(format);
    let output = command_output("docker", &full, Duration::from_secs(30)).await?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::compose_network_belongs_to_project;

    #[test]
    fn network_default_matches_project() {
        assert!(compose_network_belongs_to_project(
            "hostlet-app-abc_default",
            "hostlet-app-abc"
        ));
    }

    #[test]
    fn network_internal_matches_project() {
        assert!(compose_network_belongs_to_project(
            "hostlet-app-abc_internal",
            "hostlet-app-abc"
        ));
    }

    #[test]
    fn network_prefix_collision_excluded() {
        // "hostlet-app-abcd_default" must NOT match project "hostlet-app-abc"
        assert!(!compose_network_belongs_to_project(
            "hostlet-app-abcd_default",
            "hostlet-app-abc"
        ));
    }

    #[test]
    fn network_exact_project_name_no_suffix_excluded() {
        assert!(!compose_network_belongs_to_project(
            "hostlet-app-abc",
            "hostlet-app-abc"
        ));
    }

    #[test]
    fn network_other_project_excluded() {
        assert!(!compose_network_belongs_to_project(
            "other_default",
            "hostlet-app-abc"
        ));
    }
}
