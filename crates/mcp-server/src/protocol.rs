//! Minimal MCP *server* loop over stdio (newline-delimited JSON-RPC 2.0) — the
//! mirror image of the `entheai-mcp` client in crates/mcp, and the same
//! wire pattern as the vault-mcp style servers. This is deliberately
//! hand-rolled (no rmcp dependency) so it speaks exactly the protocol entheai's
//! own client already speaks: `initialize` → `notifications/initialized` →
//! `tools/list` → `tools/call`, one JSON object per line on stdout.
//!
//! Leniency note: a request without an `id` (e.g. a manual
//! `echo '{"method":"tools/list"}' | entheai-mcp` smoke test) still gets a
//! response (with `"id": null`), so the server is trivially probe-able.

use std::path::PathBuf;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};

use crate::tools::{Handler, ToolDef};

/// Serve the MCP protocol until stdin closes (the parent decides lifetime).
/// One request per line in, one response per line out — but each request is
/// handled on its own task, not awaited inline in the read loop. A `300s
/// entheai_run` or up-to-`[federation].deadline_secs` `entheai_dispatch`
/// used to block `ping`/`tools/list`/`entheai_job_status` for its whole
/// duration; MCP hosts then time out waiting and treat the server as dead.
/// Responses are funneled through a single writer task (order = completion
/// order, not request order — MCP clients correlate by `id`, not by line).
pub async fn serve(tools: &'static [ToolDef], server_cwd: PathBuf) -> anyhow::Result<()> {
    let mut reader = BufReader::new(tokio::io::stdin()).lines();
    let (resp_tx, mut resp_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();

    let writer_task = tokio::spawn(async move {
        let mut writer = BufWriter::new(tokio::io::stdout());
        while let Some(resp) = resp_rx.recv().await {
            if write_resp(&mut writer, &resp).await.is_err() {
                break;
            }
        }
    });

    while let Some(line) = reader.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {"code": -32700, "message": format!("parse error: {e}")}
                });
                let _ = resp_tx.send(resp);
                continue;
            }
        };
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        // Notifications carry no id and expect no response — not just
        // "notifications/initialized": any notification method (e.g.
        // "notifications/cancelled", "notifications/roots/list_changed")
        // must be swallowed the same way. Responding with `"id": null`
        // (the -32601 fallback below) is a JSON-RPC-invalid reply that MCP
        // clients log as an unknown-id error.
        if method.starts_with("notifications/") {
            continue;
        }

        let tx = resp_tx.clone();
        let server_cwd = server_cwd.clone();
        tokio::spawn(async move {
            let response = match method.as_str() {
                "initialize" => initialize_response(&params, id),
                "ping" => json!({"jsonrpc": "2.0", "result": {}, "id": id}),
                "tools/list" => tools_list_response(tools, id),
                "tools/call" => tools_call_response(tools, &params, id, &server_cwd).await,
                other => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32601, "message": format!("method not found: {other}")}
                }),
            };
            let _ = tx.send(response);
        });
    }
    drop(resp_tx);
    let _ = writer_task.await;
    Ok(())
}

/// `initialize` handshake: echo the client's requested protocol version (SDKs
/// accept their own version back), advertise this server's name/version.
fn initialize_response(params: &Value, id: Value) -> Value {
    let proto = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("2024-11-05");
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": proto,
            "capabilities": {},
            "serverInfo": {"name": "entheai-mcp", "version": env!("CARGO_PKG_VERSION")}
        }
    })
}

/// `tools/list`: every registered tool as `{name, description, inputSchema}`.
fn tools_list_response(tools: &[ToolDef], id: Value) -> Value {
    let list: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.input_schema,
            })
        })
        .collect();
    json!({"jsonrpc": "2.0", "id": id, "result": {"tools": list}})
}

/// `tools/call`: find the tool, run its handler, wrap the outcome as MCP text
/// content. Errors become `isError: true` (never a protocol error — the caller
/// sees a failed tool, not a dead server).
async fn tools_call_response(
    tools: &[ToolDef],
    params: &Value,
    id: Value,
    server_cwd: &std::path::Path,
) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32602, "message": "tools/call requires a string 'name'"}
        });
    };
    let Some(def) = tools.iter().find(|t| t.name == name) else {
        return json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32602, "message": format!("unknown tool {name:?}")}
        });
    };
    let args = match params.get("arguments") {
        Some(Value::Object(_)) => params["arguments"].clone(),
        _ => json!({}),
    };
    match call_handler(def.handler, args, server_cwd.to_path_buf()).await {
        Ok(value) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{"type": "text", "text": value.to_string()}],
                "isError": false,
            }
        }),
        Err(e) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{"type": "text", "text": format!("error: {e:#}")}],
                "isError": true,
            }
        }),
    }
}

/// Box the handler call so `Handler` (a plain fn returning a pinned future)
/// needs no generic plumbing at the call site.
async fn call_handler(handler: Handler, args: Value, server_cwd: PathBuf) -> anyhow::Result<Value> {
    handler(args, server_cwd).await
}

async fn write_resp<W: AsyncWriteExt + Unpin>(
    writer: &mut BufWriter<W>,
    value: &Value,
) -> anyhow::Result<()> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}
