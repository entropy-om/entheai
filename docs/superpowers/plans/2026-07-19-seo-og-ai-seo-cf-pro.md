# SEO + OG image + AI-SEO + Cloudflare Pro Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add OG image + social meta tags, structured data, an AI-crawler-friendly `robots.txt` + `sitemap.xml`, an `llms.txt` footer link, and turn on the relevant Cloudflare Pro zone settings for entheai.com.

**Architecture:** Two independent layers — (1) static site content additions under `public/` (meta tags, JSON-LD, robots/sitemap, footer link, a one-time-generated OG image PNG), deployed through the existing CI pipeline; (2) direct Cloudflare API calls against the zone, using a freshly-minted, properly-scoped API token (the existing `cloudflare-api` MCP credential lacks zone-settings write permission).

**Tech Stack:** Plain HTML/meta tags, JSON-LD, Playwright MCP tools (for the one-time OG image screenshot — no new npm dependency), Cloudflare REST API (zone settings, Rulesets, Custom Pages, Web Analytics).

Spec: `docs/superpowers/specs/2026-07-19-seo-og-ai-seo-cf-pro-design.md`

---

### Task 1: Generate `public/og-image.png`

**Files:**
- Create: `public/og-image.png` (binary, generated via screenshot)

- [ ] **Step 1: Create a scratch HTML file for the OG card**

Create a temporary file `/tmp/og-card.html` (NOT committed to the repo — this is a throwaway rendering source, the PNG is the actual deliverable) with exactly this content:

```html
<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  body {
    width: 1200px; height: 630px; overflow: hidden;
    background: radial-gradient(120% 90% at 50% -10%, #121a2a 0%, #070a11 55%, #04060a 100%);
    font-family: "SF Mono", "Menlo", monospace;
    display: flex; align-items: center; justify-content: center;
    position: relative;
  }
  .glow {
    position: absolute; inset: 0;
    background: radial-gradient(circle at 50% 45%, rgba(47,240,226,0.22), transparent 60%);
  }
  .inner { position: relative; text-align: center; padding: 0 80px; }
  .badge {
    display: inline-flex; align-items: center; gap: 8px;
    font-weight: 600; font-size: 16px; letter-spacing: 0.02em;
    padding: 6px 16px; border-radius: 999px;
    background: rgba(47,240,226,0.14); color: #6ff7ec;
    border: 1px solid rgba(47,240,226,0.4);
    margin-bottom: 26px;
  }
  .badge .dot { width: 8px; height: 8px; border-radius: 999px; background: #6ff7ec; box-shadow: 0 0 8px #6ff7ec; }
  .title {
    font-weight: 800; font-size: 58px; line-height: 1.05; letter-spacing: -0.03em;
    color: #e6f2f3; margin-bottom: 22px;
  }
  .grad {
    background: linear-gradient(115deg, #2ff0e2 0%, #38c8ff 42%, #ff3fb4 100%);
    -webkit-background-clip: text; background-clip: text; -webkit-text-fill-color: transparent;
  }
  .url { font-size: 24px; color: #63747f; letter-spacing: 0.02em; }
</style>
</head>
<body>
  <div class="glow"></div>
  <div class="inner">
    <div class="badge"><span class="dot"></span>macOS &middot; Apple Silicon &middot; Rust</div>
    <div class="title">A coding agent with a<br>brain that <span class="grad">fans out.</span></div>
    <div class="url">entheai.com</div>
  </div>
</body>
</html>
```

- [ ] **Step 2: Render and screenshot it at exactly 1200x630**

Using your Playwright browser tools:
1. Resize the browser viewport to exactly 1200x630 (use the browser resize tool with width=1200, height=630).
2. Navigate to `file:///tmp/og-card.html`.
3. Take a screenshot (PNG, viewport-only — not full-page) and save it to `/Users/peter.lodri/workspace/peterlodri-sec/entheai/public/og-image.png`.

- [ ] **Step 3: Verify the image**

Read the resulting `public/og-image.png` file directly (as an image) to confirm it renders correctly: dark background, teal glow, "macOS · Apple Silicon · Rust" badge, the two-line headline with "fans out." in the teal→cyan→magenta gradient, "entheai.com" at the bottom. Confirm the image dimensions are exactly 1200x630 (check file properties or re-derive from the screenshot metadata).

- [ ] **Step 4: Clean up and commit**

