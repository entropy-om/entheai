# Entheai Fusion — ONE Oracle + eBPF Sphere (POST-PRE-process layer)

**Status:** ARCHITECTURE SPEC (pre-code, awaiting oracle review)
**Scope:** merge the assistant fleet (hermes, openclaw/camofox/nemoclaw, AF agents)
into entheai as a single **Oracle** role, guarded by an **eBPF sphere** around the
fanout's pre/post-processing layer, dancing with the orchestrator over the event bus.
**Author:** peter · part of the constellation · 2026-08-06

---

## 1. Vision (one paragraph)

Today the fleet is N separate services (hermes runtime, camofox/nemoclaw/OpenClaw
pair, spider, researcher, adk, linear, inbox…) each with its own compose stack,
talking over ad-hoc HTTP. This spec fuses them: **entheai becomes the single
orchestrator** (it already is), the fleet becomes **one Oracle role** it can dispatch
to, and a **kernel-level eBPF sphere** wraps the pre/post-processing layer so the
orchestrator's decisions are grounded in what actually happened on the system —
not just what the coders claimed. The Oracle and the sphere "dance": orchestrator
decomposes → Oracle adjudicates → eBPF sphere attests → orchestrator integrates.

---

## 2. Current architecture (the ground truth)

- `crates/orchestrator/src/lib.rs` — `run_fanout()` decomposes → spawns coders in
  isolated git worktrees → integrates. Emits `FanoutEvent`s: `Decomposed`,
  `CoderStarted`, `CoderFinished`, `Integrating`, `Done`.
- `CoderExecutor` trait (`lib.rs:79`) — the seam. Implementations: `AgyExecutor`
  (`agy.rs`, spawns the Antigravity CLI), `CopilotExecutor` (`copilot.rs`).
- `crates/memory-pp/` — prompt-processing: `processor.rs`, `frozen.rs`, `judge.rs`
  (BrainJudge), `mesh.rs` (NativeMesh). This is the existing "post-process" brain.
- Event bus: `crates/bus/` (NATS, opt-in, `[nats]`), and the local `tee` in
  `bin/entheai/src/main.rs` (`FanoutEvent` mpsc).
- Fleet: hermes (API :8642), OpenClaw pair (camofox-browser + nemoclaw sandbox),
  AF agents (control-plane :8080 + per-agent), private registry
  `registry.tail2870dc.ts.net:5000`, egress squid.

---

## 3. The Oracle role

**Concept:** the strongest model in the swarm, dedicated to ADJUDICATION, not
generation. Today the orchestrator both decomposes AND synthesizes on one model.
The Oracle splits that: the orchestrator plans; the Oracle verifies every phase.

### 3.1 Where it lives

A new `crates/orchestrator/src/oracle.rs` implementing a **`Oracle` trait** (parallel
to `CoderExecutor`):

```rust
pub struct OracleAdjudication {
    pub verdict: Verdict,            // Approve | Rework { reason } | Reject { reason }
    pub confidence: f32,             // 0..1
    pub attestations: Vec<Attestation>, // claims the eBPF sphere can check
}

pub trait Oracle: Send + Sync {
    /// Model id used for adjudication (may differ from the coder/orchestrator).
    fn model(&self) -> &str;
    /// Adjudicate a phase: decompose plan, coder diff, or integration candidate.
    async fn adjudicate(
        &self,
        phase: Phase,                // Decompose | CoderDiff { branch } | Integration { branches }
        context: OracleContext,      // task, mapped files, branch diffs, prior verdicts
    ) -> anyhow::Result<OracleAdjudication>;
    /// Register a fleet endpoint as an oracle backend (hermes / AG / openclaw).
    fn register_backend(&mut self, backend: OracleBackend);
}
```

`OracleBackend` is the FUSION SEAM: it wraps any fleet service behind the Oracle
interface, so "all assistants merge into entheai" concretely means each fleet
service becomes an `OracleBackend` the one Oracle can call:

```rust
pub enum OracleBackend {
    Hermes { api: Url, key: String },          // nousresearch/hermes-agent :8642
    OpenClaw { endpoint: Url },                  // camofox + nemoclaw sandbox
    AgentField { control: Url },                 // AF control-plane :8080
    Native { model: String },                    // entheai's own strongest model
}
```

