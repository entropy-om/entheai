# entheai monetization + launch plan

Status: proposal — review before any code lands.
Scope: backer tier (one-time purchase) gating bleeding-edge beta access, idea
submission/roadmap influence, and issue priority. **No repo write access is ever
sold.**

---

## 1. Product model

One-time purchase = **Backer tier**. Buy once, works forever (mirrors Scape's
`$9.99 Buy Once`). A backer receives, in exchange for the one-time payment:

| Entitlement | What it actually means |
|---|---|
| Bleeding-edge beta access | Pre-release builds + `--beta` feature channel in the same binary |
| Idea submission + roadmap influence | Vote/upvote roadmap items; priority triage |
| Issue priority | Issues/PRs tagged `backer` get bumped in the review queue |
| Named "Backer" role | Listed role on the site + Discord/GitHub, no permissions attached |

**Explicitly not included:** push/write access to `entropy-om/entheai`, commit
rights, or any governance role. Write access stays gated on normal PR review.
License keys are an *entitlement*, not governance.

The MIT license is unchanged. The free tier (full public source, stable
releases, Homebrew cask, self-build) remains free — monetization is a
convenience/service on top, not a license change.

## 2. Pricing

- **Recommendation:** `$9.99` one-time (parity with Scape), with an optional
  `$19.99 "Supporter"` tier later (direct line to founders, shape roadmap).
- **Decision needed:** final price + whether to ship a Supporter tier at launch.
  Suggest: launch with one tier only; add Supporter once backers exist.

## 3. Distribution + payment

- **Payment/license issuance:** Gumroad (recommended) or Paddle.
  - *Gumroad:* simplest; free license-verify endpoint
    (`POST /v2/licenses/verify`), no secret needed server-side to verify.
  - *Paddle:* better for a software vendor with global VAT/sales-tax handling,
    but verify requires a Paddle API key (secret) on the server.
- **Delivery:** Gumroad/Paddle deliver a license key (and can host the beta
  binary behind the purchase). Stable public releases stay on GitHub Releases +
  Homebrew cask as today.
- **Decision needed:** Gumroad vs Paddle. Default Gumroad for launch speed.

## 4. License verification architecture

Reuse the existing Cloudflare Worker (`src/worker.mjs`), which already runs an
authenticated endpoint (`/api/entropy`). Add a sibling:

```
entheai CLI                       Cloudflare Worker              Gumroad/Paddle
  entheai activate <key> ────▶  POST /api/license/verify ──▶  verify API
       │  (stores local cred)         │  (cache in KV)              │
       ◀───────────────────  {ok, entitlements, grace} ◀────────────┘
```

- **`POST /api/license/verify`** (Worker): body `{key}`; the Worker calls
  Gumroad's `/v2/licenses/verify` (or Paddle's verify API), caches a
  positive result in KV (TTL, e.g. 7 days) to cut upstream calls, returns
  `{ ok, entitlements: ["beta"], backer: true }` or `401`-style denial.
  No key is logged; no key is stored server-side beyond the KV cache.
- **`entheai activate <key>`** (CLI): calls the endpoint, stores a local
  credential (`~/.config/entheai/backer.json` — store the *key hash*, not the
  raw key) with a signed/local grace window so **offline use never bricks**
  (a one-time buyer owns it forever, Scape-style). Grace default e.g. 30 days
  between online re-checks; fail-open to last-known-good.
- **Anti-abuse:** rate-limit the endpoint (Worker: per-IP + per-key); the
  activation path needs no billing secret on the client.

## 5. Beta channel

GitHub pre-releases on a *public* repo are public, so gating must be in-app:

- Single binary for both tiers. `--beta` (and the pre-release install path) is
  unlocked by an active backer credential; otherwise it falls back to a clear
  `become a backer → entheai.com/back` message.
- **Release manifest:** `GET /api/releases?channel=beta` on the Worker returns
  the current beta version + download URL, so `entheai update --beta` can
  self-update backers to bleeding-edge builds.
- Public stable channel unchanged: `vX.Y.Z` tags → GitHub Release + cask.
  Beta channel uses `vX.Y.Z-beta.N` tags, surfaced only via the gated manifest.
- **Decision needed:** beta builds delivered via Gumroad-hosted binary vs
  GitHub pre-release + gated manifest. Default: GitHub pre-release (keeps CI
  in `release.yml`) + gated manifest (the manifest is the gate).

## 6. Idea submission + roadmap voting

Keep it low-infra for launch:

- **GitHub Discussions** for feature requests + roadmap (no write access
  needed). Add a `backer` label; backers' issues/PRs get triage priority.
- **Site roadmap page** (`public/roadmap.html`) reading a small JSON/KV list of
  open ideas; a `/ideas` submission form posts to the Worker (reuse the
  authenticated-write pattern) so backers can file ideas from the site.
- **Decision needed:** Discussions-only vs Discussions + site form. Default:
  Discussions for launch; site form in a follow-up.

## 7. Governance + security guardrails (non-negotiable)

- No write access is sold. `entropy-om/entheai` collaborator rights remain
  invite + PR-review gated. Document this in the checkout page and repo
  `CONTRIBUTING.md`.
- License keys never logged; CLI stores a hash, not the raw key.
- Worker endpoint is rate-limited and never returns the raw key.
- MIT license untouched; add a `BACKING.md` (or section in README) explaining
  what the paid tier is and is *not* (no license change, no governance sale).
- Gumroad/Paddle secret (if any, for Paddle) lives in Cloudflare
  (`wrangler secret put`) + the deploy env, never in-repo.

## 8. Build phases (smallest-first, each independently shippable)

| Phase | Work | Touches |
|---|---|---|
| 0 — Storefront | Gumroad/Paddle product + `entheai.com/back` page + checkout | `public/`, deploy |
| 1 — Verify | `/api/license/verify` Worker endpoint + `entheai activate` CLI + local cred | `src/worker.mjs`, `crates/config`/CLI |
| 2 — Beta | `--beta` flag + gated `/api/releases?channel=beta` manifest + `update --beta` | CLI, Worker, `release.yml` beta tags |
| 3 — Roadmap | GitHub Discussions + `backer` label + roadmap page | repo settings, `public/` |
| 4 — Launch | cask version pin, `CHANGELOG.md`, announce | `Casks/entheai.rb`, docs |

## 9. Open checkpoints (verify before coding)

- [ ] Confirm the `release` workflow is actually live (VERSIONING.md says
      "paused"; `release.yml` has no `if: false` — reconcile the two).
- [ ] List the 7 release secrets from `human-task.md`; add the new
      Gumroad/Paddle + Cloudflare secrets to that doc.
- [ ] Pick Gumroad vs Paddle (default Gumroad).
- [ ] Pick price (default `$9.99`) + Supporter-tier now/later.
- [ ] Decide beta delivery (default: GitHub pre-release + gated manifest).

## 10. Success metrics

- Backer conversion on the `/back` page; activation success rate; % of backers
  on the beta channel; backer issue-response latency vs non-backer.