```bash
rm -f /tmp/og-card.html
cd /Users/peter.lodri/workspace/peterlodri-sec/entheai
git add public/og-image.png
git commit -m "feat: add OG image (1200x630 social share card)"
```

Note: `public/og-image.png` is a real committed asset, NOT gitignored — it's a one-time design artifact, not build output.

---

### Task 2: OG/Twitter meta tags + canonical link on the landing page

**Files:**
- Modify: `public/index.html`

- [ ] **Step 1: Add the meta tags**

In `public/index.html`, find this line in the `<head>`:
```html
<meta name="description" content="entheai is a personal, macOS/Apple-Silicon, terminal-native hybrid coding agent. A cloud orchestrator plans; a swarm of model-matched sub-agents builds in parallel, in isolated worktrees, merged only after tests pass.">
```

Immediately after it, add:
```html
<meta property="og:title" content="entheai — a coding agent with a brain that fans out">
<meta property="og:description" content="A cloud orchestrator plans; a swarm of model-matched sub-agents builds in parallel, in isolated worktrees, merged only after tests pass.">
<meta property="og:image" content="https://entheai.com/og-image.png">
<meta property="og:image:width" content="1200">
<meta property="og:image:height" content="630">
<meta property="og:url" content="https://entheai.com/">
<meta property="og:type" content="website">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:image" content="https://entheai.com/og-image.png">
<link rel="canonical" href="https://entheai.com/">
```

- [ ] **Step 2: Commit**

```bash
cd /Users/peter.lodri/workspace/peterlodri-sec/entheai
git add public/index.html
git commit -m "feat: add OG/Twitter card meta tags to landing page"
```

---

### Task 3: OG/Twitter meta tags + canonical link on the docs shell

**Files:**
- Modify: `public/docs/_template.html`

- [ ] **Step 1: Add the meta tags**

In `public/docs/_template.html`, find this line in the `<head>`:
```html
<meta name="description" content="entheai documentation — installation, configuration, the agent loop, the tiered router, fan-out, permissions, memory, and architecture.">
```

Immediately after it, add:
```html
<meta property="og:title" content="entheai docs">
<meta property="og:description" content="entheai documentation — installation, configuration, the agent loop, the tiered router, fan-out, permissions, memory, and architecture.">
<meta property="og:image" content="https://entheai.com/og-image.png">
<meta property="og:image:width" content="1200">
<meta property="og:image:height" content="630">
<meta property="og:url" content="https://entheai.com/docs/">
<meta property="og:type" content="website">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:image" content="https://entheai.com/og-image.png">
<link rel="canonical" href="https://entheai.com/docs/">
```

- [ ] **Step 2: Rebuild and commit**

```bash
cd /Users/peter.lodri/workspace/peterlodri-sec/entheai
unset -f node npm npx nvm 2>/dev/null
export PATH="$HOME/.nvm/versions/node/v24.17.0/bin:$PATH"
npm run build
git add public/docs/_template.html
git commit -m "feat: add OG/Twitter card meta tags to docs shell"
```

Note: only `public/docs/_template.html` is committed — `public/docs/index.html` is generated/gitignored (running `npm run build` regenerates it locally so you can verify the change, but it must not be `git add`ed).

---

### Task 4: Structured data (JSON-LD) on the landing page

**Files:**
- Modify: `public/index.html`

- [ ] **Step 1: Add the JSON-LD block**

In `public/index.html`, immediately before the closing `</head>` tag, add:
```html
<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@type": "SoftwareApplication",
  "name": "entheai",
  "description": "A personal, macOS/Apple-Silicon, terminal-native hybrid coding agent written in Rust. A cloud orchestrator plans; it fans out to a swarm of model-matched sub-agents that build in parallel and merge back verified.",
  "applicationCategory": "DeveloperApplication",
  "operatingSystem": "macOS",
  "url": "https://entheai.com/",
  "downloadUrl": "https://github.com/peterlodri-sec/entheai",
  "offers": {
    "@type": "Offer",
    "price": "0",
    "priceCurrency": "USD"
  },
  "author": {
    "@type": "Person",
    "name": "Peter Lodri"
  }
}
</script>
```

- [ ] **Step 2: Validate the JSON is well-formed**

```bash
cd /Users/peter.lodri/workspace/peterlodri-sec/entheai
python3 -c "
import re, json
html = open('public/index.html').read()
m = re.search(r'<script type=\"application/ld\+json\">(.*?)</script>', html, re.S)
json.loads(m.group(1))
print('valid JSON-LD')
"
```
Expected output: `valid JSON-LD`.

