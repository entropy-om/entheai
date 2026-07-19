//! Best-effort nudge to the obsidian-claude-code-mcp WebSocket (spec §7).
//! Every failure (socket down, Obsidian closed, timeout) is swallowed.

use std::path::Path;
use std::time::Duration;

/// Nudge Obsidian to refresh `changed` notes under the vault, if the plugin is
/// listening on `127.0.0.1:<port>`. Never errors — returns `Ok(())` regardless.
/// The `open`-file method name is the plugin's convention and may be adjusted
/// once verified against the live plugin; correctness here is the swallow, not
/// the plugin accepting the message.
pub async fn best_effort(port: u16, vault_subtree: &Path, changed: &[std::path::PathBuf]) {
    if changed.is_empty() {
        return;
    }
    // Bounded so a hung socket never stalls the session.
    let _ = tokio::time::timeout(
        Duration::from_millis(400),
        try_nudge(port, vault_subtree, changed),
    )
    .await;
}

async fn try_nudge(port: u16, vault_subtree: &Path, changed: &[std::path::PathBuf]) {
    use futures_util::SinkExt;
    let url = format!("ws://127.0.0.1:{port}");
    let Ok(Ok((mut ws, _))) = tokio::time::timeout(
        Duration::from_millis(200),
        tokio_tungstenite::connect_async(&url),
    )
    .await
    else {
        return; // socket down / not a websocket → swallow
    };
    for rel in changed {
        let abs = vault_subtree.join(rel);
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "obsidian/openFile",
            "params": { "path": abs.to_string_lossy() }
        })
        .to_string();
        let _ = ws
            .send(tokio_tungstenite::tungstenite::Message::Text(msg))
            .await;
    }
    let _ = ws.close(None).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn socket_down_is_swallowed() {
        // Nothing listening on this port → must return without panicking/erroring
        // and within the timeout budget.
        best_effort(
            0,
            Path::new("/tmp/nonexistent-vault"),
            &[PathBuf::from("Home.md")],
        )
        .await;
        // Reaching here means no panic and no hang: the contract holds.
    }

    #[tokio::test]
    async fn empty_changed_is_a_fast_noop() {
        best_effort(22360, Path::new("/tmp/whatever"), &[]).await;
    }
}
