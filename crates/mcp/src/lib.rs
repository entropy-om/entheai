//! Minimal Model Context Protocol (MCP) client over stdio (newline-delimited
//! JSON-RPC 2.0). Spawns a server, initializes, lists + calls tools; each MCP
//! tool is adapted to entheai's `Tool` trait so the agent can call it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{oneshot, Mutex};

/// Maximum size, in bytes, of a single newline-delimited JSON-RPC message
/// read from an MCP server. A malicious or broken server that streams bytes
/// without ever sending a newline would otherwise grow the reader's line
/// buffer unbounded and OOM the process; lines longer than this are
/// discarded (logged) instead of crashing the reader. Generous enough for
/// any legitimate JSON-RPC message, including large tool outputs.
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

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
    #[error("mcp timeout: {0}")]
    Timeout(String),
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
    /// Applied to the `initialize` handshake and every subsequent request
    /// (`tools/list`, `tools/call`, ...); bounds how long a hung or
    /// unresponsive server can block the caller. Sourced from
    /// `mcp_defaults.spawn_timeout_secs`.
    timeout: Duration,
}

/// Outcome of reading one newline-delimited chunk with `read_capped_line`.
enum ReadLineOutcome {
    /// A complete line (newline stripped), within the byte cap.
    Line(String),
    /// The line's length exceeded `max_bytes` before a newline was found.
    /// Its bytes (up to and including the newline, if any) were still
    /// consumed from `reader` so stream framing stays intact for the next
    /// line; the content itself is discarded.
    TooLong,
    /// End of stream with no more data.
    Eof,
}

/// Reads one newline-delimited line from `reader`, never buffering more than
/// `max_bytes` of it. Bytes beyond the cap are still consumed from the
/// underlying reader (to keep line framing intact for what follows) but are
/// not appended to the in-memory buffer, so a line of unbounded length
/// cannot grow memory unboundedly.
async fn read_capped_line<R>(reader: &mut R, max_bytes: usize) -> std::io::Result<ReadLineOutcome>
where
    R: AsyncBufRead + Unpin,
{
    let mut buf: Vec<u8> = Vec::new();
    let mut too_long = false;
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return Ok(if too_long {
                ReadLineOutcome::TooLong
            } else if buf.is_empty() {
                ReadLineOutcome::Eof
            } else {
                // EOF without a trailing newline: surface what we have,
                // matching `AsyncBufReadExt::lines()`'s behavior.
                ReadLineOutcome::Line(String::from_utf8_lossy(&buf).into_owned())
            });
        }
        if let Some(pos) = chunk.iter().position(|&b| b == b'\n') {
            if !too_long {
                if buf.len() + pos > max_bytes {
                    too_long = true;
                } else {
                    buf.extend_from_slice(&chunk[..pos]);
                }
            }
            let consumed = pos + 1;
            reader.consume(consumed);
            return Ok(if too_long {
                ReadLineOutcome::TooLong
            } else {
                ReadLineOutcome::Line(String::from_utf8_lossy(&buf).into_owned())
            });
        }
        if !too_long {
            if buf.len() + chunk.len() > max_bytes {
                too_long = true;
            } else {
                buf.extend_from_slice(chunk);
            }
        }
        let n = chunk.len();
        reader.consume(n);
    }
}

