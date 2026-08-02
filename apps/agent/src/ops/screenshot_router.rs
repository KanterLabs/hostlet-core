use super::{
    capture_url::{validate_internal_capture_url, verify_local_caddy_route},
    reported_deployment_failure, screenshot_failure_reason, Config,
    SCREENSHOT_CONTAINER_OUTPUT_PATH,
};
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ScreenshotRouterTarget {
    pub(crate) host: String,
    pub(crate) port: u16,
}

/// Resolve and verify the local-router target used by an internal screenshot.
/// The signed URL remains unchanged; this target only supplies Docker-side
/// host resolution and the router listener port.
pub(super) async fn build_screenshot_router_target(
    cfg: &Config,
    capture_url: &str,
) -> anyhow::Result<Option<ScreenshotRouterTarget>> {
    let Some(router_config) = &cfg.screenshot_router else {
        return Ok(None);
    };
    let Some(router) = cfg.local_router.as_ref() else {
        let err =
            anyhow::anyhow!("internal screenshot routing requires a configured local Caddy router");
        tracing::warn!(error = %err, "screenshot routing validation failed");
        return Err(reported_deployment_failure(
            screenshot_failure_reason(&err).to_string(),
        ));
    };
    let host = match validate_internal_capture_url(capture_url, &router_config.base_domain) {
        Ok(host) => host,
        Err(err) => {
            let err = err.context("internal screenshot routing target validation failed");
            tracing::warn!(
                error = %format!("{err:#}"),
                "screenshot routing validation failed"
            );
            return Err(reported_deployment_failure(
                screenshot_failure_reason(&err).to_string(),
            ));
        }
    };
    if let Err(err) = verify_local_caddy_route(&router.snippets_dir, &host).await {
        tracing::warn!(
            error = %format!("{err:#}"),
            host = %host,
            "screenshot routing validation failed"
        );
        return Err(reported_deployment_failure(
            screenshot_failure_reason(&err).to_string(),
        ));
    }
    Ok(Some(ScreenshotRouterTarget {
        host,
        port: router_config.port,
    }))
}

pub(super) fn screenshot_create_args_with_router(
    container_name: &str,
    size_env: &str,
    image: &str,
    capture_url: &str,
    browser_smoke: bool,
    screenshot_router_target: Option<&ScreenshotRouterTarget>,
) -> Vec<String> {
    let mut args = [
        "create",
        "--name",
        container_name,
        "--network",
        "host",
        "--security-opt",
        "no-new-privileges:true",
        "--cap-drop",
        "ALL",
        "--memory",
        "512m",
        "--cpus",
        "1",
        "--tmpfs",
        "/tmp:rw,nosuid,size=256m",
        "-e",
        size_env,
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    if browser_smoke {
        args.extend(["-e".to_string(), "HOSTLET_BROWSER_SMOKE=1".to_string()]);
    }
    if let Some(target) = screenshot_router_target {
        args.extend([
            "--add-host".to_string(),
            format!("{}:127.0.0.1", target.host),
            "-e".to_string(),
            format!("HOSTLET_SCREENSHOT_ROUTER_PORT={}", target.port),
        ]);
    }
    args.extend([
        image.to_string(),
        capture_url.to_string(),
        SCREENSHOT_CONTAINER_OUTPUT_PATH.to_string(),
    ]);
    args
}
