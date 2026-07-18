use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: String, // "system" | "user" | "assistant" | "tool"
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    Token(String),
    Done,
}

/// A model backend. v0.1 has one impl (OpenAI-compatible); the trait keeps
/// core generic so mocks (tests) and future backends slot in.
#[async_trait]
pub trait Provider: Send + Sync {
    async fn stream_chat(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
    ) -> anyhow::Result<BoxStream<'static, anyhow::Result<StreamEvent>>>;
}

#[cfg(test)]
mod type_tests {
    use super::*;

    #[test]
    fn chat_message_serializes_role_and_content() {
        let m = ChatMessage { role: "user".into(), content: "hi".into() };
        let j = serde_json::to_value(&m).unwrap();
        assert_eq!(j["role"], "user");
        assert_eq!(j["content"], "hi");
    }

    #[test]
    fn stream_event_variants_exist() {
        let _ = StreamEvent::Token("x".into());
        let _ = StreamEvent::Done;
    }
}
