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

pub(crate) fn redact_url_credentials(value: &str) -> Option<String> {
    let scheme = "https://";
    let start = value.find(scheme)?;
    let credentials_start = start + scheme.len();
    let at = value[credentials_start..].find('@')? + credentials_start;
    let mut redacted = String::with_capacity(value.len());
    redacted.push_str(&value[..start]);
    redacted.push_str("https://[redacted]@");
    redacted.push_str(&value[at + 1..]);
    Some(redacted)
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
