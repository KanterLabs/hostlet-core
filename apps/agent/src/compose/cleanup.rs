use super::*;

const CLEANUP_CONTAINER_PREFIX: &str = "hostlet-app-";
const CLEANUP_IMAGE_PREFIX: &str = "hostlet/app-";

pub(crate) async fn delete_app(cfg: Config, p: Value) -> anyhow::Result<()> {
    let app_id = p
        .get("app_id")
        .and_then(Value::as_str)
        .and_then(|v| Uuid::parse_str(v).ok());
    let route_key = p
        .get("route_key")
        .and_then(Value::as_str)
        .map(app_slug)
        .unwrap_or_else(|| app_slug(p["app_id"].as_str().unwrap_or("app")));
    if let Some(app_id) = app_id {
        let stable_project = compose_project_name(app_id);
        for project in compose_projects_for_app(app_id).await? {
            if project != stable_project {
                remove_compose_project_resources(&project).await?;
            }
        }
        remove_compose_project_resources(&stable_project).await?;
    } else if let Some(project) = p.get("compose_project").and_then(Value::as_str) {
        remove_compose_project_resources(project).await?;
    }
    for container in p
        .get("containers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !valid_container_name(container) {
            bail!("refusing to remove invalid managed container name during teardown");
        }
        run_quiet_absent_ok("docker", &["rm", "-f", container], &["No such container"]).await?;
    }
    for image in p
        .get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !valid_hostlet_image(image) {
            bail!("refusing to remove invalid managed image name during teardown");
        }
        run_quiet_absent_ok("docker", &["image", "rm", "-f", image], &["No such image"]).await?;
    }
    if cfg.local_mode {
        if let Some(router) = &cfg.local_router {
            remove_local_caddy_route(router, &route_key).await?;
            run_router_reload_quiet(router).await?;
        }
    } else {
        remove_caddy_route(&route_key).await?;
        run_quiet("caddy", &["reload", "--config", "/etc/caddy/Caddyfile"]).await?;
    }
    if let Some(app_id) = app_id {
        remove_app_data_volume(app_id).await?;
    }
    Ok(())
}

pub(crate) async fn docker_cleanup_job(p: &Value) -> anyhow::Result<()> {
    let dry_run = p.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
    let keep_containers = string_set_from_array(p.get("keep_containers"));
    let keep_images = string_set_from_array(p.get("keep_images"));
    let mut stale_release_projects = HashSet::new();
    for container in hostlet_containers_all().await? {
        let project = docker_compose_project(&container).await?;
        if let Some(project) = project
            .as_deref()
            .filter(|p| p.starts_with("hostlet-release-"))
        {
            if !keep_containers.contains(&container) {
                stale_release_projects.insert(project.to_string());
            }
            continue;
        }
        if cleanup_should_remove_container(&container, &keep_containers, project.is_some())?
            && !dry_run
        {
            run_quiet_absent_ok("docker", &["rm", "-f", &container], &["No such container"])
                .await?;
        }
    }
    if !dry_run {
        for project in stale_release_projects {
            remove_compose_project_resources(&project).await?;
        }
    }
    for image in hostlet_images().await? {
        if cleanup_should_remove_image(&image, &keep_images)? && !dry_run {
            run_quiet_absent_ok("docker", &["image", "rm", "-f", &image], &["No such image"])
                .await?;
        }
    }
    Ok(())
}

pub(crate) fn cleanup_should_remove_container(
    container: &str,
    keep: &HashSet<String>,
    compose: bool,
) -> anyhow::Result<bool> {
    if !container.starts_with(CLEANUP_CONTAINER_PREFIX) || keep.contains(container) || compose {
        return Ok(false);
    }
    if !valid_container_name(container) {
        bail!("refusing to clean invalid managed container name");
    }
    Ok(true)
}

pub(crate) fn cleanup_should_remove_image(
    image: &str,
    keep: &HashSet<String>,
) -> anyhow::Result<bool> {
    if !image.starts_with(CLEANUP_IMAGE_PREFIX) || keep.contains(image) {
        return Ok(false);
    }
    if !valid_hostlet_image(image) {
        bail!("refusing to clean invalid managed image name");
    }
    Ok(true)
}
