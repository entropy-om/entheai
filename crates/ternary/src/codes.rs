//! Packed 2-bit ternary code decoding/encoding.
//!
//! ayeOS packs each matrix row's `K` ternary codes into `K / 16` `u32` words,
//! LSB-first: element `j` of a row lives in bits `2 * (j mod 16)` of word
//! `j / 16`. A 2-bit field holds a code in `{0, 1, 2}` which maps to the
//! ternary value `code - 1` ∈ `{-1, 0, +1}`. Code 3 is reserved and must
//! never appear in real data (validated on load).

/// Number of 2-bit ternary codes packed into a single `u32` word.
pub const CODES_PER_WORD: usize = 16;

/// Bits occupied by each code inside a packed word.
pub const BITS_PER_CODE: u32 = 2;

/// Decode a single packed `u32` into its 16 raw codes (`0..=2`), LSB-first.
pub fn decode_word(word: u32, out: &mut [u8; CODES_PER_WORD]) {
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = ((word >> (BITS_PER_CODE * i as u32)) & 0b11) as u8;
    }
}

/// Map a raw code to its ternary value: `0 → -1`, `1 → 0`, `2 → +1`.
pub fn ternary_value(code: u8) -> i8 {
    debug_assert!(code <= 2, "code {code} out of ternary range");
    code as i8 - 1
}

/// Encode 16 raw codes (`0..=2`) into one packed `u32`, LSB-first.
pub fn encode_word(codes: &[u8; CODES_PER_WORD]) -> u32 {
    let mut word = 0u32;
    for (i, c) in codes.iter().enumerate() {
        debug_assert!(*c <= 2, "code {c} out of range at field {i}");
        word |= (*c as u32) << (BITS_PER_CODE * i as u32);
    }
    word
}

/// Decode a packed row (`K / 16` words, row-major over K) into raw codes.
pub fn decode_row(words: &[u32], out: &mut [u8]) {
    assert_eq!(out.len(), words.len() * CODES_PER_WORD);
    for (w, word) in words.iter().enumerate() {
        for j in 0..CODES_PER_WORD {
            out[w * CODES_PER_WORD + j] = ((word >> (BITS_PER_CODE * j as u32)) & 0b11) as u8;
        }
    }
}

/// Encode raw codes into packed words (row-major, LSB-first).
pub fn encode(codes: &[u8], words: &mut [u32]) {
    assert_eq!(codes.len(), words.len() * CODES_PER_WORD);
    words.fill(0);
    for (i, c) in codes.iter().enumerate() {
        debug_assert!(*c <= 2, "code {c} out of range at index {i}");
        words[i / CODES_PER_WORD] |= (*c as u32) << (BITS_PER_CODE * (i % CODES_PER_WORD) as u32);
    }
}

/// Fast whole-word check: does any 2-bit field hold the illegal code 3?
///
/// A field is `3` (0b11) iff both of its bits are set, so `word & (word >> 1)`
/// has bit `2i` set exactly when field `i` is 3; the `0x5555_5555` mask keeps
/// only the even (field-start) bits.
pub fn word_has_illegal_code(word: u32) -> bool {
    (word & (word >> 1)) & 0x5555_5555 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_decode_round_trip() {
        let codes: [u8; CODES_PER_WORD] = [0, 2, 1, 0, 2, 0, 1, 1, 2, 2, 0, 1, 0, 2, 1, 0];
        let word = encode_word(&codes);
        let mut decoded = [0u8; CODES_PER_WORD];
        decode_word(word, &mut decoded);
        assert_eq!(decoded, codes);
    }

    #[test]
    fn bit_layout_is_lsb_first() {
        // codes [0, 1, 2, 0, ...] -> field0=00, field1=01, field2=10, rest 00
        let mut codes = [0u8; CODES_PER_WORD];
        codes[1] = 1;
        codes[2] = 2;
        let word = encode_word(&codes);
        assert_eq!(word, (1u32 << 2) | (2u32 << 4));
    }

    #[test]
    fn row_round_trip_and_illegal_code_detection() {
        // a 64-code row packs into 4 words
        let mut codes = vec![0u8; 64];
        for (i, c) in codes.iter_mut().enumerate() {
            *c = (i % 3) as u8; // 0, 1, 2 repeating
        }
        let mut words = vec![0u32; 4];
        encode(&codes, &mut words);
        let mut decoded = vec![0u8; 64];
        decode_row(&words, &mut decoded);
        assert_eq!(decoded, codes);

        // ternary value mapping
        assert_eq!(ternary_value(0), -1);
        assert_eq!(ternary_value(1), 0);
        assert_eq!(ternary_value(2), 1);

        // illegal-code detection: 00/01/10 fine, 11 rejected
        assert!(!word_has_illegal_code(0u32));
        assert!(!word_has_illegal_code(0x5555_5555)); // every field = 1
        assert!(!word_has_illegal_code(0xAAAA_AAAA)); // every field = 2
        assert!(word_has_illegal_code(0xFFFF_FFFF)); // every field = 3
        assert!(word_has_illegal_code(0b11)); // lowest field = 3
        assert!(word_has_illegal_code(0b11 << 30)); // highest field = 3
    }
}
