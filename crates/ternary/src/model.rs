//! Full Qwen2.5-0.5B transformer runner over ayeOS ternary weights.
//!
//! The 168 ternary matrices form the linear projections of a standard
//! Qwen2.5-0.5B-Instruct: 24 layers × (q/k/v/o + mlp up/gate/down), with
//! activations UNQUANTIZED (`ternary_matmul`). Embeddings and RMSNorm weights
//! come from the exported `embeddings.f16` / `norms.f32` sidecars; `lm_head`
//! is tied to the embeddings.
//!
//! Architecture pin (verified vs. HF config + oracle review):
//! - GQA: 14 query heads / 2 KV heads, head_dim 64
//! - RoPE: HF `rotate_half` (NOT GPT-J interleaved), theta 1e6
//! - RMSNorm eps 1e-6, SiLU MLP (up/gate 4864, down 896)
//! - KV cache per layer `[kv_heads, head_dim, seq]` f32
//!
//! NOTE (oracle): the trained checkpoint's `BitLinear` forward also applies a
//! per-projection activation RMSNorm + int8 `activation_quant` + q/k/v biases.
//! Those are NOT in this runner yet — the golden-logits gate decides whether
//! they are required. See `docs/` in the deepwork plan.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rayon::prelude::*;

use crate::gemm::ternary_matmul_batch;
use crate::loader::{self, AyeosMatrix};

/// Qwen2.5-0.5B architectural constants (verified against HF config.json).
pub const HIDDEN: usize = 896;
pub const LAYERS: usize = 24;
pub const Q_HEADS: usize = 14;
pub const KV_HEADS: usize = 2;
pub const HEAD_DIM: usize = 64;
pub const INTERMEDIATE: usize = 4864;
pub const ROPE_THETA: f32 = 1_000_000.0;
pub const RMS_EPS: f32 = 1e-6;
pub const VOCAB: usize = 151_936;
pub const MAX_POS: usize = 32_768;

/// Number of `[kv_heads × head_dim]` cache rows per layer.
const KV_ROWS: usize = KV_HEADS * HEAD_DIM;

/// Per-position RoPE cos/sin tables (precomputed to `MAX_POS` at load).
struct RoPECache {
    cos: Vec<Vec<f32>>, // [pos][head_dim/2]
    sin: Vec<Vec<f32>>,
}

impl RoPECache {
    fn new(theta: f32) -> Self {
        let freqs: Vec<f32> = (0..HEAD_DIM / 2)
            .map(|i| 1.0 / theta.powf(2.0 * i as f32 / HEAD_DIM as f32))
            .collect();
        let mut cos = Vec::with_capacity(MAX_POS);
        let mut sin = Vec::with_capacity(MAX_POS);
        for pos in 0..MAX_POS {
            let p = pos as f32;
            let mut c = vec![0.0f32; HEAD_DIM / 2];
            let mut s = vec![0.0f32; HEAD_DIM / 2];
            for (i, f) in freqs.iter().enumerate() {
                let a = p * f;
                c[i] = a.cos();
                s[i] = a.sin();
            }
            cos.push(c);
            sin.push(s);
        }
        Self { cos, sin }
    }

    /// Apply HF-style `rotate_half` RoPE to one 64-dim head vector at `pos`.
    fn apply(&self, head: &mut [f32], pos: usize) {
        let c = &self.cos[pos];
        let s = &self.sin[pos];
        let half = HEAD_DIM / 2;
        for i in 0..half {
            let x1 = head[i];
            let x2 = head[i + half];
            head[i] = x1 * c[i] - x2 * s[i];
            head[i + half] = x2 * c[i] + x1 * s[i];
        }
    }
}

/// Per-layer KV cache: `k[layer][kv_rows]` → `[seq]`, same for `v`.
///
/// Shape per layer is `[2, 64, seq]` f32 (the oracle-pinned layout), stored
/// as `KV_ROWS` per-(kv-head, dim) rows so appends are cheap.
pub struct KVCache {
    k: Vec<Vec<Vec<f32>>>,
    v: Vec<Vec<Vec<f32>>>,
    /// Total tokens cached so far (prefix length).
    pub len: usize,
}

