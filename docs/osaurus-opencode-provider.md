# Routing OpenCode Zen through Osaurus (custom cloud provider)

Osaurus can proxy to cloud providers, so its local `:1337` endpoint serves **both** your local MLX models **and** cloud models like DeepSeek V4 Pro via [OpenCode Zen](https://opencode.ai) — one endpoint, one config in `entheai`. This guide adds Zen as a custom Osaurus provider.

> **Simpler alternative (skip Osaurus):** point `entheai` *directly* at Zen — uncomment the `[providers.zen]` block in `entheai.toml`, `export OPENCODE_API_KEY=…`, set `default_model = "zen/deepseek-v4-pro"`. Use this guide only if you specifically want Osaurus as a unified local gateway.

## 0. First, that 401 ("No access keys configured")

That error means **Expose to Network is ON** in Osaurus — which makes even `127.0.0.1` require an access key. Two ways to resolve it:

- **Easiest (local-only):** turn **Expose to Network OFF** (Management window `⌘⇧M` → Server). Loopback then skips auth entirely — no key needed. `entheai`'s default `[providers.osaurus]` (no `api_key_env`) just works.
- **Keep it exposed:** create an access key — `⌘⇧M` → **Server → Overview tab → Access Keys → Generate** (give it a label like `entheai`, pick an expiry). **Copy it immediately — it's shown only once** (format `osk-v1…`). Then tell `entheai` to send it:
  ```toml
  [providers.osaurus]
  base_url = "http://127.0.0.1:1337/v1"
  api_key_env = "OSAURUS_KEY"
  ```
  ```bash
  export OSAURUS_KEY='osk-v1....'
  ```
  (`entheai` sends it as `Authorization: Bearer $OSAURUS_KEY`.)

## 1. Add OpenCode Zen as a Remote Provider

Get a Zen API key first: https://opencode.ai/auth.

Open **Management window (`⌘⇧M`) → Providers → Add Provider → Custom**, and fill in:

| Field | Value |
|---|---|
| **Name** | `OpenCode Zen` |
| **Host** | `opencode.ai` |
| **Protocol** | HTTPS |
| **Port** | `443` |
| **Base Path** | `/zen/v1` |
| **API Format** | OpenAI (`/chat/completions`) |
| **Auth Type** | API Key |
| **API Key** | *your Zen key* (stored in the macOS Keychain) |
| **Enabled** | on |

Then **Save**. (Under the hood, non-secret settings land in `~/.osaurus/providers/remote.json`; the key stays in the Keychain.)

Osaurus tries to auto-discover models from Zen's `/models`. **If no models appear**, add them by hand in the provider's **manual model IDs** field, e.g.:

```
deepseek-v4-pro
deepseek-v4-flash
qwen3.7-plus
```

## 2. Confirm the model id Osaurus exposes

```bash
curl -s http://127.0.0.1:1337/v1/models | jq -r '.data[].id'
```

Cloud models appear under their **native, unprefixed** id (e.g. `deepseek-v4-pro`) — *not* `zen/…`. Copy the exact id you want.

## 3. Point entheai at it

In `entheai.toml`, use the **Osaurus** provider with the cloud model's id:

```toml
default_model = "osaurus/deepseek-v4-pro"   # the raw id from /v1/models above

[providers.osaurus]
base_url = "http://127.0.0.1:1337/v1"
# api_key_env = "OSAURUS_KEY"   # only if Expose to Network is ON (see step 0)
```

## 4. Run it

```bash
cargo build --release
./target/release/entheai --yolo "read Cargo.toml and list the workspace crates"
```

Now `entheai → Osaurus (:1337) → OpenCode Zen → DeepSeek V4 Pro`. Osaurus handles the Zen key server-side; your local model and cloud models share one endpoint.

## Notes

- **Two keys are independent:** the local access-key gate (only when Expose is ON) vs. the Zen key (server-side, in Keychain).
- If a cloud model 401s from *Zen*, your Zen key/billing is the issue (check https://opencode.ai/auth). If it 401s from *Osaurus* (`osk-v1…` message), it's step 0.
- Verify the exact model id via `/v1/models` — Osaurus may or may not auto-discover Zen's list; the manual-IDs field is the fallback.
