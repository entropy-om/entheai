use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("could not reach model provider at {url} — is the server running (e.g. `osaurus serve` for a local provider), or did you mean a cloud provider like OpenCode Zen?")]
    Unreachable {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("provider returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("failed to decode provider response: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("stream error: {0}")]
    Stream(String),
}

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
    pub fn assistant_tool_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
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
    ) -> Result<BoxStream<'static, Result<StreamEvent, ProviderError>>, ProviderError>;

    /// Non-streaming completion. `tools` is a list of OpenAI function-tool JSON
    /// schemas; pass an empty Vec for no tools. Returns the assistant message.
    async fn complete(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<AssistantResponse, ProviderError>;

    /// Streaming completion. Sends each text delta to `token_tx` (if provided) as
    /// it arrives, and returns the fully-assembled response (content + tool_calls).
    /// Default: falls back to non-streaming `complete`, emitting the whole content
    /// as a single token. `OpenAiCompatProvider` overrides this with real SSE streaming.
    async fn stream_complete(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
        token_tx: Option<futures::channel::mpsc::UnboundedSender<String>>,
    ) -> Result<AssistantResponse, ProviderError> {
        let resp = self.complete(model, messages, tools).await?;
        if let Some(tx) = &token_tx {
            if !resp.content.is_empty() {
                let _ = tx.unbounded_send(resp.content.clone());
            }
        }
        Ok(resp)
    }
}

use eventsource_stream::Eventsource;
use futures::StreamExt;

pub struct OpenAiCompatProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    retries: u32,
}

/// Provider request settings (mapped from `entheai_config::InferenceConfig` by the router).
#[derive(Debug, Clone)]
pub struct InferenceSettings {
    pub request_timeout: std::time::Duration,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub retries: u32,
}

