use async_trait::async_trait;
use serde_json::json;
use crate::{Tool, ToolError};

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
        let client = reqwest::Client::new();
        match client.get(&self.endpoint).send().await {
            Ok(resp) => {
                let status = resp.status();
                Ok(format!("OpenClaw Gateway ({}) status: HTTP {}", self.endpoint, status))
            }
            Err(e) => Ok(format!("OpenClaw Gateway ({}) probe failed: {}", self.endpoint, e)),
        }
    }

    fn tier(&self) -> entheai_permission::Tier {
        entheai_permission::Tier::Read
    }
}