- [ ] **Step 3: Commit**

```bash
git add public/index.html
git commit -m "feat: add SoftwareApplication JSON-LD structured data"
```

---

### Task 5: `public/robots.txt`

**Files:**
- Create: `public/robots.txt`

- [ ] **Step 1: Create the file**

Create `public/robots.txt`:
```
User-agent: GPTBot
Allow: /

User-agent: ChatGPT-User
Allow: /

User-agent: ClaudeBot
Allow: /

User-agent: anthropic-ai
Allow: /

User-agent: Claude-Web
Allow: /

User-agent: PerplexityBot
Allow: /

User-agent: Google-Extended
Allow: /

User-agent: CCBot
Allow: /

User-agent: Bytespider
Allow: /

User-agent: *
Allow: /

Sitemap: https://entheai.com/sitemap.xml
```

- [ ] **Step 2: Commit**

```bash
cd /Users/peter.lodri/workspace/peterlodri-sec/entheai
git add public/robots.txt
git commit -m "feat: add robots.txt with explicit AI-crawler allowlist"
```

---

### Task 6: `public/sitemap.xml`

**Files:**
- Create: `public/sitemap.xml`

- [ ] **Step 1: Create the file**

Create `public/sitemap.xml`:
```xml
<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url>
    <loc>https://entheai.com/</loc>
    <changefreq>weekly</changefreq>
    <priority>1.0</priority>
  </url>
  <url>
    <loc>https://entheai.com/docs/</loc>
    <changefreq>weekly</changefreq>
    <priority>0.8</priority>
  </url>
</urlset>
```

- [ ] **Step 2: Validate it's well-formed XML**

```bash
python3 -c "import xml.etree.ElementTree as ET; ET.parse('/Users/peter.lodri/workspace/peterlodri-sec/entheai/public/sitemap.xml'); print('valid XML')"
```
Expected output: `valid XML`.

- [ ] **Step 3: Commit**

```bash
cd /Users/peter.lodri/workspace/peterlodri-sec/entheai
git add public/sitemap.xml
git commit -m "feat: add sitemap.xml"
```

---

### Task 7: "For LLMs" footer link

**Files:**
- Modify: `public/index.html`
- Modify: `public/docs/_template.html`

- [ ] **Step 1: Add the link to the landing page footer**

In `public/index.html`, find:
```html
    <div class="footer-links">
      <a href="https://github.com/peterlodri-sec/entheai" target="_blank" rel="noopener">GitHub ↗</a>
      <a href="docs/">Docs</a>
      <a href="https://pocoo.vaked.dev" target="_blank" rel="noopener">Blog ↗</a>
      <span style="color:var(--text-faint)">License · TBD</span>
      <span style="color:var(--text-faint)">vaked-base</span>
    </div>
```
Replace it with:
```html
    <div class="footer-links">
      <a href="https://github.com/peterlodri-sec/entheai" target="_blank" rel="noopener">GitHub ↗</a>
      <a href="docs/">Docs</a>
      <a href="llms.txt">For LLMs</a>
      <a href="https://pocoo.vaked.dev" target="_blank" rel="noopener">Blog ↗</a>
      <span style="color:var(--text-faint)">License · TBD</span>
      <span style="color:var(--text-faint)">vaked-base</span>
    </div>
```

- [ ] **Step 2: Add the link to the docs shell**

In `public/docs/_template.html`, find:
```html
  <a class="gh-link" href="https://github.com/peterlodri-sec/entheai" target="_blank" rel="noopener">GitHub ↗</a>
</header>
```
Replace it with:
```html
  <a class="gh-link" href="https://github.com/peterlodri-sec/entheai" target="_blank" rel="noopener">GitHub ↗</a>
  <a class="gh-link" href="/llms.txt">For LLMs</a>
</header>
```

- [ ] **Step 3: Rebuild and commit**

```bash
cd /Users/peter.lodri/workspace/peterlodri-sec/entheai
unset -f node npm npx nvm 2>/dev/null
export PATH="$HOME/.nvm/versions/node/v24.17.0/bin:$PATH"
npm run build
git add public/index.html public/docs/_template.html
git commit -m "feat: add For LLMs footer link pointing at llms.txt"
```

---

### Task 8: Local browser verification

**Files:** none (verification only)

- [ ] **Step 1: Rebuild and serve locally**

