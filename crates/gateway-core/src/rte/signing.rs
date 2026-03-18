//! HMAC-SHA256 signing and verification for RTE protocol.
//!
//! The session secret is generated on session start and shared with the
//! client via the `session_start` SSE event. All subsequent tool requests
//! and results are signed with this secret to ensure integrity.

use sha2::Sha256;
use uuid::Uuid;

/// HMAC-SHA256 signer/verifier for RTE protocol messages.
#[derive(Debug, Clone)]
pub struct RteSigner {
    secret: Vec<u8>,
}

impl RteSigner {
    /// Create a new signer with the given secret bytes.
    pub fn new(secret: Vec<u8>) -> Self {
        Self { secret }
    }

    /// Generate a new random session secret (32 bytes).
    pub fn generate_secret() -> Vec<u8> {
        use std::time::{SystemTime, UNIX_EPOCH};
        // Use a combination of UUID and timestamp for entropy
        let uuid_bytes = Uuid::new_v4().as_bytes().to_vec();
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes();
        let mut secret = Vec::with_capacity(32);
        secret.extend_from_slice(&uuid_bytes);
        secret.extend_from_slice(&ts[..16.min(ts.len())]);
        secret
    }

    /// Compute HMAC-SHA256 for arbitrary data.
    pub fn compute_hmac(&self, data: &str) -> String {
        compute_hmac(&self.secret, data)
    }

    /// Verify an HMAC signature in constant time.
    pub fn verify_hmac(&self, data: &str, signature: &str) -> bool {
        verify_hmac(&self.secret, data, signature)
    }

    /// Sign a tool execution request.
    ///
    /// The signed payload is `"{request_id}:{tool_name}"`.
    pub fn sign_request(&self, request_id: &Uuid, tool_name: &str) -> String {
        self.compute_hmac(&format!("{}:{}", request_id, tool_name))
    }

    /// Verify a tool execution request signature.
    pub fn verify_request(&self, request_id: &Uuid, tool_name: &str, signature: &str) -> bool {
        self.verify_hmac(&format!("{}:{}", request_id, tool_name), signature)
    }

    /// Sign a tool execution result.
    ///
    /// The signed payload is `"{request_id}:{success}"`.
    pub fn sign_result(&self, request_id: &Uuid, success: bool) -> String {
        self.compute_hmac(&format!("{}:{}", request_id, success))
    }

    /// Verify a tool execution result signature.
    pub fn verify_result(&self, request_id: &Uuid, success: bool, signature: &str) -> bool {
        self.verify_hmac(&format!("{}:{}", request_id, success), signature)
    }

    /// Encode secret as base64 for transmission in SSE events.
    pub fn secret_base64(&self) -> String {
        base64_encode(&self.secret)
    }
}

/// Compute HMAC-SHA256 and return hex-encoded result.
pub fn compute_hmac(secret: &[u8], data: &str) -> String {
    use hmac::{Hmac, Mac};

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take key of any size");
    mac.update(data.as_bytes());
    let result = mac.finalize();
    hex_encode(result.into_bytes().as_slice())
}

/// Verify HMAC signature in constant time.
pub fn verify_hmac(secret: &[u8], data: &str, signature: &str) -> bool {
    let expected = compute_hmac(secret, data);
    constant_time_eq(expected.as_bytes(), signature.as_bytes())
}

/// Constant-time byte comparison to prevent timing attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Simple hex encoding (avoids pulling in the `hex` crate at runtime).
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Base64-encode bytes (simple implementation for session secrets).
fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_secret_length() {
        let secret = RteSigner::generate_secret();
        assert!(secret.len() >= 16, "Secret should be at least 16 bytes");
    }

    #[test]
    fn test_sign_and_verify_request() {
        let signer = RteSigner::new(b"test-secret-key".to_vec());
        let request_id = Uuid::new_v4();
        let tool_name = "code_execute";

        let signature = signer.sign_request(&request_id, tool_name);
        assert!(signer.verify_request(&request_id, tool_name, &signature));
    }

    #[test]
    fn test_sign_and_verify_result() {
        let signer = RteSigner::new(b"test-secret-key".to_vec());
        let request_id = Uuid::new_v4();

        let sig_success = signer.sign_result(&request_id, true);
        let sig_failure = signer.sign_result(&request_id, false);

        assert!(signer.verify_result(&request_id, true, &sig_success));
        assert!(signer.verify_result(&request_id, false, &sig_failure));
        assert!(!signer.verify_result(&request_id, true, &sig_failure));
    }

    #[test]
    fn test_invalid_signature_rejected() {
        let signer = RteSigner::new(b"test-secret-key".to_vec());
        let request_id = Uuid::new_v4();

        assert!(!signer.verify_request(&request_id, "code_execute", "invalid-sig"));
    }

    #[test]
    fn test_different_secrets_produce_different_signatures() {
        let signer1 = RteSigner::new(b"secret-1".to_vec());
        let signer2 = RteSigner::new(b"secret-2".to_vec());
        let request_id = Uuid::new_v4();

        let sig1 = signer1.sign_request(&request_id, "tool");
        let sig2 = signer2.sign_request(&request_id, "tool");

        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"hello", b"hello"));
        assert!(!constant_time_eq(b"hello", b"world"));
        assert!(!constant_time_eq(b"hello", b"hell"));
    }
}
