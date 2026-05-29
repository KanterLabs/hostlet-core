use super::*;

pub(crate) async fn doctor(root: &Path) -> anyhow::Result<()> {
    ensure_repo_root(root)?;
    let env_path = root.join(".env");
    let env = read_env_file(&env_path).unwrap_or_default();
    let client = http_client()?;
    println!("Hostlet {}", env!("CARGO_PKG_VERSION"));
    check("Docker", command_ok("docker", &["version"]));
    check(
        "Docker Compose",
        command_ok("docker", &["compose", "version"]),
    );
    check(
        "Compose config",
        compose_config_ok(root, false) || compose_config_ok(root, true),
    );
    check(
        "Compose services",
        compose_services_running(root, false) || compose_services_running(root, true),
    );
    check(".env exists", env_path.exists());
    for key in [
        "PUBLIC_WEB_URL",
        "PUBLIC_API_URL",
        "ENCRYPTION_KEY",
        "SESSION_SECRET",
        "JOB_SIGNING_SECRET",
        "LOCAL_AGENT_TOKEN",
        "HOSTLET_SETUP_TOKEN",
        "HOSTLET_ALLOWED_GITHUB_LOGINS",
        "GITHUB_CLIENT_ID",
    ] {
        check(
            &format!("{key} set"),
            env.get(key).is_some_and(|v| !v.trim().is_empty()),
        );
    }
    check_url(&client, "Web", env.get("PUBLIC_WEB_URL")).await;
    check_url(
        &client,
        "API health",
        env.get("PUBLIC_API_URL")
            .map(|v| format!("{}/health", v.trim_end_matches('/')))
            .as_ref(),
    )
    .await;
    print_operator_status(&client, &env).await;
    if let (Some(token), Some(zone_id)) = (
        env.get("CLOUDFLARE_API_TOKEN"),
        env.get("CLOUDFLARE_ZONE_ID"),
    ) {
        check(
            "Cloudflare zone access",
            cloudflare_zone_ok(&client, token, zone_id)
                .await
                .unwrap_or(false),
        );
    }
    check("Disk space", disk_space_ok(root));
    check("Recent backup", latest_backup(root).is_some());
    match latest_release(&client).await {
        Ok(release) => check(
            "Hostlet update",
            !version_is_newer(
                env!("CARGO_PKG_VERSION"),
                release.version.trim_start_matches('v'),
            ),
        ),
        Err(_) => println!("Hostlet update             unknown"),
    }
    Ok(())
}

pub(crate) async fn check_url(client: &reqwest::Client, label: &str, url: Option<&String>) {
    let Some(url) = url else {
        check(label, false);
        return;
    };
    let ok = client
        .get(url)
        .send()
        .await
        .map(|r| r.status().is_success() || r.status().is_redirection())
        .unwrap_or(false);
    check(label, ok);
}

pub(crate) async fn cloudflare_zone_ok(
    client: &reqwest::Client,
    token: &str,
    zone_id: &str,
) -> anyhow::Result<bool> {
    Ok(client
        .get(format!(
            "https://api.cloudflare.com/client/v4/zones/{zone_id}"
        ))
        .bearer_auth(token)
        .send()
        .await?
        .status()
        .is_success())
}

pub(crate) fn http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent("Hostlet CLI")
        .build()
        .context("failed to build HTTP client")
}

