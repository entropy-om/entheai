//! Native ternary (BitNet b1.58) matrix support — Phase 1.
//!
//! Loads the ayeOS ternary matrices (packed 2-bit codes + per-group scales),
//! decodes them, and runs pure-Rust ternary GEMM with activations untouched.
//! No entheai routing yet — this crate stands alone behind a minimal API.
//!
//! - [`codes`]: packed 2-bit decode/encode (LSB-first, 16 codes per `u32`)
//! - [`loader`]: `AyeosMatrix` / `AyeosIndex` loading with strict validation
//! - [`gemm`]: `ternary_matmul` — the reference CPU kernel
//! - [`quantize`]: reference quantizer/dequantizer (test-only round-trips)
#![forbid(unsafe_code)]

pub mod codes;
pub mod gemm;
pub mod loader;
pub mod quantize;
