# entheai "Second Brain" — Design

**Date:** 2026-07-19 · **Status:** approved design, pre-plan
**Scope:** the **server** (public ingestion + curation + collective-serving API). The entheai-side client (dogfeed exporter + sync puller) is adjacent and gets its **own** spec.

## 1. Purpose & role

A self-hosted, public API — the **aggregation layer of the dogfeed self-improvement flywheel** (design §5.18) — running on the project's bare-metal box (`dev-cx53`). It is **bidirectional**: every entheai instance POSTs contributions, and the server serves back a *curated collective* of learnings that syncs into each instance's local memory. It replaces the original "each instance pushes straight to Hugging Face" model with a central, curatable hub.

**Why a hub (vs. direct-to-HF):** aggregation, dedup, PII-scrubbing as defense-in-depth, a corroboration-based trust gate (essential because contributions come from *both* the owner's fleet and untrusted third parties), and a place to serve knowledge back.

**Not telemetry.** It collects *task learnings and trajectories* (what the agent did and whether it worked), never usage metrics or PII.

## 2. Two data streams (both from day one)

| Stream | Weight | Fate |
|---|---|---|
| **Learnings** | light — post-task `extract_learnings` items ("`cargo test` after edits caught the regression") | curated → **synced back** into instances' local `learnings` memory |
| **Trajectories** | heavy — full §5.18 record (prompt, plan, tool_calls, diff, model, build/test outcome, score, session_id, loop_index) | **training fuel**; batch-mirrored to `PeetPedro/ultrawhale-dogfood` (HF). Not synced back |

## 3. Trust & curation (the safety spine)

Because third parties can contribute over a public (embedded) token, **individual contributions are never trusted**. Promotion to the synced collective is corroboration-gated.

- Every POST carries an **anonymous `contributor_id`** — a random UUID generated once per install, stored client-side. It is **not PII**; it exists only to count *distinct* contributors and track reputation. Plus a per-task `session_id`.
- **Clustering (pgvector):** an incoming learning is PII-scrubbed → embedded → matched against existing clusters by cosine similarity (`τ_match ≈ 0.85`). A match from a **new** `contributor_id` **corroborates** that cluster; no match starts a fresh cluster in the **raw pool**.
- **Promotion:** a cluster promotes to the synced collective when its **distinct-corroborator count ≥ `N_promote`** (default 3; lower for high-reputation contributors) *or* a weighted confidence floor is cleared. A monotonic **promotion sequence** is assigned (the `sync` cursor).
- **Reputation weighting:** `contributor_id`s on a configured **fleet allowlist** start high-trust (promote faster / count more). Anonymous ones start neutral. A contributor whose promoted learnings later get contradicted loses reputation; their raw contributions still accumulate but corroborate more slowly.
- **Decay / contradiction (v1, simple):** clusters decay by age without fresh corroboration; an explicit "this failed" signal (a learning with `outcome=failed` matching a promoted cluster) flags it `contradicted` and demotes it. Richer NLU contradiction detection is v2.

Net: a single or spam/poison contributor **can never promote a cluster alone** — it stays quarantined in the raw pool and never reaches anyone's local memory.

## 4. Read / sync loop

`GET /v1/sync?since=<cursor>&limit=N` returns learnings promoted since the cursor. The instance folds them into its local `learnings` namespace; the existing `run_task_with_memory` retrieval then surfaces collective knowledge transparently — **zero hot-path network dependency**. A live mid-task query endpoint is a v2 option, not v1.

## 5. Stack & data model

One Rust **axum** binary + **PostgreSQL 16 + `pgvector`**. (jcode-style graph recall is the v2 upgrade — [[jcode-harness-reference]].)

Tables:
- `contributions_raw(id, contributor_id, session_id, kind, embedding vector, payload jsonb, scrubbed bool, created_at)`
- `learning_clusters(id, canonical_text, tags text[], embedding vector, distinct_contributors int, confidence real, status enum{pending,promoted,contradicted}, promotion_seq bigserial null, created_at, updated_at)`
- `cluster_members(cluster_id, contribution_id, contributor_id)` — unique `(cluster_id, contributor_id)` enforces *distinct*-contributor counting
- `trajectories(id, contributor_id, session_id, payload jsonb, exported_to_hf bool, created_at)`
- `contributors(contributor_id pk, reputation real, is_fleet bool, promoted_count int, contradicted_count int, first_seen, last_seen)`

`pgvector` HNSW index on cluster embeddings powers both corroboration matching and (future) serving.

## 6. API surface (v1)

All endpoints require `Authorization: Bearer <token>` except `/health`.
- `POST /v1/learnings` — `{contributor_id, session_id, learnings:[{text, tags?, confidence?, tool?, outcome?}]}` → `202 Accepted` (queued for curation).
- `POST /v1/trajectories` — `{contributor_id, session_id, trajectories:[<§5.18 record>...]}` → `202`.
- `GET /v1/sync?since=<seq>&limit=N` — `{cursor, learnings:[{id, text, tags, confidence, corroborations}]}` (promoted only).
- `GET /v1/stats` — coarse counts (raw, promoted, contributors, trajectories). No PII.
- `GET /health` — liveness.

## 7. Auth · privacy · abuse

- **Auth:** a single shared **bearer token** (public/embedded). Read + write share it in v1.
- **PII scrub (server-side, defense-in-depth):** regex redaction of email / phone / IPv4-6 / common API-key shapes / absolute home paths applied to *all* text before storage; a contribution containing a hard secret pattern (API key) is **rejected**, not stored. The client also scrubs (§5.18) — the server never trusts that.
- **Abuse:** per-token and per-`contributor_id`/IP **rate limits**; max payload size; max items per POST. The corroboration gate is the primary poison defense; rate limits handle spam/DoS. Append-only audit of promotions.

## 8. HF mirror

A periodic job batches new `trajectories` → maps to the `ultrawhale-dogfood` schema → pushes via HF `create_commit` (add op), sets `exported_to_hf`, refreshes `stats.json`. The **brain is now the sole pusher** (gated by `HF_TOKEN` on the server), replacing per-instance direct push. Budget caps (commits/day).

## 9. Deployment

Single static Rust binary + Postgres(+pgvector) on `dev-cx53` (Linux). Env: `DATABASE_URL`, `BRAIN_BEARER_TOKEN`, `HF_TOKEN`, `HF_REPO`, `FLEET_CONTRIBUTOR_IDS`. TLS + public exposure via a reverse proxy (Caddy) or a Cloudflare tunnel. systemd unit or container.

**Repository:** the server is a **separate repo/workspace** (`entheai-brain`), Linux-native — deliberately *not* inside the macOS-only entheai workspace (whose `.cargo/config` carries macOS-only linker flags). entheai and the brain share only a **versioned JSON wire contract** (the learning + trajectory + sync schemas), not code — keeping both build configs clean and the interface explicit.

## 10. Testing

- Unit: curation algorithm — corroboration promotion (`N_promote`), distinct-contributor enforcement, reputation weighting, dedup matching threshold, decay/contradiction, PII scrub/reject.
- Property: **a single `contributor_id` can never promote a cluster** (invariant `N_promote > 1` unless fleet-trusted above a separate floor).
- Integration: endpoints against an ephemeral Postgres (testcontainers or a CI service) — POST → curate → `GET /sync` round-trip; auth rejection; rate-limit rejection.

## 11. Scope, non-goals, open questions

**In scope:** the server (API + curation + storage + HF mirror + deploy).
**Adjacent (own spec):** the entheai **client** — the `dogfeed` exporter that POSTs off the hot path, and the `sync` puller that folds promoted learnings into local memory.
**Non-goals (v1):** jcode-style graph recall; live mid-task query; NLU contradiction detection; per-contributor secret tokens; a web UI beyond `/stats`.
**Open questions:** exact `N_promote` / `τ_match` / decay constants (tune on real data); embedding model for server-side clustering (reuse entheai's Osaurus embedder contract vs. a server-local model); whether `/sync` should page by promotion-seq or timestamp; fleet-allowlist distribution mechanism.

## 12. Success criteria

Instances POST learnings + trajectories over the bearer token; a learning corroborated by ≥`N_promote` distinct contributors promotes and appears in `GET /sync`; a single/poison contributor's learning never promotes; PII is scrubbed or rejected; trajectories mirror to HF; the server runs on `dev-cx53` behind the public token, and an entheai instance's local `learnings` memory visibly grows from the collective.