```bash
cd /Users/peter.lodri/workspace/peterlodri-sec/entheai
unset -f node npm npx nvm 2>/dev/null
export PATH="$HOME/.nvm/versions/node/v24.17.0/bin:$PATH"
npm run build
cd public && python3 -m http.server 8936
```

- [ ] **Step 2: Verify with a browser tool**

Navigate to `http://localhost:8936/` and:
- View page source (or use a code-eval tool) and confirm all the OG/Twitter meta tags from Task 2 are present with the correct content.
- Confirm the JSON-LD `<script type="application/ld+json">` block is present and, re-parsed, matches Task 4's content.
- Confirm the footer now shows a "For LLMs" link between "Docs" and "Blog ↗", and that clicking it navigates to `/llms.txt`.

Navigate to `http://localhost:8936/docs/` and:
- View page source and confirm the docs-specific OG/Twitter meta tags from Task 3 are present.
- Confirm the topbar now shows a "For LLMs" link after "GitHub ↗", and that clicking it navigates to `/llms.txt`.

Navigate to `http://localhost:8936/robots.txt` and confirm it matches Task 5's content exactly.

Navigate to `http://localhost:8936/sitemap.xml` and confirm it renders as valid XML matching Task 6's content.

Navigate to `http://localhost:8936/og-image.png` and confirm the image loads and looks correct (same check as Task 1 Step 3).

- [ ] **Step 3: Stop the local server**

```bash
pkill -f "http.server 8936"
```

---

### Task 9: Deploy and verify live

**Files:** none (verification only)

- [ ] **Step 1: Push all commits**

