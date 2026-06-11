//! Shared cryptographic helpers used by both the api and agent binaries for
//! HMAC-SHA256 message signing/verification and constant-time comparison.
//!
//! These live in `contracts` (the only crate both binaries depend on) so a
//! single implementation backs agent<->api message authentication. The api's
//! `crypto.rs` re-exports these names, so existing `crate::crypto::*` paths
//! (including cloud's overlay) resolve here unchanged.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Computes the `sha256=<hex>` HMAC-SHA256 signature of `payload` under
/// `secret`, matching GitHub's webhook signature wire format.
pub fn sign(secret: &str, payload: &[u8]) -> String {
    // Safe: HMAC accepts a key of any length, so construction never fails.
    let mut mac = <HmacSha256 as Mac>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(payload);
    format!("sha256={}", hex_bytes(&mac.finalize().into_bytes()))
}

/// Verifies that `signature` is the [`sign`] output for `payload` under
/// `secret`, comparing in constant time.
pub fn verify_signature(secret: &str, payload: &[u8], signature: &str) -> bool {
    let expected = sign(secret, payload);
    constant_time_eq(expected.as_bytes(), signature.as_bytes())
}

/// Length-independent-of-content byte comparison: returns whether `a` and `b`
/// are equal without short-circuiting on the first differing byte.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_hmac_signature() {
        let payload = br#"{"ref":"refs/heads/main"}"#;
        let sig = sign("secret", payload);
        assert!(verify_signature("secret", payload, &sig));
        assert!(!verify_signature("secret", b"{}", &sig));
    }

    #[test]
    fn sign_matches_known_answer_vector() {
        // Known-answer test pinning the exact HMAC-SHA256 hex encoding and the
        // `sha256=` prefix. Computed independently:
        //   printf 'hello world' | openssl dgst -sha256 -hmac topsecret
        // This guards the wire format that agent<->api message auth depends on.
        assert_eq!(
            sign("topsecret", b"hello world"),
            "sha256=67a6479f7b6000f050577eea8b6b5e71d3c704e73a5f5d2aa09f607fce35cf1a"
        );
        assert!(verify_signature(
            "topsecret",
            b"hello world",
            "sha256=67a6479f7b6000f050577eea8b6b5e71d3c704e73a5f5d2aa09f607fce35cf1a"
        ));
    }

    #[test]
    fn constant_time_eq_matches_byte_equality() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }
}
