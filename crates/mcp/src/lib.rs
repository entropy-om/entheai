//! Minimal Model Context Protocol (MCP) client over stdio (newline-delimited
//! JSON-RPC 2.0). Spawns a server, initializes, lists + calls tools; each MCP
//! tool is adapted to entheai's `Tool` trait so the agent can call it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{oneshot, Mutex};

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("mcp io: {0}")]
    Io(#[from] std::io::Error),
    #[error("mcp json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("mcp server closed the connection")]
    Closed,
    #[error("mcp rpc error: {0}")]
    Rpc(String),
    #[error("mcp: {0}")]
    Other(String),
}

/// A tool advertised by an MCP server.
#[derive(Debug, Clone)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Outcome of one in-flight JSON-RPC request, routed back by the reader task.
type PendingResult = Result<Value, String>;
/// Requests awaiting a response, keyed by JSON-RPC request id.
type PendingMap = HashMap<u64, oneshot::Sender<PendingResult>>;

/// Client for one MCP server connection. Cheaply cloneable via `Arc`.
pub struct McpClient {
    writer: Mutex<Box<dyn AsyncWrite + Unpin + Send>>,
    pending: Arc<Mutex<PendingMap>>,
    next_id: AtomicU64,
    /// The server's name (from config), used to namespace tool names.
    server_name: String,
}

impl McpClient {
    /// Background reader task: owns the `BufReader<R>`, reads newline-delimited
    /// JSON-RPC messages, and routes responses (messages carrying `id` plus a
    /// `result` or `error`) to the matching pending request. Messages without
    /// an `id` (notifications/logs from the server) are ignored. On EOF the
    /// remaining pending requests are failed with `Closed`.
    async fn reader_loop<R>(reader: R, pending: Arc<Mutex<PendingMap>>)
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let mut lines = BufReader::new(reader).lines();
        loop {
            let line = match lines.next_line().await {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(_) => break,
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let value: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let Some(id) = value.get("id").and_then(Value::as_u64) else {
                // Notification/log with no id: nothing to route.
                continue;
            };
            let outcome = if let Some(error) = value.get("error") {
                let msg = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown mcp error")
                    .to_string();
                Err(msg)
            } else if let Some(result) = value.get("result") {
                Ok(result.clone())
            } else {
                // Neither result nor error present; nothing to route.
                continue;
            };
            let mut pending = pending.lock().await;
            if let Some(tx) = pending.remove(&id) {
                let _ = tx.send(outcome);
            }
        }
        let mut pending = pending.lock().await;
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err("closed".to_string()));
        }
    }

    async fn write_line(&self, value: &Value) -> Result<(), McpError> {
        let mut line = serde_json::to_string(value)?;
        line.push('\n');
        let mut writer = self.writer.lock().await;
        writer.write_all(line.as_bytes()).await?;
        writer.flush().await?;
        Ok(())
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(err) = self.write_line(&msg).await {
            self.pending.lock().await.remove(&id);
            return Err(err);
        }
        match rx.await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(msg)) => Err(McpError::Rpc(msg)),
            Err(_) => Err(McpError::Closed),
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_line(&msg).await
    }

    /// Wrap an already-connected reader/writer pair (or an in-memory mock) in
    /// an `McpClient` and perform the MCP initialize handshake.
    pub async fn connect<R, W>(
        reader: R,
        writer: W,
        server_name: impl Into<String>,
    ) -> Result<Arc<Self>, McpError>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let pending: Arc<Mutex<PendingMap>> = Arc::new(Mutex::new(HashMap::new()));

        let client = Arc::new(Self {
            writer: Mutex::new(Box::new(writer)),
            pending: pending.clone(),
            next_id: AtomicU64::new(1),
            server_name: server_name.into(),
        });

        tokio::spawn(Self::reader_loop(reader, pending));

        client
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "entheai", "version": "0.1"},
                }),
            )
            .await?;

        client
            .notify("notifications/initialized", json!({}))
            .await?;

        Ok(client)
    }

    /// Spawn an MCP server subprocess, wire its stdio, and connect. Returns
    /// the client plus a guard that kills the child when dropped.
    pub async fn spawn(
        command: &str,
        args: &[String],
        server_name: impl Into<String>,
    ) -> Result<(Arc<Self>, ChildGuard), McpError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);

        let mut child = cmd.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Other("mcp child has no stdout".to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Other("mcp child has no stdin".to_string()))?;

        let client = Self::connect(stdout, stdin, server_name).await?;
        Ok((client, ChildGuard { child }))
    }

    pub async fn list_tools(&self) -> Result<Vec<McpToolDef>, McpError> {
        let result = self.request("tools/list", json!({})).await?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let defs = tools
            .into_iter()
            .filter_map(|tool| {
                let name = tool.get("name")?.as_str()?.to_string();
                let description = tool
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let input_schema = tool.get("inputSchema").cloned().unwrap_or(Value::Null);
                Some(McpToolDef {
                    name,
                    description,
                    input_schema,
                })
            })
            .collect();
        Ok(defs)
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<String, McpError> {
        let result = self
            .request("tools/call", json!({"name": name, "arguments": arguments}))
            .await?;

        let text = result
            .get("content")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|item| item.get("text").and_then(Value::as_str))
                    .collect::<String>()
            })
            .unwrap_or_default();

        let is_error = result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        if is_error {
            Err(McpError::Rpc(text))
        } else {
            Ok(text)
        }
    }
}

