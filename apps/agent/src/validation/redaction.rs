//! Security helpers: HMAC job-signature verification, constant-time comparison,
//! and redaction of secrets from log lines and logged command arguments.

use super::super::*;

pub(crate) fn verify_signature(secret: &str, payload: &[u8], signature: &str) -> bool {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(payload);
    let expected = format!(
        "sha256={}",
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    constant_time_eq(expected.as_bytes(), signature.as_bytes())
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

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
