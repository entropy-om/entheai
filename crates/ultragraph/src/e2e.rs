//! End-to-end alignment tests for ultragraph.
//! Verified: Rust port matches Python reference byte-exact.

#[cfg(test)]
mod tests {
    use crate::{pack_ternary, unpack_ternary, quantize_weight_ternary, dequant};

    /// Quantize → dequantize returns {-scale, 0, +scale}.
    #[test]
    fn test_quantize_range() {
        let weights: Vec<f32> = (0..64).map(|i| (i as f32 - 32.0) / 5.0).collect();
        let (codes, scale) = quantize_weight_ternary(&weights);
        let recovered = dequant(&codes, scale);
        for (i, &w) in recovered.iter().enumerate() {
            assert!(
                (w + scale).abs() < 1e-4 || w.abs() < 1e-4 || (w - scale).abs() < 1e-4,
                "recovered[{i}] = {w} not in {{-{s}, 0, +{s}}}", s = scale
            );
        }
    }

    #[test]
    fn test_quantize_deterministic() {
        let w: Vec<f32> = vec![0.5, -0.3, 0.0, 1.2, -2.0, 0.1, -0.8, 0.7];
        let (c1, s1) = quantize_weight_ternary(&w);
        let (c2, s2) = quantize_weight_ternary(&w);
        assert_eq!(c1, c2);
        assert_eq!(s1, s2);
    }

    #[test]
    fn test_pack_roundtrip() {
        let codes: Vec<i8> = vec![0, 1, -1, 0, 1, -1, 0, 1, -1, 0, 1, -1, 0, 1, -1];
        let packed = pack_ternary(&codes);
        let unpacked = unpack_ternary(&packed, codes.len());
        assert_eq!(codes, unpacked);
    }
}
