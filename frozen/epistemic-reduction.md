+++
name = "epistemic-reduction"
domain = "concepts / memory"
triggers = ["salience", "relevance judging", "what to keep", "epistemic reduction", "novelty detection"]
rank = 0.6
+++
Concepts like Reasoning, Salience, and Relevance have no equations — they're
Epistemology-level, not Science-level. A model that "decides what matters"
(a retrieval reranker, a proactive relevance judge, a context pruner) is
performing epistemic reduction: converting a messy, irreducible situation
into a usable model by discarding the irrelevant. That discarding is
irreducibly approximate and fallible — not a defect to be engineered away,
but the nature of the operation itself.

Two corollaries worth keeping in view:
- "You can only learn that which you already almost know" (Winston) — a
  relevance judge (or a retrieval mesh) can only recognize salience against
  a baseline of what it already models; it will systematically miss novelty
  that doesn't resemble anything it has seen.
- "In order to detect that something is new you need to recognize everything
  old" (Anderson) — proactive surfacing needs a strong baseline of "expected"
  ambient activity before "unexpected → worth waking for" becomes a
  meaningful signal, not just noise-matching.

Practical carry-over for this codebase: BrainJudge's "surfacing nothing is
prioritized over false positives" default (`crates/memory-pp/src/judge.rs`)
and kompress-core's pruning heuristics (`crates/kompress-core/src/loss.rs`,
`is_must_keep`'s hand-rolled override) are both epistemic-reduction
mechanisms — don't chase a "correct" salience function; there isn't one to
converge to. Tune the false-positive/false-negative tradeoff deliberately
per mechanism, and expect irreducible misses rather than treating them as
bugs.

Source: "Experimental Epistemology for AI" (Monica Anderson,
experimental-epistemology.ai), shared 2026-07-22 alongside ongoing BRAIN
memory-system work in this repo.
