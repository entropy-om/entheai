+++
name = "voting-ensemble-paradox"
domain = "concepts / ensembles"
triggers = ["voting ensemble", "k-of-N", "multi-checkpoint", "consensus voting", "AND voting", "ensemble collapse", "unanimity gate"]
rank = 0.6
+++
A safety pattern that looks strictly conservative — gate a decision behind
several independent checkpoints/verifiers and require them to agree before
acting — can silently invert. Under AND (unanimity-to-keep) voting across
voters trained or tuned on asymmetric data floors, the weakest voter on each
stratum vetoes every other voter's "keep" for that stratum. The ensemble's
combined decision becomes a stratum-wise union of each weakest voter's
rejections: the global operating point is Pareto-dominated by the frontier of
any single strong voter, and when the weakest voter differs across strata, no
individual voter actually occupies the ensemble's behavior. Adding more or
stronger voters cannot rescue this — each stratum stays pinned by whichever
voter is weakest *there*. A 3-checkpoint majority ensemble can regress
*below* the best single checkpoint (measured: 0.031 worse in the source
paper) purely from this collapse, not from any one voter being bad.

The corrective isn't "vote smarter" — it's separating three distinct
mechanisms instead of relying on the vote alone: (A) penalize failure modes
directly during training/tuning (a weighted loss on the tokens/cases that
must not be dropped), (B) a deterministic post-inference override that
force-keeps/force-flags the must-not-fail cases regardless of what any voter
says, and (C) a self-labeling loop that uses A+B as an oracle to retrain,
eventually internalizing B so the override becomes redundant.

Practical carry-over for this codebase: `crates/kompress-core`'s
`is_must_keep()` (`loss.rs`) already implements mechanism (B) — a
score-independent hard override ahead of the learned pruning decision — but
mechanisms (A) and (C) are unbuilt (no training-time critical-token penalty,
no self-labeling retrain loop). More broadly: any k-of-N or unanimous-gate
pattern in this repo — a review/verify fan-out, an adversarial-verify workflow
stage, multiple `BrainJudge`-style relevance checks — should be checked for
this exact failure mode before trusting "more voters = safer." Prefer a
deterministic override for the cases that must never fail, over stacking
more voters and hoping unanimity converges to the right answer.

Source: "Asymmetric Loss Modulation Resolves the Voting Ensemble Paradox in
Learned Context-Pruning Ensembles" (kompress-v8 paper, kompress.vaked.dev),
the same paper `is_must_keep` (Mechanism B) was ported from earlier this
session — this node captures the paper's own headline result (the paradox
itself), which the earlier port didn't.
