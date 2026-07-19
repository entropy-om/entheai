# SEO + OG image + AI-SEO + Cloudflare Pro utilization — design

Status: approved
Date: 2026-07-19

## Context

entheai.com currently ships a landing page and a markdown-driven docs site
(both deployed via GitHub Actions to Cloudflare Workers, see
`docs/superpowers/specs/2026-07-18-llm-first-docs-and-ci-deploy-design.md`)
plus `llms.txt`/`llms-full.txt` for LLM/agent consumption. It has no social
share metadata, no structured data, no `robots.txt`/`sitemap.xml`, and the
Cloudflare zone (on the Pro plan) has its default settings untouched.

This spec covers making the site properly discoverable and shareable —
"SEO", an OG image, explicit AI-crawler welcome, and turning on the
Cloudflare Pro features that make sense for a small static site.

## Goals

1. **OG image + social meta tags**: a real 1200×630 PNG (Concept A: hero
   headline + teal/magenta glow + badge, matching the landing page hero
   exactly) plus standard OpenGraph/Twitter Card meta tags on both the
   landing page and the docs shell.
2. **Structured data**: a `SoftwareApplication` JSON-LD block on the landing
   page.
3. **AI-crawler allowlist + sitemap**: `public/robots.txt` explicitly
   welcoming known AI crawler user-agents, plus `public/sitemap.xml` listing
   the two real routes.
4. **llms.txt discoverability**: a small "For LLMs" footer link on both the
   landing page and docs shell, pointing at `/llms.txt`. No changes to the
   generation logic itself (already reviewed/working).
5. **Cloudflare Pro zone settings**: security level, WAF managed ruleset,
   Bot Fight Mode, Brotli, HTTP/3, cache level, browser cache TTL, custom
   error pages, Web Analytics — per the table below.

## Non-goals

- No per-doc-page dynamic OG images (a single static image for the whole
  site is sufficient at this stage).
- No changes to `llms.txt`/`llms-full.txt` generation logic (Task 4 of the
  prior spec) — this spec only adds a footer link pointing at the existing
  output.
- No Cloudflare Argo Smart Routing (a separate paid add-on, not part of Pro,
  not requested).
- No changes to DNS records beyond what's already there (the `vaked-base`
  TXT record from the prior session stays as-is).

## OG image + social meta tags

### Image generation

A one-off Node script (`scripts/generate-og-image.mjs`) using Playwright:
renders a fixed-size (1200×630) HTML fragment matching the approved Concept
A design (dark abyss gradient background, "macOS · Apple Silicon · Rust"
badge, the exact hero headline with the teal→cyan→magenta gradient span on
"fans out.", `entheai.com` wordmark bottom-left) and screenshots it to
`public/og-image.png`. This is a one-time build artifact (committed to git,
not gitignored, since it doesn't change unless the design changes) — not
part of the `npm run build` pipeline, since it has no dependency on the
docs content and would be wasteful to regenerate on every build.

### Meta tags

Added to `public/index.html` `<head>`:
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

Added to `public/docs/_template.html` `<head>` (docs-specific title/description,
same image/canonical pattern with `https://entheai.com/docs/` as the URL):
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

## Structured data

Added to `public/index.html`, a `<script type="application/ld+json">` block:
```json
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
```

## AI-crawler allowlist + sitemap

`public/robots.txt`:
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

`public/sitemap.xml`:
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

## llms.txt discoverability

A small addition to the footer-links row in both `public/index.html` and
`public/docs/_template.html`: `<a href="/llms.txt">For LLMs</a>`, styled
identically to the existing footer/topbar links — no new CSS classes
needed.

## Cloudflare Pro zone settings

Executed directly via the Cloudflare API (a properly zone-settings-scoped
token minted from the master user token already provided this session —
the existing `cloudflare-api` MCP credential lacks zone-settings write
permission, same boundary issue encountered with the CI deploy token).

| Setting | API field | Value |
|---|---|---|
| Security level | `security_level` | `medium` |
| WAF | Managed Ruleset (Rulesets API, phase `http_request_firewall_managed`) | Enabled, default ruleset |
| Bot Fight Mode | `bot_fight_mode` | `on` |
| Brotli | `brotli` | `on` |
| HTTP/3 | `http3` | `on` |
| Cache level | `cache_level` | `aggressive` |
| Browser cache TTL | `browser_cache_ttl` | `14400` (4 hours) |
| Custom error pages | Custom Pages API, `500`/`1000` error types | Branded page matching the site's dark/teal design (reuses `public/404.html`'s visual style) |
| Web Analytics | Web Analytics API (`rum` site tag) | Enabled for `entheai.com` |

Each setting is applied via its own idempotent API call — safe to re-run,
no destructive changes to existing zone configuration (DNS records, SSL
settings, and the existing custom domain routes are untouched).

## Validation

- After generating `public/og-image.png`, view it directly to confirm it
  renders correctly at 1200×630 before committing.
- After deploy, verify OG tags with a real browser (view source / Playwright)
  and confirm `og:image` actually loads.
- Verify `robots.txt` and `sitemap.xml` are reachable and well-formed
  (valid XML, correct content-type) after deploy.
- Verify each Cloudflare Pro setting via a `GET` of the same endpoint after
  the `PATCH`/`PUT`, confirming the value stuck.

## Deferred / future work

- Dynamic per-page OG images (would need a Worker route generating images
  on the fly — out of scope for a site this size).
- Cloudflare Argo Smart Routing (separate paid add-on).
