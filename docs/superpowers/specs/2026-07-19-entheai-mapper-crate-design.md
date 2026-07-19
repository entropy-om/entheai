# entheai — `entheai-mapper` crate: structuring large prompts/tasks/files for fan-out

**Design spec** · 2026-07-19 · status: shipped — revised post-review (see §9)

## 1. Summary

Today, `crates/orchestrator::decompose_messages` sends the raw task string
as-is to the orchestrator LLM, which returns a JSON array of `{role, task}`
sub-tasks (`parse_decomposition`). There is no pre-processing: a large,
unstructured prompt, or a task that references specific files, reaches the
decompose call as one undifferentiated blob of text.

This slice adds a new crate, **`entheai-mapper`**, whose sole responsibility
is to turn a raw task string (plus any files it references, explicitly or
inline) into a structured, sectioned bundle. The mapper never decides
fan-out — it only prepares clean material for the orchestrator LLM's existing
decompose step to reason over.

## 2. Scope & session boundaries

New crate: `crates/mapper` (`entheai-mapper`), added to the workspace
`Cargo.toml` members list. One integration edit in
`crates/orchestrator/src/lib.rs` (`run_fanout`/`run_fanout_readonly` call the
mapper before `decompose_messages`). No TUI code changes are required — see
§5. Does not touch `crates/memory`, `crates/companion`, `crates/radio`,
`crates/viz`, or the second-brain server. Scoped `git add <exact paths>`
only; push immediately after each commit per the repo's multi-session
convention.

## 3. Public API (`crates/mapper/src/lib.rs`)

```rust
pub struct Mapper;

impl Mapper {
    /// Structure `task` (+ any `@{path}`/bare-path references it contains, plus
    /// any caller-supplied `files`) into a `MappedInput`. Never errors: unreadable
    /// files are skipped, not surfaced as failures.
    pub async fn map(root: &Path, task: &str, files: &[PathBuf]) -> MappedInput;
}

pub struct MappedInput {
    pub sections: Vec<PromptSection>,
    pub file_chunks: Vec<FileChunk>,
    /// True if more file chunks were discovered than the crate's fixed safety
    /// cap allows; `file_chunks` was truncated rather than risk blowing the
    /// decompose call's context window. Added post-review — see §9.
    pub truncated: bool,
}

impl MappedInput {
    /// Render sections + file chunks into one labeled text block, suitable as
    /// the user message body for the orchestrator's decompose call.
    pub fn render(&self) -> String;
}

pub struct PromptSection {
    pub heading: Option<String>,
    pub body: String,
}

pub struct FileChunk {
    pub path: PathBuf,
    pub chunk_index: usize,
    pub total_chunks: usize,
    pub content: String,
}
```

Dependencies: `tokio` (fs) + `std` only — no `entheai-config` /
`entheai-providers`, keeping the crate a pure, independently-testable
utility. No `regex` dependency (not currently in the workspace); path-token
and `@{...}` detection use hand-rolled string scanning.

## 4. Behavior

### 4.1 Prompt structuring (always runs)

Markdown-aware sectioning: `#`/`##` headings and numbered/bulleted lists are
grouped into named `PromptSection`s. If no structure is detected, the whole
`task` becomes a single section with `heading: None`.

### 4.2 File discovery (three layers, merged/deduped by canonical path, root-scoped)

1. **Explicit `files` param** — caller-supplied (programmatic callers; not
   used by the TUI in this slice).
2. **`@{path}` tokens in the task text** — the user-facing trigger syntax.
   Detected via a hand-rolled scan for `@{`...`}` spans (no regex dep),
   resolved against `root`. In the rendered section body, each token is
   replaced with a short `[file: path]` marker; the actual content is
   surfaced separately via `file_chunks`.
3. **Fallback bare path-token scanning** — for path-like substrings not
   already captured by (1)/(2) (e.g. `crates/foo/src/bar.rs` mentioned without
   `@{}`). Best-effort; unresolvable tokens are ignored.

Every candidate from all three layers is resolved and rejected if it escapes
`root` — see §4.2.1 (added post-review, §9). Deduping happens by canonical
path, not by the literal joined string, so equivalent spellings of the same
file (`foo.rs` vs `./foo.rs`) collapse to one entry.

#### 4.2.1 Root containment (security)

`resolve_and_dedupe` canonicalizes both `root` and every resolved candidate,
then rejects any candidate whose canonical path doesn't start with the
canonical root — the same containment discipline `crates/tools/src/fs.rs`'s
`resolve_in_root` already enforces for every other file-access path in this
codebase (absolute-path escapes and `../` traversal are both rejected;
symlinks are defeated because containment is checked against the
*canonicalized* target, not the lexical one). Without this, an `@{path}` or
bare-path reference could point anywhere the process can read (e.g.
`@{/Users/me/.ssh/id_rsa}` or `@{../../secrets.env}`), and — because the
mapper's whole purpose is to embed file content into a prompt sent to a
remote LLM provider — that content would be exfiltrated off-machine. This
was caught in code review after initial ship; see §9.

### 4.3 File chunking

Each discovered file is read and split into size-bounded line chunks
(~200 lines per chunk, snapped so no chunk cuts a line in half). Unreadable
or binary files are skipped silently (debug-logged) — a bad file reference
never aborts the map. A fixed ceiling (`MAX_FILE_CHUNKS`, currently 50; see
§9) caps the total chunks a single `map` call will include, so a large
reference set can't blow the decompose call's context window;
`MappedInput::truncated` is set and `render()` appends a note when this
triggers.

### 4.4 Rendering

`MappedInput::render()` produces a single text block, e.g.:

```
## Section: Requirements
...body...

## Section (untitled)
...body with @{...} tokens replaced by [file: path]...

### File: crates/foo/src/bar.rs (chunk 1/3)
...chunk content...
```