```bash
cd /Users/peter.lodri/workspace/peterlodri-sec/entheai
git fetch origin main
git merge-base --is-ancestor HEAD~7 origin/main && echo "base still current, safe to push" || echo "origin moved, fetch and rebase/resolve before pushing"
git push origin main
```
(There have been repeated cases in this repo where `origin/main` moves between other concurrent sessions' commits — if the safety check above says "origin moved," fetch, inspect what changed, and reconcile before force-pushing or overwriting anything.)

- [ ] **Step 2: Watch the deploy workflow**

```bash
gh run list --repo peterlodri-sec/entheai --workflow=deploy.yml --limit 1
```
Get the run ID from the output, then:
```bash
gh run watch <RUN_ID> --repo peterlodri-sec/entheai --interval 5
```
Expected: all steps green.

- [ ] **Step 3: Verify live with a browser tool**

Repeat Task 8 Step 2's checks against `https://entheai.com/` and `https://entheai.com/docs/` instead of localhost. Additionally, fetch `https://entheai.com/og-image.png` and confirm it returns 200 with `content-type: image/png`.

---

### Task 10: Mint a zone-settings-scoped Cloudflare API token

**Files:** none (infrastructure only)

- [ ] **Step 1: Look up the exact permission group IDs needed**

Run (uses the already-authenticated `cloudflare-api` MCP tool, which has permission-group read access):
```
Query GET /user/tokens/permission_groups, filter for these exact names, and note their ids:
- "Zone Settings Write" (scope: zone)
- "Firewall Services Write" or "WAF Write" (for managed rulesets — check exact name, Cloudflare has renamed this a few times)
- "Zone WAF Write"
- "Page Rules Write" (may not be needed if not using legacy page rules)
- "Custom Pages Write"
```
Note the exact ids returned — do not guess, the plan intentionally does not hardcode these since permission group names/ids can differ by account state and were last confirmed on 2026-07-18 for a different set of permissions.

- [ ] **Step 2: Mint the token using the master user token**

The master user API token was provided directly in conversation (starts `cfut_`) — it has "API Tokens Edit" permission and was used once already in this session to mint the CI deploy token. Reuse the same pattern: a single Bash script that creates the token via `curl` (not the `cloudflare-api` MCP tool, to keep the raw values in one shell process) and never echoes the master token or the new token to stdout. Structure:

```bash
set -euo pipefail
MASTER_TOKEN="<paste the same master token used earlier in this session>"
CF_ZONE_ID="09a3145ea24ea5b2b821cc8283097f9b"

# Use the permission group IDs found in Step 1
ZONE_SETTINGS_WRITE="<id from step 1>"
WAF_WRITE="<id from step 1>"
CUSTOM_PAGES_WRITE="<id from step 1>"

TOKEN_JSON=$(curl -s -X POST "https://api.cloudflare.com/client/v4/user/tokens" \
  -H "Authorization: Bearer ${MASTER_TOKEN}" \
  -H "Content-Type: application/json" \
  --data @- <<JSON
{
  "name": "entheai-zone-config-$(date +%s 2>/dev/null || echo manual)",
  "policies": [
    {
      "effect": "allow",
      "resources": { "com.cloudflare.api.account.zone.${CF_ZONE_ID}": "*" },
      "permission_groups": [
        { "id": "${ZONE_SETTINGS_WRITE}" },
        { "id": "${WAF_WRITE}" },
        { "id": "${CUSTOM_PAGES_WRITE}" }
      ]
    }
  ]
}
JSON
)

SUCCESS=$(echo "$TOKEN_JSON" | jq -r '.success')
if [ "$SUCCESS" != "true" ]; then
  echo "$TOKEN_JSON" | jq -c '.errors'
  exit 1
fi

ZONE_TOKEN=$(echo "$TOKEN_JSON" | jq -r '.result.value')
# Keep $ZONE_TOKEN in this shell session's memory only — do not print it,
# do not write it to a file, do not set it as a GitHub secret (it's only
# needed for the one-off zone configuration in Task 11, not for CI).
```

Note: do NOT use a literal `$(date +%s)` inside the JSON heredoc if your shell doesn't expand it inside single-quoted-style heredocs — verify the `name` field renders as a real string before sending (a quick `echo` of just the request body, which contains no secrets, is fine and encouraged here to catch this).

- [ ] **Step 3: Verify the new token works**

```bash
curl -s -X GET "https://api.cloudflare.com/client/v4/zones/${CF_ZONE_ID}/settings/security_level" \
  -H "Authorization: Bearer $ZONE_TOKEN" -H "Content-Type: application/json"
```
Expected: `{"success":true, ...}` with the current security level value (not an authentication error).

Keep this shell session (with `$ZONE_TOKEN` in memory) open for Task 11 — do not start a fresh shell, since the token only exists in this process's memory.

---

### Task 11: Apply the Cloudflare Pro zone settings

**Files:** none (infrastructure only)

Using the `$ZONE_TOKEN` from Task 10, apply each setting. Run each as its own command so a failure in one doesn't block the others; check `"success":true` in each response.

- [ ] **Step 1: Security level → medium**

```bash
curl -s -X PATCH "https://api.cloudflare.com/client/v4/zones/${CF_ZONE_ID}/settings/security_level" \
  -H "Authorization: Bearer $ZONE_TOKEN" -H "Content-Type: application/json" \
  --data '{"value":"medium"}'
```

- [ ] **Step 2: Bot Fight Mode → on**

```bash
curl -s -X PATCH "https://api.cloudflare.com/client/v4/zones/${CF_ZONE_ID}/settings/bot_fight_mode" \
  -H "Authorization: Bearer $ZONE_TOKEN" -H "Content-Type: application/json" \
  --data '{"value":"on"}'
```

- [ ] **Step 3: Brotli → on**

```bash
curl -s -X PATCH "https://api.cloudflare.com/client/v4/zones/${CF_ZONE_ID}/settings/brotli" \
  -H "Authorization: Bearer $ZONE_TOKEN" -H "Content-Type: application/json" \
  --data '{"value":"on"}'
```

- [ ] **Step 4: HTTP/3 → on**

```bash
curl -s -X PATCH "https://api.cloudflare.com/client/v4/zones/${CF_ZONE_ID}/settings/http3" \
  -H "Authorization: Bearer $ZONE_TOKEN" -H "Content-Type: application/json" \
  --data '{"value":"on"}'
```

- [ ] **Step 5: Cache level → aggressive**

```bash
curl -s -X PATCH "https://api.cloudflare.com/client/v4/zones/${CF_ZONE_ID}/settings/cache_level" \
  -H "Authorization: Bearer $ZONE_TOKEN" -H "Content-Type: application/json" \
  --data '{"value":"aggressive"}'
```

- [ ] **Step 6: Browser cache TTL → 4 hours**

```bash
curl -s -X PATCH "https://api.cloudflare.com/client/v4/zones/${CF_ZONE_ID}/settings/browser_cache_ttl" \
  -H "Authorization: Bearer $ZONE_TOKEN" -H "Content-Type: application/json" \
  --data '{"value":14400}'
```

- [ ] **Step 7: WAF Managed Ruleset → enabled**

First check whether a Managed Ruleset is already deployed on the entry-point ruleset phase:
```bash
curl -s -X GET "https://api.cloudflare.com/client/v4/zones/${CF_ZONE_ID}/rulesets/phases/http_request_firewall_managed/entrypoint" \
  -H "Authorization: Bearer $ZONE_TOKEN" -H "Content-Type: application/json"
```
If the response shows `"success":false` with a "not found" style error, deploy Cloudflare's default Managed Ruleset:
```bash
curl -s -X PUT "https://api.cloudflare.com/client/v4/zones/${CF_ZONE_ID}/rulesets/phases/http_request_firewall_managed/entrypoint" \
  -H "Authorization: Bearer $ZONE_TOKEN" -H "Content-Type: application/json" \
  --data '{
    "rules": [
      {
        "action": "execute",
        "action_parameters": {
          "id": "efb7b8c949ac4650a09736fc376e9aee"
        },
        "expression": "true",
        "description": "Execute Cloudflare Managed Ruleset"
      }
    ]
  }'
```
If it already returns an existing ruleset with rules configured, leave it as-is (don't overwrite an existing custom configuration) and note this in your report.

- [ ] **Step 8: Web Analytics → enabled**

```bash
curl -s -X POST "https://api.cloudflare.com/client/v4/zones/${CF_ZONE_ID}/rum/site_info" \
  -H "Authorization: Bearer $ZONE_TOKEN" -H "Content-Type: application/json" \
  --data '{"host":"entheai.com","auto_install":true}'
```
If this endpoint or field shape doesn't match (`rum` site-info APIs have changed across Cloudflare API versions), note the exact error and skip rather than guessing at a different shape — this is a nice-to-have, not a hard requirement.

- [ ] **Step 9: Custom error page (500)**

Cloudflare's Custom Pages API requires the page content to be hosted at a URL Cloudflare can fetch, OR uploaded as a "single_page_app"/HTML string depending on API version. First check what the current custom pages endpoint expects:
```bash
curl -s -X GET "https://api.cloudflare.com/client/v4/zones/${CF_ZONE_ID}/custom_pages" \
  -H "Authorization: Bearer $ZONE_TOKEN" -H "Content-Type: application/json"
```
Read the response shape. If it lists available custom page slots (e.g., `500_errors`, `1000_errors`) with a `url` field expecting a hosted HTML page: use `https://entheai.com/404.html` (the existing branded 404 page, which already matches the site's dark/teal design) as the URL for the `500_errors` slot:
```bash
curl -s -X PUT "https://api.cloudflare.com/client/v4/zones/${CF_ZONE_ID}/custom_pages/500_errors" \
  -H "Authorization: Bearer $ZONE_TOKEN" -H "Content-Type: application/json" \
  --data '{"url":"https://entheai.com/404.html","state":"customized"}'
```
If the API shape differs from this, report the actual shape you found rather than guessing further — this is the lowest-priority item in this task and fine to leave for a follow-up if it doesn't fit this pattern cleanly.

- [ ] **Step 10: Verify each setting stuck**

```bash
for s in security_level bot_fight_mode brotli http3 cache_level browser_cache_ttl; do
  echo "=== $s ==="
  curl -s -X GET "https://api.cloudflare.com/client/v4/zones/${CF_ZONE_ID}/settings/$s" \
    -H "Authorization: Bearer $ZONE_TOKEN" -H "Content-Type: application/json" | jq -c '.result.value'
done
```
Expected: `"medium"`, `"on"`, `"on"`, `"on"`, `"aggressive"`, `14400` respectively.

- [ ] **Step 11: Discard the zone token**

```bash
unset ZONE_TOKEN MASTER_TOKEN
```
This token was only needed for this one-off configuration pass — no GitHub secret update needed, nothing to commit (this task made no repo changes).

---

### Task 12: Final live verification

**Files:** none (verification only)

- [ ] **Step 1: Confirm site still loads correctly end-to-end**

Using a browser tool, navigate to `https://entheai.com/` and `https://entheai.com/docs/`. Confirm both still render and function identically to before (nav, search, theme toggle, copy buttons, etc. — the zone-level settings changes in Task 11 should be transparent to the site's functionality; if anything looks different or broken, that's a regression to investigate, most likely from the WAF managed ruleset being too aggressive for this site's own JS/assets — check Cloudflare's Security Events log for the zone if something seems blocked).

- [ ] **Step 2: Spot-check response headers reflect the new settings**

```bash
curl -sI https://entheai.com/ 2>&1 | grep -i "content-encoding\|alt-svc"
```
Expected: `content-encoding: br` (Brotli) and an `alt-svc` header advertising HTTP/3 (`h3=...`).
