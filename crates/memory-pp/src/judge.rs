//! BrainJudge — background proactive frozen-node relevance judge.
//! See docs/superpowers/specs/2026-07-22-brain-v1-dyad-design.md.
//!
//! BrainJudge monitors recent ambient activity (tool outputs, transcript turns),
//! enforces a cooldown window to prevent thrashing, and queries a local LLM with a
//! precision-tuned prompt to determine if any frozen node should proactively wake.
//! Surfacing nothing ("none") is prioritized over false positives.
//!
//! Ported onto `adk_rust::Llm` (from `entheai_providers::Provider`) as part of
//! the adk-rust migration's Task 7 scope: this is the "memory/brain worker"
//! half of full adk-rust adoption, closing the gap that `entheai_providers`
//! couldn't otherwise be deleted while BrainJudge depended on it.

use crate::frozen::FrozenStore;
use adk_rust::{Content, Llm, LlmRequest};
use futures::StreamExt;
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
    llm: Arc<dyn Llm>,
    model: String,
    frozen: FrozenStore,
    cooldown: Duration,
    last_fired_ms: AtomicI64,
    tx: mpsc::UnboundedSender<BrainJudgeEvent>,
}

impl BrainJudge {
    /// Construct a new `BrainJudge` background worker and its event receiver channel.
    pub fn new(
        llm: Arc<dyn Llm>,
        model: impl Into<String>,
        frozen: FrozenStore,
        cooldown: Duration,
    ) -> (Self, mpsc::UnboundedReceiver<BrainJudgeEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let judge = Self {
            llm,
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

        let llm = Arc::clone(&self.llm);
        let model = self.model.clone();
        let node_names: Vec<String> = self.frozen.nodes().iter().map(|n| n.name.clone()).collect();
        let tx = self.tx.clone();

        tokio::spawn(async move {
            let judge_call = async move {
                let request = LlmRequest {
                    model: model.clone(),
                    contents: vec![Content::new("user").with_text(prompt)],
                    config: None,
                    tools: Default::default(),
                    previous_response_id: None,
                };
                let mut stream = llm.generate_content(request, false).await.ok()?;
                let mut content: Option<Content> = None;
                while let Some(chunk) = stream.next().await {
                    if let Ok(resp) = chunk {
                        if resp.content.is_some() {
                            content = resp.content;
                            break;
                        }
                    }
                }
                content
            };

            let Ok(Some(content)) = tokio::time::timeout(Duration::from_secs(5), judge_call).await
            else {
                return; // timeout, provider error, or empty response → surface nothing
            };
            let text: String = content.parts.iter().filter_map(|p| p.text()).collect();
            let answer = text.trim().to_lowercase();
            if answer == "none" {
                return;
            }
            // Exact match only: the prompt demands "ONLY the topic name or
            // 'none'", so a substring match would let stray prose that
            // happens to mention a node name (the model ignoring the format
            // instruction) wake it — exactly the false-positive the "none"
            // default is meant to guard against.
            if let Some(matched) = node_names.iter().find(|n| answer == n.to_lowercase()) {
                let _ = tx.send(BrainJudgeEvent::Woke(matched.clone()));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adk_rust::LlmResponse;
    use async_trait::async_trait;

    struct FakeLlm {
        response: String,
    }

    #[async_trait]
    impl Llm for FakeLlm {
        fn name(&self) -> &str {
            "fake"
        }

        async fn generate_content(
            &self,
            _req: LlmRequest,
            _stream: bool,
        ) -> adk_rust::Result<adk_rust::LlmResponseStream> {
            let resp = LlmResponse {
                content: Some(Content::new("model").with_text(self.response.clone())),
                ..Default::default()
            };
            Ok(Box::pin(futures::stream::once(async { Ok(resp) })))
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
        let llm = Arc::new(FakeLlm { response: "nixos".to_string() });
        let (judge, mut rx) =
            BrainJudge::new(llm, "test/model", test_store(), Duration::from_millis(1));

        judge.notify("Deploying web application to Hetzner VPS via NixOS");

        let ev = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("should receive event within timeout")
            .expect("channel should not be closed");
        assert_eq!(ev, BrainJudgeEvent::Woke("nixos".to_string()));
    }

    #[tokio::test]
    async fn test_brain_judge_suppresses_on_none() {
        let llm = Arc::new(FakeLlm { response: "none".to_string() });
        let (judge, mut rx) =
            BrainJudge::new(llm, "test/model", test_store(), Duration::from_millis(1));

        judge.notify("Refactoring CSS button padding");

        let res = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await;
        assert!(
            res.is_err(),
            "should suppress event when LLM returns 'none'"
        );
    }

    #[tokio::test]
    async fn test_brain_judge_cooldown_suppresses() {
        let llm = Arc::new(FakeLlm { response: "nixos".to_string() });
        let (judge, mut rx) =
            BrainJudge::new(llm, "test/model", test_store(), Duration::from_secs(60));

        judge.notify("First trigger");
        let _ = rx.recv().await;

        judge.notify("Second trigger within 60s cooldown");
        let res = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await;
        assert!(res.is_err(), "cooldown must suppress second trigger");
    }
}
