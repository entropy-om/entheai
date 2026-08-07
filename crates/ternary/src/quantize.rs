//! Reference ternary quantizer + dequantizer (test-only round-trip support).
//!
//! Per group of `group_size` weights the quantizer computes:
//!
//! ```text
//! scale_g = max(mean(|w| over group), 1e-7)
//! code    = round(clamp(w / scale_g, −1, +1)) + 1   →   {0, 1, 2}
//! ```
//!
//! Dequantization is the inverse mapping `w = (code − 1) · scale_g`. This
//! module exists purely so tests can prove `quantize(dequantize(codes)) ==
//! codes` over the real matrices — it is not part of the inference path.

use crate::codes;
use crate::loader::AyeosMatrix;

/// Smallest allowed group scale (avoids divide-by-zero on all-zero groups).
pub const MIN_SCALE: f64 = 1e-7;

/// Quantize one `in_features`-length row of weights into packed codes + scales.
///
/// `words` must have `K / 16` elements; `scales` must have `K / group_size`.
pub fn quantize_row(weights: &[f32], group_size: usize, words: &mut [u32], scales: &mut [f32]) {
    let k = weights.len();
    assert!(group_size > 0 && group_size.is_multiple_of(codes::CODES_PER_WORD));
    assert_eq!(k % group_size, 0);
    assert_eq!(words.len(), k / codes::CODES_PER_WORD);
    assert_eq!(scales.len(), k / group_size);

    words.fill(0);
    let mut raw = vec![0u8; group_size];
    for g in 0..k / group_size {
        let group = &weights[g * group_size..(g + 1) * group_size];
        // scale_g = max(mean(|w| over group), 1e-7)
        let mean_abs: f64 =
            group.iter().map(|w| (*w as f64).abs()).sum::<f64>() / group_size as f64;
        let scale = mean_abs.max(MIN_SCALE) as f32;
        scales[g] = scale;

        for (j, w) in group.iter().enumerate() {
            let v = ((w / scale).clamp(-1.0, 1.0).round() as i8 + 1) as u8; // {0, 1, 2}
            debug_assert!(v <= 2, "quantized code {v} out of range");
            raw[j] = v;
        }
        let base = g * group_size;
        for w in 0..group_size / codes::CODES_PER_WORD {
            let mut packed = 0u32;
            for j in 0..codes::CODES_PER_WORD {
                packed |= (raw[w * codes::CODES_PER_WORD + j] as u32) << (2 * j as u32);
            }
            words[base / codes::CODES_PER_WORD + w] = packed;
        }
    }
}

/// Dequantize one row: `out[k] = (code[k] − 1) · scale[k / group_size]`.
///
/// `words` must have `K / 16` elements; `scales` must have `K / group_size`.
pub fn dequantize_row(words: &[u32], scales: &[f32], group_size: usize, out: &mut [f32]) {
    let k = out.len();
    assert!(group_size > 0);
    assert_eq!(k % group_size, 0);
    assert_eq!(words.len(), k / codes::CODES_PER_WORD);
    assert_eq!(scales.len(), k / group_size);

    for (j, slot) in out.iter_mut().enumerate() {
        let word = words[j / codes::CODES_PER_WORD];
        let code = ((word >> (2 * (j & (codes::CODES_PER_WORD - 1)))) & 0b11) as i8;
        debug_assert!(code <= 2, "code {code} out of ternary range");
        *slot = (code - 1) as f32 * scales[j / group_size];
    }
}

/// Dequantize a whole matrix into its dense `dim × in_features` weight vector.
pub fn dequantize(m: &AyeosMatrix) -> Vec<f32> {
    let mut out = vec![0.0f32; m.dim * m.in_features];
    for p in 0..m.dim {
        let row_words = &m.codes[p * m.words_per_row()..(p + 1) * m.words_per_row()];
        let row_scales = &m.scales[p * m.groups_per_row()..(p + 1) * m.groups_per_row()];
        dequantize_row(
            row_words,
            row_scales,
            m.group_size,
            &mut out[p * m.in_features..(p + 1) * m.in_features],
        );
    }
    out
}

/// Quantize dense `dim × in_features` weights back into packed codes + scales.
pub fn quantize(
    weights: &[f32],
    dim: usize,
    in_features: usize,
    group_size: usize,
) -> (Vec<u32>, Vec<f32>) {
    assert_eq!(weights.len(), dim * in_features);
    let mut codes_out = vec![0u32; dim * in_features / codes::CODES_PER_WORD];
    let mut scales_out = vec![0f32; dim * in_features / group_size];
    for p in 0..dim {
        let row = &weights[p * in_features..(p + 1) * in_features];
        quantize_row(
            row,
            group_size,
            &mut codes_out[p * (in_features / codes::CODES_PER_WORD)
                ..(p + 1) * (in_features / codes::CODES_PER_WORD)],
            &mut scales_out[p * (in_features / group_size)..(p + 1) * (in_features / group_size)],
        );
    }
    (codes_out, scales_out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::{load_dir, AyeosMatrix};
    use std::path::PathBuf;

    /// Real ayeos data dir: `AYEOS_DATA_DIR` env override, else relative to the
    /// crate root (tests run with CWD = package root).
    fn data_dir() -> PathBuf {
        if let Ok(dir) = std::env::var("AYEOS_DATA_DIR") {
            return PathBuf::from(dir);
        }
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../pocoo.vaked.dev/demos/quantal")
    }

    fn load_all() -> Vec<AyeosMatrix> {
        let dir = data_dir();
        load_dir(&dir).unwrap_or_else(|e| {
            panic!(
                "load_dir({}) failed: {e} — set AYEOS_DATA_DIR to the quantal data dir",
                dir.display()
            )
        })
    }

    #[test]
    fn quantize_dequantize_round_trip_on_real_matrices() {
        let matrices = load_all();
        // Spread a sample across the 168.
        let sample: Vec<&AyeosMatrix> = [0usize, 20, 40, 60, 80, 100, 120, 140, 167]
            .into_iter()
            .map(|i| &matrices[i])
            .collect();
        for m in sample {
            let weights = dequantize(m);
            let (codes, _scales) = quantize(&weights, m.dim, m.in_features, m.group_size);
            assert_eq!(
                codes, m.codes,
                "quantize(dequantize(codes)) != codes for {}",
                m.name
            );
        }
    }

    #[test]
    fn quantize_round_trip_hand_built() {
        // One row, K=64 (single group): two nonzeros, rest zeros — exercises the
        // 1e-7 scale floor and the all-zero code path.
        let mut weights = vec![0.0f32; 64];
        weights[0] = 2.0;
        weights[1] = -1.0;
        let mut words = vec![0u32; 4];
        let mut scales = vec![0f32; 1];
        quantize_row(&weights, 64, &mut words, &mut scales);

        assert!(
            (scales[0] - 3.0f32 / 64.0).abs() < 1e-6,
            "scale {}",
            scales[0]
        );

        let mut back = vec![0.0f32; 64];
        dequantize_row(&words, &scales, 64, &mut back);
        assert!((back[0] - scales[0]).abs() < 1e-6, "back[0] {}", back[0]);
        assert!((back[1] + scales[0]).abs() < 1e-6, "back[1] {}", back[1]);
        assert_eq!(back[2], 0.0);
    }
}
