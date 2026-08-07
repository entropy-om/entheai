//! Pure-Rust ternary GEMM over ayeOS matrices.
//!
//! Reference CPU kernel (activations NOT quantized):
//!
//! ```text
//! y[p] = Σ_k x[k] · (code[p, k] − 1) · scale[p, k / 64]
//! ```
//!
//! Per output row the reduction over K is sequential — each group's ternary
//! contribution is accumulated in order, then multiplied by the group scale
//! after the group completes (no cross-K parallelism). Output rows are
//! independent, so parallelism comes from rayon over the `dim` rows.

use rayon::prelude::*;

use crate::codes;
use crate::loader::AyeosMatrix;

/// Single-row matmul `out[p] = Σ_k x[k]·(code[p,k]−1)·scale[p, k/64]`.
///
/// `x` must have `in_features` elements; `out` must have `dim` elements.
/// Rows of the weight matrix are computed in parallel over rayon's pool.
pub fn ternary_matmul(x: &[f32], m: &AyeosMatrix, out: &mut [f32]) {
    assert_eq!(
        x.len(),
        m.in_features,
        "x has {} elements, matrix in_features is {}",
        x.len(),
        m.in_features
    );
    assert_eq!(
        out.len(),
        m.dim,
        "out has {} elements, matrix dim is {}",
        out.len(),
        m.dim
    );

    out.par_iter_mut().enumerate().for_each(|(p, y)| {
        *y = ternary_row(x, m, p);
    });
}

/// Batched matmul over `M` stacked `in_features`-length inputs (`M×K`, row-major).
///
/// `out` must have `M × dim` elements. Parallelism is over the output rows
/// (`M × dim` tasks).
pub fn ternary_matmul_batch(xs: &[f32], m: &AyeosMatrix, out: &mut [f32]) {
    assert_ne!(m.in_features, 0);
    assert_eq!(
        xs.len() % m.in_features,
        0,
        "xs length {} not a multiple of in_features {}",
        xs.len(),
        m.in_features
    );
    let batch = xs.len() / m.in_features;
    assert_eq!(
        out.len(),
        batch * m.dim,
        "out length {} != batch {batch} × dim {}",
        out.len(),
        m.dim
    );

    out.par_chunks_mut(m.dim)
        .enumerate()
        .for_each(|(r, out_row)| {
            let x = &xs[r * m.in_features..(r + 1) * m.in_features];
            for (p, y) in out_row.iter_mut().enumerate() {
                *y = ternary_row(x, m, p);
            }
        });
}

/// Compute output row `p` for input `x`: sequential over K, scale after group.
fn ternary_row(x: &[f32], m: &AyeosMatrix, p: usize) -> f32 {
    let group = m.group_size;
    let groups_per_row = m.groups_per_row();
    let words_per_row = m.words_per_row();

    let row_words = &m.codes[p * words_per_row..(p + 1) * words_per_row];
    let row_scales = &m.scales[p * groups_per_row..(p + 1) * groups_per_row];

    let mut acc: f32 = 0.0;
    for g in 0..groups_per_row {
        // Group-wise accumulation of x[k]·(code−1) over the group's 64 columns,
        // strictly sequential over K.
        let mut gacc: f32 = 0.0;
        for j in 0..group {
            // element g·64 + j lives in bits 2·(j mod 16) of word (g·64 + j)/16
            let word = row_words[(g * group + j) / codes::CODES_PER_WORD];
            let code = ((word >> (2 * (j & (codes::CODES_PER_WORD - 1)))) & 0b11) as i8;
            debug_assert!(code <= 2, "code {code} out of ternary range");
            let t = code - 1; // {−1, 0, +1}
            if t != 0 {
                gacc += x[g * group + j] * t as f32;
            }
        }
        // Scale multiply after the group accumulate.
        acc += gacc * row_scales[g];
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes;
    use crate::loader::AyeosMatrix;

    /// Hand-built 2×64 matrix with known codes/scales (one 64-wide group/row).
    fn hand_built_matrix() -> AyeosMatrix {
        let dim = 2usize;
        let k = 64usize;
        // Row 0: values −1,+1,0,−1,+1,…  |  Row 1: values +1,−1,0,+1,−1,…
        let mut codes = vec![0u8; dim * k];
        for i in 0..k {
            codes[i] = match i % 3 {
                0 => 0,
                1 => 2,
                _ => 1,
            };
            codes[k + i] = match i % 3 {
                0 => 2,
                1 => 0,
                _ => 1,
            };
        }
        let mut packed = vec![0u32; dim * k / codes::CODES_PER_WORD];
        codes::encode(&codes, &mut packed);
        AyeosMatrix {
            name: "test.up_proj".into(),
            dim,
            in_features: k,
            group_size: 64,
            codes: packed,
            scales: vec![0.5, 0.25],
        }
    }

    #[test]
    fn gemm_matches_manually_computed_reference() {
        let m = hand_built_matrix();
        // Deterministic x with known values.
        let x: Vec<f32> = (0..m.in_features)
            .map(|i| ((i * 7 + 3) % 10) as f32 * 0.1)
            .collect();
        let mut out = vec![0.0f32; m.dim];
        ternary_matmul(&x, &m, &mut out);

        // Reference computed independently: y[p] = Σ_k x[k]·(code−1)·scale[p, k/64]
        // (decode codes first, per-element multiply, f64 accumulation).
        let mut raw = vec![0u8; m.dim * m.in_features];
        for p in 0..m.dim {
            let row_words = &m.codes[p * m.words_per_row()..(p + 1) * m.words_per_row()];
            codes::decode_row(
                row_words,
                &mut raw[p * m.in_features..(p + 1) * m.in_features],
            );
        }
        for p in 0..m.dim {
            let mut expected = 0.0f64;
            for k in 0..m.in_features {
                let t = raw[p * m.in_features + k] as i64 - 1;
                let s = m.scales[p * m.groups_per_row() + k / m.group_size] as f64;
                expected += x[k] as f64 * t as f64 * s;
            }
            assert!(
                (out[p] as f64 - expected).abs() < 1e-4,
                "row {p}: got {} expected {expected}",
                out[p]
            );
        }
    }

    #[test]
    fn gemm_batch_matches_single() {
        let m = hand_built_matrix();
        let x1: Vec<f32> = (0..m.in_features).map(|i| (i as f32) * 0.01).collect();
        let x2: Vec<f32> = (0..m.in_features)
            .map(|i| (i as f32) * -0.02 + 0.5)
            .collect();

        let mut out1 = vec![0.0f32; m.dim];
        let mut out2 = vec![0.0f32; m.dim];
        ternary_matmul(&x1, &m, &mut out1);
        ternary_matmul(&x2, &m, &mut out2);

        let xs = [&x1[..], &x2[..]].concat();
        let mut out_batch = vec![0.0f32; 2 * m.dim];
        ternary_matmul_batch(&xs, &m, &mut out_batch);

        assert_eq!(&out_batch[..m.dim], &out1[..]);
        assert_eq!(&out_batch[m.dim..], &out2[..]);
    }
}
