//! Quantization primitives (BitNet b1.58 style) — port of `ultragraph/quant.py`.
//! Weights are ternary {−1,0,+1}; activations are int8. One value per byte.

/// Matches Python `EPS` — keep identical or the fixture scales won't reproduce.
pub const EPS: f32 = 1e-5;

/// Absmean ternary quantization. Returns `(q, scale)` with `q ∈ {−1,0,+1}` and
/// `q[i] as f32 * scale ≈ w[i]`. `scale = mean(|w|) + EPS` (0 for an empty slice).
///
/// Port of `quantize_weight_ternary`: `q = clip(round(w / scale), -1, 1)`.
pub fn quantize_weight_ternary(w: &[f32]) -> (Vec<i8>, f32) {
    if w.is_empty() {
        return (Vec::new(), 0.0);
    }
    let sum_abs: f32 = w.iter().map(|v| v.abs()).sum();
    let mut m = sum_abs / (w.len() as f32);
    if !m.is_finite() {
        m = 0.0;
    }
    let scale = m + EPS;
    let q = w
        .iter()
        .map(|&v| {
            let mut val = v / scale;
            if val.is_nan() {
                val = 0.0;
            }
            val.round().clamp(-1.0, 1.0) as i8
        })
        .collect();
    (q, scale)
}

/// Absmax int8 activation quantization. Returns `(q, scale)` with `q ∈ [−127,127]`
/// and `q[i] as f32 * scale ≈ x[i]`. `scale = max(|x|)/127 + EPS`.
///
/// Port of `quantize_act_int8`: `q = clip(round(x / scale), -127, 127)`.
pub fn quantize_act_int8(x: &[f32]) -> (Vec<i8>, f32) {
    if x.is_empty() {
        return (Vec::new(), 0.0);
    }
    let max_abs = x.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
    let mut m = max_abs;
    if !m.is_finite() {
        m = 0.0;
    }
    let scale = m / 127.0 + EPS;
    let q = x
        .iter()
        .map(|&v| {
            let mut val = v / scale;
            if val.is_nan() {
                val = 0.0;
            }
            val.round().clamp(-127.0, 127.0) as i8
        })
        .collect();
    (q, scale)
}

/// Inverse of the quantizers: `q[i] as f32 * scale`.
pub fn dequant(q: &[i8], scale: f32) -> Vec<f32> {
    q.iter().map(|&v| (v as f32) * scale).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quant_empty() {
        let (q_w, scale_w) = quantize_weight_ternary(&[]);
        assert!(q_w.is_empty());
        assert_eq!(scale_w, 0.0);

        let (q_a, scale_a) = quantize_act_int8(&[]);
        assert!(q_a.is_empty());
        assert_eq!(scale_a, 0.0);

        let deq = dequant(&[], 1.0);
        assert!(deq.is_empty());
    }

    #[test]
    fn test_quant_dequant_roundtrip() {
        let input = vec![1.0, -0.5, 0.0, 2.0];
        let (q, scale) = quantize_weight_ternary(&input);
        assert_eq!(q.len(), input.len());
        for &val in &q {
            assert!((-1..=1).contains(&val));
        }
        let restored = dequant(&q, scale);
        assert_eq!(restored.len(), input.len());
    }
}
