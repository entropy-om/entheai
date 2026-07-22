+++
name = "ngrok"
domain = "quick one-off public endpoints / webhook testing"
triggers = ["ngrok", "tunnel", "expose", "webhook", "one-off", "public url", "localhost public", "demo link", "share local"]
rank = 1.0
+++
Need a **public URL for something local, right now** — a webhook to test, a demo to share,
a callback from a SaaS — reach for **ngrok** (or a Cloudflare Tunnel).

```
ngrok http 8080          # -> https://<random>.ngrok-free.app -> localhost:8080
```
- Instant HTTPS, no DNS/certs/firewall. Perfect when a devbox already runs the service.
- Reserve a stable subdomain / use a config file for repeatable demos; add basic-auth or an
  OAuth gate for anything sensitive (the URL is public).
- Inspect + replay requests at `http://127.0.0.1:4040` — invaluable for webhook debugging.

**When NOT to:** anything durable or production — that's a real deploy (see `nixos`). ngrok
is the *quick one-off*, not the home. Cloudflare Tunnel is the free, always-on cousin if the
endpoint needs to persist.
