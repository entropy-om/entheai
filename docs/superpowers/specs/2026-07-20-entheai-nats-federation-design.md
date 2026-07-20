# entheai NATS Federation — Design

**Status:** draft for review (doc-first, per user). **Date:** 2026-07-20.
**Scope:** federate entheai across the tailnet using the existing **`crabcc-nats`** server as
a central event bus + coordination substrate. Three layers, shipped as independent slices:
**F1 event bus**, **F2 distributed swarm**, **F3 shared state**.

> Nothing here is built yet. This spec exists to agree the shape (subjects, streams, worker
> protocol, auth, failure modes) before code. Each slice gets its own plan + TDD build.

## 0. The substrate (LIVE — provisioned 2026-07-20)

Dedicated hub **`entheai-nats.tail2870dc.ts.net`** (tailnet `100.103.159.94`), a Hetzner
**cx53… (actually cpx52: 12 vCPU / 24 GB, fsn1)** box running **NATS 2.14.2 + JetStream**,
tailnet-only (Hetzner cloud firewall drops the public `:4222`/`:8222`; reachable only over
WireGuard). Provisioned via `hcloud` + a cloud-init (`scratchpad/hetzner/nats-cloud-init.yaml`);
**verified** end-to-end (token-authed pub/sub round-trip over the tailnet). Replaces the earlier
`crabcc-nats` (retired — no longer used).

- **Auth:** token (`authorization { token }`); the token was generated on-box (`openssl rand`) and
  lives in entheai's gitignored `.env` as `NATS_TOKEN` (+ `NATS_URL`). Clients send it in the
  CONNECT `auth_token` field.
- **JetStream** on (`store_dir=/var/lib/nats/jetstream`, 40 GB file store) — durable streams,
  work-queues, KV, object store available.
- `max_payload: 8 MB`.

**Client:** [`async-nats`](https://docs.rs/async-nats) (official Tokio client, JetStream + KV +
object-store support). One new workspace dep; TLS is not needed on the tailnet (WireGuard already
encrypts), so we connect plaintext to `NATS_URL` with the `NATS_TOKEN`.

## 1. Design principles

1. **Opt-in + fail-safe.** Federation is off unless `[nats].enabled = true` *and* a connection
   succeeds. Any NATS failure (unreachable, auth, timeout) → entheai falls back to today's
   **local** fan-out (tokio tasks + local git worktrees). Federation never blocks a run. This
   mirrors the existing obsidian/MCP fail-safe posture.
2. **The event stream already exists.** `orchestrator::FanoutEvent` is emitted today to an
   optional `UnboundedSender`. F1 is "add a second sink that publishes to NATS" — minimal seam.
3. **JSON on the wire.** serde JSON payloads (matches entheai's existing style, human-debuggable
   over `nats sub`). 8 MB payload ceiling is ample for events; large blobs (git bundles) go to the
   object store, not raw messages.
4. **Tailnet-only trust model.** No Funnel/public exposure. Subject-level NATS permissions scope
   what each node may publish/subscribe (§7).
5. **One config block, creds out of git.** `[nats]` in `entheai.toml` (public repo → no secrets);
   the creds path/token comes from `.env` (gitignored), same pattern as `VALYU_MCP_URL`.

## 2. Subject taxonomy

All under an `entheai.` root, namespaced by session so many runs/instances coexist:

```
entheai.fanout.<session>.decomposed            # F1 events (fire-and-forget, core NATS)
entheai.fanout.<session>.coder.started
entheai.fanout.<session>.coder.finished
entheai.fanout.<session>.integrating
entheai.fanout.<session>.done
entheai.presence.<node_id>                      # F2 node heartbeat/announce
entheai.work.coder                              # F2 JetStream work-queue (durable)
entheai.result.<session>.<index>                # F2 coder result (or JS)
entheai.state.<namespace>                        # F3 KV bucket keyspace
```

`<session>` = the existing fan-out session UUID; `<node_id>` = tailnet hostname (stable).

## 3. Slice F1 — Event bus (`entheai-bus` crate)  ·  shippable first

**Goal:** every fan-out run publishes its lifecycle to NATS, so any subscriber on the tailnet
(tailscope, a `/federation` TUI pane, another entheai, `nats sub 'entheai.fanout.>'`) sees it live.

