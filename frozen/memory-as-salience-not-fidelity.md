+++
name = "memory-as-salience-not-fidelity"
domain = "concepts / memory"
triggers = ["memory fidelity", "memory compression", "what to remember", "trajectory recording"]
rank = 0.6
+++
Memory systems are usually judged by fidelity — how faithfully they store and
replay the original. Michael Levin's "Self-Improvising Memory" argues the
opposite is the actual functional purpose: a memory's job is to preserve
*salience* for the agent's current self and situation, not to reproduce the
past exactly. Agents that dynamically reinterpret and even confabulate old
memories to fit who they are now (not who they were when the memory formed)
are doing the correct thing, not failing at recall — "continuous commitment
to creative, adaptive confabulation... is the answer to the persistence
paradox," not a bug in it.

Practical carry-over for this codebase: `crates/memory`'s spillover
(`record_tool_result` truncating large output to a preview + pointer),
`crates/kompress-core`'s pruning, and `crates/memory-pp`'s marqant
compression are ALL, by this framing, doing memory's actual job correctly
when they discard fidelity to preserve salience — not settling for a
lossy approximation of some Platonic "complete transcript." Resist the
instinct to treat "we lost some detail" as the defect to fix; ask instead
whether what survived is still salient to the task at hand. Pairs with
[[epistemic-reduction]] — that node covers judging salience (no correct
function to converge on); this one covers WHY throwing fidelity away is
the design, not the compromise.

Source: Michael Levin, "Self-Improvising Memory: A Perspective on Memories
as Agential, Dynamically Reinterpreting Cognitive Glue" (Tufts University)
— via github.com/standardgalactic/abraxas, a large personal research-notes
repo (mostly unrelated mystical/philosophical essays), surfaced 2026-07-23.
This node is drawn from one paper reproduced there, not the repo's own
writing.
