use super::*;

pub(super) async fn ensure_stable_named_volumes(
    cfg: &Config,
    deployment_id: Uuid,
    compose_text: &str,
    stable_project: &str,
) -> anyhow::Result<()> {
    for volume_name in compose_named_volumes(compose_text)? {
        let stable_volume = format!("{stable_project}_{volume_name}");
        let project_label = format!("com.docker.compose.project={stable_project}");
        let volume_label = format!("com.docker.compose.volume={volume_name}");
        run_log(
            cfg,
            deployment_id,
            "docker",
            &[
                "volume",
                "create",
                "--label",
                &project_label,
                "--label",
                &volume_label,
                &stable_volume,
            ],
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_volumes_include_web_only_storage() {
        let compose = "services:\n  web:\n    image: alpine\n    volumes:\n      - app-data:/data\n  cache:\n    image: redis\nvolumes:\n  app-data:\n  cache-data:\n";
        assert_eq!(
            compose_named_volumes(compose).unwrap(),
            vec!["app-data", "cache-data"]
        );
    }
}
