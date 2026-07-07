//! AES-256-GCM encrypt/decrypt helpers for tenant BYOK API keys.
//!
//! This is the **single** module in the workspace that owns crypto logic.
//! Both `gmrag-api` (T45 BYOK resolver) and `gmrag-worker` (T40/T41 BYOK
//! factories) call these functions instead of duplicating AES-GCM code.
//!
//! ## Design invariants
//!
//! - **AAD bound to tenant_id**: every ciphertext is bound to a specific
//!   tenant via AES-GCM associated data (`aad = tenant_id.as_bytes()`).
//!   This prevents copying a ciphertext from tenant A and decrypting it
//!   under tenant B's context — even if the encryption key is the same.
//! - **Separate nonce + ciphertext**: the on-disk schema uses two `BYTEA`
//!   columns (`api_key_nonce`, `api_key_ciphertext`). This module returns
//!   them as a tuple `(Vec<u8>, Vec<u8>)` to match that schema directly,
//!   avoiding any combined-encoding mismatch.
//! - **32-byte key**: AES-256 requires a 32-byte key. The key is loaded
//!   from `GMRAG_TENANT_KEY_ENCRYPTION_KEY` (base64-encoded 32 bytes) via
//!   `Config::tenant_key_encryption_key`.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::rngs::OsRng;
use rand::RngCore;

/// Encrypt `plaintext` with AES-256-GCM, binding the ciphertext to `aad`
/// (typically `tenant_id.as_bytes()`).
///
/// Returns `(ciphertext, nonce)` as separate byte vectors to match the
/// `api_key_ciphertext BYTEA` + `api_key_nonce BYTEA` schema.
///
/// The nonce is 12 bytes generated via `OsRng` (cryptographically secure).
/// Each call produces a fresh nonce, so encrypting the same plaintext twice
/// yields different ciphertexts.
pub fn encrypt_with_aad(
    plaintext: &str,
    key: &[u8; 32],
    aad: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
    let cipher = Aes256Gcm::new(key.into());
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext.as_bytes(),
                aad,
            },
        )
        .map_err(|_| CryptoError::Encrypt)?;
    Ok((ciphertext, nonce_bytes.to_vec()))
}

/// Decrypt an AES-256-GCM ciphertext that was produced by [`encrypt_with_aad`].
///
/// `nonce` must be exactly 12 bytes. `aad` must be the same bytes passed to
/// `encrypt_with_aad` — a mismatch causes a decrypt error (intentional:
/// this is the tenant-binding security property).
pub fn decrypt_with_aad(
    ciphertext: &[u8],
    nonce: &[u8],
    key: &[u8; 32],
    aad: &[u8],
) -> Result<String, CryptoError> {
    if nonce.len() != 12 {
        return Err(CryptoError::InvalidNonceLen(nonce.len()));
    }
    let cipher = Aes256Gcm::new(key.into());
    let nonce = Nonce::from_slice(nonce);
    let plaintext = cipher
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Decrypt)?;
    String::from_utf8(plaintext).map_err(|_| CryptoError::Utf8)
}

/// Error type for crypto operations.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("AES-GCM encrypt failed")]
    Encrypt,
    #[error("AES-GCM decrypt failed (wrong key, corrupted ciphertext, or AAD mismatch)")]
    Decrypt,
    #[error("nonce must be 12 bytes, got {0}")]
    InvalidNonceLen(usize),
    #[error("decrypted plaintext is not valid UTF-8")]
    Utf8,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        key
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = test_key();
        let plaintext = "sk-test-api-key-12345";
        let aad = b"tenant-uuid-bytes-here";

        let (ciphertext, nonce) =
            encrypt_with_aad(plaintext, &key, aad).expect("encrypt must succeed");
        let decrypted =
            decrypt_with_aad(&ciphertext, &nonce, &key, aad).expect("decrypt must succeed");

        assert_eq!(plaintext, decrypted);
    }

    #[test]
    fn nonce_is_12_bytes() {
        let key = test_key();
        let (_, nonce) = encrypt_with_aad("secret", &key, b"aad").unwrap();
        assert_eq!(nonce.len(), 12);
    }

    #[test]
    fn each_encrypt_produces_different_ciphertext() {
        let key = test_key();
        let aad = b"tenant-a";
        let (ct1, n1) = encrypt_with_aad("same-secret", &key, aad).unwrap();
        let (ct2, n2) = encrypt_with_aad("same-secret", &key, aad).unwrap();
        // Nonces must differ (random).
        assert_ne!(n1, n2);
        // Ciphertexts must differ (different nonce → different output).
        assert_ne!(ct1, ct2);
    }

    #[test]
    fn decrypt_wrong_key_returns_error() {
        let key1 = test_key();
        let key2 = test_key();
        let aad = b"tenant-a";
        let (ciphertext, nonce) = encrypt_with_aad("secret", &key1, aad).unwrap();
        assert!(decrypt_with_aad(&ciphertext, &nonce, &key2, aad).is_err());
    }

    #[test]
    fn decrypt_wrong_aad_returns_error() {
        let key = test_key();
        let (ciphertext, nonce) = encrypt_with_aad("secret", &key, b"tenant-a").unwrap();
        // Different AAD → decrypt must fail (tenant binding).
        assert!(decrypt_with_aad(&ciphertext, &nonce, &key, b"tenant-b").is_err());
    }

    #[test]
    fn decrypt_corrupted_ciphertext_returns_error() {
        let key = test_key();
        let aad = b"tenant-a";
        let (mut ciphertext, nonce) = encrypt_with_aad("secret", &key, aad).unwrap();
        // Flip a byte in the ciphertext.
        ciphertext[0] ^= 0xFF;
        assert!(decrypt_with_aad(&ciphertext, &nonce, &key, aad).is_err());
    }

    #[test]
    fn decrypt_invalid_nonce_length_returns_error() {
        let key = test_key();
        let aad = b"tenant-a";
        let (ciphertext, _) = encrypt_with_aad("secret", &key, aad).unwrap();
        // Wrong nonce length (11 bytes instead of 12).
        let bad_nonce = vec![0u8; 11];
        let err = decrypt_with_aad(&ciphertext, &bad_nonce, &key, aad).unwrap_err();
        assert!(matches!(err, CryptoError::InvalidNonceLen(11)));
    }

    #[test]
    fn encrypt_decrypt_empty_plaintext() {
        let key = test_key();
        let aad = b"tenant-a";
        let (ciphertext, nonce) = encrypt_with_aad("", &key, aad).unwrap();
        let decrypted = decrypt_with_aad(&ciphertext, &nonce, &key, aad).unwrap();
        assert_eq!(decrypted, "");
    }

    #[test]
    fn encrypt_decrypt_unicode_plaintext() {
        let key = test_key();
        let aad = b"tenant-a";
        let plaintext = "sk-キー-🔑-日本語";
        let (ciphertext, nonce) = encrypt_with_aad(plaintext, &key, aad).unwrap();
        let decrypted = decrypt_with_aad(&ciphertext, &nonce, &key, aad).unwrap();
        assert_eq!(plaintext, decrypted);
    }
}