### 3.2 Where it hooks into `run_fanout`

Three adjudication gates (the "dance" touchpoints):

| Gate | Phase | What the Oracle checks |
|------|-------|------------------------|
| G1 | after `Decomposed` | Is the plan sound? Are sub-tasks well-scoped? (`Rework` → re-decompose) |
| G2 | after each `CoderFinished` | Does the diff match the task? Is it minimal + non-destructive? |
| G3 | before `Integrating` | Do the branches compose? Any conflict/regression risk? |

Each gate is **advisory by default** (log + record), **blocking when configured**
(`[oracle].gate = "block"`) — so the fleet can run loose today and tighten later.

### 3.3 Config

```toml
[oracle]
enabled = false          # default OFF — the fleet keeps today's behavior
gate = "advisory"        # advisory | block
model = "vaked/qwen3-coder:30b"   # coder.vaked.dev free tier, or a fleet backend
backends = []            # ["hermes", "openclaw", "agentfield"] — the fusion list

[oracle.backends.hermes]
api = "http://100.105.72.88:8642"
```

---

## 4. The eBPF sphere (POST-PRE-process layer)

**Concept:** a kernel-level observability/attestation ring around the fanout's
pre-processing (mapper/decompose) and post-processing (integrate/synthesize)
phases. It answers: *what did the coder ACTUALLY touch?* with kernel truth.

### 4.1 Platform honesty (must be stated)

eBPF is **Linux-only**. entheai runs on darwin (Apple Silicon). So:

- The **sphere lives on the fleet's Linux hosts** (dev-cx53, hetzner, agent-node-01)
  where the coders actually run in worktrees.
- On the darwin orchestrator host, the sphere is **disabled / DTrace-backed**
  (a thin shim — no eBPF claims).
- The sphere is a **sidecar process** (`entheai-sphere`) that the orchestrator
  talks to over the event bus — not in-process (kernel privilege boundary).

### 4.2 What it traces (the pre/post layer ring)

| Layer | eBPF hook | What it attests |
|-------|-----------|-----------------|
| PRE (mapper/decompose) | `security_file_open` / `tracepoint:sys_enter_*` | Which files the decompose/map actually read → ground the `MappedInput` |
| CODER worktree | `security_file_open` + `bpf_trace_printk` on write syscalls | Every file the coder opened/wrote in its worktree — the REAL diff, not the claimed one |
| POST (integrate) | `security_file_open` + net egress (`tc`/`kprobe:tcp_sendmsg`) | Which branches merged + what egress the integration made (squid audit, kernel-verified) |
| Egress | `kprobe:tcp_sendmsg` | Where each process actually dialed out (the "squid tells the truth" replacement) |

### 4.3 The attestation contract

The sphere emits `Attestation`s — hashed, order-able claims:

```rust
pub struct Attestation {
    pub ts: u64,
    pub pid: u32,
    pub proc: String,          // container / coder id
    pub kind: FileOp | Egress | Merge,
    pub path: Option<String>,  // canonical path
    pub hash: [u8; 32],        // sha256 of the file content (for diffs)
    pub src: String,           // remote addr for egress
}
```

These flow into the Oracle's `OracleAdjudication.attestations` — so G2/G3 verdicts
are grounded in what eBPF saw, not just what the coder reported. **This is the
"dance": orchestrator plans → sphere observes → oracle adjudicates on observed truth.**

### 4.4 Implementation shape (Linux sidecar)

```
entheai-sphere/
  bpf/            # the BPF programs (libbpf-based, CO-RE)
    file_trace.bpf.c
    egress_trace.bpf.c
  src/            # Rust sidecar: loads BPF, emits Attestations to NATS/bus
  attester.rs     # sha256 content hashing + attestation building
```

Events: `entheai.fanout.<session>.attest.{file,egress,merge}` (the existing NATS
bus, `[nats]` section — already the federation path).

---

## 5. The Dance (orchestrator  oracle  sphere)

