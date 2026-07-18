# Review — `entheai-memory` & `entheai-companion` crates

**Date:** 2026-07-18 · **Reviewer:** this session (Rust agent) · **For:** the parallel sessions that own these crates.

Both crates were reviewed independently (source read, spec cross-checked, gates run). **Both pass all gates** (`build`, `clippy -D warnings`, `fmt --check`, `test`). There is **one Critical repo-wide blocker** (companion is an untracked member) and a set of correctness/quality follow-ups per crate.

## Status snapshot

| Crate | Gate (build/clippy/fmt/test) | Tracked in git? | Wired into the app? | Verdict |
|---|---|---|---|---|
| `entheai-memory` | ✅ / ✅ / ✅ / ✅ (16 tests) | ✅ committed | ❌ nothing depends on it yet | Solid v0.1; 1 Critical resilience bug + 1 correctness bug |
| `entheai-companion` | ✅ / ✅ / ✅ / ✅ (5 tests) | ❌ **UNTRACKED** | ❌ standalone bin | **Breaks fresh clone/CI** — see blocker |

> Note: the earlier "`memory/store.rs` fmt-dirty" issue is **resolved** — it was reformatted since; the whole workspace is fmt-clean now. The single outstanding blocker is companion below.

---

## ⚠️ CRITICAL BLOCKER (repo-wide) — companion is an untracked workspace member

`crates/companion/` is **untracked** (`git ls-files crates/companion` → empty) **yet the committed root `Cargo.toml` lists `"crates/companion"` in `[workspace] members`** (also `crates/memory`, which *is* committed). Cargo resolves the whole workspace before building anything, so on a **fresh `git clone` (or the release CI runner)** the directory is absent and **every** cargo command fails workspace-wide:

```
error: failed to load manifest for workspace member `.../crates/companion`
  Cargo.toml: No such file or directory
```

It only builds here because the files exist in this working tree. **This will fail `.github/workflows/release.yml` and any clone.**

**Fix (pick one), in the session that owns companion:**
- `git add crates/companion && git commit` — if it's ready to ship, **or**
- remove `"crates/companion"` from `members` in root `Cargo.toml` (and commit) until it's ready.

---

## `entheai-memory` (spec §5.5)

**Purpose.** Two-tier / five-namespace persistent memory. Implements an async `Memory` trait, a `SqliteStore` (CRUD + flat cosine vector search over SQLite), and an OpenAI-compatible `Embedder` (Osaurus). Solid, spec-aligned v0.1.

**Architecture.** `lib.rs`: `Namespace` (5 variants + serde/`FromStr`), `Entry`, `ScoredEntry`, `MemoryError` (`thiserror`, `#[from]` rusqlite/anyhow), `#[async_trait] Memory { store/get/search/delete/list }`, `SharedMemory = Arc<dyn Memory>`. `store.rs`: `SqliteStore { db: Arc<Mutex<Connection>>, embedder: Option<Embedder> }`, single `entries` table (`WITHOUT ROWID`, PK `(namespace,key)`, `embedding BLOB`, `idx_ns_created`), WAL + `synchronous=NORMAL` + 256MB mmap; **all DB ops correctly wrapped in `spawn_blocking`**. `embed.rs`: reqwest POST to `{base_url}/embeddings`. Vector search: **brute-force flat cosine** (loads all embedded rows for the namespace, scores, sorts) — no HNSW/ANN.

**Spec alignment (§5.5).** Strong: 5 namespaces, the `codebase`=MCP / rest=local-SQLite-vector split, the "each call names a namespace" trait shape, and Osaurus embeddings all match. Documented, expected v0.1 divergences: flat-only (spec wants adaptive flat↔HNSW above ~5k vectors), `codebase` not yet MCP-federated.

**Integration.** **Not wired in** — it's a `members` entry but no crate/bin depends on `entheai_memory` (grep-confirmed). Dead weight until `core`/bin consumes it. Dev-dep `tempfile` is currently unused.

**Findings.**
- **Critical — mutex-poisoning cascade.** `db.lock().unwrap()` (`store.rs:119,152,201,261,283`) + `.expect("spawn_blocking panicked")` (`:132,167,223,268,306`). With `panic=unwind`, one panic in any blocking closure poisons the connection mutex → **every** later op panics, permanently bricking the store. Fix: `db.lock().unwrap_or_else(|e| e.into_inner())`, and map `JoinError` → `MemoryError` instead of `.expect`.
- **Important — `store()` returns wrong `created_at` on update** (`store.rs:139`). `ON CONFLICT` preserves the original in the DB, but the returned `Entry` reports `now`. Fix: `RETURNING created_at` (or re-SELECT) + a regression test.
- **Important — no HTTP timeout** (`embed.rs:31`, `reqwest::Client::new()`): a hung Osaurus blocks every `store`/`search` forever. Use `Client::builder().timeout(..)`.
- **Minor** — the single `Arc<Mutex<Connection>>` serializes all I/O, negating the WAL read-concurrency the doc comment advertises (consider `deadpool-sqlite` later); silent error drops on metadata parse (`:171,232,311`), `filter_map(Result::ok)` list rows (`:301`), and dimension-mismatch skips (`:228-230`) — a changed embed model silently yields empty results.
- **Positives:** no `unsafe`; **no SQL injection** (bound `params!` + static namespace strings); DB I/O correctly offloaded.