impl KVCache {
    /// Fresh cache for `layers` transformer layers.
    pub fn new() -> Self {
        Self {
            k: vec![vec![Vec::new(); KV_ROWS]; LAYERS],
            v: vec![vec![Vec::new(); KV_ROWS]; LAYERS],
            len: 0,
        }
    }
}

impl Default for KVCache {
    fn default() -> Self {
        Self::new()
    }
}

/// The loaded quantal ternary model: matrices + embeddings + norms.
pub struct TernaryModel {
    weights: Vec<AyeosMatrix>,
    matrix_idx: HashMap<String, usize>,
    /// `embeddings.f16`, raw fp16 bits, row-major `[VOCAB × HIDDEN]`.
    embeddings: Vec<u16>,
    /// `norms.f32`, `[49 × HIDDEN]`: layers 0..23 input_layernorm then
    /// post_attention_layernorm, row 48 = final norm.
    norms: Vec<f32>,
    rope: RoPECache,
}

impl TernaryModel {
    /// Load the full model from `model_dir` (index.json + m*.json +
    /// embeddings.f16 + norms.f32).
    pub fn load(model_dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let dir = model_dir.as_ref();
        let weights = loader::load_dir(dir)
            .map_err(|e| anyhow::anyhow!("loading ayeOS matrices from {}: {e}", dir.display()))?;
        if weights.len() != 168 {
            anyhow::bail!(
                "expected 168 ayeOS matrices, found {} in {}",
                weights.len(),
                dir.display()
            );
        }
        let mut matrix_idx = HashMap::with_capacity(weights.len());
        for (i, m) in weights.iter().enumerate() {
            matrix_idx.insert(m.name.clone(), i);
        }

        let emb_bytes = fs::read(dir.join("embeddings.f16"))
            .map_err(|e| anyhow::anyhow!("cannot read embeddings.f16: {e}"))?;
        let embeddings: Vec<u16> = emb_bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        anyhow::ensure!(
            embeddings.len() == VOCAB * HIDDEN,
            "embeddings.f16 has {} elements, expected {VOCAB}×{HIDDEN} = {}",
            embeddings.len(),
            VOCAB * HIDDEN
        );

        let norm_bytes = fs::read(dir.join("norms.f32"))
            .map_err(|e| anyhow::anyhow!("cannot read norms.f32: {e}"))?;
        let norms: Vec<f32> = norm_bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        anyhow::ensure!(
            norms.len() == (2 * LAYERS + 1) * HIDDEN,
            "norms.f32 has {} elements, expected {}",
            norms.len(),
            (2 * LAYERS + 1) * HIDDEN
        );

        Ok(Self {
            weights,
            matrix_idx,
            embeddings,
            norms,
            rope: RoPECache::new(ROPE_THETA),
        })
    }

    /// Batch matmul `x` (S×K) against matrix `proj` of `layer`.
    fn matmul(&self, x: &[f32], layer: usize, proj: &str) -> anyhow::Result<Vec<f32>> {
        let name = format!("model.layers.{layer}.{proj}");
        let idx = *self
            .matrix_idx
            .get(&name)
            .ok_or_else(|| anyhow::anyhow!("missing matrix {name}"))?;
        let m = &self.weights[idx];
        let batch = x.len() / m.in_features;
        let mut out = vec![0.0f32; batch * m.dim];
        ternary_matmul_batch(x, m, &mut out);
        Ok(out)
    }

    /// RMSNorm rows (S×HIDDEN) with per-row normalization; `w` is `[HIDDEN]`.
    fn rmsnorm_rows(&self, rows: &[f32], w: &[f32]) -> Vec<f32> {
        let s = rows.len() / HIDDEN;
        let mut out = vec![0.0f32; rows.len()];
        for i in 0..s {
            let x = &rows[i * HIDDEN..(i + 1) * HIDDEN];
            let o = &mut out[i * HIDDEN..(i + 1) * HIDDEN];
            let mut ss = 0.0f32;
            for v in x {
                ss += v * v;
            }
            let inv = 1.0 / (ss / HIDDEN as f32 + RMS_EPS).sqrt();
            for d in 0..HIDDEN {
                o[d] = x[d] * w[d] * inv;
            }
        }
        out
    }

