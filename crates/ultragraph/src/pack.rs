//! Dense ternary bit-packing — port of `ultragraph/pack.py`.
//!
//! Five ternary values pack into one byte via base-3 (3^5 = 243 < 256): map
//! {−1,0,+1} → {0,1,2}, weight by place values [1,3,9,27,81], sum per group of 5.
//! 8 bits / 5 weights = 1.6 bits/weight (a whisker above the log2(3) ≈ 1.58 limit).

/// Base-3 place values for the 5 values packed into each byte.
pub const PLACE_VALUES: [u16; 5] = [1, 3, 9, 27, 81];

/// Pack a ternary array (values in {−1,0,+1}) into bytes, 5 values per byte.
/// The input is flattened; the tail is zero-padded to a multiple of 5. Returns a
/// `Vec<u8>` of length `ceil(q.len() / 5)`.
///
/// Panics/undefined for values outside {−1,0,+1} (Python raises ValueError).
pub fn pack_ternary(q: &[i8]) -> Vec<u8> {
    let _ = q;
    todo!("agy port: (v+1) base-3 encode, 5 per byte; see SPEC.md §pack")
}

/// Inverse of [`pack_ternary`]; returns the first `n` values as `i8` in {−1,0,+1}.
/// For each byte, successive `% 3` digits (then `/= 3`) give the 5 values, `− 1`.
pub fn unpack_ternary(packed: &[u8], n: usize) -> Vec<i8> {
    let _ = (packed, n);
    todo!("agy port: base-3 decode, minus 1; see SPEC.md §pack + reference.json pack")
}
