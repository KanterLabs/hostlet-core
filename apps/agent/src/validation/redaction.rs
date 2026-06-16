//! Security helpers: HMAC job-signature verification, constant-time comparison,
//! and redaction of secrets from log lines and logged command arguments.

// HMAC job-signature verification lives in `hostlet_contracts::crypto` (shared
// with the api). Re-exported so the crate-wide `verify_signature` path used by
// `runtime.rs` resolves unchanged.
pub(crate) use hostlet_contracts::crypto::verify_signature;

pub(crate) fn redact(line: &str) -> String {
    if let Some(redacted) = redact_url_credentials(line) {
        return redacted;
    }
    let lowered = line.to_lowercase();
    let sensitive = [
        "token",
        "secret",
        "password",
        "passwd",
        "api_key",
        "apikey",
        "access_key",
        "private key",
        "authorization:",
        "bearer ",
        "credential",
    ];
    if sensitive.iter().any(|needle| lowered.contains(needle)) {
        "[redacted]".into()
    } else {
        line.into()
    }
}

/// Redacts credentials from every `https://user:pass@host` URL found on `value`.
///
/// Returns `Some(redacted)` when at least one credentialed URL was found;
/// returns `None` when the line contains no credentialed URLs, allowing the
/// caller to fall through to keyword-based redaction.
pub(crate) fn redact_url_credentials(value: &str) -> Option<String> {
    let scheme = "https://";
    let mut result = String::new();
    let mut pos = 0;
    let mut found_any = false;

    while pos < value.len() {
        let Some(scheme_offset) = value[pos..].find(scheme) else {
            break;
        };
        let scheme_start = pos + scheme_offset;
        let authority_start = scheme_start + scheme.len();

        // The authority (user:pass@host) ends at the first '/' after '://',
        // or at end-of-string for bare hosts.
        let authority_end = value[authority_start..]
            .find('/')
            .map(|i| authority_start + i)
            .unwrap_or(value.len());
        let authority = &value[authority_start..authority_end];

        if let Some(at_offset) = authority.find('@') {
            // Credential-bearing URL: redact the userinfo section.
            found_any = true;
            result.push_str(&value[pos..scheme_start]);
            result.push_str("https://[redacted]@");
            // Continue scanning after the '@'.
            pos = authority_start + at_offset + 1;
        } else {
            // No credentials in this URL; copy up to the end of the authority
            // and keep scanning for further URLs.
            result.push_str(&value[pos..authority_end]);
            pos = authority_end;
        }
    }

    if found_any {
        result.push_str(&value[pos..]);
        Some(result)
    } else {
        None
    }
}

pub(crate) fn command_args_for_log(args: &[&str]) -> Vec<String> {
    let mut output = Vec::with_capacity(args.len());
    let mut redact_next = false;
    for arg in args {
        if redact_next {
            output.push(redact_env_arg(arg));
            redact_next = false;
            continue;
        }
        if *arg == "-e" || *arg == "--env" {
            output.push((*arg).to_string());
            redact_next = true;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--env=") {
            output.push(format!("--env={}", redact_env_arg(value)));
            continue;
        }
        output.push(redact(arg));
    }
    output
}

pub(crate) fn redact_env_arg(arg: &str) -> String {
    match arg.split_once('=') {
        Some((key, _)) if !key.is_empty() => format!("{key}=[redacted]"),
        _ => "[redacted]".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_url_credentials_handles_single_url() {
        let line = "clone https://user:pass@host.example/repo.git";
        let result = redact_url_credentials(line).expect("should detect credentials");
        assert_eq!(result, "clone https://[redacted]@host.example/repo.git");
    }

    #[test]
    fn redact_url_credentials_handles_multiple_urls_per_line() {
        // Regression: the old implementation only redacted the first URL.
        let line = "clone https://user:pass@host1.example/repo and push https://admin:secret@host2.example/repo";
        let result = redact_url_credentials(line).expect("should detect credentials");
        assert!(
            !result.contains("user:pass"),
            "first credential must be redacted; got: {result}"
        );
        assert!(
            !result.contains("admin:secret"),
            "second credential must be redacted; got: {result}"
        );
        assert!(result.contains("https://[redacted]@host1.example/repo"));
        assert!(result.contains("https://[redacted]@host2.example/repo"));
    }

    #[test]
    fn redact_url_credentials_returns_none_for_non_credentialed_urls() {
        let line = "fetching https://host.example/path";
        assert!(
            redact_url_credentials(line).is_none(),
            "URL without credentials must return None"
        );
    }

    #[test]
    fn redact_url_credentials_preserves_non_credentialed_urls_mixed_with_credentialed() {
        let line = "a https://pub.example/path and https://u:p@priv.example/path";
        let result = redact_url_credentials(line).expect("should detect credentials");
        assert!(result.contains("https://pub.example/path"));
        assert!(result.contains("https://[redacted]@priv.example/path"));
        assert!(!result.contains("u:p"));
    }
}
