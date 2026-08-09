//! Native ternary (BitNet b1.58) matrix support.
//!
//! Phase 1: loads the ayeOS ternary matrices (packed 2-bit codes + per-group
//! scales), decodes them, and runs pure-Rust ternary GEMM with activations
//! untouched. Phase 2 adds the full Qwen2.5 transformer runner + chat
//! tokenizer (still standalone — the adk-rust adapter lives in crates/core).
//!
//! - [`codes`]: packed 2-bit decode/encode (LSB-first, 16 codes per `u32`)
//! - [`loader`]: `AyeosMatrix` / `AyeosIndex` loading with strict validation
//! - [`gemm`]: `ternary_matmul` — the reference CPU kernel
//! - [`quantize`]: reference quantizer/dequantizer (test-only round-trips)
//! - [`model`]: `TernaryModel` — Qwen2.5-0.5B transformer runner over ayeOS weights
//! - [`tokenizer`]: `ChatTokenizer` — vendored `tokenizer.json` + hand-rolled
//!   Qwen `<|im_start|>` chat template, dual stop tokens
#![forbid(unsafe_code)]

pub mod codes;
pub mod gemm;
pub mod loader;
pub mod model;
pub mod quantize;
pub mod tokenizer;
