use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use anyhow::{bail, Context};
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::{distributions::Alphanumeric, Rng, RngCore};
use sha2::{Digest, Sha256};

// HMAC signing + constant-time comparison live in `hostlet_contracts::crypto`
// (the crate both binaries share) so the agent and api use one implementation.
// Re-exported here so existing `crate::crypto::*` paths — including cloud's
// overlay, which does not override this file — resolve unchanged.
pub use hostlet_contracts::crypto::{constant_time_eq, sign, verify_signature};

#[derive(Clone)]
pub struct Crypto {
    cipher: Aes256Gcm,
}

impl Crypto {
    pub fn from_env(allow_insecure_dev_defaults: bool) -> anyhow::Result<Self> {
        let key = match nonempty_env("ENCRYPTION_KEY") {
            Some(key) => key,
            None => {
                bail!("ENCRYPTION_KEY is required; generate one with `openssl rand -base64 32`")
            }
        };
        let crypto = Self::new(&key)?;
        // `new` already guarantees the key decodes to exactly 32 bytes. This
        // extra check rejects short *base64* strings (e.g. obvious dev
        // placeholders) outside of explicit dev-defaults mode: a real 32-byte
        // key encodes to a 44-character base64 string, so any value this short
        // is a placeholder even if it happened to decode to 32 bytes.
        if !allow_insecure_dev_defaults && key.len() < 32 {
            bail!("ENCRYPTION_KEY must not be a short development value");
        }
        Ok(crypto)
    }

    pub fn new(base64_key: &str) -> anyhow::Result<Self> {
        let key = STANDARD
            .decode(base64_key)
            .context("ENCRYPTION_KEY must be base64")?;
        if key.len() != 32 {
            bail!("ENCRYPTION_KEY must decode to 32 bytes");
        }
        Ok(Self {
            // Safe: the length check above guarantees `key` is exactly the
            // 32 bytes AES-256 requires, so the only error variant cannot occur.
            cipher: Aes256Gcm::new_from_slice(&key).unwrap(),
        })
    }

    pub fn encrypt(&self, plaintext: &str) -> anyhow::Result<String> {
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let mut out = nonce.to_vec();
        let encrypted = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext.as_bytes())
            .map_err(|_| anyhow::anyhow!("encryption failed"))?;
        out.extend(encrypted);
        Ok(STANDARD.encode(out))
    }

    pub fn decrypt(&self, ciphertext: &str) -> anyhow::Result<String> {
        let bytes = STANDARD.decode(ciphertext)?;
        if bytes.len() < 13 {
            bail!("ciphertext is too short");
        }
        let (nonce, data) = bytes.split_at(12);
        let plaintext = self
            .cipher
            .decrypt(Nonce::from_slice(nonce), data)
            .map_err(|_| anyhow::anyhow!("decryption failed"))?;
        Ok(String::from_utf8(plaintext)?)
    }
}

pub fn hash_token(token: &str) -> String {
    STANDARD.encode(Sha256::digest(token.as_bytes()))
}

pub fn verify_token(token: &str, expected_hash: &str) -> bool {
    let actual = hash_token(token);
    constant_time_eq(actual.as_bytes(), expected_hash.as_bytes())
}

pub fn random_token(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

/// Reads an environment variable, trims it, and returns `None` when it is
/// missing or empty after trimming.
///
/// Defined here (a file cloud does not override) so cloud's overlay inherits a
/// single binary-local definition; env access is binary-local policy, so this
/// stays out of `contracts`.
pub(crate) fn nonempty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypts_and_decrypts() {
        let key = STANDARD.encode([3u8; 32]);
        let crypto = Crypto::new(&key).unwrap();
        let encrypted = crypto.encrypt("secret").unwrap();
        assert_ne!(encrypted, "secret");
        assert_eq!(crypto.decrypt(&encrypted).unwrap(), "secret");
    }

    #[test]
    fn validates_tokens() {
        let hash = hash_token("server-token");
        assert!(verify_token("server-token", &hash));
        assert!(!verify_token("wrong", &hash));
    }
}
