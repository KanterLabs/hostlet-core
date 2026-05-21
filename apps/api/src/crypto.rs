use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use anyhow::{bail, Context};
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use rand::{distributions::Alphanumeric, Rng, RngCore};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;
const DEV_ENCRYPTION_KEY_BYTES: [u8; 32] = [7u8; 32];

#[derive(Clone)]
pub struct Crypto {
    cipher: Aes256Gcm,
}

impl Crypto {
    pub fn from_env(allow_insecure_dev_defaults: bool) -> anyhow::Result<Self> {
        let key = match nonempty_env("ENCRYPTION_KEY") {
            Some(key) => key,
            None if allow_insecure_dev_defaults => STANDARD.encode(DEV_ENCRYPTION_KEY_BYTES),
            None => {
                bail!("ENCRYPTION_KEY is required; generate one with `openssl rand -base64 32`")
            }
        };
        if !allow_insecure_dev_defaults && key == STANDARD.encode(DEV_ENCRYPTION_KEY_BYTES) {
            bail!("ENCRYPTION_KEY is using the insecure development default");
        }
        Self::new(&key)
    }

    pub fn new(base64_key: &str) -> anyhow::Result<Self> {
        let key = STANDARD
            .decode(base64_key)
            .context("ENCRYPTION_KEY must be base64")?;
        if key.len() != 32 {
            bail!("ENCRYPTION_KEY must decode to 32 bytes");
        }
        Ok(Self {
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

pub fn sign(secret: &str, payload: &[u8]) -> String {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(payload);
    format!("sha256={}", hex_bytes(&mac.finalize().into_bytes()))
}

pub fn verify_signature(secret: &str, payload: &[u8], signature: &str) -> bool {
    let expected = sign(secret, payload);
    constant_time_eq(expected.as_bytes(), signature.as_bytes())
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn random_token(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn nonempty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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

    #[test]
    fn verifies_hmac_signature() {
        let payload = br#"{"ref":"refs/heads/main"}"#;
        let sig = sign("secret", payload);
        assert!(verify_signature("secret", payload, &sig));
        assert!(!verify_signature("secret", b"{}", &sig));
    }
}
