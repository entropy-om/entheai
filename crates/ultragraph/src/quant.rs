//! Quantization primitives (BitNet b1.58 style) — port of `ultragraph/quant.py`.
//! Weights are ternary {−1,0,+1}; activations are int8. One value per byte.

/// Matches Python `EPS` — keep identical or the fixture scales won't reproduce.
pub const EPS: f32 = 1e-5;

/// Absmean ternary quantization. Returns `(q, scale)` with `q ∈ {−1,0,+1}` and
/// `q[i] as f32 * scale ≈ w[i]`. `scale = mean(|w|) + EPS` (0 for an empty slice).
///
/// Port of `quantize_weight_ternary`: `q = clip(round(w / scale), -1, 1)`.
pub fn quantize_weight_ternary(w: &[f32]) -> (Vec<i8>, f32) {
    let _ = w;
    todo!("agy port: see SPEC.md §quant + reference.json quant_weight")
}

/// Absmax int8 activation quantization. Returns `(q, scale)` with `q ∈ [−127,127]`
/// and `q[i] as f32 * scale ≈ x[i]`. `scale = max(|x|)/127 + EPS`.
///
/// Port of `quantize_act_int8`: `q = clip(round(x / scale), -127, 127)`.
pub fn quantize_act_int8(x: &[f32]) -> (Vec<i8>, f32) {
    let _ = x;
    todo!("agy port: see SPEC.md §quant + reference.json quant_act")
}

/// Inverse of the quantizers: `q[i] as f32 * scale`.
pub fn dequant(q: &[i8], scale: f32) -> Vec<f32> {
    let _ = (q, scale);
    todo!("agy port: q[i] as f32 * scale")
}
