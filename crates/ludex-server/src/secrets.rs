//! Authenticated encryption for secrets stored at rest.
//!
//! A secret is encrypted with AES-256-GCM under a key derived from the server
//! secret with HKDF-SHA256. Each purpose supplies its own salt, info and
//! additional authenticated data, so ciphertexts from one feature can never be
//! decrypted by another. The stored format is
//! `v1.<base64url(nonce || ciphertext)>`; the nonce is random for every write,
//! so saving the same secret twice produces different ciphertext.
//!
//! Callers must never log or serialise the plaintext or the ciphertext.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;

/// Prefix of the stored ciphertext format.
const PREFIX: &str = "v1.";

/// Derives the AES-256-GCM key for one purpose from the server secret.
fn derive_key(server_secret: &[u8], salt: &[u8], info: &[u8]) -> [u8; 32] {
    let hkdf = Hkdf::<Sha256>::new(Some(salt), server_secret);
    let mut key = [0_u8; 32];
    hkdf.expand(info, &mut key)
        .expect("32 bytes is a valid HKDF output length");
    key
}

/// Encrypts `plaintext` under a key derived from `server_secret`.
///
/// `salt`, `info` and `aad` bind the ciphertext to one feature and format;
/// decrypting with different values fails.
pub fn encrypt(
    server_secret: &[u8],
    salt: &[u8],
    info: &[u8],
    aad: &[u8],
    plaintext: &str,
) -> Result<String> {
    let cipher = Aes256Gcm::new_from_slice(&derive_key(server_secret, salt, info))
        .context("build AES-256-GCM cipher")?;
    let mut nonce_bytes = [0_u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: plaintext.as_bytes(),
                aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("encrypt secret"))?;
    let mut blob = Vec::with_capacity(12 + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    Ok(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(blob)))
}

/// Decrypts a value produced by [`encrypt`] with the same purpose parameters.
pub fn decrypt(
    server_secret: &[u8],
    salt: &[u8],
    info: &[u8],
    aad: &[u8],
    stored: &str,
) -> Result<String> {
    let encoded = stored
        .strip_prefix(PREFIX)
        .context("unsupported secret format")?;
    let blob = URL_SAFE_NO_PAD.decode(encoded).context("decode secret")?;
    if blob.len() <= 12 {
        anyhow::bail!("secret ciphertext is truncated");
    }
    let (nonce_bytes, ciphertext) = blob.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(&derive_key(server_secret, salt, info))
        .context("build AES-256-GCM cipher")?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(nonce_bytes),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("decrypt secret (server secret changed?)"))?;
    String::from_utf8(plaintext).context("secret is not valid UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SALT: &[u8] = b"test-salt";
    const INFO: &[u8] = b"test-info";
    const AAD: &[u8] = b"test-aad";

    #[test]
    fn secrets_round_trip_and_are_purpose_bound() {
        let server_secret = b"server-secret-used-for-settings-encryption";
        let first = encrypt(server_secret, SALT, INFO, AAD, "hunter2").expect("encrypt");
        let second = encrypt(server_secret, SALT, INFO, AAD, "hunter2").expect("encrypt again");
        assert_ne!(first, second, "nonce must be random");
        assert_eq!(
            decrypt(server_secret, SALT, INFO, AAD, &first).expect("decrypt"),
            "hunter2"
        );
        assert!(decrypt(b"another-server-secret", SALT, INFO, AAD, &first).is_err());
        assert!(decrypt(server_secret, b"other-salt", INFO, AAD, &first).is_err());
        assert!(decrypt(server_secret, SALT, INFO, b"other-aad", &first).is_err());
        assert!(decrypt(server_secret, SALT, INFO, AAD, "garbage").is_err());
    }
}
