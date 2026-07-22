# Prompt-Processing — design

## The idea

In Hungarian you can drop the pronoun and still be understood, because the *full lived
context* — everything the speaker has experienced — resolves the reference. Standard
retrieval-augmented generation does the opposite: it **compresses early**. It chunks and
summarizes and embeds the past into lossy vectors, then hopes the top-K nearest survived
the compression well enough to answer. Whatever nuance the summary dropped is gone before
the question is even asked.

Prompt-processing inverts that order. **Keep the past raw.** Search the *raw* space so no
reference is lost to premature compression. Compress **last** — only the findings that
turned out to matter for *this* prompt, distilled with structure intact. Compression stops
being a precondition for storage and becomes the final act of understanding.

Concretely, processing a prompt is three stages:

1. **Collect** — a unified raw experiential store, uncompressed.
2. **Search** — a mesh of tiny 1-bit LLMs resolves the prompt's references against the raw store.
3. **Compress** — marqant distills the findings, deterministically, into what the model sees.

## Goal

Add a new, opt-in **retrieval mode** to `crates/memory` that processes each prompt this way,
falling back cleanly to today's top-K retrieval whenever it is off, unavailable, or slow.
The default path and behaviour are unchanged; this is additive.

## Ownership note (read first)

This extends `crates/memory`, which is **Rahul's** (`rahulmranga`, CODEOWNERS) — our standing
guardrail is "don't touch `crates/memory`." Building here overrides that guardrail on the
user's direction, so the implementation must be **coordinated with Rahul**: share this spec
and the plan with him, do the work on a branch, and land it through his review. The design
below is deliberately shaped as an *additive mode* (a new variant behind the existing
`MemoryRuntime`/`retrieve_before` seam) so it composes with his crate rather than rewriting it.

## Where it plugs in (the existing seams)

`crates/memory` today is a two-tier, five-namespace store (`Namespace`: Codebase, Learnings,
Trajectories, Tools, Subagents). Its engine embeds `content`, and `search(namespace, query,
limit)` returns `ScoredEntry` by cosine similarity. The agent loop calls
`run_task_with_memory` (`crates/core/src/lib.rs`), which invokes `mem.retrieve_before(&user_msg)`
and injects the result immediately before the last user turn. Prompt-processing is a second
implementation *behind that same `retrieve_before` seam* — same call site, same injection
point, different internals — selected by config.

## Architecture

### Stage 1 — the raw experiential store

A new **raw tier** alongside the embedded namespaces. It stores experiences uncompressed:
full session transcripts (every turn), tool outputs and diffs, obsidian notes, codebase
snapshots, and ingested external sources. Two things sit next to the raw bytes:

- a **lexical index** (tantivy/BM25) and a **vector index**, both of which point *at* the raw
  payloads — the index only *locates* a passage, it never replaces its content.
- the raw content itself, retrievable in full (SQLite blob / content-addressed file).

Append-only, retention-scoped. The invariant that makes the whole idea work: **the stored
content is never lossily rewritten** — only the *finding aids* (indexes) are derived/compressed.

### Stage 2 — search: the 1-bit LLM mesh

Per prompt, a layered search that is cheap on the wide part and smart on the narrow part:

- **Recall (cheap, wide):** lexical + vector index produce a large candidate set of raw spans.
- **Re-rank + gather (smart, narrow):** a **mesh of ternary 1-bit LLMs** — the user's
  `ultra-graph` (a pure-Python byte-graph that *is* a BitNet-b1.58 model; weights ∈ {−1,0,+1},
  int8 activations) — reads candidate raw spans and agentically expands/votes. Because each
  model is ~1 bit, running *many* of them over the raw store is affordable — the point of the
  "mesh." They resolve the prompt's under-specified references (the "it", "like before", "the
  auth thing") against the raw experience and return the set of relevant **raw** findings.

`ultra-graph` is Python; it runs as a **sidecar** — a supervised subprocess speaking stdio
JSON-RPC, spawned and lifecycle-managed the way MCP servers already are in this codebase. A
strict timeout bounds it; exceeding it fails fast to the fallback (below).

### Stage 3 — compress: marqant

The raw findings go through **marqant** (`mq`, the user's Rust markdown semantic compressor,
run as a subprocess — it is not on crates.io, so no dependency edge). marqant is
**deterministic and model-free**: it token-reduces while preserving structure, so the
distillation can't hallucinate. The output — the compressed brief, optionally with the
highest-signal raw spans attached verbatim — is injected ahead of the user turn.

### Data flow