    /// Embed `tokens` into f32 rows (S×HIDDEN) from the fp16 embedding table.
    fn embed_rows(&self, tokens: &[u32]) -> anyhow::Result<Vec<f32>> {
        let mut out = vec![0.0f32; tokens.len() * HIDDEN];
        for (i, t) in tokens.iter().enumerate() {
            let t = *t as usize;
            if t >= VOCAB {
                anyhow::bail!("token id {t} out of vocab range 0..{VOCAB}");
            }
            let e = &self.embeddings[t * HIDDEN..(t + 1) * HIDDEN];
            let row = &mut out[i * HIDDEN..(i + 1) * HIDDEN];
            for (d, bits) in e.iter().enumerate() {
                row[d] = half::f16::from_bits(*bits).to_f32();
            }
        }
        Ok(out)
    }

    /// lm_head: dot one post-norm hidden row against every embedding row.
    /// Returns `[VOCAB]` logits.
    fn lm_head(&self, hidden: &[f32]) -> Vec<f32> {
        debug_assert_eq!(hidden.len(), HIDDEN);
        let mut logits = vec![0.0f32; VOCAB];
        logits
            .par_chunks_mut(2048)
            .enumerate()
            .for_each(|(chunk_i, chunk)| {
                let base = chunk_i * 2048;
                for (j, v) in (base..base + chunk.len()).enumerate() {
                    let e = &self.embeddings[v * HIDDEN..(v + 1) * HIDDEN];
                    let mut acc = 0.0f32;
                    for d in 0..HIDDEN {
                        acc += hidden[d] * half::f16::from_bits(e[d]).to_f32();
                    }
                    chunk[j] = acc;
                }
            });
        logits
    }

    /// Forward `tokens` (S ≥ 1) through all layers, appending to `cache`.
    ///
    /// Returns the final-norm hidden rows `[S × HIDDEN]` (no lm_head — call
    /// [`TernaryModel::lm_head`] on the row you need logits for).
    fn forward_hidden(&self, cache: &mut KVCache, tokens: &[u32]) -> anyhow::Result<Vec<f32>> {
        let s = tokens.len();
        anyhow::ensure!(cache.len + s <= MAX_POS, "context exceeds {MAX_POS}");
        let mut hidden = self.embed_rows(tokens)?;

        // All layers see the same tokens; the global prefix position before
        // this call is `start` (cache.len) and each layer's rows grow to
        // `start + s` (cache.len is bumped once, after the layer loop).
        for layer in 0..LAYERS {
            let in_norm = &self.norms[(2 * layer) * HIDDEN..(2 * layer + 1) * HIDDEN];
            let post_norm = &self.norms[(2 * layer + 1) * HIDDEN..(2 * layer + 2) * HIDDEN];

            // ---- attention ----
            let normed = self.rmsnorm_rows(&hidden, in_norm);
            let q = self.matmul(&normed, layer, "self_attn.q_proj")?; // S×896
            let k = self.matmul(&normed, layer, "self_attn.k_proj")?; // S×128
            let v = self.matmul(&normed, layer, "self_attn.v_proj")?; // S×128

            let mut q_heads = reshape_heads(&q, Q_HEADS); // [S][14][64]
            let mut k_heads = reshape_heads(&k, KV_HEADS); // [S][2][64]
            let v_heads = reshape_heads(&v, KV_HEADS); // [S][2][64]

            let start = cache.len;
            for (i, pos) in (start..start + s).enumerate() {
                for head in q_heads[i].iter_mut() {
                    self.rope.apply(head, pos);
                }
                for head in k_heads[i].iter_mut() {
                    self.rope.apply(head, pos);
                }
            }
            append_cache(&mut cache.k[layer], &k_heads);
            append_cache(&mut cache.v[layer], &v_heads);

            let attn = self.attention(layer, &q_heads, cache, start, s); // [S][896]
            let mut o = self.matmul(&attn, layer, "self_attn.o_proj")?;
            for i in 0..s {
                let row = &mut o[i * HIDDEN..(i + 1) * HIDDEN];
                for d in 0..HIDDEN {
                    row[d] += hidden[i * HIDDEN + d];
                }
            }
            hidden = o;

            // ---- mlp ----
            let normed2 = self.rmsnorm_rows(&hidden, post_norm);
            let up = self.matmul(&normed2, layer, "mlp.up_proj")?; // S×4864
            let gate = self.matmul(&normed2, layer, "mlp.gate_proj")?; // S×4864
            let mut act = Vec::with_capacity(s * INTERMEDIATE);
            for i in 0..s * INTERMEDIATE {
                act.push(silu(gate[i]) * up[i]);
            }
            let mut down = self.matmul(&act, layer, "mlp.down_proj")?; // S×896
            for i in 0..s {
                let row = &mut down[i * HIDDEN..(i + 1) * HIDDEN];
                for d in 0..HIDDEN {
                    row[d] += hidden[i * HIDDEN + d];
                }
            }
            hidden = down;
        }

        let final_norm = &self.norms[(2 * LAYERS) * HIDDEN..];
        cache.len += s; // global prefix length grows by exactly one batch
        Ok(self.rmsnorm_rows(&hidden, final_norm))
    }

