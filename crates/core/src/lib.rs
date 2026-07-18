use entheai_providers::{ChatMessage, Provider, StreamEvent};
use futures::StreamExt;

/// Where streamed tokens go (stdout in the CLI, the TUI later).
pub trait TokenSink {
    fn emit(&mut self, token: &str);
}

pub struct Agent<P: Provider> {
    provider: P,
    model: String,
}

impl<P: Provider> Agent<P> {
    pub fn new(provider: P, model: String) -> Self {
        Self { provider, model }
    }

    /// Run one turn: stream the model's reply to `sink`, return the full text.
    pub async fn run_turn(
        &self,
        messages: Vec<ChatMessage>,
        sink: &mut impl TokenSink,
    ) -> anyhow::Result<String> {
        let mut stream = self.provider.stream_chat(&self.model, messages).await?;
        let mut full = String::new();
        while let Some(ev) = stream.next().await {
            match ev? {
                StreamEvent::Token(t) => {
                    full.push_str(&t);
                    sink.emit(&t);
                }
                StreamEvent::Done => break,
            }
        }
        Ok(full)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use entheai_providers::{ChatMessage, Provider, StreamEvent};
    use futures::stream::{self, BoxStream};

    struct MockProvider {
        tokens: Vec<&'static str>,
    }

    #[async_trait]
    impl Provider for MockProvider {
        async fn stream_chat(
            &self,
            _model: &str,
            _messages: Vec<ChatMessage>,
        ) -> anyhow::Result<BoxStream<'static, anyhow::Result<StreamEvent>>> {
            let mut evs: Vec<anyhow::Result<StreamEvent>> = self
                .tokens
                .iter()
                .map(|t| Ok(StreamEvent::Token((*t).to_string())))
                .collect();
            evs.push(Ok(StreamEvent::Done));
            Ok(Box::pin(stream::iter(evs)))
        }
    }

    struct CollectSink(String);
    impl TokenSink for CollectSink {
        fn emit(&mut self, token: &str) {
            self.0.push_str(token);
        }
    }

    #[tokio::test]
    async fn run_turn_streams_and_returns_full_text() {
        let agent = Agent::new(
            MockProvider {
                tokens: vec!["Hel", "lo"],
            },
            "m".into(),
        );
        let mut sink = CollectSink(String::new());
        let msgs = vec![ChatMessage {
            role: "user".into(),
            content: "hi".into(),
        }];
        let full = agent.run_turn(msgs, &mut sink).await.unwrap();
        assert_eq!(full, "Hello");
        assert_eq!(sink.0, "Hello");
    }
}
