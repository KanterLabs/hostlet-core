use super::capture_url::{
    validate_capture_url, validate_internal_capture_url, verify_local_caddy_route,
};
use super::*;

#[test]
fn validate_capture_url_accepts_public_targets() {
    assert!(validate_capture_url("https://demo.example.com/").is_ok());
    assert!(validate_capture_url("https://demo.example.com:8443/").is_ok());
    assert!(validate_capture_url("http://172.32.0.1/").is_ok());
    assert!(validate_capture_url("http://100.128.0.1/").is_ok());
    assert!(validate_capture_url("http://9.9.9.9/").is_ok());
}

#[test]
fn validate_capture_url_rejects_non_http_schemes() {
    assert!(validate_capture_url("file:///etc/passwd").is_err());
}

#[test]
fn validate_capture_url_rejects_private_and_local_targets() {
    for value in [
        "http://localhost:3000/",
        "http://127.0.0.1:8080/",
        "http://10.0.0.5/",
        "http://172.16.0.1/",
        "http://172.31.255.1/",
        "http://192.168.1.10/",
        "http://169.254.169.254/latest/meta-data/",
        "http://100.64.1.1/",
        "http://0.0.0.0/",
        "http://[::1]/",
        "http://[fe80::1]/",
        "http://[fd00::1]/",
        "http://[::ffff:127.0.0.1]/",
        "http://metadata/",
        "http://LOCALHOST/",
    ] {
        assert!(
            validate_capture_url(value).is_err(),
            "expected rejection for {value}"
        );
    }
}

#[test]
fn internal_capture_url_requires_one_canonical_https_tenant_label() {
    assert_eq!(
        validate_internal_capture_url("https://TENANT.example.com/", "example.com").unwrap(),
        "tenant.example.com"
    );
    assert_eq!(
        validate_internal_capture_url("https://tenant.example.com:443/path", "example.com")
            .unwrap(),
        "tenant.example.com"
    );

    for value in [
        "http://tenant.example.com/",
        "https://example.com/",
        "https://nested.tenant.example.com/",
        "https://user:password@tenant.example.com/",
        "https://@tenant.example.com/",
        "https://tenant.example.com:8443/",
        "https://127.0.0.1/",
        "https://tenant.example.com./",
        "https://tenant.other.example.com/",
        "https://tenant.example.com.evil/",
    ] {
        assert!(
            validate_internal_capture_url(value, "example.com").is_err(),
            "expected internal route rejection for {value}"
        );
    }
}

#[test]
fn screenshot_create_args_use_container_copy_path_without_host_bind() {
    let args = screenshot_create_args(
        "hostlet-screenshot-job",
        "HOSTLET_SCREENSHOT_SIZE=1280x720",
        "local/hostlet-screenshotter:test",
        "https://demo.example.com/",
        false,
    );

    assert_eq!(args.first().map(String::as_str), Some("create"));
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--name", "hostlet-screenshot-job"]));
    assert!(!args.iter().any(|arg| arg == "-v"));
    assert!(args
        .iter()
        .any(|arg| arg == SCREENSHOT_CONTAINER_OUTPUT_PATH));
    assert!(!args.iter().any(|arg| arg == "--add-host"));
    assert!(!args
        .iter()
        .any(|arg| arg == "HOSTLET_SCREENSHOT_ROUTER_PORT=8081"));
    assert!(SCREENSHOT_CONTAINER_OUTPUT_PATH.ends_with(".webp"));
    assert_eq!(SCREENSHOT_CONTENT_TYPE, "image/webp");
    assert!(!SCREENSHOT_CONTAINER_OUTPUT_PATH.starts_with("/tmp/"));
}

#[test]
fn browser_smoke_create_args_enable_runtime_probe() {
    let args = screenshot_create_args(
        "hostlet-browser-job",
        "HOSTLET_SCREENSHOT_SIZE=1280x720",
        "local/hostlet-screenshotter:test",
        "https://demo.example.com/",
        true,
    );
    assert!(args
        .windows(2)
        .any(|pair| pair == ["-e", "HOSTLET_BROWSER_SMOKE=1"]));
}