    /// GQA attention for `s` new tokens. `q_heads[si][qh][d]`; the layer's KV
    /// rows in `cache` already include the new tokens. Returns flat `S×896`.
    fn attention(
        &self,
        layer: usize,
        q_heads: &[Vec<Vec<f32>>],
        cache: &KVCache,
        start: usize,
        s: usize,
    ) -> Vec<f32> {
        let group = Q_HEADS / KV_HEADS; // 7 q-heads share one kv-head
        let inv_sqrt = 1.0 / (HEAD_DIM as f32).sqrt();
        let kc = &cache.k[layer];
        let vc = &cache.v[layer];
        let mut out = vec![0.0f32; s * Q_HEADS * HEAD_DIM];

        for si in 0..s {
            let past = start + si; // attend to j in 0..=past (causal)
            for qh in 0..Q_HEADS {
                let g = qh / group;
                let mut scores = vec![0.0f32; past + 1];
                for j in 0..=past {
                    let mut acc = 0.0f32;
                    for d in 0..HEAD_DIM {
                        acc += q_heads[si][qh][d] * kc[g * HEAD_DIM + d][j];
                    }
                    scores[j] = acc * inv_sqrt;
                }
                softmax_in_place(&mut scores);
                for d in 0..HEAD_DIM {
                    let mut acc = 0.0f32;
                    for j in 0..=past {
                        acc += scores[j] * vc[g * HEAD_DIM + d][j];
                    }
                    out[si * Q_HEADS * HEAD_DIM + qh * HEAD_DIM + d] = acc;
                }
            }
        }
        out
    }

    /// Full prefill: final-token logits for `tokens` (fresh cache).
    pub fn logits_last(&self, tokens: &[u32]) -> anyhow::Result<Vec<f32>> {
        let mut cache = KVCache::new();
        let hidden = self.forward_hidden(&mut cache, tokens)?;
        Ok(self.lm_head(&hidden[HIDDEN * (tokens.len() - 1)..]))
    }

    /// Greedy decode `max_new` tokens after `prompt`, stopping on a stop token
    /// (`<|endoftext|>` / `<|im_end|>`). Prompt tokens are not returned.
    pub fn generate(&self, prompt: &[u32], max_new: usize) -> anyhow::Result<Vec<u32>> {
        let mut cache = KVCache::new();
        self.forward_hidden(&mut cache, prompt)?;
        let mut out = Vec::new();
        let mut last = *prompt
            .last()
            .ok_or_else(|| anyhow::anyhow!("empty prompt"))?;
        for _ in 0..max_new {
            let hidden = self.forward_hidden(&mut cache, &[last])?;
            let logits = self.lm_head(&hidden);
            let next = argmax(&logits);
            if next == crate::tokenizer::STOP_END_OF_TEXT
                || next == crate::tokenizer::STOP_IM_END
            {
                break;
            }
            out.push(next);
            last = next;
        }
        Ok(out)
    }
}

