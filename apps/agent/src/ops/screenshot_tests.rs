use super::*;

#[test]
fn validate_capture_url_accepts_http_and_https() {
    assert!(validate_capture_url("http://localhost:3000/").is_ok());
    assert!(validate_capture_url("https://demo.example.com/").is_ok());
}

#[test]
fn validate_capture_url_rejects_non_http_schemes() {
    assert!(validate_capture_url("file:///etc/passwd").is_err());
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
    assert!(!SCREENSHOT_CONTAINER_OUTPUT_PATH.starts_with("/tmp/"));
}

#[test]
fn screenshot_container_name_is_job_scoped() {
    let job_id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();

    assert_eq!(
        screenshot_container_name(job_id),
        "hostlet-screenshot-11111111-2222-3333-4444-555555555555"
    );
}
