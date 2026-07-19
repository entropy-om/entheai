# entheai — `entheai-mapper` crate: structuring large prompts/tasks/files for fan-out

**Design spec** · 2026-07-19 · status: approved — ready for implementation planning

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

### 4.2 File discovery (three layers, merged/deduped by resolved path)

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

### 4.3 File chunking

Each discovered file is read and split into size-bounded line chunks
(~200 lines per chunk, snapped so no chunk cuts a line in half). Unreadable
or binary files are skipped silently (debug-logged) — a bad file reference
never aborts the map.

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
the raw `task` string. The original `task` is still used verbatim for the
synthesis step and the final report (only the decompose input changes).

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

## 8. Out of scope (this slice)

- TUI autocomplete/highlighting for `@{file}` (§5).
- Structure-aware (AST/tree-sitter) file chunking — line-bounded only.
- Config knobs for chunk size / max chunks (fixed constants for now; can be
  lifted into `entheai-config` in a later slice per the config-refactor
  design if needed).
