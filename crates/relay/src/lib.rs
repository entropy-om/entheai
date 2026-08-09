//! entheai-relay: a post-prompt-processing layer that relays a prompt through
//! a fixed chain of language hops before it reaches the model — Hungarian
//! slang -> Lovari (Vlax Romani) -> English -> Mandarin, translated by
//! meaning rather than by sound at every hop.
//!
//! Each hop is one LLM completion via [`adk_rust::Llm`], the same trait
//! `crates/memory-pp`'s `BrainJudge` already drives (see
//! `crates/memory-pp/src/judge.rs`) — no tools, no session, just a
//! request/response round trip. Stateless entry point, mirrors
//! `crates/mapper`'s shape: one struct, one `run` call, every hop's output
//! returned so the chain stays inspectable rather than only exposing the end
//! result.

use adk_rust::{Content, Llm, LlmRequest};
use futures::StreamExt;
use std::sync::Arc;
use std::time::Duration;

/// Per-hop wall-clock budget before that hop is treated as failed.
pub const DEFAULT_HOP_TIMEOUT: Duration = Duration::from_secs(20);

/// One leg of the relay. `label` doubles as the hop name in error messages;
/// `prompt` builds that hop's translation request from the previous hop's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hop {
    HungarianToLovari,
    LovariToEnglish,
    EnglishToMandarin,
}

impl Hop {
    fn label(self) -> &'static str {
        match self {
            Hop::HungarianToLovari => "hungarian->lovari",
            Hop::LovariToEnglish => "lovari->english",
            Hop::EnglishToMandarin => "english->mandarin",
        }
    }

    fn prompt(self, text: &str) -> String {
        match self {
            Hop::HungarianToLovari => format!(
                "Translate the following Hungarian slang/colloquial text into Lovari \
                 (the Vlax Romani dialect spoken by Hungarian Roma). Preserve the tone \
                 and register — informal stays informal. Output ONLY the Lovari \
                 translation: no notes, no transliteration guide, no quotes.\n\n{text}"
            ),
            Hop::LovariToEnglish => format!(
                "Translate the following Lovari (Vlax Romani) text into natural, \
                 idiomatic English. Output ONLY the English translation: no notes, \
                 no quotes.\n\n{text}"
            ),
            Hop::EnglishToMandarin => format!(
                "Translate the following English text into natural Mandarin Chinese \
                 using standard hanzi. Translate by MEANING, not by sound: never \
                 render a word, name, or phrase as phonetic sound-alike hanzi (no \
                 pinyin-transliteration loanwords) — find the semantically \
                 equivalent Mandarin expression instead, paraphrasing if needed. \
                 Output ONLY the Mandarin translation: no pinyin, no notes, no \
                 quotes.\n\n{text}"
            ),
        }
    }
}

/// Why a hop failed. Every variant names the hop so a caller chaining several
/// relays can tell which leg broke without re-deriving it from context.
#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error("relay hop '{hop}' timed out after {timeout:?}")]
    Timeout {
        hop: &'static str,
        timeout: Duration,
    },
    #[error("relay hop '{hop}' failed: {reason}")]
    Failed { hop: &'static str, reason: String },
    #[error("relay hop '{hop}' returned an empty translation")]
    Empty { hop: &'static str },
}

/// A prompt's full path through the relay: the original plus every hop's
/// output, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayedPrompt {
    pub hungarian_slang: String,
    pub lovari: String,
    pub english: String,
    pub mandarin: String,
}

impl RelayedPrompt {
    /// The chain's final output — what a caller wanting just the end result wants.
    pub fn final_text(&self) -> &str {
        &self.mandarin
    }
}

/// Stateless entry point: relays one prompt through the language chain via `llm`.
pub struct Relay {
    llm: Arc<dyn Llm>,
    model: String,
    hop_timeout: Duration,
}

impl Relay {
    pub fn new(llm: Arc<dyn Llm>, model: impl Into<String>) -> Self {
        Self {
            llm,
            model: model.into(),
            hop_timeout: DEFAULT_HOP_TIMEOUT,
        }
    }

    /// Override the per-hop timeout (default [`DEFAULT_HOP_TIMEOUT`]).
    pub fn with_hop_timeout(mut self, timeout: Duration) -> Self {
        self.hop_timeout = timeout;
        self
    }

    /// Relay `hungarian_slang` through Lovari -> English -> Mandarin, returning
    /// every hop's output. Fails fast on the first hop that times out, errors,
    /// or returns an empty translation — never returns a partially-garbled chain.
    pub async fn run(&self, hungarian_slang: &str) -> Result<RelayedPrompt, RelayError> {
        let lovari = self.hop(Hop::HungarianToLovari, hungarian_slang).await?;
        let english = self.hop(Hop::LovariToEnglish, &lovari).await?;
        let mandarin = self.hop(Hop::EnglishToMandarin, &english).await?;
        Ok(RelayedPrompt {
            hungarian_slang: hungarian_slang.to_string(),
            lovari,
            english,
            mandarin,
        })
    }

