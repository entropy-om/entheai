# entheai Obsidian Wiki-Sync Layer — Design

**Date:** 2026-07-19 · **Status:** approved design, pre-plan
**Scope:** make Obsidian a **first-class wiki memory layer** for entheai. When Obsidian is available locally, entheai keeps a per-project vault continuously synced as a generated, backlinked "project brain" — seeded on first run and updated **live** while entheai runs, via a per-session file watcher. One-way: repo + entheai → vault. macOS / Apple Silicon.

**Decisions locked (2026-07-19, via brainstorming):**
- **Content** — *publish + generated wiki*: mirror the repo's docs **and** generate notes from entheai's own knowledge (`.remember/` sessions, the CLAUDE.md/Repowise index, an architecture map). Memory-highlights (from the SQLite recall store) is a deferred v1.1 seam.
- **Direction** — one-way (repo + entheai → vault). Edits stay in-repo; Obsidian is a read/navigate surface. No two-way reconciliation.
- **Trigger** — a **live file watcher**, debounced.
- **Lifecycle** — **per-session**: entheai spawns the watcher from the cwd repo on launch and stops it on exit. No persistent daemon, no project registry.
- **Write mechanism** — **hybrid**: filesystem-direct writes are the source of truth (always work, Obsidian open or not); an optional best-effort **MCP nudge** refreshes/opens changed notes when Obsidian is running.
- **Activation floor** — the feature is gated by **vault-resolution**, not by any file (no README requirement). Generators are per-source conditional; the managed subtree is created lazily (never empty).
- **Ownership** — this is a distinct integration (`crates/obsidian`). It is **not** the SQLite recall memory (`crates/memory`, owned by @rahulmranga); the only touchpoint is the deferred, read-only memory-highlights seam.

## 1. Purpose

A script or an undocumented project has no docs to lean on — which is exactly where a wiki memory layer earns its keep: the generated session notes and architecture map become the *only* knowledge surface. When Obsidian is available locally, entheai should keep a per-project vault alive as a browsable, backlinked project brain, updated in near-real-time as the agent works, with **zero ongoing user action**. Because the vault is git-backed, every regeneration is diffable and revertable.

## 2. Scope

**In:** a new `crates/obsidian` library (pure render layer + runtime: watcher, vault writer, optional MCP nudge); thin per-session wiring in `bin/entheai`; a `[obsidian]` config block; the generators for the docs mirror, architecture note, session notes, and section indexes; detection + vault-resolution + activation-floor logic.

