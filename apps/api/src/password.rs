//! Control-plane password hashing, verification, and policy.
//!
//! Moved out of `crate::auth` so the Cloud auth overlay can call core instead
//! of forking. Kept api-local (not in `hostlet-contracts`) to avoid pulling
//! argon2 into the agent and CLI binaries that depend on contracts.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| anyhow::anyhow!("argon2 password hashing failed: {err}"))
}

pub fn verify_password(hash: &str, password: &str) -> anyhow::Result<bool> {
    let parsed = PasswordHash::new(hash)
        .map_err(|err| anyhow::anyhow!("stored password hash is invalid: {err}"))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

pub fn valid_control_plane_password(password: &str) -> bool {
    password.chars().count() >= 12 && !password.chars().any(|c| c.is_control())
}
