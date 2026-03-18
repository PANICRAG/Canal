//! Perceptual hashing for screen change detection.
//!
//! Uses average hash (aHash): resize to 8x8 grayscale, threshold against mean.
//! Fast (<5ms), robust to JPEG compression, sensitive to layout changes.

use base64::Engine;

/// Compute perceptual hash of a base64-encoded JPEG/PNG image.
///
/// Returns 64-bit hash (8x8 grid of above/below mean intensity).
/// Returns 0 on decode failure (invalid base64 or image data).
pub fn compute_phash(base64_data: &str) -> u64 {
    let bytes = match base64::engine::general_purpose::STANDARD.decode(base64_data) {
        Ok(b) => b,
        Err(_) => return 0,
    };
    let img = match image::load_from_memory(&bytes) {
        Ok(i) => i,
        Err(_) => return 0,
    };
    let gray = img
        .grayscale()
        .resize_exact(8, 8, image::imageops::FilterType::Nearest);
    let pixels: Vec<u8> = gray.to_luma8().into_raw();

    let mean: f32 = pixels.iter().map(|&p| p as f32).sum::<f32>() / 64.0;

    let mut hash: u64 = 0;
    for (i, &pixel) in pixels.iter().enumerate() {
        if pixel as f32 >= mean {
            hash |= 1 << i;
        }
    }
    hash
}

/// Compute similarity between two perceptual hashes.
///
/// Returns 0.0 (completely different) to 1.0 (identical).
/// Based on normalized Hamming distance.
pub fn hash_similarity(hash1: u64, hash2: u64) -> f32 {
    let hamming = (hash1 ^ hash2).count_ones();
    1.0 - (hamming as f32 / 64.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_hashes() {
        assert!(
            (hash_similarity(0xFFFF_FFFF_FFFF_FFFF, 0xFFFF_FFFF_FFFF_FFFF) - 1.0).abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn test_opposite_hashes() {
        assert!(hash_similarity(0x0000_0000_0000_0000, 0xFFFF_FFFF_FFFF_FFFF).abs() < f32::EPSILON);
    }

    #[test]
    fn test_similar_hashes() {
        let h1 = 0xFFFF_FFFF_FFFF_FFFF;
        let h2 = 0xFFFF_FFFF_FFFF_FFFE; // 1 bit different
        let sim = hash_similarity(h1, h2);
        assert!(sim > 0.98);
    }

    #[test]
    fn test_compute_phash_invalid_input() {
        assert_eq!(compute_phash("not-valid-base64!!!"), 0);
    }

    #[test]
    fn test_compute_phash_empty() {
        assert_eq!(compute_phash(""), 0);
    }
}
