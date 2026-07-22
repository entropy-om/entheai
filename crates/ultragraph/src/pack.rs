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
    if q.is_empty() {
        return Vec::new();
    }
    let num_bytes = q.len().div_ceil(5);
    let mut out = Vec::with_capacity(num_bytes);
    for chunk in q.chunks(5) {
        let mut byte_sum: u16 = 0;
        for (k, &val) in chunk.iter().enumerate() {
            assert!(
                (-1..=1).contains(&val),
                "pack_ternary expects values in {{-1, 0, +1}}"
            );
            let d = (val + 1) as u16;
            byte_sum += d * PLACE_VALUES[k];
        }
        out.push(byte_sum as u8);
    }
    out
}

/// Inverse of [`pack_ternary`]; returns the first `n` values as `i8` in {−1,0,+1}.
/// For each byte, successive `% 3` digits (then `/= 3`) give the 5 values, `− 1`.
pub fn unpack_ternary(packed: &[u8], n: usize) -> Vec<i8> {
    let mut out = Vec::with_capacity(packed.len() * 5);
    for &byte in packed {
        let mut b = byte as u16;
        for _ in 0..5 {
            let val = (b % 3) as i8 - 1;
            out.push(val);
            b /= 3;
        }
    }
    out.truncate(n);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pack_unpack_empty() {
        let packed = pack_ternary(&[]);
        assert!(packed.is_empty());
        let unpacked = unpack_ternary(&packed, 0);
        assert!(unpacked.is_empty());
    }

    #[test]
    fn test_pack_unpack_non_multiple_of_5() {
        let tern = vec![-1, 0, 1, 1, -1, 0, 1]; // length 7
        let packed = pack_ternary(&tern);
        assert_eq!(packed.len(), 2);
        let unpacked = unpack_ternary(&packed, tern.len());
        assert_eq!(unpacked, tern);
    }

    #[test]
    #[should_panic(expected = "pack_ternary expects values in {-1, 0, +1}")]
    fn test_pack_invalid_value() {
        pack_ternary(&[2]);
    }
}