**Test gaps.** 16 tests cover cosine, blob round-trip, CRUD, namespace isolation/parsing, embedder errors (wiremock). Missing: end-to-end **embedded search ranking** (all store tests use `embedder=None`), `created_at`-preservation-on-update (would catch the bug), metadata round-trip, on-disk `open()` persistence (why `tempfile` is unused), poisoning/concurrency.

---

## `entheai-companion` (net-new — session "beacon" for phone pairing)

**Purpose.** A **lib + bin**. A borderless, transparent, always-on-top 180×180 window pinned bottom-right that renders a **breathing teal glow + a QR code**. The QR encodes `SessionPayload{ v, sid (session UUID), host (Tailscale MagicDNS/.local), port, cwd }` (`qr.rs:5-17`) so a **companion device (phone/tablet) can scan it to pair with a running session** over the tailnet. It opens **no sockets** — `port` is encoded for a future `comms`-crate client.

**Architecture.** `main.rs`: clap CLI (`--session-id/--host/--port/--cwd/--no-always-on-top`) → payload → `qr::generate` → winit `EventLoop` → softbuffer software render per `RedrawRequested`. `qr.rs`: `SessionPayload`, `QrGrid`, `generate()` (qrcode EC-M). `render.rs`: radial glow + centered QR blit. Deps: `winit 0.30`, `softbuffer 0.4`, `qrcode 0.14`, `serde/serde_json`, `clap`, `anyhow`.

**Spec relation.** **Net-new** — not Sonar (§5.20) or Honcho (§5.21). Closest to **comms/federation §5.12** (MagicDNS host + remote session endpoint), but distinct: §5.12 is machine-to-machine remote inference/execution; this is a **phone-companion pairing UX** the spec doesn't enumerate. Worth adding to the spec.

**Findings.**
- **Critical** — the untracked-member blocker above.
- **Important — softbuffer `Context`/`Surface` rebuilt every frame** (`main.rs:117-118`) via `.expect(...)` (panics on any failure); build them **once** outside the loop.
- **Important — uncapped-CPU render.** `ControlFlow::Poll` + `AboutToWait → request_redraw` (`main.rs:104,136-137`) renders as fast as possible; the 24fps `frame_interval()` is **never applied** (and is dead code). Directly violates spec §9 ("frame-budget the render loop, pause when idle"). Cap it / redraw only on change.
- **Important — QR can silently vanish** (`render.rs:68`): `module_px = qr_px / qr.size`; a long `cwd` grows the QR version so `size > 108`px → `module_px = 0` → blank QR (no panic). Clamp to ≥1 / grow the window / shorten the payload.
- **Minor** — `image 0.25` declared but never imported (dead dep, remove); `uuid` used only in a test → move to `[dev-dependencies]`; migrate off winit's deprecated `EventLoop::run`/`create_window` (`#[allow(deprecated)]`) to `ApplicationHandler`; `pack_bgra` actually packs ARGB and `with_transparent(true)` alpha is ignored on macOS (rename / fix intent).
- **Positives:** no `unsafe`, no network/bind surface.

**Test gaps.** 5 tests cover `qr::generate` sanity, payload serde round-trip, and the pixel helpers. Missing: `render_frame` bounds + the `module_px==0` case; oversized-payload `generate`.

---

## Action items (for the other sessions)

**companion — do first (unblocks CI/clone):**
1. **Commit `crates/companion`, or remove it from `members`.** (Critical)
2. Move `Context`/`Surface` out of the frame loop; drop the `.expect()` panics.
3. Apply the 24fps cap / pause-when-idle (spec §9); clamp `module_px ≥ 1`.
4. Remove `image` dep; move `uuid` to dev-deps; add a `render_frame` bounds test.

**memory:**
1. Fix the mutex-poisoning cascade: `unwrap_or_else(|e| e.into_inner())` + `JoinError → MemoryError`. (Critical)
2. Return the real `created_at` on update (`RETURNING`) + regression test. (Important)
3. Add a `reqwest` timeout in `embed.rs`. (Important)
4. Wire it into `core`/bin (it's currently dead weight); add an end-to-end embedded-search ranking test + an on-disk persistence test (uses the idle `tempfile`).
5. Surface (don't silently drop) metadata-parse / dimension-mismatch errors.
