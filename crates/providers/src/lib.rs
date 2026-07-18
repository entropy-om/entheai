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

use eventsource_stream::Eventsource;
use futures::StreamExt;

pub struct OpenAiCompatProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl OpenAiCompatProvider {
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into(),
            api_key,
        }
    }
}

#[async_trait]
impl Provider for OpenAiCompatProvider {
    async fn stream_chat(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
    ) -> anyhow::Result<BoxStream<'static, anyhow::Result<StreamEvent>>> {
        let body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
        });
        let mut req = self
            .client
            .post(format!(
                "{}/chat/completions",
                self.base_url.trim_end_matches('/')
            ))
            .json(&body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("provider returned {status}: {body}");
        }

        // v0.2: add request timeout, filter empty/role-only deltas, detect mid-body error payloads
        let stream = resp.bytes_stream().eventsource().map(|item| {
            let event = item.map_err(|e| anyhow::anyhow!(e))?;
            if event.data.trim() == "[DONE]" {
                return Ok(StreamEvent::Done);
            }
            let v: serde_json::Value = serde_json::from_str(&event.data)?;
            let token = v["choices"][0]["delta"]["content"]
                .as_str()
                .unwrap_or("")
                .to_string();
            Ok(StreamEvent::Token(token))
        });

        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod type_tests {
    use super::*;

    #[test]
    fn chat_message_serializes_role_and_content() {
        let m = ChatMessage {
            role: "user".into(),
            content: "hi".into(),
        };
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

#[cfg(test)]
mod openai_tests {
    use super::*;
    use futures::StreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn streams_and_concatenates_delta_content() {
        let server = MockServer::start().await;
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
                   data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n\
                   data: [DONE]\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse),
            )
            .mount(&server)
            .await;

        let p = OpenAiCompatProvider::new(server.uri(), None);
        let mut stream = p
            .stream_chat(
                "m",
                vec![ChatMessage {
                    role: "user".into(),
                    content: "hi".into(),
                }],
            )
            .await
            .unwrap();

        let mut out = String::new();
        let mut saw_done = false;
        while let Some(ev) = stream.next().await {
            match ev.unwrap() {
                StreamEvent::Token(t) => out.push_str(&t),
                StreamEvent::Done => saw_done = true,
            }
        }
        assert_eq!(out, "Hello");
        assert!(saw_done);
    }

    #[tokio::test]
    async fn http_error_status_surfaces_status_and_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_string("{\"error\":{\"message\":\"bad model\"}}"),
            )
            .mount(&server)
            .await;

        let p = OpenAiCompatProvider::new(server.uri(), None);
        let result = p
            .stream_chat(
                "m",
                vec![ChatMessage {
                    role: "user".into(),
                    content: "hi".into(),
                }],
            )
            .await;

        assert!(result.is_err());
        let msg = format!("{}", result.err().unwrap());
        assert!(
            msg.contains("400"),
            "error should include status, got: {msg}"
        );
        assert!(
            msg.contains("bad model"),
            "error should include body, got: {msg}"
        );
    }
}
