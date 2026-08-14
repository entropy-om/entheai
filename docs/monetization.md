# entheai monetization + launch plan

Status: in progress — payment rail switched to Stripe direct (Gumroad rejected
Hungary payout details; see §3). Backer tier (one-time purchase) gating
bleeding-edge beta access, idea submission/roadmap influence, and issue
priority. **No repo write access is ever sold.**

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

- **Price:** `€9.99` one-time. Priced in EUR to settle cleanly to a Hungarian
  IBAN (HUF is zero-decimal for payouts; EUR/HUF avoids Stripe FX conversion
  fees). Optionally a `$19.99 "Supporter"` tier later.
- **Currency note:** customers are charged in EUR; the seller's Stripe account
  settles in HUF to a Hungarian bank (IBAN). No Gumroad-style `$100` minimum —
  Stripe's minimum is ~one base unit and the seller chooses the schedule.

## 3. Distribution + payment — **Stripe direct**

**Gumroad was dropped.** Its payout partner (Stripe) rejected the Hungary
payout details: "LLC" is not a valid Hungarian entity type, the bank-account
name mismatched the (bogus) business name, and DOB year was missing. Hungary is
a mandatory bank-payout country (no PayPal escape hatch) and publishing is
blocked until payout validates. The seller already has a Stripe account, so the
plan switched to Stripe direct.

- **Checkout:** Stripe Payment Link (hosted checkout) for a one-time product,
  price `999` in `eur` (minor units). Button on `public/back.html` links to it.
- **License issuance:** Stripe has no license-key primitive. A webhook on
  `checkout.session.completed` (only when `payment_status == "paid"`) generates
  a key and stores it in KV. This is the source of truth.
- **No Stripe API keys in the Worker.** The `whsec_…` webhook signing secret is
  the only trust anchor; the Worker never calls the Stripe API. (Minimal
  attack surface.)
- **Seller setup (Stripe dashboard):** create a one-time Product/Price in EUR
  (`€9.99`), create a Payment Link (copy URL into `public/back.html`), create a
  Webhook endpoint `https://entheai.com/api/stripe/webhook` listening for
  `checkout.session.completed`, copy its `whsec_` secret, then
  `wrangler secret put STRIPE_WEBHOOK_SECRET` and
  `wrangler kv namespace create LICENSES` (paste id into `wrangler.jsonc`).
- Stable public releases stay on GitHub Releases + Homebrew cask as today.

## 4. License verification architecture

Reuse the existing Cloudflare Worker (`src/worker.mjs`). Flow:

```
buyer → Stripe Payment Link → pays → checkout.session.completed
        webhook → Worker → verify whsec_ signature → generate ENTH- key → KV
        buyer lands on /back/claim?session_id=… → GET /api/license/claim → key
entheai CLI
  entheai activate <key> ────▶ POST /api/license/verify ──▶ KV lookup
       │  (stores key hash)        ◀────────────── {ok, entitlements, backer}
       └── ~/.config/entheai/backer.json (sha256 hash, NOT raw key)
```

- **License key format:** `ENTH-` + 16 chars from `ABCDEFGHJKMNPQRSTUVWXYZ23456789`
  (no 0/O/1/I/L), 4 groups of 4: `ENTH-XXXX-XXXX-XXXX-XXXX`.
- **KV (binding `LICENSES`):** `license:<key>` → `{session_id, email, product,
  created_at, entitlements:["beta"]}` (no TTL — lifetime); `session:<id>` → key
  (idempotency + claim lookup).
- **Endpoints:** `POST /api/stripe/webhook` (signature-verified), `POST
  /api/license/verify` (`{ok, entitlements}` or 401), `GET /api/license/claim?
  session_id=…` (returns the key), `GET /api/releases?channel=beta` (manifest
  seam, reads KV `releases:beta`).
- **`entheai activate <key>`** (CLI): verifies, stores `backer.json` holding the
  key's sha256 hash (never the raw key). Offline-fail-open: a valid local
  credential is trusted without re-check.
- **Anti-abuse:** webhook is signature-gated; `verify` returns only booleans +
  entitlements, never raw key material it didn't issue.

## 5. Beta channel

GitHub pre-releases on a *public* repo are public, so gating must be in-app:

- Single binary for both tiers. `--beta` is unlocked by an active backer
  credential; otherwise it falls back to a clear
  `become a backer → entheai.com/back` message.
- **Release manifest:** `GET /api/releases?channel=beta` returns the current
  beta version + download URL, so `entheai update --beta` can self-update
  backers to bleeding-edge builds. (`// # Stream N lands here:` marks where
  beta-gated behavior attaches.)
- Public stable channel unchanged: `vX.Y.Z` tags → GitHub Release + cask.
  Beta channel uses `vX.Y.Z-beta.N` tags, surfaced only via the gated manifest.

## 6. Idea submission + roadmap voting

Keep it low-infra for launch:

- **GitHub Discussions** for feature requests + roadmap (no write access
  needed). Add a `backer` label; backers' issues/PRs get triage priority.
- **Site roadmap page** (`public/roadmap.html`) reading a small JSON/KV list of
  open ideas; a `/ideas` submission form posts to the Worker (reuse the
  authenticated-write pattern) so backers can file ideas from the site.

## 7. Governance + security guardrails (non-negotiable)

- No write access is sold. `entropy-om/entheai` collaborator rights remain
  invite + PR-review gated. Documented on the checkout page.
- License keys never logged; CLI stores a hash, not the raw key.
- Webhook is signature-verified (`Stripe-Signature` HMAC-SHA256, 5-min replay
  window); no Stripe API keys live in the Worker at all.
- MIT license untouched. The `back` page states the entitlement-vs-governance
  line in plain language.
- Secrets (`STRIPE_WEBHOOK_SECRET`) live in Cloudflare (`wrangler secret put`),
  never in-repo.

## 8. Build phases

| Phase | Work | Status |
|---|---|---|
| 0 — Storefront | `public/back.html` + `public/back/claim.html` + cover/thumbnail art | in progress |
| 1 — Verify | `/api/stripe/webhook` + `/api/license/verify` + `/api/license/claim` + `entheai activate` + local cred | in progress |
| 2 — Beta | `--beta` flag + `/api/releases?channel=beta` manifest + `update --beta` | pending |
| 3 — Roadmap | GitHub Discussions + `backer` label + roadmap page | pending |
| 4 — Launch | Payment Link live, `whsec_` secret + KV provisioned, cask pin, announce | pending |

## 9. Open checkpoints

- [ ] Seller: create Stripe Product/Price in EUR (€9.99) + Payment Link → paste URL into `public/back.html`.
- [ ] Seller: create Stripe Webhook endpoint → copy `whsec_` → `wrangler secret put STRIPE_WEBHOOK_SECRET`.
- [ ] Seller: `wrangler kv namespace create LICENSES` → paste id into `wrangler.jsonc`.
- [ ] Confirm `release` workflow is live (VERSIONING.md says "paused"; `release.yml` has no `if: false`).
- [ ] Test end-to-end in Stripe test mode (card `4242…`, `stripe trigger checkout.session.completed`) before live.
- [ ] Confirm Hungary payout settlement currency (HUF) + add EUR bank account or accept FX, if needed.

## 10. Success metrics

- Backer conversion on the `/back` page; activation success rate; % of backers
  on the beta channel; backer issue-response latency vs non-backer.