    async fn hop(&self, hop: Hop, text: &str) -> Result<String, RelayError> {
        let request = LlmRequest {
            model: self.model.clone(),
            contents: vec![Content::new("user").with_text(hop.prompt(text))],
            config: None,
            tools: Default::default(),
            previous_response_id: None,
        };

        let call = async {
            let mut stream = self
                .llm
                .generate_content(request, false)
                .await
                .map_err(|e| RelayError::Failed {
                    hop: hop.label(),
                    reason: e.to_string(),
                })?;
            while let Some(chunk) = stream.next().await {
                let resp = chunk.map_err(|e| RelayError::Failed {
                    hop: hop.label(),
                    reason: e.to_string(),
                })?;
                let Some(content) = resp.content else {
                    continue;
                };
                let text: String = content.parts.iter().filter_map(|p| p.text()).collect();
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    continue;
                }
                return Ok(trimmed.to_string());
            }
            Err(RelayError::Empty { hop: hop.label() })
        };

        tokio::time::timeout(self.hop_timeout, call)
            .await
            .map_err(|_| RelayError::Timeout {
                hop: hop.label(),
                timeout: self.hop_timeout,
            })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adk_rust::LlmResponse;
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Returns one canned response per call, in order; panics if called more
    /// times than it has responses queued (keeps tests honest about hop count).
    struct QueuedLlm {
        responses: Mutex<std::collections::VecDeque<String>>,
    }

    impl QueuedLlm {
        fn new(responses: impl IntoIterator<Item = &'static str>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().map(String::from).collect()),
            }
        }
    }

    #[async_trait]
    impl Llm for QueuedLlm {
        fn name(&self) -> &str {
            "queued"
        }

        async fn generate_content(
            &self,
            _req: LlmRequest,
            _stream: bool,
        ) -> adk_rust::Result<adk_rust::LlmResponseStream> {
            let next = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("QueuedLlm called more times than it has responses queued");
            let resp = LlmResponse {
                content: Some(Content::new("model").with_text(next)),
                ..Default::default()
            };
            Ok(Box::pin(futures::stream::once(async { Ok(resp) })))
        }
    }

    struct EmptyLlm;

    #[async_trait]
    impl Llm for EmptyLlm {
        fn name(&self) -> &str {
            "empty"
        }

        async fn generate_content(
            &self,
            _req: LlmRequest,
            _stream: bool,
        ) -> adk_rust::Result<adk_rust::LlmResponseStream> {
            let resp = LlmResponse {
                content: Some(Content::new("model").with_text("   ")),
                ..Default::default()
            };
            Ok(Box::pin(futures::stream::once(async { Ok(resp) })))
        }
    }

    struct HangingLlm;

    #[async_trait]
    impl Llm for HangingLlm {
        fn name(&self) -> &str {
            "hanging"
        }

        async fn generate_content(
            &self,
            _req: LlmRequest,
            _stream: bool,
        ) -> adk_rust::Result<adk_rust::LlmResponseStream> {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            unreachable!("timeout should fire first")
        }
    }

    #[tokio::test]
    async fn run_chains_all_three_hops_in_order() {
        let llm = Arc::new(QueuedLlm::new(["Sar san?", "How are you?", "你好吗?"]));
        let relay = Relay::new(llm, "test/model");

        let relayed = relay.run("Mizu van?").await.unwrap();

        assert_eq!(relayed.hungarian_slang, "Mizu van?");
        assert_eq!(relayed.lovari, "Sar san?");
        assert_eq!(relayed.english, "How are you?");
        assert_eq!(relayed.mandarin, "你好吗?");
        assert_eq!(relayed.final_text(), "你好吗?");
    }

    #[tokio::test]
    async fn empty_hop_response_errors_instead_of_propagating_blank_text() {
        let relay = Relay::new(Arc::new(EmptyLlm), "test/model");

        let err = relay.run("Mizu van?").await.unwrap_err();

        assert!(matches!(
            err,
            RelayError::Empty {
                hop: "hungarian->lovari"
            }
        ));
    }

    #[tokio::test]
    async fn hop_timeout_surfaces_which_hop_and_never_hangs_the_caller() {
        let relay = Relay::new(Arc::new(HangingLlm), "test/model")
            .with_hop_timeout(Duration::from_millis(20));

        let err = relay.run("Mizu van?").await.unwrap_err();

        assert!(matches!(
            err,
            RelayError::Timeout {
                hop: "hungarian->lovari",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn stops_after_the_first_failing_hop_without_calling_later_hops() {
        // Only one response queued: if the crate accidentally called a second
        // hop despite the first failing, QueuedLlm's `expect` would panic.
        let llm = Arc::new(QueuedLlm::new(["   "]));
        let relay = Relay::new(llm, "test/model");

        let err = relay.run("Mizu van?").await.unwrap_err();

        assert!(matches!(
            err,
            RelayError::Empty {
                hop: "hungarian->lovari"
            }
        ));
    }
}