impl OpenAiCompatProvider {
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into(),
            api_key,
            max_tokens: None,
            temperature: None,
            retries: 0,
        }
    }

    /// Apply provider request settings: rebuilds the client with a request
    /// timeout and records sampling + retry policy. Non-breaking (opt-in).
    pub fn with_inference(mut self, s: InferenceSettings) -> Self {
        self.client = reqwest::Client::builder()
            .timeout(s.request_timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        self.max_tokens = s.max_tokens;
        self.temperature = s.temperature;
        self.retries = s.retries;
        self
    }

    /// Build, send, and status-check a POST to `/chat/completions`.
    /// Injects configured sampling params and retries transient failures.
    /// Callers consume the returned response (streaming vs. JSON) as needed.
    async fn post_chat(
        &self,
        mut body: serde_json::Value,
    ) -> Result<reqwest::Response, ProviderError> {
        if let Some(mt) = self.max_tokens {
            body["max_tokens"] = serde_json::json!(mt);
        }
        if let Some(t) = self.temperature {
            // Round-trip the f32 through its shortest decimal string so a
            // configured value like 0.1 serializes as 0.1 rather than the
            // widened f64 0.10000000149… that `t as f64` would produce.
            let t = format!("{t}").parse::<f64>().unwrap_or(t as f64);
            body["temperature"] = serde_json::json!(t);
        }
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let mut attempt = 0;
        loop {
            let mut req = self.client.post(&url).json(&body);
            if let Some(key) = &self.api_key {
                req = req.bearer_auth(key);
            }
            let result = async {
                let resp = req
                    .send()
                    .await
                    .map_err(|source| ProviderError::Unreachable {
                        url: url.clone(),
                        source,
                    })?;
                let status = resp.status();
                if !status.is_success() {
                    let body = resp.text().await.unwrap_or_default();
                    return Err(ProviderError::Status {
                        status: status.as_u16(),
                        body,
                    });
                }
                Ok(resp)
            }
            .await;
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) if attempt < self.retries && is_retryable(&e) => {
                    attempt += 1;
                    let backoff_ms = 200u64.saturating_mul(1u64 << (attempt - 1).min(10));
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

fn is_retryable(e: &ProviderError) -> bool {
    matches!(e, ProviderError::Unreachable { .. })
        || matches!(e, ProviderError::Status { status, .. } if *status >= 500)
}

#[async_trait]
impl Provider for OpenAiCompatProvider {
    async fn stream_chat(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
    ) -> Result<BoxStream<'static, Result<StreamEvent, ProviderError>>, ProviderError> {
        let body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
        });
        let resp = self.post_chat(body).await?;

        // v0.2: add request timeout, filter empty/role-only deltas, detect mid-body error payloads
        let stream = resp.bytes_stream().eventsource().map(|item| {
            let event = item.map_err(|e| ProviderError::Stream(e.to_string()))?;
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
    ) -> Result<AssistantResponse, ProviderError> {
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": false,
        });
        if !tools.is_empty() {
            body["tools"] = serde_json::Value::Array(tools);
        }
        let resp = self.post_chat(body).await?;
        // `Response::json`/`text` surface a reqwest transport error (not a serde error),
        // so it can't route through `Decode`'s `#[from]`; read the body then parse it.
        let text = resp
            .text()
            .await
            .map_err(|e| ProviderError::Stream(e.to_string()))?;
        let v: serde_json::Value = serde_json::from_str(&text)?;
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

    async fn stream_complete(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
        token_tx: Option<futures::channel::mpsc::UnboundedSender<String>>,
    ) -> Result<AssistantResponse, ProviderError> {
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
        });
        if !tools.is_empty() {
            body["tools"] = serde_json::Value::Array(tools);
        }
        let resp = self.post_chat(body).await?;

        let mut content = String::new();
        // Tool-call fragments accumulate here, keyed by the delta's `index`;
        // arguments arrive as JSON-string fragments that must be concatenated
        // in arrival order to reassemble the full arguments string.
        let mut tcs: Vec<(String, String, String)> = Vec::new();

        let mut stream = resp.bytes_stream().eventsource();
        while let Some(item) = stream.next().await {
            let event = item.map_err(|e| ProviderError::Stream(e.to_string()))?;
            if event.data.trim() == "[DONE]" {
                break;
            }
            let v: serde_json::Value = serde_json::from_str(&event.data)?;
            let delta = &v["choices"][0]["delta"];

            if let Some(s) = delta["content"].as_str() {
                if !s.is_empty() {
                    content.push_str(s);
                    if let Some(tx) = &token_tx {
                        let _ = tx.unbounded_send(s.to_string());
                    }
                }
            }

            if let Some(arr) = delta["tool_calls"].as_array() {
                for tc in arr {
                    let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                    while tcs.len() <= idx {
                        tcs.push((String::new(), String::new(), String::new()));
                    }
                    if let Some(id) = tc["id"].as_str() {
                        tcs[idx].0 = id.to_string();
                    }
                    if let Some(name) = tc["function"]["name"].as_str() {
                        tcs[idx].1.push_str(name);
                    }
                    if let Some(args) = tc["function"]["arguments"].as_str() {
                        tcs[idx].2.push_str(args);
                    }
                }
            }
        }

        let tool_calls: Vec<ToolCall> = tcs
            .into_iter()
            .filter(|(_, name, _)| !name.is_empty())
            .map(|(id, name, arguments)| ToolCall {
                id,
                kind: "function".to_string(),
                function: FunctionCall { name, arguments },
            })
            .collect();

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
        let j = serde_json::to_value(ChatMessage::assistant_tool_calls("", vec![call])).unwrap();
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
mod stream_complete_tests {
    use super::*;
    use futures::StreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn stream_complete_streams_text_and_returns_content() {
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
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let resp = p
            .stream_complete("m", vec![ChatMessage::user("hi")], vec![], Some(tx))
            .await
            .unwrap();

        let tokens: Vec<String> = rx.collect().await;
        assert_eq!(tokens, vec!["Hel".to_string(), "lo".to_string()]);
        assert_eq!(resp.content, "Hello");
        assert!(resp.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn stream_complete_assembles_tool_calls_from_deltas() {
        let server = MockServer::start().await;
        let sse = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"\"}}]}}]}\n\n\
                   data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n\
                   data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"x\\\"}\"}}]}}]}\n\n\
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
        let resp = p
            .stream_complete("m", vec![ChatMessage::user("hi")], vec![], None)
            .await
            .unwrap();

        assert_eq!(resp.content, "");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "call_1");
        assert_eq!(resp.tool_calls[0].function.name, "read_file");
        assert_eq!(resp.tool_calls[0].function.arguments, "{\"path\":\"x\"}");
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

#[cfg(test)]
mod inference_tests {
    use super::*;

    #[tokio::test]
    async fn request_timeout_fires() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(2)))
            .mount(&server)
            .await;
        let provider =
            OpenAiCompatProvider::new(server.uri(), None).with_inference(InferenceSettings {
                request_timeout: std::time::Duration::from_millis(200),
                max_tokens: None,
                temperature: None,
                retries: 0,
            });
        let err = provider
            .complete("m", vec![ChatMessage::user("hi")], vec![])
            .await
            .unwrap_err();
        assert!(
            matches!(err, ProviderError::Unreachable { .. }),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn sampling_params_sent_when_set() {
        use wiremock::matchers::{body_partial_json, method};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_partial_json(
                serde_json::json!({"max_tokens": 512, "temperature": 0.1}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "ok"}}]
            })))
            .mount(&server)
            .await;
        let provider =
            OpenAiCompatProvider::new(server.uri(), None).with_inference(InferenceSettings {
                request_timeout: std::time::Duration::from_secs(30),
                max_tokens: Some(512),
                temperature: Some(0.1),
                retries: 0,
            });
        let resp = provider
            .complete("m", vec![ChatMessage::user("hi")], vec![])
            .await
            .unwrap();
        assert_eq!(resp.content, "ok");
    }

    #[tokio::test]
    async fn retry_recovers_from_transient_5xx() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(2)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "recovered"}}]
            })))
            .with_priority(2)
            .mount(&server)
            .await;
        let provider =
            OpenAiCompatProvider::new(server.uri(), None).with_inference(InferenceSettings {
                request_timeout: std::time::Duration::from_secs(30),
                max_tokens: None,
                temperature: None,
                retries: 2,
            });
        let resp = provider
            .complete("m", vec![ChatMessage::user("hi")], vec![])
            .await
            .unwrap();
        assert_eq!(resp.content, "recovered");
    }
}