- New crate **`entheai-bus`**: `async-nats` wrapper. `Bus::connect(cfg) -> Option<Bus>` (None on
  any failure → caller degrades). `Bus::publish_event(session, &FanoutEvent)`.
- Wire type: a serde-serializable mirror of `FanoutEvent` (the enum is `#[derive(Clone)]` today;
  add `Serialize`/`Deserialize` behind the bus, or a `BusEvent` DTO in `entheai-bus` to avoid
  making `orchestrator` depend on serde-for-wire). **Decision:** DTO in `entheai-bus` keyed off the
  event — keeps `orchestrator` NATS-agnostic.
- **Seam:** `run_fanout` already takes `events: Option<UnboundedSender<FanoutEvent>>`. Add a bus
  tee: when `[nats].enabled`, `main.rs`/tui spawns a task that drains a cloned receiver → `bus.publish_event`.
  `orchestrator` gains **zero** NATS knowledge. (Alternative: pass a `&Bus` into `run_fanout` — rejected, it couples the crate.)
- **Verification:** publish a run, subscribe from another tailnet host (or the box) with `nats sub`,
  assert the 6 event kinds arrive in order. tailscope can grow a "federation feed" later.

**F1 is ~a day**, self-contained, and the foundation everything else observes.

## 4. Slice F2 — Distributed swarm (`entheai-federation` crate)  ·  the big one

**Goal:** coder sub-tasks run on *other tailnet nodes*, not just local tokio tasks. The
orchestrator becomes a dispatcher; idle nodes become workers.

### Flow
1. Orchestrator decomposes (unchanged) → for each coder sub-task, publishes a **`WorkItem`** to the
   JetStream **work-queue** stream `WORK` (subject `entheai.work.coder`, `WorkQueuePolicy` so each
   item is delivered to exactly one worker).
2. A `WorkItem` carries: `session`, `index`, `role`, `task`, **`repo` + `base_sha`** (how the worker
   obtains the tree — see below), `verify_cmd`, and a `deadline`.
3. Worker nodes run `entheai-worker` (the existing headless bin, extended) subscribed as a durable
   pull consumer. On pull: materialize the repo at `base_sha` in an isolated worktree, run the coder
   sub-agent (existing `run_coder` logic), commit, optionally verify.
4. Worker publishes a **`WorkResult`** (`status`, `committed`, `verify_ok`, + the change as a **git
   bundle** in the JetStream **object store**, keyed `result/<session>/<index>`).
5. Orchestrator collects results (subscribe `entheai.result.<session>.*`), fetches each bundle,
   applies it to a fresh integration branch (reusing the existing integrate/guard machinery), and
   emits `Done`.

### The repo-consistency problem (the crux)
Workers need the *same* tree. Options, in preference order:
- **(a) Git bundle over object store (self-contained, recommended).** Orchestrator publishes a
  bundle of `base_sha` (or just relies on a shared remote for the base, shipping only the *delta*
  back). Worker clones from the bundle → runs → bundles its commit back. No external git server
  needed; works purely over NATS. Bounded by object-store size (fine for source repos; large repos
  ship a shallow bundle of `base_sha`).
- **(b) Shared git remote.** All nodes `git fetch <base_sha>` from `origin` (GitHub) or a bare repo
  on the tailnet; workers push coder branches there; orchestrator fetches. Lighter payloads, but
  needs every node to have repo credentials + the commit pushed first.
- **Decision needed** (see §9): (a) is more self-contained; (b) is lighter if a shared remote is a
  given. Likely **(a) for portability, with (b) as an optimization** when `origin` already has `base_sha`.

### Reliability
- **Leases + redelivery:** work-queue `AckWait` = the task deadline; a crashed worker's item is
  redelivered to another. `MaxDeliver` caps retries → after N, the orchestrator runs that coder
  **locally** (degrade, never lose the task).
- **Presence/heartbeat:** workers announce on `entheai.presence.<node>`; the orchestrator can show
  the fleet + fall back to local if zero workers.