```
prompt ─▶ recall (lexical+vector over raw index)
        ─▶ ultra-graph 1-bit mesh: agentic re-rank + gather  →  relevant RAW findings
        ─▶ marqant: deterministic structure-preserving compression  →  brief
        ─▶ inject before last user turn (existing retrieve_before seam)  →  model
                     │
   any stage fails/times out ─▶ fall back to today's top-K retrieval (no regression)
```

## Fail-safe (fast, loud, no regression)

Every added dependency is behind a process boundary and a timeout:

- sidecar unreachable / mesh over budget / marqant missing / raw store empty → log loudly and
  **fall back to the current `search`+top-K retrieval**. Prompt-processing never blocks or
  degrades a run; with the mode off it is byte-identical to today.
- Failures surface to the caller with a clear reason (which stage, why) rather than silently
  producing worse context.

## Configuration

```toml
[memory]
mode = "prompt-processing"   # default: "topk" (today's behaviour); off unless set
# prompt_processing sub-table:
#   sidecar_cmd   = "python -m ultragraph.serve"   # the mesh sidecar
#   mesh_size     = 8                               # ternary models in the mesh
#   search_deadline_ms = 1500                       # fail fast to fallback past this
#   marqant_cmd   = "mq"                            # compression subprocess
#   raw_retention_days = 90
```

## Ingest (phased)

The raw store is populated incrementally; the design does not require boiling the ocean first:

1. **Phase 1 — the agent's own experience:** session transcripts + tool outputs/diffs, captured
   as runs happen (a hook off the existing trajectory-recording path).
2. **Phase 2 — the workspace:** codebase snapshots + obsidian notes.
3. **Phase 3 — external:** ingested sources (docs, URLs) via the existing skills/obsidian paths.

Each phase is independently useful; Phase 1 alone gives "search everything this agent has ever
done, raw."

## Components & interfaces (sketch — the plan will finalize)

- `crates/memory`: a new `RetrievalMode { TopK, PromptProcessing }`; `MemoryRuntime` dispatches
  `retrieve_before` on it. The `PromptProcessing` path owns the raw tier + the two indexes and
  orchestrates recall → sidecar → marqant.
- **Raw tier**: `RawStore` — `ingest(kind, bytes, meta)`, `recall(query, k) -> Vec<RawSpan>`
  (lexical+vector), `get(span_id) -> RawContent`. Never returns a rewritten payload.
- **Mesh client**: `MeshSearch` — a thin Rust client over the stdio-JSON-RPC sidecar:
  `rerank(query, spans, deadline) -> Vec<RawSpan>`; times out to fallback.
- **Compressor**: `Marqant` — `compress(findings) -> Brief` via the `mq` subprocess; deterministic.
- **Sidecar** (Python, in-repo under e.g. `sidecars/ultragraph/`): loads `ultragraph-1bit`,
  serves `rerank`, is stateless per request.

## Error handling & boundaries

- The Python sidecar and `mq` are **subprocesses** — a crash there cannot take down entheai;
  it degrades to fallback. Reader caps + timeouts mirror the existing MCP/shell hardening.
- The raw store's growth is bounded by `raw_retention_days`; ingest is append-only and
  idempotent (content-addressed) so re-ingesting a session is a no-op.

## Testing

- **Mode plumbing + fallback:** unit-test that `mode = "topk"` is byte-identical to today, and
  that each failure (no sidecar, sidecar timeout, no `mq`, empty raw store) falls back cleanly.
- **RawStore:** ingest→recall→get round-trips; recall returns raw (never rewritten) payloads;
  retention prunes; re-ingest is idempotent.
- **Mesh client:** stub the sidecar (a fake stdio-JSON-RPC responder) to test rerank + the
  deadline→fallback path without Python.
- **marqant:** golden tests — deterministic input → fixed compressed output.
- **Integration:** one end-to-end with the real sidecar + `mq` behind a feature/CI gate.

## Scope check

One subsystem: a retrieval mode + its raw store + two subprocess boundaries. It is a single
implementation plan, sequenced by the three ingest phases and the three pipeline stages.
Slice 1 (Phase-1 ingest + the topk↔prompt-processing switch + the fallback) is a complete,
testable increment on its own; the mesh and marqant stages layer on behind the timeout.

## Dependencies pulled in

- **ultra-graph** (`ultragraph-1bit`, PyPI, MIT) — Python sidecar, 1-bit mesh. Runtime process
  boundary; no Rust dependency edge.
- **marqant** (`mq`, the user's Rust tool, unpublished) — subprocess. No crates.io edge.
- Both are the user's own / 8b-ecosystem, MIT.
