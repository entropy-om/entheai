//! entheai-ultragraph — a native Rust port of the *deployed-inference core* of the
//! user's `ultra-graph` (a byte-graph that is a 1-bit / ternary BitNet-b1.58 LLM).
//!
//! Scope (the "core 1-bit model, end-to-end"): load a deployed `.ugm` module and
//! run it — no training, no autograd, no fp32 masters. Weights are ternary
//! {−1,0,+1}; activations are int8. Training stays in the Python package.
//!
//! Faithfulness contract: this crate reproduces, byte-for-byte, the values in
//! `tests/fixtures/reference.json` from `tests/fixtures/model.ugm` — the same
//! numbers the Python `ultragraph.ugm.UGMFile.run` / `quant` / `pack` /
//! `ByteTokenizer` produce. See `SPEC.md` and `tests/conformance.rs`.
//!
//! Port status: COMPLETE for the deployed-inference core — quant, pack, the
//! byte-tokenizer, and the `.ugm` loader + forward interpreter are ported and the
//! conformance test reproduces the Python reference byte-exact. (Implemented via
//! `agy` on the user's Ultra models — the recursive-development path — and verified
//! here: tests + clippy green, changes scoped to this crate, fixtures untouched.)

pub mod pack;
pub mod quant;
pub mod tokenize;
pub mod ugm;

pub use pack::{pack_ternary, unpack_ternary};
pub use quant::{dequant, quantize_act_int8, quantize_weight_ternary};
pub use tokenize::ByteTokenizer;
pub use ugm::{UgmFile, UgmTree, UgmUltraEdge};
