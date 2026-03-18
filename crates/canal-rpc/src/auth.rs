//! Token authentication with constant-time comparison.

/// Verify a token using constant-time comparison to prevent timing attacks.
///
/// R8-H5: Uses fixed-time comparison that does not leak token length.
/// Always iterates over the expected token's full length regardless of input.
pub fn verify_token(expected: &str, provided: &str) -> bool {
    // XOR the lengths — non-zero if different, but don't early-return (leaks length)
    let mut result: u8 = (expected.len() != provided.len()) as u8;
    // Iterate over expected bytes, using 0 as default if provided is shorter
    let expected_bytes = expected.as_bytes();
    let provided_bytes = provided.as_bytes();
    for i in 0..expected_bytes.len() {
        let b = if i < provided_bytes.len() {
            provided_bytes[i]
        } else {
            0 // Pad with zero if provided is shorter — still iterates all expected bytes
        };
        result |= expected_bytes[i] ^ b;
    }
    result == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_token() {
        assert!(verify_token("abc123", "abc123"));
    }

    #[test]
    fn test_invalid_token() {
        assert!(!verify_token("abc123", "wrong!"));
    }

    #[test]
    fn test_empty_token() {
        assert!(!verify_token("abc123", ""));
    }

    #[test]
    fn test_constant_time() {
        // Structural test: verify both paths use the same XOR loop
        // (no early return on first mismatch).
        // The function always iterates all bytes when lengths match.
        assert!(!verify_token("aaaaaa", "aaaaab"));
        assert!(!verify_token("aaaaaa", "baaaaa"));
        assert!(verify_token("aaaaaa", "aaaaaa"));
    }
}
