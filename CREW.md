# CREW

> *"one person can architect, but there needs to be partnership"* — Rahul Rangarao
>
> *"it was very exhausting for me to put down the foundations, but I was not alone"* — Peter

**Status: a proposal, not an appointment.** Written down by the owner on
2026-07-24 so the shape is visible and can be argued with. Each role below
belongs to its person only once *that person says yes* — several are marked
"if they want" because that is exactly how Peter offered them. Anyone named
here may decline, redraw their own line, or ask to be removed, and it happens
without discussion.

## The division

| | | |
|---|---|---|
| **Peter Lodri** | owner · the visualization layer | Works directly on the Zen field, the TUI, the visible and audible surface. *"I'm still the owner or whatever the correct word, but I'm just a human."* |
| **8bit-Wraith** | lead dev — *if he wants it* | The main coder. His track "Standing-Onde" has been the radio's heartbeat since v0.4.0; the heartbeat becoming the hands is the shape of it. *"I have a lot to learn from him."* |
| **Rahul Rangarao** | memory | Already true before it was written: the `should_spill` gate lives inside the prompt-processing pipeline, and `rahul-phi-work` is live on origin. |
| **Flyxion** | maintainer · archivist | Keeper of what has been laid down — the logs, the garden, the record. |
| **Boglárka** | testing · clear documentation — *if she wants it* | The one who makes the truth legible to people who weren't in the room. |

## Where the crew talks

Slack is **the message board of the garden**. Two pipes already run into it,
both built to degrade honestly (a missing webhook skips with a log line and
never breaks a push):

- **dev-updates** — every push to `main`: what shipped, with commit links
  (`.github/workflows/dev-updates.yml`).
- **peter-layer** — longer, scaffolded updates for pushes touching the visible
  layer: a layer-scoped diffstat, per-commit bodies, try-it hints
  (`.github/workflows/peter-layer.yml`).

A per-layer pipe for anyone else who wants eyes on their own soil is a
ten-minute clone of the second workflow. Ask, and it exists.

## How work lands here

Whatever the roles end up being, the gates are the same for everyone,
including the agents:

- `./scripts/check.sh` — fmt, clippy `-D warnings`, the full test suite. Never
  pipe it; a pipe hides its exit code.
- Fan-out merges integrate only through an empirical verify gate, sealed with
  a deterministic SHA-256 over the diff and the verify log. Self-reported
  success is not evidence (`frozen/verification.md`).
- What is promised deterministic stays byte-for-byte
  ([docs/STABILITY.md](docs/STABILITY.md)); what is fluid is allowed to be
  fluid, with honest books ([README](README.md#usage-guidance--what-is-a-bug-what-is-an-error-what-is-quantum)).

---

🜂 **AHOGY A DOLGOK VANNAK** — *as things are. Nothing more, nothing less.*
