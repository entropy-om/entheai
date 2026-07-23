+++
name = "prediction-error-learning"
domain = "concepts / relevance systems"
triggers = ["prediction error", "predictive coding", "relevance judge accuracy", "brain judge tuning"]
rank = 0.5
+++
Predictive coding's core loop: predict → compare prediction to actual outcome
→ when wrong, update the model that generated the prediction, not just the
single answer. The prediction itself isn't the valuable part; the error
signal is, because it's the only thing that improves the next prediction.

Practical carry-over for this codebase: `BrainJudge` (`crates/memory-pp/src/judge.rs`)
is a predictor — "is this activity relevant to frozen node X?" — but it is
currently a STATIC predictor: the same prompt, same precision-biased default,
run forever with no feedback loop. It never learns from a case where it
surfaced nothing but should have (or vice versa). Predictive coding's framing
says the natural next step isn't a better one-shot prompt — it's capturing
the error signal (a user manually invoking a frozen node BrainJudge missed,
or dismissing one it surfaced) and feeding it back into the judge, the same
way `frozen::FrozenNode.rank` is described as "curated prior; experience-updated
later" in `frozen/README.md`. Don't over-invest in perfecting the static
predictor; invest in making the miss visible and feedable, per [[epistemic-reduction]]'s
point that no static salience function is ever going to be "correct."

Source: informal ChatGPT explainer on predictive coding — via
github.com/standardgalactic/abraxas, a large personal research-notes repo
(mostly unrelated mystical/philosophical essays), surfaced 2026-07-23. The
concept (predictive coding / prediction-error minimization) is standard
neuroscience/ML theory, not the repo author's own work.
