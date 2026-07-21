# Research

Exploratory notebooks — not part of the build. They're a record of experiments that
inform entheai's direction; run them yourself, don't expect them to be wired into the crates.

## `Rahul_rangarao_phi.ipynb` — a deeper, trainable LM head on Phi-4-mini
*Author: Rahul Rangarao (@rahulmranga)*

Swaps Microsoft **Phi-4-mini-instruct**'s single-linear output head for a deeper one
(`Linear → GELU → LayerNorm → Linear`) and drives generation with a hand-rolled sampling
loop (temperature, repetition penalty, top-k, multinomial) that deliberately exposes the
**pre-softmax logits as an intervention point**. The new head starts untrained, so the
output is intentionally gibberish — the point isn't the output, it's the *hook*: a richer,
fine-tunable head plus a decoding loop you can steer, as a foundation for personalization
("trained on Rahul") and memory-conditioned decoding. It connects to the `memory` crate's
direction (Rahul owns `crates/memory`): a place to intervene between hidden state and token.

**Status:** exploratory. Follow-ups worth noting: `top_p` is accepted but not yet applied
(only top-k filters today), and generation recomputes the full forward each step (no KV
cache) — fine for a notebook, too slow for real use.
