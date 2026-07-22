//! BrainJudge — background proactive frozen-node relevance judge.
//! See docs/superpowers/specs/2026-07-22-brain-v1-dyad-design.md.
//!
//! BrainJudge monitors recent ambient activity (tool outputs, transcript turns),
//! enforces a cooldown window to prevent thrashing, and queries a local LLM with a
//! precision-tuned prompt to determine if any frozen node should proactively wake.
//! Surfacing nothing ("none") is prioritized over false positives.

use crate::frozen::FrozenStore;
use entheai_providers::{ChatMessage, Provider};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrainJudgeEvent {
    /// A frozen node was proactively judged relevant and woken.
    Woke(String),
}

pub struct BrainJudge {
    provider: Arc<dyn Provider>,
    model: String,
    frozen: FrozenStore,
    cooldown: Duration,
    last_fired_ms: AtomicI64,
    tx: mpsc::UnboundedSender<BrainJudgeEvent>,
}

impl BrainJudge {
    /// Construct a new `BrainJudge` background worker and its event receiver channel.
    pub fn new(
        provider: Arc<dyn Provider>,
        model: impl Into<String>,
        frozen: FrozenStore,
        cooldown: Duration,
    ) -> (Self, mpsc::UnboundedReceiver<BrainJudgeEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let judge = Self {
            provider,
            model: model.into(),
            frozen,
            cooldown,
            last_fired_ms: AtomicI64::new(0),
            tx,
        };
        (judge, rx)
    }

    /// Notify the judge of recent ambient activity. Enforces a cooldown window and
    /// spawns a lightweight LLM completion task with a strict precision prompt.
    pub fn notify(&self, activity: &str) {
        if activity.trim().is_empty() || self.frozen.nodes().is_empty() {
            return;
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let last = self.last_fired_ms.load(Ordering::Relaxed);
        if now_ms - last < self.cooldown.as_millis() as i64 {
            return; // within cooldown window — suppress proactive trigger
        }
        self.last_fired_ms.store(now_ms, Ordering::Relaxed);

        let names: Vec<&str> = self
            .frozen
            .nodes()
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        let prompt = format!(
            "You are a strict, precision-focused memory relevance judge.\n\n\
             Recent activity:\n{activity}\n\n\
             Available topic names:\n{}\n\n\
             TASK: Is this activity directly and specifically relevant to one of the topics above?\n\n\
             STRICT RULES:\n\
             1. Reply with the single exact topic name ONLY if the activity is directly and unambiguously relevant.\n\
             2. If uncertain, loosely related, or no topic matches directly, reply with \"none\".\n\
             3. Default to \"none\". Surfacing nothing is always safer than surfacing an irrelevant topic.\n\
             4. Output ONLY the topic name or \"none\" — no explanations, preamble, or extra text.",
            names.join(", "),
        );

        let messages = vec![ChatMessage::user(prompt)];
        let provider = Arc::clone(&self.provider);
        let model = self.model.clone();
        let node_names: Vec<String> = self.frozen.nodes().iter().map(|n| n.name.clone()).collect();
        let tx = self.tx.clone();

        tokio::spawn(async move {
            let Ok(resp) = tokio::time::timeout(
                Duration::from_secs(5),
                provider.complete(&model, &messages, &[]),
            )
            .await
            else {
                return; // timeout → surface nothing
            };
            let Ok(resp) = resp else {
                return; // provider error → surface nothing
            };
            let answer = resp.content.trim().to_lowercase();
            if answer == "none" {
                return;
            }
            if let Some(matched) = node_names
                .iter()
                .find(|n| answer.contains(&n.to_lowercase()))
            {
                let _ = tx.send(BrainJudgeEvent::Woke(matched.clone()));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use entheai_providers::{AssistantResponse, Provider, ProviderError, StreamEvent};
    use futures::stream::BoxStream;

    struct FakeProvider {
        response: String,
    }

    #[async_trait]
    impl Provider for FakeProvider {
        async fn stream_chat(
            &self,
            _model: &str,
            _messages: &[ChatMessage],
        ) -> Result<BoxStream<'static, Result<StreamEvent, ProviderError>>, ProviderError> {
            unimplemented!("BrainJudge only uses complete()")
        }

        async fn complete(
            &self,
            _model: &str,
            _messages: &[ChatMessage],
            _tools: &[serde_json::Value],
        ) -> Result<AssistantResponse, ProviderError> {
            Ok(AssistantResponse {
                content: self.response.clone(),
                tool_calls: vec![],
            })
        }
    }

    fn test_store() -> FrozenStore {
        use crate::frozen::FrozenNode;
        let node = FrozenNode {
            name: "nixos".to_string(),
            domain: "reproducible systems".to_string(),
            triggers: vec!["hetzner".to_string()],
            mcp: None,
            rank: 1.0,
            knowledge: "NixOS deployment guide".to_string(),
        };
        FrozenStore::from_nodes(vec![node])
    }

    #[tokio::test]
    async fn test_brain_judge_wakes_on_direct_relevance() {
        let provider = Arc::new(FakeProvider {
            response: "nixos".to_string(),
        });
        let (judge, mut rx) = BrainJudge::new(
            provider,
            "test/model",
            test_store(),
            Duration::from_millis(1),
        );

        judge.notify("Deploying web application to Hetzner VPS via NixOS");

        let ev = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("should receive event within timeout")
            .expect("channel should not be closed");
        assert_eq!(ev, BrainJudgeEvent::Woke("nixos".to_string()));
    }

    #[tokio::test]
    async fn test_brain_judge_suppresses_on_none() {
        let provider = Arc::new(FakeProvider {
            response: "none".to_string(),
        });
        let (judge, mut rx) = BrainJudge::new(
            provider,
            "test/model",
            test_store(),
            Duration::from_millis(1),
        );

        judge.notify("Refactoring CSS button padding");

        let res = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await;
        assert!(
            res.is_err(),
            "should suppress event when LLM returns 'none'"
        );
    }

    #[tokio::test]
    async fn test_brain_judge_cooldown_suppresses() {
        let provider = Arc::new(FakeProvider {
            response: "nixos".to_string(),
        });
        let (judge, mut rx) = BrainJudge::new(
            provider,
            "test/model",
            test_store(),
            Duration::from_secs(60),
        );

        judge.notify("First trigger");
        let _ = rx.recv().await;

        judge.notify("Second trigger within 60s cooldown");
        let res = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await;
        assert!(res.is_err(), "cooldown must suppress second trigger");
    }
}