**Out (explicit, related follow-ups):**
- **Two-way sync** (vault → repo). Obsidian edits are never pushed back in v1.
- A **persistent launchd daemon** + multi-project registry (v1 is per-session only).
- **Memory-highlights** — top learnings/trajectories rendered from `crates/memory` — deferred to v1.1 behind a read-only seam (that crate is @rahulmranga's).
- Non-macOS vault paths (Linux/Windows Obsidian dirs); Obsidian Sync / Publish; real-time collaborative editing.

## 3. Architecture

A new workspace crate `crates/obsidian` (mirrors the `viz` / `launcher` / `memory` pattern), split into a pure, exhaustively-testable render layer and a thin runtime layer. `bin/entheai` owns the session lifecycle.

```
crates/obsidian/src/
  lib.rs          # re-exports; ObsidianConfig→runtime mapping; public spawn/stop entry
  render.rs       # PURE: RepoContext + change set → Vec<VaultNote>. No I/O.
  generators.rs   # PURE: one fn per note kind (docs mirror, architecture, sessions, indexes, Home MOC)
  resolve.rs      # vault resolution + activation floor (dir probing only)
  watcher.rs      # notify-based debounced watcher → change batches
  writer.rs       # VaultWriter: managed-subtree FS writes, hashing, manifest, orphan GC, atomic rename
  nudge.rs        # best-effort MCP WebSocket nudge (:22360), degrades to no-op
```

**Layer boundaries.**
- **Pure render layer** (`render.rs` + `generators.rs`): input is a `RepoContext { root, docs, remember, crate_layout, repowise_index, changed: Option<Vec<PathBuf>> }`; output is `Vec<VaultNote { rel_path: PathBuf, markdown: String }>`. Deterministic, no filesystem writes, no async — trivially unit-testable with fixture repos. `render_all(ctx)` produces the full set (seed); `render_changed(ctx, &changed)` produces only the affected notes (incremental).
- **Runtime layer** (`watcher`/`writer`/`nudge`): the only code that touches the filesystem, the clock, or the network. Each is small and independently testable.

## 4. Detection, vault resolution, and the activation floor

**Resolution** (first hit wins), performed once at session start:
1. `[obsidian] vault_path` is set (non-empty) → expand `~`, use it.
2. else auto-detect `~/Library/Mobile Documents/iCloud~md~obsidian/<repo-name>`, where `<repo-name>` is the basename of the canonicalized repo root. It must be a **real vault** — the directory exists and contains a `.obsidian/` folder — otherwise it does not match (guards against writing into a coincidentally same-named folder).
3. else → no vault resolves.

**Activation floor** (no README gate):
- If **no vault resolves** → the feature is a silent no-op (one `debug`-level log line), and entheai runs exactly as before.
- If a vault resolves, run `render_all` once. If it yields **zero notes** (bare directory: no docs, no README/AGENTS/CHANGELOG, no `.remember/` yet) → **do not create** the `entheai-sync/` subtree; the watcher still starts, and the subtree is created lazily the moment the first note has content (e.g. once entheai writes `.remember/`).
- Each generator is **per-source conditional**: missing `docs/` → no docs mirror; missing README → no README note; not a cargo/git project → the architecture note degrades to a plain file listing. A missing source skips *that note*, never the whole feature.

This makes the two edge cases behave correctly: a **script with a vault** still gets a useful wiki (sessions + file/architecture map + any stray docs); a **README-less scratch dir without a vault** stays silently off.

## 5. The generated wiki

All generated notes live under a **managed subtree** `<vault>/entheai-sync/` (configurable). entheai owns this subtree and may regenerate or delete files **within it only** — hand-written notes elsewhere in the vault (e.g. `Home.md`) are never touched. Every generated note carries YAML front-matter identifying it:

```yaml
---
generated_by: entheai
source: docs/architecture.md      # repo-relative source, if any
updated: 2026-07-19T17:40:00Z     # stamped by the writer at write time
---
```

Notes (each conditional on its source):
- **`Home.md`** — a Map-of-Content: `[[wikilinks]]` to every section + a one-line description each. The vault's landing page for the project.
- **docs mirror** — every `docs/**/*.md` plus top-level `README.md`, `AGENTS.md`, `CHANGELOG.md`, `VERSIONING.md`. Relative markdown links that point within the mirrored set are rewritten to Obsidian `[[wikilinks]]`; `docs/images/*` are referenced by relative path (Obsidian resolves vault-relative embeds).
- **`Architecture.md`** — generated from the workspace layout (each `crates/*` + `bin/*` with a one-line role) and folds in the CLAUDE.md Repowise index (entry points, hotspots, health) as linked context. Degrades to a plain file listing outside a cargo workspace.
- **`Sessions/`** — from `.remember/` (`now.md`, `today-*.md`, `recent.md`, `archive.md`, `core-memories.md`) → dated session notes, tagged for graph navigation.
- **`Specs-and-Plans.md`** and **`Research.md`** — indexes of `docs/superpowers/specs/` + `plans/` and `docs/research/`, each entry a `[[wikilink]]` into the mirror.
- **`Memory-Highlights.md`** — *v1.1, deferred*: top learnings/trajectories read **read-only** from `crates/memory`'s store, behind a seam so it composes additively once @rahulmranga's memory wiring lands. Not built in v1.

## 6. Data flow (session lifecycle)

```
entheai start (cwd = repo)
  └─ resolve vault ── none ─▶ no-op, run agent as normal
        │ resolved
        ├─ render_all → zero notes? ─ yes ─▶ skip seed (lazy)
        │                             no  ─▶ seed: write all notes to entheai-sync/
        └─ spawn Watcher (session-scoped tokio task)
              loop: fs event → debounce(≈500ms, coalesce) → render_changed
                    → VaultWriter (hash-diff, skip no-ops, atomic write, GC orphans)
                    → ObsidianNudge (best-effort, if Obsidian up)
entheai exit
  └─ stop Watcher (drop the task/handle); no teardown of vault content
```

The watcher observes the configured `watch` paths (`docs`, `.remember`, `README.md`, `AGENTS.md`, `CHANGELOG.md`, `VERSIONING.md` by default) plus the crate layout for the architecture note. It never blocks the agent loop — it runs on its own task and all its errors are contained (§8).

## 7. Write mechanism (hybrid)

**FS-direct (source of truth).** `VaultWriter` writes each `VaultNote` to `<vault>/<subtree>/<rel_path>`:
- **Atomic**: write to a temp file in the same directory, then `rename` into place — Obsidian never reads a half-written note.
- **No-op skip**: a manifest `<subtree>/.entheai-sync-manifest.json` maps `rel_path → content-hash`; unchanged notes are not rewritten (keeps iCloud/git churn and Obsidian re-index minimal).
- **Orphan GC**: notes in the manifest whose source no longer renders are deleted from the subtree and the manifest — so removing a `docs/*.md` removes its mirror.
- **Confinement**: the writer refuses to write or delete any path that does not canonicalize to inside `<vault>/<subtree>/`. This is a hard invariant, asserted in code and tested.

**MCP nudge (best-effort).** When `mcp_nudge = true` and the `obsidian-claude-code-mcp` WebSocket (`127.0.0.1:<mcp_port>`, default 22360) is reachable, `ObsidianNudge` sends a fire-and-forget refresh/open message for changed notes so Obsidian surfaces them live. Any failure (socket down, Obsidian closed, plugin disabled, timeout) is swallowed — the FS write already succeeded. This is purely a UX enhancement.

## 8. Error handling

The layer is **fail-safe and non-blocking** — it must never crash or stall the agent:
- Vault unresolved / not writable → feature off, one log line.
- iCloud path not yet materialized on disk → treat the directory as writable and write anyway (iCloud syncs the change up).
- `notify` watcher init/stream error → log a `warn`, disable the watcher for the session; the seed (if it happened) still stands.
- Writer error on a single note (permission, disk) → log, skip that note, continue with the rest.
- MCP nudge failure → ignored entirely.
- Any panic in the watcher task is isolated (the task is `spawn`ed, not inlined) and never propagates to `main`.

## 9. Config — `[obsidian]`

```toml
[obsidian]
enabled = true                 # no-op unless a vault resolves (§4)
vault_path = ""                # empty → auto-detect iCloud Obsidian/<repo-name>
subtree = "entheai-sync"       # managed folder inside the vault
watch = ["docs", ".remember", "README.md", "AGENTS.md", "CHANGELOG.md", "VERSIONING.md"]
debounce_ms = 500
mcp_nudge = true               # best-effort refresh when Obsidian is up
mcp_port = 22360
include_architecture = true
include_sessions = true
```

Realized with the workspace's established `#[serde(default = "fn")]` + `impl Default` pattern (see `crates/config`). All keys are optional; an omitted `[obsidian]` block yields these defaults.

## 10. Testing

- **Pure render layer** (`render.rs` + `generators.rs`) — the bulk of the coverage, no I/O:
  - Fixture repo → expected `VaultNote` set: docs mirror present, relative links rewritten to `[[wikilinks]]`, front-matter stamped, `Home.md` MOC backlinks every section.
  - Per-source conditionality: no `docs/` → no docs mirror note; no README → no README note; non-cargo dir → architecture degrades to a file listing.
  - Activation floor: bare fixture → `render_all` returns zero notes.
  - `render_changed`: touching one source yields only the affected note(s).
- **VaultWriter** (temp-dir):
  - Write, then a second write with identical content is a no-op (hash-skip); changed content rewrites.
  - Atomic rename (no partial file observable).
  - Orphan GC: a note whose source vanished is deleted from subtree + manifest.
  - **Confinement**: a `rel_path` containing `../` (or an absolute path) is rejected — nothing is written outside `<vault>/<subtree>/`.
- **Resolution** (temp-dir): each of the three rules; the `.obsidian/`-presence guard rejects a same-named non-vault directory.
- **Watcher**: a debounce/coalesce unit test with an **injected fake event source** — a burst of N events yields exactly one `render_changed` batch. Real `notify` integration behind a `#[ignore]`/manual test.
- **MCP nudge**: mocked socket accepts the message; socket-down path returns `Ok(())` (best-effort) and never errors.

## 11. Ownership & placement

New `crates/obsidian` + `[obsidian]` config + thin `bin/entheai` session wiring. Fully distinct from `crates/memory` (@rahulmranga). The single touchpoint into his crate — `Memory-Highlights.md` — is deferred to v1.1 and is read-only.

## 12. Success criteria

- Launching entheai in a repo that has a resolvable vault seeds `<vault>/entheai-sync/` with a docs mirror + architecture + session + index notes, and `Home.md` links them.
- Editing a `docs/*.md` (or entheai writing `.remember/`) updates the corresponding vault note within ~1s, atomically, without rewriting unchanged notes.
- Deleting a source removes its mirrored note (orphan GC).
- Running in a repo with **no** vault (or a `/tmp` scratch dir) is a complete no-op — no subtree, no error, agent unaffected.
- A **README-less script project that has a vault** still produces a useful wiki (sessions + architecture/file map).
- With Obsidian open + the plugin enabled, changed notes visibly refresh; with Obsidian closed, writes still land and sync via iCloud/git.
- Nothing outside `<vault>/entheai-sync/` is ever written or deleted.

## 13. Non-goals (restated)

Two-way sync · persistent launchd daemon + multi-project registry · memory-highlights (v1.1 seam) · non-macOS vault paths · Obsidian Sync/Publish · runtime toggling of the watcher · README-as-gate.