This replaces the raw `task` string as the user message passed into
`decompose_messages` in `crates/orchestrator/src/lib.rs`.

## 5. TUI trigger (`@{file}`) — no code change required

The user-facing trigger for the mapper is typing `@{path/to/file}` directly
in the TUI's chat input. This requires **no TUI changes**: `app.input`
already accepts arbitrary characters via `KeyCode::Char` in `handle_key`
(`crates/tui/src/lib.rs:773`), and a submitted message already flows
verbatim — `Action::Submit(text)` → `entheai_orchestrator::run_fanout(&config,
&root, &text, ...)` (`crates/tui/src/lib.rs:435-462`) — so `@{...}` tokens
reach the mapper unmodified once `run_fanout` calls `Mapper::map`. This is
verified with a passthrough test (§7), not a TUI code change. No
autocomplete/highlighting is in scope for this slice.

## 6. Integration point (`crates/orchestrator`)

`run_fanout` and `run_fanout_readonly` call `Mapper::map(root, task, &[])`
before building `decompose_messages`, passing `mapped.render()` in place of
the raw `task` string.

> **Revised post-review (§9):** the original version of this section said
> "the original `task` is still used verbatim for the synthesis step and the
> final report." That's still true for `run_fanout`'s human-facing final
> report (`format_v2_report`, plain text formatting, no LLM call) — but it
> was **wrong** for `run_fanout_readonly`'s synthesis step and both
> functions' empty-decomposition fallbacks, which also go through
> `orchestrate_once`. `orchestrate_once` never registers any tools (its
> `ToolRegistry` is always empty), so those calls have no way to read a
> referenced file themselves; feeding them the raw, unresolved `@{path}`
> marker made file references a silent dead end on those paths. All three
> now receive `mapped.render()` as well — see §9.

## 7. Testing

- Section splitting: headings, lists, and plain (no-structure) input.
- `@{path}` extraction (found/missing file) and fallback bare-path scanning.
- Chunk boundaries: exact multiples of the chunk size, remainders, empty
  file, unreadable/binary file (skipped, not erroring the batch).
- `render()` output shape (sections + file chunks appear as labeled blocks).
- Orchestrator-level test: `run_fanout`'s decompose call receives rendered
  mapper output, not the raw task string.
- TUI passthrough test: an input string containing `@{...}` survives
  `Action::Submit` unmodified into the text passed to `run_fanout`.
- *(Added post-review, §9)* Root-containment tests: relative `../` traversal
  and absolute-path escapes are both rejected, even when the target file
  exists; an in-root file resolves normally alongside a rejected escape in
  the same call. Dedup-of-equivalent-forms test (`foo.rs` vs `./foo.rs`).
  `MAX_FILE_CHUNKS` truncation test (cap hit + under-cap no-op). Orchestrator
  contract test that synthesis and both fallbacks receive mapped content.

## 8. Out of scope (this slice)

- TUI autocomplete/highlighting for `@{file}` (§5).
- Structure-aware (AST/tree-sitter) file chunking — line-bounded only.
- Config knobs for chunk size / max chunks — `MAX_FILE_CHUNKS` (§9) is a
  fixed safety ceiling, not a tunable one; can still be lifted into
  `entheai-config` in a later slice per the config-refactor design if needed.

## 9. Post-review revision (2026-07-19, same day)

A workflow-backed code review (high effort) run against the shipped feature
found one security bug and several correctness/reliability gaps, all fixed
same-day in commit `243523f` on top of the original implementation
(`12aefe8`..`c2e7865`):

| # | Severity | Finding | Fix |
|---|---|---|---|
| 1 | **Security** | `resolve_and_dedupe` had no root-containment check — absolute paths and `../` traversal were resolved and read verbatim, so a task could exfiltrate any file the process can read to the remote LLM provider. | Canonicalize `root` and every candidate; reject anything whose canonical path doesn't start with the canonical root (mirrors `crates/tools/src/fs.rs`'s `resolve_in_root`). See §4.2.1. |
| 2 | Correctness | `run_fanout_readonly`'s synthesis step used raw `task`, not `mapped.render()`, even on the successful path — `orchestrate_once` has no tools, so an unresolved `@{file}` marker there was unreadable. | Synthesis now uses `mapped.render()`. See §6. |
| 3 | Correctness | Both functions' empty-decomposition fallback used raw `task` instead of `mapped.render()`, for the same no-tools reason. | Fallbacks now use `mapped.render()`. See §6. |
| 4 | Correctness/reliability | No cap on total rendered file content — a large reference set could overflow the decompose model's context window and hard-fail a task that would otherwise succeed. | Added `MAX_FILE_CHUNKS` (50) as a fixed safety ceiling; `MappedInput.truncated` + a `render()` note when hit. See §3, §4.3. |
| 5 | Correctness (plausible) | Dedup keyed on the literal joined path string, not canonical identity — `foo.rs` and `./foo.rs` both passed the "unseen" check and the file was embedded twice. | Fixed by the same canonicalization as #1 — dedup now keys on canonical path. See §4.2. |
| 6 | Efficiency | Per-candidate existence checks and per-file reads ran sequentially in a loop — an "I/O-in-loop / N+1" pattern already flagged as an open performance risk elsewhere in this codebase. | Both now run concurrently via `tokio::spawn`, with results collected back in deterministic (submission, not completion) order — so `render()`'s output ordering stays reproducible run-to-run. |

10 new/updated tests across `crates/mapper` and `crates/orchestrator` cover
all six items (root-containment, dedup, truncation, and the
synthesis/fallback contract). Full verification: `cargo test --workspace`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings`, and
`cargo fmt --all -- --check` all clean at commit `243523f`.
