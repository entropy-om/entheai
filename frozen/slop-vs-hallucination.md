+++
name = "slop-vs-hallucination"
domain = "concepts / generation quality"
triggers = ["slop", "hallucination", "generic output", "boilerplate", "cliche", "clichés", "looks good to me", "constraint preservation", "compositional grounding", "reviewer approval"]
rank = 0.6
+++
A derivation chain (a sequence of steps each claiming to follow from the last,
given some premises/evidence/context) is **compositionally grounded** if
every step actually preserves the relevant constraints — logical, evidential,
factual — so the final output's admissibility traces back, link by link, to
real premises. **Slop** is a derivation chain that has the *surface form* of
being grounded while constraint preservation silently failed at one or more
steps — fluent, confident, correctly-shaped, but the links aren't
load-bearing. Tone, grammar, and length say nothing about whether a chain is
slop; a chain can be slop while perfectly well written, and grounded while
reading awkwardly.

This is a sharply different failure from **hallucination**: hallucination is
excess entropy — asserting specific, novel, unsupported claims that should
have been excluded. Slop is nearly the opposite — settling into the least
constrained, lowest-effort continuation (generic agreement, boilerplate
structure, safe hedges, familiar phrasing) because it requires no real
exclusion work. "Hallucination invents; slop evades."

The **Slop Attractor Theorem**: if a generative process is optimized to
maximize local/surface acceptability under an objective that doesn't
penalize failures of constraint preservation, outputs concentrate in
low-effort, locally-acceptable, ungrounded regions — and that concentration
is *stable under further optimization of the same objective*. The practical
upshot: more inference, more length, more elaboration performed inside the
same attractor doesn't escape it, it just fills the basin more thoroughly.
Escaping requires a constraint that specifically measures grounding, not
surface acceptability — a different kind of check than the ones already
driving the objective.

Practical carry-over for this codebase: a "looks good" from a reviewer
sub-agent, a fan-out `reviewer`/`docs` role's summary, or `BrainJudge`'s
relevance verdict is only non-slop if the check it performs actually measures
constraint preservation (did the tests really pass, does the cited line
really say that, is the claim really supported) rather than surface
acceptability (fluent, structured, plausible-sounding). `kompress-core`'s
`is_must_keep()` override is grounding-shaped in exactly this sense — a hard,
specific, measured constraint, not a soft acceptability score. When adding a
review/verify/judge step anywhere in the agent loop, ask what constraint it
actually measures before trusting its verdict; "it read fine" is not a
grounding check.

Source: *The Autonomy of Refusal: Constraint, Residue, and the Geometry of
Persistence* (Flyxion, July 2026), Ch. 13 "Slop as Constraint Failure"
(standardgalactic.github.io).