```
        ┌──────────────────────── entheai (darwin orchestrator) ───────────────────────┐
        │                                                                              │
        │  run_fanout():                                                               │
        │    map+decompose ──▶ [Oracle G1] ──▶ spawn coders (agy / oracle backends)     │
        │        │                                  │                                   │
        │        │                          [SPHERE traces coder worktree]              │
        │        ▼                                  ▼                                   │
        │    [Oracle G2] ◀── attestations ── entheai-sphere (Linux sidecar)             │
        │        │                                                                      │
        │    integrate ──▶ [Oracle G3] ◀── attestations (merge + egress)                │
        │        ▼                                                                      │
        │    Done ──▶ memory-pp ingests adjudications + attestations                    │
        └──────────────────────────────────────────────────────────────────────────────┘
```

The **event bus is the dance floor**: `FanoutEvent` (orchestrator → oracle),
`Attestation` (sphere → oracle), `OracleAdjudication` (oracle → orchestrator). All
three are async streams; the orchestrator awaits verdicts at the gates.

---

## 6. Migration path (fleet → fused)

| Step | What | Result |
|------|------|--------|
| 1 | Add `Oracle` trait + `oracle.rs` skeleton (enabled=false) | Fusion seam exists, no behavior change |
| 2 | Wire G1/G2/G3 gates as advisory (log-only) | The dance starts recording |
| 3 | Add `OracleBackend::Hermes` → hermes API :8642 | Fleet's strongest runtime is now an Oracle backend |
| 4 | Add `OracleBackend::OpenClaw` → camofox/nemoclaw | OpenClaw pair is an Oracle backend |
| 5 | Add `OracleBackend::AgentField` → control-plane :8080 | AF agents are an Oracle backend |
| 6 | `entheai-sphere` sidecar on dev-cx53 (file+egress BPF) | POST-PRE layer is kernel-attested |
| 7 | Wire attestations into G2/G3 | Oracle adjudicates on observed truth |
| 8 | `[oracle].gate = "block"` when confidence is high | The fleet tightens |

Steps 1–2 are darwin-safe (no eBPF). Steps 3–5 need the fleet live. Step 6 is
Linux-only and needs `registry.tail2870dc.ts.net` running (the fleet registry).

---

## 7. Risks & open questions

1. **Oracle model cost** — every gate doubles model calls. Mitigation: G1/G2/G3
   only run on diffs/plans above a size threshold; `advisory` mode first.
2. **eBPF CO-RE availability** on the fleet hosts (kernel headers, BTF). dev-cx53
   must expose `/sys/kernel/btf/vmlinux`. Check before committing to the sidecar.
3. **"ONE oracle" semantics** — is the Oracle ONE model, or ONE INTERFACE over many
   backends? This spec assumes **interface** (one `Oracle` trait, many backends) —
   the strongest reading of "merge into one".
4. **The registry is down** — `registry.tail2870dc.ts.net:5000` doesn't resolve on
   dev-cx53. The fleet registry must run before steps 3–5 (camofox/nemoclaw images).
5. **Attestation trust** — eBPF attests the CODER HOST, not the orchestrator's
   darwin host. Cross-host attestation needs the NATS bus (already designed).
6. **`memory-pp` integration** — adjudications + attestations should feed the
   prompt-processing soil (the `PpTrajectorySink` pattern exists). Spec'd, not coded.

---

## 8. Oracle review (2026-08-06) — folded into this spec

Verdict: **sound to start step 1** after two P0 seam fixes; the sphere needs
reframing before step 6. Decisions below are the resolved version.

### P0 fixes (accepted)

1. **Trait shape corrected.** `OracleBackend` now carries **rework dispatch** so
   "merge the fleet" is real: the enum is the seam, not just a judgment wrapper.
   `register_backend` removed from the trait (unusable through `Arc<dyn Oracle>`);
   registration lives on the concrete `FusionOracle` struct. `model(&self)` replaced
   by `adjudicator_id` on the adjudication result (lifetime-free identity).

