//! entheai-vision: an image post-processing layer. A pasted image (bytes
//! already in hand — e.g. from a clipboard) or a file path gets analyzed by
//! whichever backend is available:
//!
//! 1. **The Antigravity CLI (`agy`, Google's Gemini)** — preferred when
//!    configured, mirroring `crates/orchestrator`'s `AgyExecutor`: shell out,
//!    any failure (missing binary, non-zero exit, timeout) degrades to backend 2.
//! 2. **A configured vision-capable `adk_rust::Llm`** — e.g. a local
//!    Gemma/hf-mac endpoint reachable the same way every other provider in
//!    this workspace is (`entheai_core::model_resolve::resolve_model`),
//!    driven the same single-hop way `crates/relay`'s `Relay` drives a
//!    translation hop.
//!
//! Never blends the two: exactly one backend produces the final text.

mod format;

use adk_rust::{Content, Llm, LlmRequest};
use futures::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Wall-clock budget for either backend before it's treated as failed.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// An image to analyze: bytes already in hand, or a path to read from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageInput {
    /// Already-decoded bytes (e.g. pasted from a clipboard or piped via
    /// stdin) plus their MIME type, since there's no filename to infer it from.
    Pasted { mime_type: String, bytes: Vec<u8> },
    /// A file on disk; MIME type is inferred from the extension, falling
    /// back to a magic-byte sniff of its contents.
    Path(PathBuf),
}

impl ImageInput {
    pub fn pasted(mime_type: impl Into<String>, bytes: Vec<u8>) -> Self {
        ImageInput::Pasted {
            mime_type: mime_type.into(),
            bytes,
        }
    }

    pub fn path(path: impl Into<PathBuf>) -> Self {
        ImageInput::Path(path.into())
    }

    /// Build a `Pasted` image from raw bytes with no filename to derive a
    /// MIME type from (e.g. piped in over stdin) — inferred from the bytes'
    /// own magic-byte header instead.
    pub fn sniffed(bytes: Vec<u8>) -> Result<Self, VisionError> {
        let mime_type = format::sniff_mime(&bytes).ok_or(VisionError::UnknownPastedFormat)?;
        Ok(ImageInput::Pasted {
            mime_type: mime_type.to_string(),
            bytes,
        })
    }