/// Keeps an MCP server subprocess alive for the session; killed on drop.
pub struct ChildGuard {
    child: tokio::process::Child,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// Adapts one MCP tool to entheai's `Tool` trait, namespaced by server name
/// to avoid collisions between tools from different MCP servers.
pub struct McpTool {
    client: Arc<McpClient>,
    def: McpToolDef,
    full_name: String,
}

impl McpTool {
    pub fn new(client: Arc<McpClient>, def: McpToolDef) -> Self {
        let full_name = format!("{}__{}", client.server_name, def.name);
        Self {
            client,
            def,
            full_name,
        }
    }
}

#[async_trait::async_trait]
impl entheai_tools::Tool for McpTool {
    fn name(&self) -> &str {
        &self.full_name
    }

    fn schema(&self) -> Value {
        let parameters = match &self.def.input_schema {
            Value::Null => json!({"type": "object", "properties": {}}),
            Value::Object(map) if map.is_empty() => json!({"type": "object", "properties": {}}),
            other => other.clone(),
        };
        json!({
            "type": "function",
            "function": {
                "name": self.full_name,
                "description": self.def.description,
                "parameters": parameters,
            }
        })
    }

    async fn call(&self, args: Value) -> Result<String, entheai_tools::ToolError> {
        self.client
            .call_tool(&self.def.name, args)
            .await
            .map_err(|err| entheai_tools::ToolError::Mcp(err.to_string()))
    }
}

/// List tools on `client` and wrap each as an `McpTool`.
pub async fn load_tools(client: Arc<McpClient>) -> Result<Vec<McpTool>, McpError> {
    let defs = client.list_tools().await?;
    Ok(defs
        .into_iter()
        .map(|def| McpTool::new(client.clone(), def))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use entheai_tools::Tool;
    use tokio::io::DuplexStream;
    use tokio::time::{timeout, Duration};

    /// Minimal in-memory MCP server: reads newline-delimited JSON-RPC
    /// requests off `server_read` and writes responses to `server_write`.
    fn spawn_mock_server(server_read: DuplexStream, server_write: DuplexStream) {
        tokio::spawn(async move {
            let mut lines = BufReader::new(server_read).lines();
            let mut writer = server_write;
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let msg: Value = match serde_json::from_str(trimmed) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
                let id = msg.get("id").cloned();

                match method {
                    "initialize" => {
                        let resp = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "protocolVersion": "2024-11-05",
                                "serverInfo": {"name": "mock", "version": "0"},
                                "capabilities": {},
                            }
                        });
                        write_resp(&mut writer, &resp).await;
                    }
                    "notifications/initialized" => {
                        // Notification: no response expected.
                    }
                    "tools/list" => {
                        let resp = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "tools": [
                                    {
                                        "name": "echo",
                                        "description": "echoes text",
                                        "inputSchema": {
                                            "type": "object",
                                            "properties": {"text": {"type": "string"}}
                                        }
                                    }
                                ]
                            }
                        });
                        write_resp(&mut writer, &resp).await;
                    }
                    "tools/call" => {
                        let params = msg.get("params").cloned().unwrap_or(Value::Null);
                        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                        if name == "echo" {
                            let text = params
                                .get("arguments")
                                .and_then(|a| a.get("text"))
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let resp = json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": {
                                    "content": [{"type": "text", "text": format!("echoed: {text}")}],
                                    "isError": false,
                                }
                            });
                            write_resp(&mut writer, &resp).await;
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    async fn write_resp(writer: &mut DuplexStream, value: &Value) {
        let mut line = serde_json::to_string(value).unwrap();
        line.push('\n');
        writer.write_all(line.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();
    }

    /// Wires a mock server to a fresh `McpClient` over two in-memory duplex
    /// pipes (client-writes->server-reads, server-writes->client-reads).
    async fn connect_to_mock() -> Arc<McpClient> {
        let (client_read, server_write) = tokio::io::duplex(8192);
        let (server_read, client_write) = tokio::io::duplex(8192);
        spawn_mock_server(server_read, server_write);

        timeout(
            Duration::from_secs(2),
            McpClient::connect(client_read, client_write, "mock"),
        )
        .await
        .expect("connect timed out")
        .expect("connect failed")
    }

    #[tokio::test]
    async fn connect_completes_handshake_without_hanging() {
        let _client = connect_to_mock().await;
    }

    #[tokio::test]
    async fn list_tools_returns_the_echo_tool() {
        let client = connect_to_mock().await;
        let tools = timeout(Duration::from_secs(2), client.list_tools())
            .await
            .expect("list_tools timed out")
            .expect("list_tools failed");

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(tools[0].description, "echoes text");
        assert_eq!(
            tools[0].input_schema,
            json!({"type": "object", "properties": {"text": {"type": "string"}}})
        );
    }

    #[tokio::test]
    async fn call_tool_echoes_text() {
        let client = connect_to_mock().await;
        let out = timeout(
            Duration::from_secs(2),
            client.call_tool("echo", json!({"text": "hi"})),
        )
        .await
        .expect("call_tool timed out")
        .expect("call_tool failed");

        assert_eq!(out, "echoed: hi");
    }

    #[tokio::test]
    async fn mcp_tool_adapter_calls_through_to_the_mock_server() {
        let client = connect_to_mock().await;
        let defs = timeout(Duration::from_secs(2), client.list_tools())
            .await
            .expect("list_tools timed out")
            .expect("list_tools failed");
        let def = defs.into_iter().next().expect("mock advertises one tool");

        let tool = McpTool::new(client, def);
        assert_eq!(tool.name(), "mock__echo");
        assert_eq!(tool.schema()["function"]["name"], "mock__echo");

        let out = timeout(Duration::from_secs(2), tool.call(json!({"text": "yo"})))
            .await
            .expect("call timed out")
            .expect("call failed");
        assert_eq!(out, "echoed: yo");
    }
}
