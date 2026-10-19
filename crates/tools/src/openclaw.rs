use crate::{Tool, ToolError};
use async_trait::async_trait;
use serde_json::json;

/// OpenClaw Health Check tool for entheai.
pub struct OpenClawProbe {
    endpoint: String,
}

impl OpenClawProbe {
    pub fn new(endpoint: String) -> Self {
        Self { endpoint }
    }
}

impl Default for OpenClawProbe {
    fn default() -> Self {
        Self {
            endpoint: "https://do-openclaw.tail2870dc.ts.net/".to_string(),
        }
    }
}

#[async_trait]
impl Tool for OpenClawProbe {
    fn name(&self) -> &str {
        "openclaw_probe"
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "name": "openclaw_probe",
            "description": "Probe OpenClaw Gateway on DigitalOcean over Tailscale for status and health",
            "parameters": {
                "type": "object",
                "properties": {},
                "required": []
            }
        })
    }

    async fn call(&self, _args: serde_json::Value) -> Result<String, ToolError> {
        // No timeout previously: a stalled tailnet host hung the agent turn
        // indefinitely (this tool is Tier::Read so it may auto-run unattended).
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .unwrap_or_default();
        match client.get(&self.endpoint).send().await {
            Ok(resp) => {
                let status = resp.status();
                Ok(format!(
                    "OpenClaw Gateway ({}) status: HTTP {}",
                    self.endpoint, status
                ))
            }
            Err(e) => Ok(format!(
                "OpenClaw Gateway ({}) probe failed: {}",
                self.endpoint, e
            )),
        }
    }

    fn tier(&self) -> entheai_permission::Tier {
        entheai_permission::Tier::Read
    }
}