/// Reshape flat `S × (heads × head_dim)` into `[S][heads][head_dim]`.
fn reshape_heads(x: &[f32], heads: usize) -> Vec<Vec<Vec<f32>>> {
    let s = x.len() / (heads * HEAD_DIM);
    let mut out = vec![vec![vec![0.0f32; HEAD_DIM]; heads]; s];
    for si in 0..s {
        for h in 0..heads {
            out[si][h].copy_from_slice(
                &x[si * heads * HEAD_DIM + h * HEAD_DIM
                    ..si * heads * HEAD_DIM + (h + 1) * HEAD_DIM],
            );
        }
    }
    out
}

/// Append `heads` (S×kv×dim) to the per-(kv,dim) cache rows.
fn append_cache(rows: &mut [Vec<f32>], heads: &[Vec<Vec<f32>>]) {
    for token in heads {
        for (kv, head) in token.iter().enumerate() {
            for (d, val) in head.iter().enumerate() {
                rows[kv * HEAD_DIM + d].push(*val);
            }
        }
    }
}

/// In-place softmax over a score row.
fn softmax_in_place(scores: &mut [f32]) {
    let max = scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0f32;
    for x in scores.iter_mut() {
        *x = (*x - max).exp();
        sum += *x;
    }
    let inv = 1.0 / sum;
    for x in scores.iter_mut() {
        *x *= inv;
    }
}

/// SiLU activation: `x · sigmoid(x)`.
fn silu(x: f32) -> f32 {
    x * (1.0 / (1.0 + (-x).exp()))
}

