//! Byte-level tokenizer — port of `ultragraph/tokenize.py`. Lossless for any
//! UTF-8 text, vocab of 256: a token IS a byte. Pairs with a 256-row embedding.

pub struct ByteTokenizer;

impl ByteTokenizer {
    pub const VOCAB_SIZE: usize = 256;

    /// UTF-8 bytes of `text` as ids in `0..=255`.
    pub fn encode(text: &str) -> Vec<u8> {
        text.as_bytes().to_vec()
    }

    /// Ids back to a String (lossy on invalid UTF-8, matching Python `errors=replace`).
    pub fn decode(ids: &[u8]) -> String {
        String::from_utf8_lossy(ids).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_roundtrip() {
        let text = "Hello, world! héllo 🦀";
        let ids = ByteTokenizer::encode(text);
        let decoded = ByteTokenizer::decode(&ids);
        assert_eq!(decoded, text);
    }

    #[test]
    fn test_tokenize_invalid_utf8() {
        let invalid = vec![0xFF, 0xFE];
        let decoded = ByteTokenizer::decode(&invalid);
        assert!(decoded.contains('\u{FFFD}'));
    }
}
