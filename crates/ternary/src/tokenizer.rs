//! Chat tokenizer for the quantal ternary model (Qwen2.5-0.5B-Instruct).
//!
//! Loads a vendored `tokenizer.json` via the `tokenizers` crate and applies
//! the Qwen `<|im_start|>` / `<|im_end|>` chat template **by hand** (~15 lines,
//! deliberately NOT the Jinja template — no runtime template engine).
//!
//! Stop conditions (oracle review correction #1): generation stops on BOTH
//! `151643` (`<|endoftext|>`) AND `151645` (`<|im_end|>`). Stop tokens are
//! never decoded into text.

use std::path::Path;

use tokenizers::Tokenizer;

/// `<|endoftext|>` — hard stop token.
pub const STOP_END_OF_TEXT: u32 = 151643;
/// `<|im_start|>` — chat role marker (not a stop).
pub const IM_START: u32 = 151644;
/// `<|im_end|>` — chat turn terminator; hard stop token.
pub const STOP_IM_END: u32 = 151645;

/// Wrapper over a `tokenizers` `Tokenizer` + the hand-rolled chat template.
pub struct ChatTokenizer {
    tokenizer: Tokenizer,
}

impl ChatTokenizer {
    /// Load `tokenizer.json` from `model_dir`.
    pub fn load(model_dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = model_dir.as_ref().join("tokenizer.json");
        let tokenizer = Tokenizer::from_file(&path)
            .map_err(|e| anyhow::anyhow!("cannot load tokenizer at {}: {e}", path.display()))?;
        Ok(Self { tokenizer })
    }

    /// Encode `text` into token ids (special tokens inline, no synthetic BOS).
    pub fn encode(&self, text: &str) -> anyhow::Result<Vec<u32>> {
        let enc = self
            .tokenizer
            .encode(text, false)
            .map_err(|e| anyhow::anyhow!("encode failed: {e}"))?;
        Ok(enc.get_ids().to_vec())
    }

    /// Decode ids back to text. Callers must never pass stop-token ids here.
    pub fn decode(&self, ids: &[u32]) -> anyhow::Result<String> {
        self.tokenizer
            .decode(ids, true)
            .map_err(|e| anyhow::anyhow!("decode failed: {e}"))
    }

    /// Is `id` a hard stop token (`<|endoftext|>` or `<|im_end|>`)?
    pub fn is_stop(&self, id: u32) -> bool {
        id == STOP_END_OF_TEXT || id == STOP_IM_END
    }

    /// Map an adk/OpenAI-style content role to a Qwen chat role.
    ///
    /// Oracle review correction #4: `user` → `user`, `model` → `assistant`,
    /// `system` → `system`, and `function`/`tool` turns are SKIPPED (a
    /// function result is not a chat message this model can consume).
    fn map_role(role: &str) -> Option<&'static str> {
        match role {
            "user" => Some("user"),
            "model" => Some("assistant"),
            "assistant" => Some("assistant"),
            "system" => Some("system"),
            "function" | "tool" => None,
            other => {
                // Unknown roles degrade to user text rather than dropping content.
                let _ = other;
                Some("user")
            }
        }
    }

    /// Apply the hand-rolled Qwen `<|im_start|>` chat template.
    ///
    /// `messages` is `(role, text)` pairs; roles are mapped via
    /// [`ChatTokenizer::map_role`]. The template ends with an open
    /// `<|im_start|>assistant\n` prefix so the next decoded token starts the
    /// assistant turn.
    pub fn apply_chat_template(&self, messages: &[(impl AsRef<str>, impl AsRef<str>)]) -> String {
        let mut out = String::new();
        for (role, text) in messages {
            let Some(role) = Self::map_role(role.as_ref()) else {
                continue;
            };
            out.push_str("<|im_start|>");
            out.push_str(role);
            out.push('\n');
            out.push_str(text.as_ref());
            out.push_str("<|im_end|>\n");
        }
        out.push_str("<|im_start|>assistant\n");
        out
    }
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
    fn loads_vendored_tokenizer_json() {
        let tok = ChatTokenizer::load(data_dir())
            .unwrap_or_else(|e| panic!("tokenizer load failed — set AYEOS_DATA_DIR: {e}"));
        assert!(tok.is_stop(STOP_END_OF_TEXT));
        assert!(tok.is_stop(STOP_IM_END));
        assert!(!tok.is_stop(IM_START));
        assert!(!tok.is_stop(0));
    }

    #[test]
    fn chat_template_produces_expected_roles() {
        let tok = ChatTokenizer::load(data_dir()).unwrap();
        let text = tok.apply_chat_template(&[
            ("system", "You are a helpful assistant."),
            ("user", "Hello!"),
            ("model", "Hi there!"),
            ("function", "ignored tool result"),
        ]);
        assert_eq!(
            text,
            "<|im_start|>system\nYou are a helpful assistant.<|im_end|>\n\
             <|im_start|>user\nHello!<|im_end|>\n\
             <|im_start|>assistant\nHi there!<|im_end|>\n\
             <|im_start|>assistant\n"
        );
    }

    #[test]
    fn template_encode_includes_special_token_ids() {
        let tok = ChatTokenizer::load(data_dir()).unwrap();
        let text = tok.apply_chat_template(&[("user", "Hello")]);
        let ids = tok.encode(&text).unwrap();
        // The template's literal <|im_start|> / <|im_end|> must tokenize to the
        // reserved ids (151644 / 151645), and generation must stop on the latter.
        assert!(ids.contains(&IM_START), "template must contain <|im_start|> id");
        assert!(
            ids.contains(&STOP_IM_END),
            "template must contain <|im_end|> id"
        );
        assert!(
            ids.iter().all(|id| !tok.is_stop(*id) || *id == STOP_IM_END),
            "template ends with <|im_end|> but must not contain <|endoftext|>"
        );
    }

    #[test]
    fn encode_decode_round_trips() {
        let tok = ChatTokenizer::load(data_dir()).unwrap();
        let ids = tok.encode("The quick brown fox jumps over the lazy dog.").unwrap();
        assert!(!ids.is_empty());
        assert!(ids.iter().all(|id| !tok.is_stop(*id)));
        let text = tok.decode(&ids).unwrap();
        assert_eq!(text, "The quick brown fox jumps over the lazy dog.");
    }
}
