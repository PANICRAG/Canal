//! AES-256-GCM credential encryption for secrets at rest (TOTP, tokens, etc.).
//!
//! Key is read from `CREDENTIAL_ENCRYPTION_KEY` environment variable.
//! Format: `base64(nonce[12] || ciphertext || tag[16])`.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm};
use base64::Engine;

/// Environment variable for the credential encryption key.
const KEY_ENV: &str = "CREDENTIAL_ENCRYPTION_KEY";

/// Encrypt a plaintext value using AES-256-GCM.
///
/// Uses the key from `CREDENTIAL_ENCRYPTION_KEY` env var. If the env var is not
/// set, returns the value unchanged (dev mode — logged as warning on first call).
pub fn encrypt_credential(value: &str) -> String {
    let Some(key) = get_key() else {
        return value.to_string();
    };
    encrypt_with_key(value, &key)
}

/// Decrypt a value encrypted by [`encrypt_credential`].
///
/// If `CREDENTIAL_ENCRYPTION_KEY` is not set, returns the value as-is (assumes
/// plaintext from dev mode). If decryption fails, returns an error.
pub fn decrypt_credential(encrypted: &str) -> Result<String, String> {
    let Some(key) = get_key() else {
        return Ok(encrypted.to_string());
    };
    decrypt_with_key(encrypted, &key)
}

fn get_key() -> Option<String> {
    use std::sync::Once;
    static WARN: Once = Once::new();

    match std::env::var(KEY_ENV) {
        Ok(k) if !k.is_empty() => Some(k),
        _ => {
            WARN.call_once(|| {
                tracing::warn!(
                    "CREDENTIAL_ENCRYPTION_KEY not set — credentials stored without encryption (dev mode only)"
                );
            });
            None
        }
    }
}

fn encrypt_with_key(value: &str, key: &str) -> String {
    let key_bytes = derive_key_bytes(key);
    let cipher = Aes256Gcm::new_from_slice(&key_bytes).expect("AES-256-GCM key must be 32 bytes");
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, value.as_bytes())
        .expect("AES-256-GCM encryption should not fail");

    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);
    base64::engine::general_purpose::STANDARD.encode(&combined)
}

fn decrypt_with_key(encrypted: &str, key: &str) -> Result<String, String> {
    let key_bytes = derive_key_bytes(key);
    let combined = base64::engine::general_purpose::STANDARD
        .decode(encrypted)
        .map_err(|e| format!("base64 decode: {e}"))?;

    if combined.len() < 12 {
        return Err("ciphertext too short".to_string());
    }

    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = aes_gcm::Nonce::from_slice(nonce_bytes);
    let cipher = Aes256Gcm::new_from_slice(&key_bytes).map_err(|e| format!("invalid key: {e}"))?;

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "decryption failed: invalid key or corrupted data".to_string())?;

    String::from_utf8(plaintext).map_err(|e| format!("utf8: {e}"))
}

/// Derive a 32-byte key from a string. Zero-pads if short, truncates if long.
fn derive_key_bytes(key: &str) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    let src = key.as_bytes();
    let len = src.len().min(32);
    bytes[..len].copy_from_slice(&src[..len]);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = "test-key-for-aes-256-gcm-32byte!";
        let value = "my-totp-secret-ABCDEF123456";
        let encrypted = encrypt_with_key(value, key);
        let decrypted = decrypt_with_key(&encrypted, key).unwrap();
        assert_eq!(decrypted, value);
    }

    #[test]
    fn test_wrong_key_fails() {
        let value = "secret-value";
        let encrypted = encrypt_with_key(value, "correct-key-32-bytes-long!!!!!!!");
        let result = decrypt_with_key(&encrypted, "wrong-key-also-32-bytes-long!!!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_different_encryptions_differ() {
        let key = "test-key-for-aes-256-gcm-32byte!";
        let value = "same-value";
        let enc1 = encrypt_with_key(value, key);
        let enc2 = encrypt_with_key(value, key);
        // Different nonces → different ciphertexts
        assert_ne!(enc1, enc2);
        // But both decrypt to the same value
        assert_eq!(decrypt_with_key(&enc1, key).unwrap(), value);
        assert_eq!(decrypt_with_key(&enc2, key).unwrap(), value);
    }
}