```rust
pub trait Oracle: Send + Sync {
    async fn adjudicate(&self, phase: Phase, ctx: OracleContext)
        -> anyhow::Result<OracleAdjudication>;
    // no &mut self, no lifetime-bound identity
}

pub struct FusionOracle {
    backends: Vec<OracleBackend>,   // concrete struct owns the registry
    router: RouterConfig,           // model resolution via [oracle] config
}

pub enum OracleBackend {
    Hermes { api: Url, key_env: String },        // key via api_key_env (workspace convention)
    OpenClaw { endpoint: Url },                  // camofox + nemoclaw
    AgentField { control: Url },
    Native { model: String },                    // entheai's own strongest model
}

pub struct OracleAdjudication {
    pub verdict: Verdict,               // Approve | Rework{reason, dispatch_to: Option<OracleBackend>} | Reject{reason}
    pub confidence: f32,
    pub attestations: Vec<Attestation>,
}
```

2. **Sphere premise fixed.** Ground truth: coders run **locally on darwin** unless a
   fleet executor is wired AND `workers_available()` (lib.rs:846-852). The sphere is
   Linux-only ⇒ on the primary path it attests nothing. Resolution:
   - **Where coders run** is now an explicit `[oracle].coders` decision
     (`local` | `fleet`). The sphere only enters the loop when `coders = fleet`.
   - **G2 without attestations is defined**: fall back to diff-vs-task review (git
     diff + MergeSeal, lib.rs:643), **never stall** waiting for a sidecar.
   - The §5 diagram now states this honestly.

### P1 fixes (accepted)

- **G2 rework loop** reuses `RepairLedger` (lib.rs:987-993 — the designated loop
  governor) + extends `WorktreePool` for late spawns. G2 gates ONLY Passed/Skipped
  coders (NoChanges/Failed/Unverifiable already excluded) and runs **concurrently**
  (doesn't serialize the fanout tail).
- **Block semantics defined**: blocking triggers only on `Reject`/`Rework` above a
  confidence threshold (Approve-confidence never blocks). G3-blocked outcome =
  leave branches unintegrated (guard already keeps them), surfaced in
  `format_v2_report`. Adjudications appear in the report (advisory verdicts visible).
- **Trust boundary stated**: NATS attestation subjects need per-host nkeys/JWT auth;
  eBPF moves trust from coder-process to host-root — a root-compromised coder host
  controls the sidecar too. "Kernel truth" is exactly that far, no further.
- **Key handling**: `key_env` names an env var (workspace convention, config/src/lib.rs:126).
- **Sphere reframed (step 6 shrinks)**: `git diff` + MergeSeal already answer
  "what the coder wrote". eBPF's REAL wins: (1) sandbox-escape/external-write
  detection, (2) **per-PROCESS egress attribution** (squid can't attribute to pid
  behind NAT — eBPF's unique value), (3) mapper-read grounding for MappedInput.
  Implementation: **fanotify/fsnotify watching worktree roots** for file ops
  (portable, no BTF, darwin-capable via FSEvents) + **eBPF only for egress**.
  BTF confirmed present on dev-cx53 (`/sys/kernel/btf/vmlinux`, 7.2.0-cachyos).
- **G3 conditional**: skip the model call when no merge conflicts + all seals pass.
- **G1 hook** bound (re-decompose retry via RepairLedger/VRR-stop, re-run
  ensure_coder, re-emit Decomposed). Advisory records → `TrajectorySink`.
- **Darwin shim**: FSEvents/filesystem-watch (not DTrace — no entitlements).
- **OracleBackend asymmetry** flagged: Hermes doesn't expose its model, Native does —
  no model-aware routing across backends yet (documented limitation).

### Open questions — resolved

- One-model vs interface: **interface, because it now carries rework** (P0-1).
- eBPF dev-cx53 vs defer: **defer, but "where coders run" answered now** (`fleet`).
- Gate default advisory: **confirmed** (safe).
- Registry prerequisite: **required before step 4** (openclaw images), soft for 3
  (hermes API is live — curl :8642 confirms). Stand up independently.

---

## 9. What needs a decision before code

- [x] Oracle = one interface with rework dispatch (RESOLVED by oracle review)
- [x] eBPF on dev-cx53: BTF confirmed; sphere = fanotify + eBPF-egress (RESOLVED)
- [x] Gate default: advisory, block on high-confidence Reject/Rework (RESOLVED)
- [x] Registry prerequisite: required for step 4 only (RESOLVED)
- [ ] Where coders run: `[oracle].coders = "local" | "fleet"` — confirm the default

*Signed — peter · for the constellation · the Oracle and the sphere dance together*
