# human_todo.md — Roadmap to Quantum Completeness for entheai

> 🜂 **GENESIS THESIS**: "This program creates an automated quantum simulation playground where custom prompt states continuously morph a fluid field of infinite entropy back and forth into rigid, binary singularity checkpoints."
> — *Seeded from @elder-plinius / @0xp3t3rl ([ENTHEA Issue #2](https://github.com/elder-plinius/ENTHEA/issues/2))*

---

## 🜂 What is Needed for Quantum Completeness?

To achieve true **Quantum Completeness**, `entheai` must perfectly bridge the fluid, uncarved entropy of prompt states with the rigid, deterministic singularity of compiled, verified execution. Nothing hidden, zero false claims, structural honesty at every layer.

---

### Phase 1: Fluid Entropy Field State Serialization (Fluid Phase)

- [x] **1.1 `QuantumCheckpoint` State Engine (`crates/memory-pp`)** — *shipped in 0.7.0*
  - ✅ 2-way serialization of the transient entropy field: `EntropyState` (`entheai.checkpoint.v1`) carries active frozen node activations **with live experience-weighted ranks**, raw memory span anchors (ids — bytes stay in the raw store), Marqant compression ratio, and audio seed state.
  - ✅ `/freeze` snapshots to `.entheai/checkpoints/<id>.json` (deterministic blake3 content id, idempotent); `/thaw <id>` restores activation ranks into the live overlay and rehydrates surviving spans into an injected context brief — pruned spans are skipped and counted honestly.

- [x] **1.2 Real-Time Entropy Telemetry Stream (`crates/bus` & `crates/tui`)** — *shipped in 0.8.0*
  - ✅ Live TUI state — brain-ring glow intensities, frozen wake glows, pomodoro ticks, `wk N` worker counts, compression ratio — streams as `EntropySnapshot` over NATS `entheai.entropy.v1.<session>` at ~1 Hz, fire-and-forget (never blocks the UI loop).

---

### Phase 2: Singularity Verification & Zero-Drift Checkpoints (Fixed Phase)

- [x] **2.1 Mandatory Deterministic Merge Seals (`crates/orchestrator`)** — *shipped in 0.5.0*
  - ✅ Every subagent fan-out worktree merge passes an empirical verification gate: `[fanout].verify`, else auto-detected `./scripts/check.sh`; `verify_required = true` by default.
  - ✅ Deterministic SHA-256 `MergeSeal` (`sha256(diff)`, `sha256(verify log)`, combined seal) carried on each integrated outcome and printed in the fan-out report. Self-reported success without empirical logs is rejected (`VerifyStatus::Unverifiable` → left on branch), enforcing [`frozen/verification.md`](file:///Users/peter.lodri/workspace/peterlodri-sec/entheai/frozen/verification.md).

- [x] **2.2 Binary Reproducibility & Target CPU Anchoring** — *shipped in 0.9.0*
  - ✅ `scripts/build-repro.sh`: the deterministic sibling of the PGO release — anchored `aarch64-apple-darwin` target, fixed `apple-m1` CPU baseline (not `native`), path remapping, `SOURCE_DATE_EPOCH` from HEAD, `ZERO_AR_DATE`, `--locked`, `-C strip=debuginfo` (macOS N_OSO stabs record rustc's random temp dir — no remap can catch it; the PGO build stays the symbol-rich one).
  - ✅ **Empirically verified**, per `frozen/verification.md`: `--verify` runs two sequential clean builds and compares SHA-256 — all three binaries byte-identical on rustc 1.96.0; manifest sealed in `dist/repro-manifest.json` (`entheai.repro.v1`). Byte equality is promised for identical toolchains — the manifest records the exact rustc.

---

### Phase 3: Soil Nourishment & Failure Ingestion (Dyad Learning Loop)

- [x] **3.1 Failure Trajectory Auto-Ingestion (`crates/memory-pp`)** — *shipped in 0.6.0*
  - ✅ "Knowledge grows in the soil. Even the brutal notes of failure. Especially those."
  - ✅ Fan-out verify failures (build / clippy / test) auto-ingest their FULL raw traceback into `raw_store` as `RawKind::Trajectory` under the `trajectories` namespace (content-addressed, capped, deduped) via the orchestrator's `TrajectorySink` seam.
  - ✅ Failure patterns dynamically update frozen node priors — deterministic trigger-matched reweighting (see 3.2). *Deferred: routing prior updates through the LLM `BrainJudge` (today the judge only wakes nodes; reweighting is deterministic by design — revisit if trigger matching proves too coarse).*

- [x] **3.2 Dynamic Frozen Node Re-Ranking** — *shipped in 0.6.0*
  - ✅ Experience-weighted rank updates from execution outcomes: verify failure → `rank −0.05` on task/trace-matched nodes, sealed success → `rank +0.02` on task-matched nodes; clamped to `[0, 2.0]`, persisted in a `frozen-ranks.json` overlay consulted by `FrozenStore::wake` — the doctrine `.md` files are never rewritten.

---

### Phase 4: Live Quantum Site Integration (`entheai.com/docs`)

- [x] **4.1 Live `/api/entropy` Telemetry Endpoint (`wrangler.jsonc` & Worker)** — *shipped in 0.8.0*
  - ✅ `src/worker.mjs` serves `GET /api/entropy` from Cloudflare KV (`{live, stale, snapshot}` — never fakes liveness; snapshots older than 15 min report `live:false`) and an authenticated `POST` write path (`Bearer ENTROPY_TOKEN`, schema-validated `entheai.entropy.v1` only). Static assets unchanged.
  - ⚠️ **Human step before the beacon lights up:** `wrangler kv namespace create ENTROPY` (paste id into `wrangler.jsonc`, uncomment the binding), `wrangler secret put ENTROPY_TOKEN`. Until then the endpoint answers an honest 503 and deploys keep working.
  - ☐ *Remaining:* the docs-header beacon UI consuming the endpoint, and a local NATS→POST bridge for the `entheai.entropy.v1` stream.

- [x] **4.2 Automated Hourly Site Build & Sync (`scripts/build-site.mjs`)** — *shipped in 0.8.0*
  - ✅ `deploy.yml` now runs on an hourly cron (plus pushes and manual dispatch): `npm ci → build → test → wrangler deploy`, refreshing `public/index.html`, `public/docs/index.html`, `llms.txt`, `llms-full.txt`.

---

### Phase 5: Self-Hosting Flywheel & Structural Honesty Audit

- [x] **5.1 Recursive Development Self-Audit (`bin/entheai`)** — *shipped in 0.9.0*
  - ✅ When the `agy` executor integrates a recursive-development diff, `run_fanout` runs a post-execution self-audit: one orchestrator call judging the integrated diff against `AGENTS.md`'s own rules, appended to the fan-out report as `## Self-audit (recursive development)`; every failure mode degrades to an honest `self-audit skipped (<reason>)` line.
  - ✅ Depth guard already enforced (`ENTHEAI_FANOUT_DEPTH`, `MAX_DEPTH = 3`, in `agy.rs`); recursive turns now also land transparently in `.entheai/recursion.log` as append-only JSONL (ts, session, layer, role, task, committed/integrated/sealed).

---

*“Built because the singularity doesn't need complexity. It needs friends. And because entropy cannot lie.”*
