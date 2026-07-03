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
fn screenshot_create_args_use_container_copy_path_without_host_bind() {
    let args = screenshot_create_args(
        "hostlet-screenshot-job",
        "HOSTLET_SCREENSHOT_SIZE=1280x720",
        "local/hostlet-screenshotter:test",
        "https://demo.example.com/",
    );

    assert_eq!(args.first().map(String::as_str), Some("create"));
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--name", "hostlet-screenshot-job"]));
    assert!(!args.iter().any(|arg| arg == "-v"));
    assert!(args
        .iter()
        .any(|arg| arg == SCREENSHOT_CONTAINER_OUTPUT_PATH));
    assert!(SCREENSHOT_CONTAINER_OUTPUT_PATH.ends_with(".webp"));
    assert_eq!(SCREENSHOT_CONTENT_TYPE, "image/webp");
    assert!(!SCREENSHOT_CONTAINER_OUTPUT_PATH.starts_with("/tmp/"));
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
