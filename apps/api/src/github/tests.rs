use super::{repo_inspect_failure, StatusCode};
use hostlet_contracts::{parse_github_repo, valid_commit_sha};

#[test]
fn rejects_branch_delete_zero_sha() {
    assert!(!valid_commit_sha(
        "0000000000000000000000000000000000000000"
    ));
}

#[test]
fn accepts_normal_commit_sha() {
    assert!(valid_commit_sha("0123456789abcdef0123456789abcdef01234567"));
}

#[test]
fn parses_github_repo_inputs() {
    assert_eq!(
        parse_github_repo("https://github.com/go-gitea/gitea"),
        Some("go-gitea/gitea".into())
    );
    assert_eq!(
        parse_github_repo("git@github.com:owner/repo.git"),
        Some("owner/repo".into())
    );
    assert_eq!(parse_github_repo("owner/repo"), Some("owner/repo".into()));
    assert_eq!(parse_github_repo("https://example.com/owner/repo"), None);
}

#[test]
fn repo_inspect_failure_404_gives_not_found_with_check_name_hint() {
    let (status, body) = repo_inspect_failure(Some(404));
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body.contains("not found"),
        "body should mention 'not found': {body}"
    );
    assert!(
        body.contains("owner/repo") || body.contains("reconnect"),
        "body should hint at fix: {body}"
    );
}

#[test]
fn repo_inspect_failure_401_gives_bad_gateway_with_reconnect_hint() {
    let (status, body) = repo_inspect_failure(Some(401));
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(body.contains("401"), "body should include status: {body}");
    assert!(
        body.contains("Reconnect"),
        "body should suggest reconnect: {body}"
    );
}

#[test]
fn repo_inspect_failure_403_gives_bad_gateway() {
    let (status, body) = repo_inspect_failure(Some(403));
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(body.contains("403"), "body should include status: {body}");
}

#[test]
fn repo_inspect_failure_429_gives_rate_limit_message() {
    let (status, body) = repo_inspect_failure(Some(429));
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(
        body.contains("rate-limited"),
        "body should mention rate limit: {body}"
    );
}

#[test]
fn repo_inspect_failure_other_and_none_give_generic_bad_gateway() {
    for input in [None, Some(500u16), Some(503)] {
        let (status, body) = repo_inspect_failure(input);
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            body, "GitHub repository could not be inspected",
            "unexpected body for {input:?}"
        );
    }
}