    /// Resolve to `(mime_type, bytes)`, reading from disk for the `Path` variant.
    async fn load(&self) -> Result<(String, Vec<u8>), VisionError> {
        match self {
            ImageInput::Pasted { mime_type, bytes } => Ok((mime_type.clone(), bytes.clone())),
            ImageInput::Path(path) => {
                let bytes = tokio::fs::read(path).await.map_err(|e| VisionError::Read {
                    path: path.clone(),
                    reason: e.to_string(),
                })?;
                let mime_type = format::mime_type_for(path, &bytes)
                    .ok_or_else(|| VisionError::UnknownFormat(path.clone()))?;
                Ok((mime_type.to_string(), bytes))
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VisionError {
    #[error("reading image {path}: {reason}")]
    Read { path: PathBuf, reason: String },
    #[error(
        "could not determine an image format for {0} (unrecognized extension and file signature)"
    )]
    UnknownFormat(PathBuf),
    #[error("could not determine an image format from its contents (no filename to derive an extension from)")]
    UnknownPastedFormat,
    #[error("agy (Antigravity CLI) failed: {0}")]
    AgyFailed(String),
    #[error("vision model call failed: {0}")]
    ModelFailed(String),
    #[error("vision call timed out after {0:?}")]
    Timeout(Duration),
    #[error("vision backend returned an empty response")]
    Empty,
}

/// Analyzes one image per call. Stateless entry point (mirrors
/// `crates/relay`'s `Relay`): construct once with the fallback model, then
/// optionally opt into the `agy` CLI as the preferred backend.
pub struct VisionProcessor {
    /// `Some(model)` => try the `agy` CLI first, passing it this model name.
    agy_model: Option<String>,
    llm: Arc<dyn Llm>,
    model: String,
    timeout: Duration,
}

impl VisionProcessor {
    /// Build a processor whose only backend is `llm`/`model` (a vision-capable
    /// `adk_rust::Llm`, e.g. a local Gemma/hf-mac endpoint).
    pub fn new(llm: Arc<dyn Llm>, model: impl Into<String>) -> Self {
        Self {
            agy_model: None,
            llm,
            model: model.into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Prefer the `agy` (Antigravity CLI / Gemini) backend, falling back to
    /// the model backend on any failure — missing binary, non-zero exit,
    /// empty output, or timeout.
    pub fn with_agy(mut self, agy_model: impl Into<String>) -> Self {
        self.agy_model = Some(agy_model.into());
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Analyze `image` per `instruction` (e.g. "describe this", "what error
    /// is shown in this screenshot?"). Tries `agy` first when configured;
    /// any failure there is logged and falls through to the model backend.
    pub async fn process(
        &self,
        image: &ImageInput,
        instruction: &str,
    ) -> Result<String, VisionError> {
        if let Some(agy_model) = &self.agy_model {
            match self.run_agy(agy_model, image, instruction).await {
                Ok(text) => return Ok(text),
                Err(e) => {
                    log::warn!(
                        "vision: agy backend unavailable/failed ({e}) — falling back to model"
                    );
                }
            }
        }
        self.run_model(image, instruction).await
    }

    /// `agy` wants a real file path, not bytes on stdin — pasted bytes are
    /// spilled to a temp file first (dropped, and so cleaned up, once the
    /// caller is done with the returned path).
    async fn agy_path(
        &self,
        image: &ImageInput,
    ) -> Result<(Option<tempfile::TempDir>, PathBuf), VisionError> {
        match image {
            ImageInput::Path(path) => Ok((None, path.clone())),
            ImageInput::Pasted { mime_type, bytes } => {
                let dir = tempfile::tempdir()
                    .map_err(|e| VisionError::AgyFailed(format!("tempdir: {e}")))?;
                let path = dir
                    .path()
                    .join(format!("pasted.{}", format::extension_for_mime(mime_type)));
                tokio::fs::write(&path, bytes)
                    .await
                    .map_err(|e| VisionError::AgyFailed(format!("spilling pasted image: {e}")))?;
                Ok((Some(dir), path))
            }
        }
    }

    /// Shell out to `agy -p "<instruction>\n\n@<path>" --model <agy_model>`.
    /// `@path` is Gemini CLI's own documented file-include syntax — `agy`
    /// wraps that same Gemini tooling, so the instruction plus an `@`
    /// reference to the image is what the CLI itself expects inline in `-p`.
    async fn run_agy(
        &self,
        agy_model: &str,
        image: &ImageInput,
        instruction: &str,
    ) -> Result<String, VisionError> {
        let (_tmp_guard, path) = self.agy_path(image).await?;
        let prompt = format!("{instruction}\n\n@{}", path.display());

        let spawn = tokio::process::Command::new("agy")
            .arg("-p")
            .arg(&prompt)
            .arg("--model")
            .arg(agy_model)
            .arg("--sandbox")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();

        let out = tokio::time::timeout(self.timeout, spawn)
            .await
            .map_err(|_| VisionError::Timeout(self.timeout))?
            .map_err(|e| VisionError::AgyFailed(format!("spawning agy: {e}")))?;

        if !out.status.success() {
            return Err(VisionError::AgyFailed(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if text.is_empty() {
            return Err(VisionError::Empty);
        }
        Ok(text)
    }

    /// One `adk_rust::Llm` completion with the image attached as inline data
    /// — the same request/response round trip `crates/relay`'s `Relay` drives
    /// per hop, just with an image part alongside the text.
    async fn run_model(
        &self,
        image: &ImageInput,
        instruction: &str,
    ) -> Result<String, VisionError> {
        let (mime_type, bytes) = image.load().await?;
        let content = Content::new("user")
            .with_text(instruction)
            .with_inline_data(mime_type, bytes);
        let request = LlmRequest {
            model: self.model.clone(),
            contents: vec![content],
            config: None,
            tools: Default::default(),
            previous_response_id: None,
        };

        let call = async {
            let mut stream = self
                .llm
                .generate_content(request, false)
                .await
                .map_err(|e| VisionError::ModelFailed(e.to_string()))?;
            while let Some(chunk) = stream.next().await {
                let resp = chunk.map_err(|e| VisionError::ModelFailed(e.to_string()))?;
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
            Err(VisionError::Empty)
        };

        tokio::time::timeout(self.timeout, call)
            .await
            .map_err(|_| VisionError::Timeout(self.timeout))?
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
            req: LlmRequest,
            _stream: bool,
        ) -> adk_rust::Result<adk_rust::LlmResponseStream> {
            // The model backend must actually attach the image, not just the
            // instruction text — assert that here rather than trusting it.
            let has_image = req.contents[0]
                .parts
                .iter()
                .any(|p| matches!(p, adk_rust::Part::InlineData { .. }));
            assert!(has_image, "request must carry the image as inline data");

            let resp = LlmResponse {
                content: Some(Content::new("model").with_text(self.response.clone())),
                ..Default::default()
            };
            Ok(Box::pin(futures::stream::once(async { Ok(resp) })))
        }
    }

    fn png_bytes() -> Vec<u8> {
        vec![
            0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 1, 2, 3,
        ]
    }

    #[test]
    fn sniffed_infers_mime_from_bytes_alone() {
        let image = ImageInput::sniffed(png_bytes()).unwrap();
        assert_eq!(
            image,
            ImageInput::Pasted {
                mime_type: "image/png".to_string(),
                bytes: png_bytes(),
            }
        );
    }

    #[test]
    fn sniffed_errors_on_unrecognized_bytes() {
        let err = ImageInput::sniffed(b"not an image".to_vec()).unwrap_err();
        assert!(matches!(err, VisionError::UnknownPastedFormat));
    }

    #[tokio::test]
    async fn model_backend_returns_the_llm_answer_with_pasted_bytes() {
        let llm = Arc::new(FakeLlm {
            response: "a screenshot of a terminal".to_string(),
        });
        let processor = VisionProcessor::new(llm, "test/model");
        let image = ImageInput::pasted("image/png", png_bytes());

        let answer = processor.process(&image, "describe this").await.unwrap();

        assert_eq!(answer, "a screenshot of a terminal");
    }

    #[tokio::test]
    async fn model_backend_reads_and_sniffs_a_path_based_image() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot"); // no extension: forces the sniff path
        std::fs::write(&path, png_bytes()).unwrap();

        let llm = Arc::new(FakeLlm {
            response: "ok".to_string(),
        });
        let processor = VisionProcessor::new(llm, "test/model");

        let answer = processor
            .process(&ImageInput::path(&path), "describe this")
            .await
            .unwrap();

        assert_eq!(answer, "ok");
    }

    #[tokio::test]
    async fn unreadable_path_errors_without_ever_reaching_the_model() {
        struct PanicLlm;
        #[async_trait]
        impl Llm for PanicLlm {
            fn name(&self) -> &str {
                "panic"
            }
            async fn generate_content(
                &self,
                _req: LlmRequest,
                _stream: bool,
            ) -> adk_rust::Result<adk_rust::LlmResponseStream> {
                panic!("model backend must not be called for an unreadable path");
            }
        }
        let processor = VisionProcessor::new(Arc::new(PanicLlm), "test/model");

        let err = processor
            .process(&ImageInput::path("/no/such/file.png"), "describe this")
            .await
            .unwrap_err();

        assert!(matches!(err, VisionError::Read { .. }));
    }

    #[tokio::test]
    async fn unrecognized_format_errors_before_calling_the_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, b"just some text, not an image").unwrap();

        struct PanicLlm;
        #[async_trait]
        impl Llm for PanicLlm {
            fn name(&self) -> &str {
                "panic"
            }
            async fn generate_content(
                &self,
                _req: LlmRequest,
                _stream: bool,
            ) -> adk_rust::Result<adk_rust::LlmResponseStream> {
                panic!("model backend must not be called for an unrecognized format");
            }
        }
        let processor = VisionProcessor::new(Arc::new(PanicLlm), "test/model");

        let err = processor
            .process(&ImageInput::path(&path), "describe this")
            .await
            .unwrap_err();

        assert!(matches!(err, VisionError::UnknownFormat(_)));
    }

    /// `agy` genuinely isn't installed in this workspace's build/test
    /// environment, so this exercises the real fallback path (not a stub):
    /// the agy spawn fails with "No such file or directory" and `process`
    /// still returns the model backend's answer.
    #[tokio::test]
    async fn falls_back_to_the_model_when_agy_is_not_on_path() {
        let llm = Arc::new(FakeLlm {
            response: "fallback answer".to_string(),
        });
        let processor = VisionProcessor::new(llm, "test/model").with_agy("gemini-3.6-flash-high");
        let image = ImageInput::pasted("image/png", png_bytes());

        let answer = processor.process(&image, "describe this").await.unwrap();

        assert_eq!(answer, "fallback answer");
    }
}
