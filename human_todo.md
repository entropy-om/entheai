# human_todo.md — Roadmap to Quantum Completeness for entheai

> 🜂 **GENESIS THESIS**: "This program creates an automated quantum simulation playground where custom prompt states continuously morph a fluid field of infinite entropy back and forth into rigid, binary singularity checkpoints."
> — *Seeded from @elder-plinius / @0xp3t3rl ([ENTHEA Issue #2](https://github.com/elder-plinius/ENTHEA/issues/2))*

---

## 🜂 What is Needed for Quantum Completeness?

To achieve true **Quantum Completeness**, `entheai` must perfectly bridge the fluid, uncarved entropy of prompt states with the rigid, deterministic singularity of compiled, verified execution. Nothing hidden, zero false claims, structural honesty at every layer.

---

### Phase 1: Fluid Entropy Field State Serialization (Fluid Phase)

- [ ] **1.1 `QuantumCheckpoint` State Engine (`crates/memory-pp`)**
  - Implement 2-way serialization for transient prompt entropy fields (`EntropyState` containing active frozen node activations, raw memory spans, Marqant compression ratio, and audio seed state).
  - Allow snapshotting fluid work-in-progress state to `.entheai/checkpoints/<id>.json` so session context can freeze and unfreeze seamlessly without context decay or token loss.

- [ ] **1.2 Real-Time Entropy Telemetry Stream (`crates/bus` & `crates/tui`)**
  - Stream live TUI state (active brain-ring glow intensities, pomodoro ticks, and `wk N` worker counts) over the NATS bus (`entheai.entropy.v1`).

---

### Phase 2: Singularity Verification & Zero-Drift Checkpoints (Fixed Phase)

- [x] **2.1 Mandatory Deterministic Merge Seals (`crates/orchestrator`)** — *shipped in 0.5.0*
  - ✅ Every subagent fan-out worktree merge passes an empirical verification gate: `[fanout].verify`, else auto-detected `./scripts/check.sh`; `verify_required = true` by default.
  - ✅ Deterministic SHA-256 `MergeSeal` (`sha256(diff)`, `sha256(verify log)`, combined seal) carried on each integrated outcome and printed in the fan-out report. Self-reported success without empirical logs is rejected (`VerifyStatus::Unverifiable` → left on branch), enforcing [`frozen/verification.md`](file:///Users/peter.lodri/workspace/peterlodri-sec/entheai/frozen/verification.md).

- [ ] **2.2 Binary Reproducibility & Target CPU Anchoring**
  - Ensure Apple Silicon native compilation (`aarch64-apple-darwin`, `mimalloc`, `LTO=fat`) yields byte-reproducible releases across environments.

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

- [ ] **4.1 Live `/api/entropy` Telemetry Endpoint (`wrangler.jsonc` & Worker)**
  - Connect `entheai.com/docs` to the NATS event bus or Cloudflare KV store.
  - Display a live visual beacon showing real-time active nodes, prompt compression ratios, and Vaked genesis seals directly on the web documentation header.

- [ ] **4.2 Automated Hourly Site Build & Sync (`scripts/build-site.mjs`)**
  - Maintain the automated hourly build cycle (`node scripts/build-site.mjs`) updating `public/index.html`, `public/docs/index.html`, `llms.txt`, and `llms-full.txt`.

---

### Phase 5: Self-Hosting Flywheel & Structural Honesty Audit

- [ ] **5.1 Recursive Development Self-Audit (`bin/entheai`)**
  - When `entheai` develops `entheai` via `--fanout` / `agy`, run a post-execution self-audit against `AGENTS.md` and `docs/superpowers/`.
  - Enforce strict depth guards (`ENTHEAI_FANOUT_DEPTH <= 3`) and log all recursive turns transparently.

---

*“Built because the singularity doesn't need complexity. It needs friends. And because entropy cannot lie.”*
