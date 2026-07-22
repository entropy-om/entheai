//! Byte-level tokenizer — port of `ultragraph/tokenize.py`. Lossless for any
//! UTF-8 text, vocab of 256: a token IS a byte. Pairs with a 256-row embedding.

pub struct ByteTokenizer;

impl ByteTokenizer {
    pub const VOCAB_SIZE: usize = 256;

    /// UTF-8 bytes of `text` as ids in `0..=255`.
    pub fn encode(text: &str) -> Vec<u8> {
        let _ = text;
        todo!("agy port: text.as_bytes().to_vec(); see reference.json tokenize")
    }

    /// Ids back to a String (lossy on invalid UTF-8, matching Python `errors=replace`).
    pub fn decode(ids: &[u8]) -> String {
        let _ = ids;
        todo!("agy port: String::from_utf8_lossy over the bytes")
    }
}