/// Index of the largest logit (greedy decode).
fn argmax(logits: &[f32]) -> u32 {
    let mut best = 0usize;
    let mut best_v = logits[0];
    for (i, v) in logits.iter().enumerate().skip(1) {
        if *v > best_v {
            best_v = *v;
            best = i;
        }
    }
    best as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Model dir for tests: `AYEOS_DATA_DIR` env override, else relative to the
    /// crate root (same convention as `loader::tests`).
    fn data_dir() -> PathBuf {
        if let Ok(dir) = std::env::var("AYEOS_DATA_DIR") {
            return PathBuf::from(dir);
        }
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../pocoo.vaked.dev/demos/quantal")
    }

    #[test]
    fn loads_full_model_from_disk() {
        let m = TernaryModel::load(data_dir())
            .unwrap_or_else(|e| panic!("model load failed — set AYEOS_DATA_DIR: {e}"));
        assert_eq!(m.weights.len(), 168);
        assert_eq!(m.embeddings.len(), VOCAB * HIDDEN);
        assert_eq!(m.norms.len(), (2 * LAYERS + 1) * HIDDEN);
        // All seven projections of layer 0 resolve.
        for proj in [
            "self_attn.q_proj",
            "self_attn.k_proj",
            "self_attn.v_proj",
            "self_attn.o_proj",
            "mlp.up_proj",
            "mlp.gate_proj",
            "mlp.down_proj",
        ] {
            let name = format!("model.layers.0.{proj}");
            assert!(m.matrix_idx.contains_key(&name), "missing {name}");
        }
    }

    #[test]
    fn rmsnorm_matches_manual_reference() {
        let m = TernaryModel::load(data_dir()).unwrap();
        // Two full 896-length rows: row 0 = [1,2,3,4,0,…], row 1 = [-1,0.5,0.25,0.125,0,…].
        let mut rows = vec![0.0f32; 2 * HIDDEN];
        rows[0] = 1.0;
        rows[1] = 2.0;
        rows[2] = 3.0;
        rows[3] = 4.0;
        rows[HIDDEN] = -1.0;
        rows[HIDDEN + 1] = 0.5;
        rows[HIDDEN + 2] = 0.25;
        rows[HIDDEN + 3] = 0.125;
        let w = vec![0.5f32; HIDDEN];
        let out = m.rmsnorm_rows(&rows, &w);
        // out = x·w / sqrt(mean(x²) + eps); mean([1,2,3,4,0…]²) = 30/896
        let denom0 = (30.0f32 / HIDDEN as f32 + RMS_EPS).sqrt();
        for (i, v) in out.iter().take(4).enumerate() {
            let expected = rows[i] * 0.5 / denom0;
            assert!((v - expected).abs() < 1e-6, "row0 idx {i}: {v} vs {expected}");
        }
        let denom1 = ((1.0 + 0.25 + 0.0625 + 0.015625) / HIDDEN as f32 + RMS_EPS).sqrt();
        for (i, v) in out.iter().enumerate().take(HIDDEN + 4).skip(HIDDEN) {
            let expected = rows[i] * 0.5 / denom1;
            assert!((v - expected).abs() < 1e-6, "row1 idx {i}: {v} vs {expected}");
        }
    }

    #[test]
    fn rope_rotate_half_matches_manual_reference() {
        let rope = RoPECache::new(ROPE_THETA);
        // head_dim 64, first frequency = theta^0 = 1.0; at pos 1 angle = 1.0.
        let mut head = vec![0.0f32; HEAD_DIM];
        head[0] = 1.0;
        head[HEAD_DIM / 2] = 0.5;
        rope.apply(&mut head, 1);
        let c0 = 1.0f32.cos();
        let s0 = 1.0f32.sin();
        // x1=1, x2=0.5: out[0] = 1*c0 - 0.5*s0; out[32] = 0.5*c0 + 1*s0
        let exp0 = 1.0 * c0 - 0.5 * s0;
        let exp32 = 0.5 * c0 + 1.0 * s0;
        assert!((head[0] - exp0).abs() < 1e-6);
        assert!((head[HEAD_DIM / 2] - exp32).abs() < 1e-6);
    }

    #[test]
    fn silu_and_softmax_match_reference() {
        assert!((silu(0.0) - 0.0).abs() < 1e-7);
        assert!((silu(1.0) - 0.7310586).abs() < 1e-6);
        let mut scores = vec![1.0f32, 2.0, 3.0];
        softmax_in_place(&mut scores);
        let s: f32 = scores.iter().sum();
        assert!((s - 1.0).abs() < 1e-6);
        assert!(scores[2] > scores[1] && scores[1] > scores[0]);
    }

    #[test]
    fn forward_produces_finite_logits_for_short_prompt() {
        // Real forward over the full model — the golden-logits gate is the
        // acceptance test; this just pins shape + finiteness + determinism.
        let m = TernaryModel::load(data_dir()).unwrap();
        let tok = crate::tokenizer::ChatTokenizer::load(data_dir()).unwrap();
        let prompt = tok
            .apply_chat_template(&[("system", "You are a helpful assistant."), ("user", "Hi.")]);
        let ids = tok.encode(&prompt).unwrap();
        assert!(!ids.is_empty());
        let logits = m.logits_last(&ids).unwrap();
        assert_eq!(logits.len(), VOCAB);
        assert!(logits.iter().all(|v| v.is_finite()));
        // Deterministic: rerun gives identical logits.
        let again = m.logits_last(&ids).unwrap();
        assert_eq!(logits, again);
        // Argmax is a real vocab id (not garbage beyond the table).
        let best = argmax(&logits);
        assert!((best as usize) < VOCAB);
    }

    /// Golden-logits gate (pinned regression): the final-token logits for the
    /// two fixed gate prompts MUST match the MLX-QUANT fork reference within
    /// ~1e-2. Values below were captured 2026-08-10 from
    /// `quantal_golden_logits.py` (mlx 0.32.1.dev, vanilla forward); the full
    /// acceptance run lives in `MLX-QUANT/scripts/quantal_compare_logits.py`.
    ///
    /// prompt 1 (31 tok) top-5: 145216=43.461, 81129=39.460, 143145=39.188,
    ///                          145139=38.695, 146656=38.402
    /// prompt 2 (40 tok) top-5: 72612=19.159, 33298=19.024, 41585=18.198,
    ///                          89417=17.906, 40330=17.672
    const GOLDEN_GATE_TOP3_P1: [(u32, f32); 3] = [
        (145216, 43.4608),
        (81129, 39.4597),
        (143145, 39.1882),
    ];
    const GOLDEN_GATE_TOP3_P2: [(u32, f32); 3] = [
        (72612, 19.1590),
        (33298, 19.0239),
        (41585, 18.1981),
    ];
    /// Gate tolerance: the gate's ~1e-2 with a 4× margin for the pinned values
    /// (observed max_abs was 1.3e-3).
    const GOLDEN_GATE_TOL: f32 = 5e-3;

    fn gate_prompt_tokens(
        tok: &crate::tokenizer::ChatTokenizer,
        system: &str,
        user: &str,
    ) -> Vec<u32> {
        let prompt = tok.apply_chat_template(&[("system", system), ("user", user)]);
        tok.encode(&prompt).unwrap()
    }

    #[test]
    fn golden_logits_gate_matches_mlx_reference() {
        // The acceptance gate for the whole native-ternary effort: Rust runner
        // vs the MLX-QUANT fork ternary forward on the two fixed prompts.
        let m = TernaryModel::load(data_dir()).unwrap();
        let tok = crate::tokenizer::ChatTokenizer::load(data_dir()).unwrap();

        let p1 = gate_prompt_tokens(
            &tok,
            "You are a helpful assistant.",
            "What is the capital of France? Answer in one word.",
        );
        assert_eq!(p1.len(), 31, "prompt 1 must tokenize to 31 ids (sync with Python gate)");
        let l1 = m.logits_last(&p1).unwrap();
        let (ref_best1, ref_top1) = GOLDEN_GATE_TOP3_P1[0];
        assert_eq!(argmax(&l1), ref_best1, "prompt 1 argmax token");
        for (tid, ref_val) in GOLDEN_GATE_TOP3_P1 {
            let got = l1[tid as usize];
            assert!(
                (got - ref_val).abs() <= GOLDEN_GATE_TOL,
                "prompt 1 token {tid}: got {got}, ref {ref_val}"
            );
        }
        assert!((l1[ref_best1 as usize] - ref_top1).abs() <= GOLDEN_GATE_TOL);

        let p2 = gate_prompt_tokens(
            &tok,
            "You are Qwen, created by Alibaba Cloud. You are a helpful assistant.",
            "Explain the concept of recursion in one short paragraph.",
        );
        assert_eq!(p2.len(), 40, "prompt 2 must tokenize to 40 ids (sync with Python gate)");
        let l2 = m.logits_last(&p2).unwrap();
        let (ref_best2, _) = GOLDEN_GATE_TOP3_P2[0];
        assert_eq!(argmax(&l2), ref_best2, "prompt 2 argmax token");
        for (tid, ref_val) in GOLDEN_GATE_TOP3_P2 {
            let got = l2[tid as usize];
            assert!(
                (got - ref_val).abs() <= GOLDEN_GATE_TOL,
                "prompt 2 token {tid}: got {got}, ref {ref_val}"
            );
        }
    }

    #[test]
    fn generate_decodes_deterministically_and_stops() {
        // Greedy decode smoke over the real model: deterministic output, never
        // emits a stop token, honors the requested max_new length.
        let m = TernaryModel::load(data_dir()).unwrap();
        let tok = crate::tokenizer::ChatTokenizer::load(data_dir()).unwrap();
        let ids = gate_prompt_tokens(&tok, "You are a helpful assistant.", "Hi.");

        let out = m.generate(&ids, 6).unwrap();
        assert!(out.len() <= 6, "must not exceed max_new");
        assert!(
            out.iter().all(|t| !tok.is_stop(*t)),
            "generated tokens must never include a stop token: {out:?}"
        );
        assert!(
            out.iter().all(|t| (*t as usize) < VOCAB),
            "generated tokens must be valid vocab ids"
        );
        // Deterministic: same seed state → identical output.
        let again = m.generate(&ids, 6).unwrap();
        assert_eq!(out, again);
    }
}