- **`max_parallel`** still bounds concurrency, now fleet-wide via the work-queue depth.
- **Security:** workers run untrusted-ish model output with full tools in an isolated worktree on a
  *remote* box — same yolo posture as today but now off-machine. Gate F2 behind explicit config +
  the securefs/policy hardening noted in the ultrawhale ports (Tier-1) before wide use.

## 5. Slice F3 — Shared state (JetStream KV / object store)

**Goal:** entheai instances on different nodes share memory (learnings, trajectories, codebase
index) so the federation learns collectively.

- JetStream **KV** buckets per memory namespace (`entheai-learnings`, `entheai-trajectories`, …);
  large embeddings/blobs → object store.
- **Seam only in this spec:** `crates/memory` is **@rahulmranga's** — F3 must be co-designed with
  him and is gated on memory-v1 Tasks 9–10 landing first. entheai-federation would expose a
  `StateStore` trait the memory crate *optionally* backs with NATS-KV; no edits to `crates/memory`
  here. **Lowest priority of the three.**

## 6. Config

```toml
[nats]
enabled = false                        # opt-in; off by default
url_env = "NATS_URL"                   # nats://entheai-nats.tail2870dc.ts.net:4222 (from .env)
token_env = "NATS_TOKEN"               # token, from .env — never inline in this tracked file
# name = "<node id>"                    # defaults to hostname

[federation]                            # F2
enabled = false
role = "auto"                           # "dispatch" | "worker" | "auto" (both)
repo_transport = "bundle"               # "bundle" | "remote"
```

`.env` (gitignored, already populated): `NATS_URL=nats://entheai-nats.tail2870dc.ts.net:4222` and
`NATS_TOKEN=<token>`. Nothing secret in the tracked `entheai.toml`. (Migrate token → nkey/JWT +
per-subject permissions before F2 goes wide — §7.)

## 7. Auth + security

- `auth_required: true` → connect with an nkey/JWT `.creds` file (async-nats
  `ConnectOptions::credentials_file`). User provides the creds path (§ open questions).
- **Subject permissions** (configured on the NATS server, not here) should scope: a worker may
  `sub entheai.work.coder` + `pub entheai.result.>` + `pub entheai.presence.<self>`; a dispatcher the
  inverse. Prevents a compromised node from forging others' results. Document the recommended NATS
  `authorization` block in the F2 plan.
- Tailnet-only; no Funnel. WireGuard already encrypts, so NATS TLS is optional.

## 8. Failure modes (all degrade to local, never fatal)

| failure | behavior |
|---|---|
| `[nats].enabled=false` or connect fails | F1/F2 disabled; local fan-out as today |
| NATS drops mid-run | in-flight local work continues; publishes best-effort; reconnect w/ backoff |
| zero workers online (F2) | dispatcher runs coders locally (existing path) |
| worker crashes mid-task | lease (`AckWait`) expires → redelivered; after `MaxDeliver` → local |
| object-store bundle missing/corrupt | that coder marked failed, others integrate; branch kept |

## 9. Open questions

1. ~~Creds~~ — **RESOLVED.** Token auth; `NATS_URL` + `NATS_TOKEN` in entheai's gitignored `.env`,
   verified working over the tailnet. (Future hardening: migrate to nkey/JWT + subject permissions
   per §7 before F2 goes wide.)
2. **Repo transport (F2)** — bundle-over-NATS (self-contained) vs shared git remote (lighter)?
   Is there already a shared bare repo / does every node have `origin` (GitHub) creds?
3. **Worker fleet** — which nodes should run `entheai-worker`? (crabcc-ccx33, nixai-base, dev-cx53…)
   Do they have the Rust toolchain + provider keys (`.env`)?
4. **F3 timing** — defer until memory-v1 Tasks 9–10 (Rahul) land? (recommended.)

## 10. Proposed order

**F1 event bus** (this week, self-contained, unblocks a federation feed in tailscope/TUI) →
**F2 distributed swarm** (multi-session, the headline capability) → **F3 shared state**
(after Rahul's memory wiring, co-designed). Each slice: its own `docs/superpowers/plans/…` +
TDD build + verification on the dev-cx53 sandbox against crabcc-nats.

---
_Built with 🤖 Claude Code. Companion to the fan-out orchestrator
(`crates/orchestrator`) and the dev-cx53 sandbox workflow._
