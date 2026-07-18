use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FunctionCall {
    pub name: String,
    /// Raw JSON string of arguments, per the OpenAI wire format.
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String, // always "function" for v0.1
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ChatMessage {
    pub role: String, // "system" | "user" | "assistant" | "tool"
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
            ..Default::default()
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
            ..Default::default()
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
            ..Default::default()
        }
    }
    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".into(),
            tool_calls: Some(tool_calls),
            ..Default::default()
        }
    }
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    Token(String),
    Done,
}

#[derive(Debug, Clone, Default)]
pub struct AssistantResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
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

    /// Non-streaming completion. `tools` is a list of OpenAI function-tool JSON
    /// schemas; pass an empty Vec for no tools. Returns the assistant message.
    async fn complete(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> anyhow::Result<AssistantResponse>;
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

    /// Build, send, and status-check a POST to `/chat/completions`.
    /// Callers consume the returned response (streaming vs. JSON) as needed.
    async fn post_chat(&self, body: serde_json::Value) -> anyhow::Result<reqwest::Response> {
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
        Ok(resp)
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
        let resp = self.post_chat(body).await?;

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

    async fn complete(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> anyhow::Result<AssistantResponse> {
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": false,
        });
        if !tools.is_empty() {
            body["tools"] = serde_json::Value::Array(tools);
        }
        let resp = self.post_chat(body).await?;
        let v: serde_json::Value = resp.json().await?;
        let msg = &v["choices"][0]["message"];
        let content = msg["content"].as_str().unwrap_or("").to_string();
        let tool_calls: Vec<ToolCall> = match msg.get("tool_calls") {
            Some(tc) if tc.is_array() => serde_json::from_value(tc.clone())?,
            _ => Vec::new(),
        };
        Ok(AssistantResponse {
            content,
            tool_calls,
        })
    }
}

#[cfg(test)]
mod type_tests {
    use super::*;

    #[test]
    fn chat_message_serializes_role_and_content() {
        let m = ChatMessage::user("hi");
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
mod message_tests {
    use super::*;

    #[test]
    fn user_message_serializes_minimally() {
        let j = serde_json::to_value(ChatMessage::user("hi")).unwrap();
        assert_eq!(j["role"], "user");
        assert_eq!(j["content"], "hi");
        assert!(j.get("tool_calls").is_none());
        assert!(j.get("tool_call_id").is_none());
    }

    #[test]
    fn assistant_tool_call_message_serializes_tool_calls() {
        let call = ToolCall {
            id: "call_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: "{\"path\":\"x\"}".into(),
            },
        };
        let j = serde_json::to_value(ChatMessage::assistant_tool_calls(vec![call])).unwrap();
        assert_eq!(j["role"], "assistant");
        assert!(j.get("content").is_none(), "empty content must be omitted");
        assert_eq!(j["tool_calls"][0]["id"], "call_1");
        assert_eq!(j["tool_calls"][0]["type"], "function");
        assert_eq!(j["tool_calls"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn tool_result_message_serializes_tool_call_id() {
        let j = serde_json::to_value(ChatMessage::tool_result("call_1", "file contents")).unwrap();
        assert_eq!(j["role"], "tool");
        assert_eq!(j["content"], "file contents");
        assert_eq!(j["tool_call_id"], "call_1");
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
            .stream_chat("m", vec![ChatMessage::user("hi")])
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
        let result = p.stream_chat("m", vec![ChatMessage::user("hi")]).await;

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

#[cfg(test)]
mod complete_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn complete_parses_content_and_tool_calls() {
        let server = MockServer::start().await;
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "read_file", "arguments": "{\"path\":\"Cargo.toml\"}" }
                    }]
                }
            }]
        });
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;

        let p = OpenAiCompatProvider::new(server.uri(), None);
        let resp = p
            .complete("m", vec![ChatMessage::user("hi")], vec![])
            .await
            .unwrap();
        assert_eq!(resp.content, "");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].function.name, "read_file");
        assert_eq!(resp.tool_calls[0].id, "call_1");
    }

    #[tokio::test]
    async fn complete_handles_plain_text_answer() {
        let server = MockServer::start().await;
        let body = serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "hello there" } }]
        });
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;

        let p = OpenAiCompatProvider::new(server.uri(), None);
        let resp = p
            .complete("m", vec![ChatMessage::user("hi")], vec![])
            .await
            .unwrap();
        assert_eq!(resp.content, "hello there");
        assert!(resp.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn complete_surfaces_http_error_status_and_body() {
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
        let result = p.complete("m", vec![ChatMessage::user("hi")], vec![]).await;
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