impl McpClient {
    /// Background reader task: owns the `BufReader<R>`, reads newline-delimited
    /// JSON-RPC messages (each capped at `max_line_bytes` to bound memory
    /// against a server that streams bytes without a newline), and routes
    /// responses (messages carrying `id` plus a `result` or `error`) to the
    /// matching pending request. Messages without an `id`
    /// (notifications/logs from the server) are ignored. On EOF the
    /// remaining pending requests are failed with `Closed`.
    async fn reader_loop<R>(reader: R, pending: Arc<Mutex<PendingMap>>, max_line_bytes: usize)
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let mut reader = BufReader::new(reader);
        loop {
            let line = match read_capped_line(&mut reader, max_line_bytes).await {
                Ok(ReadLineOutcome::Line(line)) => line,
                Ok(ReadLineOutcome::TooLong) => {
                    eprintln!(
                        "mcp: dropping an oversized line (> {max_line_bytes} bytes) from server"
                    );
                    continue;
                }
                Ok(ReadLineOutcome::Eof) => break,
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

    /// Send a JSON-RPC request and await its response, bounded by
    /// `self.timeout`. A server that never replies (hung, or a
    /// misconfigured non-MCP command like `cat` that echoes the request
    /// back without a `result`/`error`) fails this call with
    /// `McpError::Timeout` instead of hanging forever; the pending entry is
    /// removed on timeout so it can't linger or be resolved later.
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
        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(msg))) => Err(McpError::Rpc(msg)),
            Ok(Err(_)) => Err(McpError::Closed),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(McpError::Timeout(format!(
                    "mcp server '{}' did not respond to '{}' within {}s",
                    self.server_name,
                    method,
                    self.timeout.as_secs()
                )))
            }
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
    /// an `McpClient` and perform the MCP initialize handshake. `timeout`
    /// bounds the handshake itself (via `request`) as well as every request
    /// made through the returned client afterward — a server that accepts
    /// the connection but never replies fails with `McpError::Timeout`
    /// rather than hanging `connect` (or the caller) forever.
    pub async fn connect<R, W>(
        reader: R,
        writer: W,
        server_name: impl Into<String>,
        timeout: Duration,
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
            timeout,
        });

        tokio::spawn(Self::reader_loop(reader, pending, MAX_LINE_BYTES));

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
    /// the client plus a guard that kills the child when dropped. `timeout`
    /// bounds the initialize handshake and every later request (see
    /// `connect`); typically `mcp_defaults.spawn_timeout_secs`.
    pub async fn spawn(
        command: &str,
        args: &[String],
        server_name: impl Into<String>,
        timeout: Duration,
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

        let client = Self::connect(stdout, stdin, server_name, timeout).await?;
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
    /// pipes (client-writes->server-reads, server-writes->client-reads),
    /// using `client_timeout` as the client's per-request timeout.
    async fn connect_to_mock_with_timeout(client_timeout: Duration) -> Arc<McpClient> {
        let (client_read, server_write) = tokio::io::duplex(8192);
        let (server_read, client_write) = tokio::io::duplex(8192);
        spawn_mock_server(server_read, server_write);

        timeout(
            Duration::from_secs(2),
            McpClient::connect(client_read, client_write, "mock", client_timeout),
        )
        .await
        .expect("connect timed out")
        .expect("connect failed")
    }

    async fn connect_to_mock() -> Arc<McpClient> {
        connect_to_mock_with_timeout(Duration::from_secs(2)).await
    }

    #[tokio::test]
    async fn connect_completes_handshake_without_hanging() {
        let _client = connect_to_mock().await;
    }

    /// Bug 1: a server that accepts the connection but never replies to
    /// `initialize` must not hang `connect` forever — it should fail with
    /// `McpError::Timeout` within the configured (here, short) bound.
    #[tokio::test]
    async fn connect_times_out_when_server_never_replies_to_initialize() {
        let (client_read, _server_write) = tokio::io::duplex(8192);
        let (_server_read, client_write) = tokio::io::duplex(8192);
        // No mock server is attached to either end: nothing ever reads the
        // request or writes a response, simulating a connected-but-silent
        // server.

        let result = timeout(
            Duration::from_secs(2),
            McpClient::connect(
                client_read,
                client_write,
                "silent",
                Duration::from_millis(100),
            ),
        )
        .await
        .expect("connect() hung past the outer test timeout — timeout not enforced");

        match result {
            Err(McpError::Timeout(_)) => {}
            Err(other) => panic!("expected McpError::Timeout, got a different error: {other}"),
            Ok(_) => panic!("expected connect() to time out, but it succeeded"),
        }
    }

    /// Bug 1 (exact scenario from the report): a misconfigured `command =
    /// "cat"` server echoes the request straight back — valid JSON, matching
    /// `id`, but with neither `result` nor `error` — so the reader loop
    /// `continue`s and the pending request is never resolved by a normal
    /// response. `connect` must still time out rather than hang.
    #[tokio::test]
    async fn connect_times_out_against_a_cat_like_echo_server() {
        let (client_read, server_write) = tokio::io::duplex(8192);
        let (server_read, client_write) = tokio::io::duplex(8192);

        tokio::spawn(async move {
            let mut lines = BufReader::new(server_read).lines();
            let mut writer = server_write;
            while let Ok(Some(line)) = lines.next_line().await {
                let mut out = line;
                out.push('\n');
                if writer.write_all(out.as_bytes()).await.is_err() {
                    break;
                }
                let _ = writer.flush().await;
            }
        });

        let result = timeout(
            Duration::from_secs(2),
            McpClient::connect(
                client_read,
                client_write,
                "cat-mcp",
                Duration::from_millis(150),
            ),
        )
        .await
        .expect("connect() hung against a cat-like echo server — timeout not enforced");

        match result {
            Err(McpError::Timeout(_)) => {}
            Err(other) => panic!("expected McpError::Timeout, got a different error: {other}"),
            Ok(_) => panic!("expected connect() to time out, but it succeeded"),
        }
    }

    /// Bug 1: a mid-session request (after a successful handshake) to a
    /// server that stops responding must also time out rather than hang the
    /// caller forever, and the client must remain usable afterward (a single
    /// unresolved request can't wedge the whole client).
    #[tokio::test]
    async fn request_times_out_mid_session_and_client_stays_usable() {
        let client = connect_to_mock_with_timeout(Duration::from_millis(150)).await;

        // The mock server silently ignores unknown methods (no response),
        // simulating a server that stops responding mid-session.
        let result = timeout(
            Duration::from_secs(2),
            client.request("not/a/real/method", json!({})),
        )
        .await
        .expect("request() hung instead of honoring the per-request timeout");

        match result {
            Err(McpError::Timeout(_)) => {}
            other => panic!("expected McpError::Timeout, got {other:?}"),
        }

        // The timed-out request's pending entry must not wedge later calls.
        let tools = timeout(Duration::from_secs(2), client.list_tools())
            .await
            .expect("list_tools timed out after a prior request timed out")
            .expect("list_tools failed after a prior request timed out");
        assert_eq!(tools.len(), 1);
    }

    /// Bug 2: a line far larger than the cap, with no newline for a long
    /// stretch, must not grow the reader's buffer past the cap, must not
    /// panic, and must not prevent later well-formed messages from being
    /// routed correctly.
    #[tokio::test]
    async fn reader_loop_survives_an_oversized_line_and_still_routes_later_responses() {
        let (client_read, mut server_write) = tokio::io::duplex(1 << 20);
        let pending: Arc<Mutex<PendingMap>> = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(1, tx);

        // Tiny cap so the test doesn't need to push megabytes of data.
        const TEST_CAP: usize = 64;
        tokio::spawn(McpClient::reader_loop(
            client_read,
            pending.clone(),
            TEST_CAP,
        ));

        // Oversized line: well over the 64-byte cap, followed by a newline.
        let oversized = format!("{}\n", "a".repeat(TEST_CAP * 4));
        server_write.write_all(oversized.as_bytes()).await.unwrap();

        // A normal, properly small JSON-RPC response for id=1 must still be
        // routed correctly after the oversized line is discarded.
        let resp = json!({"jsonrpc": "2.0", "id": 1, "result": {"ok": true}});
        let mut line = serde_json::to_string(&resp).unwrap();
        line.push('\n');
        server_write.write_all(line.as_bytes()).await.unwrap();
        server_write.flush().await.unwrap();

        let outcome = timeout(Duration::from_secs(2), rx)
            .await
            .expect("reader_loop did not route the response after an oversized line")
            .expect("pending sender dropped");
        assert_eq!(outcome.unwrap(), json!({"ok": true}));
    }

    /// Bug 2 (unit-level): `read_capped_line` itself never buffers more than
    /// `max_bytes`, correctly flags the oversized line as `TooLong`, and
    /// resumes normal line reading afterward.
    #[tokio::test]
    async fn read_capped_line_bounds_an_oversized_line_and_recovers() {
        let (read_half, mut write_half) = tokio::io::duplex(4096);
        let mut input = Vec::new();
        input.extend_from_slice(&b"a".repeat(50));
        input.push(b'\n');
        input.extend_from_slice(b"hello\n");
        write_half.write_all(&input).await.unwrap();
        write_half.flush().await.unwrap();
        drop(write_half); // signals EOF once the buffered bytes are consumed

        let mut reader = BufReader::new(read_half);

        match read_capped_line(&mut reader, 10).await.unwrap() {
            ReadLineOutcome::TooLong => {}
            _ => panic!("expected TooLong for the oversized line"),
        }
        match read_capped_line(&mut reader, 10).await.unwrap() {
            ReadLineOutcome::Line(line) => assert_eq!(line, "hello"),
            _ => panic!("expected Line(\"hello\") after the oversized line"),
        }
        match read_capped_line(&mut reader, 10).await.unwrap() {
            ReadLineOutcome::Eof => {}
            _ => panic!("expected Eof at end of input"),
        }
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
