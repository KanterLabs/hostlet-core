//! Control-plane password hashing, verification, and policy.
//!
//! These are self-contained (argon2 + std only) and were split out of the main
//! auth module to keep the cookie/session/HTTP code separate from the password
//! primitives, lowering coupling.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};

pub(super) fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| anyhow::anyhow!("argon2 password hashing failed: {err}"))
}

pub(super) fn verify_password(hash: &str, password: &str) -> anyhow::Result<bool> {
    let parsed = PasswordHash::new(hash)
        .map_err(|err| anyhow::anyhow!("stored password hash is invalid: {err}"))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

pub(super) fn valid_control_plane_password(password: &str) -> bool {
    password.chars().count() >= 12 && !password.chars().any(|c| c.is_control())
}