#[test]
fn screenshot_router_args_add_only_the_validated_host_and_port() {
    let target = ScreenshotRouterTarget {
        host: "tenant.example.com".into(),
        port: 8081,
    };
    let args = screenshot_create_args_with_router(
        "hostlet-router-job",
        "HOSTLET_SCREENSHOT_SIZE=1280x720",
        "local/hostlet-screenshotter:test",
        "https://TENANT.example.com/?signed=1",
        false,
        Some(&target),
    );

    assert!(args
        .windows(2)
        .any(|pair| pair == ["--add-host", "tenant.example.com:127.0.0.1"]));
    assert!(args
        .windows(2)
        .any(|pair| pair == ["-e", "HOSTLET_SCREENSHOT_ROUTER_PORT=8081"]));
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--security-opt", "no-new-privileges:true"]));
    assert!(args.windows(2).any(|pair| pair == ["--cap-drop", "ALL"]));
    assert_eq!(
        args.last().map(String::as_str),
        Some("/app/hostlet-screenshot.webp")
    );
    assert!(args
        .iter()
        .any(|arg| arg == "https://TENANT.example.com/?signed=1"));
}

#[tokio::test]
async fn local_caddy_route_verification_requires_an_exact_domain_comment() {
    let dir = std::env::temp_dir().join(format!("hostlet-screenshot-router-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    tokio::fs::write(
        dir.join("other.caddy"),
        "# hostlet-domain: tenant.example.com.evil\n",
    )
    .await
    .unwrap();
    assert!(verify_local_caddy_route(&dir, "tenant.example.com")
        .await
        .is_err());

    tokio::fs::write(
        dir.join("tenant.caddy"),
        "# hostlet-domain: tenant.example.com\nreverse_proxy 127.0.0.1:3000\n",
    )
    .await
    .unwrap();
    assert!(verify_local_caddy_route(&dir, "tenant.example.com")
        .await
        .is_ok());
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[test]
fn screenshot_failure_reason_maps_known_categories() {
    let cases = [
        (
            "blocked request to http://10.0.0.1 (resolves to a private or local address)",
            SCREENSHOT_ERR_BLOCKED,
        ),
        (
            "capture_url must use a public hostname",
            SCREENSHOT_ERR_BLOCKED,
        ),
        (
            "screenshotter exited with exit status: 1: page.goto: net::ERR_BLOCKED_BY_CLIENT",
            SCREENSHOT_ERR_BLOCKED,
        ),
        (
            "screenshotter exited with exit status: 1: page.goto: Timeout 15000ms exceeded",
            SCREENSHOT_ERR_TIMEOUT,
        ),
        ("docker timed out after 45 seconds", SCREENSHOT_ERR_TIMEOUT),
        (
            "screenshotter exited with exit status: 1: page.goto: net::ERR_CONNECTION_REFUSED",
            SCREENSHOT_ERR_SITE,
        ),
        (
            "capture rejected: navigation returned HTTP 503",
            SCREENSHOT_ERR_SITE,
        ),
        (
            "capture rejected: Cloudflare security challenge (cf-mitigated: challenge)",
            SCREENSHOT_ERR_CHALLENGE,
        ),
        (
            "too many redirects while validating screenshot target",
            SCREENSHOT_ERR_SITE,
        ),
        (
            "screenshotter container create failed with exit status: 125: no such image",
            SCREENSHOT_ERR_SERVICE,
        ),
        (
            "screenshotter did not produce an image",
            SCREENSHOT_ERR_SERVICE,
        ),
        (
            "browser smoke rejected: uncaught page error: startup exploded",
            SCREENSHOT_ERR_RUNTIME,
        ),
        (
            "browser smoke rejected: page remained blank or near-blank",
            SCREENSHOT_ERR_BLANK,
        ),
        (
            "capture rejected: page failed the visual-readiness probe after retry",
            SCREENSHOT_ERR_BLANK,
        ),
        (
            "internal screenshot routing target validation failed: capture_url must not include userinfo",
            SCREENSHOT_ERR_ROUTER,
        ),
        (
            "no installed local Caddy snippet matches # hostlet-domain: tenant.example.com",
            SCREENSHOT_ERR_ROUTER,
        ),
    ];
    for (message, expected) in cases {
        assert_eq!(
            screenshot_failure_reason(&anyhow::anyhow!("{message}")),
            expected,
            "unexpected category for {message}"
        );
    }
}

#[test]
fn screenshot_container_name_is_job_scoped() {
    let job_id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();

    assert_eq!(
        screenshot_container_name(job_id),
        "hostlet-screenshot-11111111-2222-3333-4444-555555555555"
    );
}
