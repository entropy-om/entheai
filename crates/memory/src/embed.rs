use std::time::Duration;

use serde::Deserialize;

/// OpenAI-compatible embeddings client.
///
/// Talks to an embeddings endpoint (Osaurus by default, `http://127.0.0.1:1337/v1`).
/// Caches nothing — each call is a fresh HTTP request. For batch embedding,
/// call `embed_batch`.
#[derive(Debug, Clone)]
pub struct Embedder {
    client: reqwest::Client,
    base_url: String,
    model: String,
}

/// The OpenAI embeddings response shape we care about.
#[derive(Debug, Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedData>,
}

#[derive(Debug, Deserialize)]
struct EmbedData {
    embedding: Vec<f32>,
}

impl Embedder {
    /// Create a new embedder with a configurable request timeout.
    /// `base_url` should include the `/v1` prefix
    /// (e.g. `http://127.0.0.1:1337/v1`). `timeout_secs` is clamped to a
    /// minimum of 1 second.
    pub fn new(base_url: impl Into<String>, model: impl Into<String>, timeout_secs: u64) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs.max(1)))
            .build()
            .expect("reqwest Client::builder() should never fail");
        Embedder {
            client,
            base_url: base_url.into(),
            model: model.into(),
        }
    }

    /// Embed a single text, returning a float vector.
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, anyhow::Error> {
        let resp = self
            .client
            .post(format!("{}/embeddings", self.base_url))
            .json(&serde_json::json!({
                "model": self.model,
                "input": text,
            }))
            .send()
            .await?
            .error_for_status()?;

        let body: EmbedResponse = resp.json().await?;
        body.data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| anyhow::anyhow!("empty embedding response"))
    }

    /// Embed multiple texts in one request (if the provider supports array input).
    pub async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, anyhow::Error> {
        let resp = self
            .client
            .post(format!("{}/embeddings", self.base_url))
            .json(&serde_json::json!({
                "model": self.model,
                "input": texts,
            }))
            .send()
            .await?
            .error_for_status()?;

        let body: EmbedResponse = resp.json().await?;
        Ok(body.data.into_iter().map(|d| d.embedding).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn embed_single_returns_vector() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"embedding": [0.1, 0.2, 0.3]}]
            })))
            .mount(&server)
            .await;

        let embedder = Embedder::new(server.uri() + "/v1", "nomic-embed-text", 30);
        let vec = embedder.embed("hello").await.unwrap();
        assert_eq!(vec, vec![0.1, 0.2, 0.3]);
    }

    #[tokio::test]
    async fn embed_batch_returns_vectors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"embedding": [1.0]},
                    {"embedding": [2.0]}
                ]
            })))
            .mount(&server)
            .await;

        let embedder = Embedder::new(server.uri() + "/v1", "nomic-embed-text", 30);
        let vecs = embedder
            .embed_batch(&["a".into(), "b".into()])
            .await
            .unwrap();
        assert_eq!(vecs.len(), 2);
    }

    #[tokio::test]
    async fn embed_error_status_surfaces() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let embedder = Embedder::new(server.uri() + "/v1", "nomic-embed-text", 30);
        let err = embedder.embed("boom").await.unwrap_err();
        assert!(err.to_string().contains("500"), "{err}");
    }

    #[tokio::test]
    async fn embedder_honors_timeout() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_secs(2))
                    .set_body_json(serde_json::json!({"data":[{"embedding":[0.1]}]})),
            )
            .mount(&server)
            .await;
        let emb = Embedder::new(server.uri() + "/v1", "m", 1); // 1s timeout
        let err = emb.embed("hi").await.unwrap_err();
        // anyhow's `Display` only surfaces the top-level message; the
        // "operation timed out" detail lives in the source chain, which
        // shows up via `Debug` (`Caused by: ...`). Check both.
        let s = format!("{err} {err:?}").to_lowercase();
        assert!(s.contains("timeout") || s.contains("timed out"), "{err:?}");
    }
}
