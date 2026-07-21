# STONEGEN — Procedural Stoner Music Generation, Terminal-Native Rust

Ultraplan + research brief. Goal: a terminal-native Rust program that **procedurally generates and plays endless stoner / desert / Kyuss-style instrumental music in real time**, fully offline, optionally steered by a local LLM served via **Osaurus**.

---

## 1. What "stoner / desert / Kyuss" means, as machine parameters

Research-backed genre fingerprint (translate to code, not vibes):

| Trait | Value to encode |
|---|---|
| Tempo | slow–mid, **~60–95 BPM** (Kyuss ~90). Feel the weight of each note. |
| Tuning | **down-tuned** (drop D / drop C / C standard). Root sits low. Fat, bass-heavy. |
| Tone | **fuzz + heavy distortion**, thick low end, huge bass. Waveshaping, not clean. |
| Scales | **minor pentatonic + blues scale** (bluesy), **Phrygian** for the "exotic desert" riffs. Root often A or lower. |
| Structure | **hypnotic repetition** — short riff cell, looped with slow variation. Riff = self-sustaining landscape, not technical runs. |
| Rhythm | **palm-muted chug** + pocket groove. Locks to the beat, swings slightly. Space between hits. |
| Space | reverb + delay = "vast arid expanse" ambience. Slow analog drift. |
| Sections | intro drone → riff A → groove → breakdown/drone → riff A′ → out. |

Primary sources: [Stoner rock — Wikipedia](https://en.wikipedia.org/wiki/Stoner_rock), [Kyuss style lesson (Phrygian / A minor / A blues)](https://www.guitarmasterclass.net/ls/Kyuss-Style/), [Riff/Tempo/Groove — Monster Riff](https://monsterriff.com/2020/09/17/what-is-stoner-riff-tempo-groove/).

---

## 2. Architecture — three brains, one signal path

```
 ┌─────────────┐   spec (JSON)   ┌──────────────┐  timeline   ┌───────────┐  ctrl ring  ┌──────────┐  cpal
 │ BRAIN       │ ───────────────▶│ CORE          │ ──────────▶│ SCHEDULER │ ──────────▶ │ SYNTH     │ ───────▶ 🔊
 │ (Osaurus /  │                 │ music theory  │   events    │ clock →    │ lock-free   │ fundsp    │ speakers
 │  algo dir.) │◀── fallback ────│ riff/arr gen  │             │ note on/off│ (rtrb)      │ voices+FX │
 └─────────────┘                 └──────────────┘             └───────────┘             └──────────┘
        ▲                                                                                     │
        │                                   ┌──────────────┐  param edits / meters            │
        └───────────────────────────────────│ TUI (ratatui)│◀────────────────────────────────┘
                                            └──────────────┘
```

**Layered so audio never blocks and the LLM is never a hard dependency.**

### Layer A — SYNTH (real-time audio, `cpal` + `fundsp`)
- `cpal` opens the default output device; audio runs on its own real-time callback thread.
- **No allocation, no locks, no I/O in the callback.** Control comes in via a lock-free SPSC ring (`rtrb` or `ringbuf`).
- `fundsp` builds the DSP graph with its inline graph notation. Voices:
  - **Guitar**: detuned unison saw/square × N, into **fuzz** (`tanh`/hard-clip/asymmetric waveshaper), resonant low-pass, slow LFO drift on pitch+cutoff. Palm-mute = short amp envelope + extra low-pass.
  - **Bass**: sine/tri + sub octave + mild drive. Huge. Follows riff root.
  - **Drums (synthesized, zero assets)**: kick = pitch-enveloped sine; snare = noise burst + tone; hats = filtered noise. Later option: embedded WAV one-shots.
  - **Ambience bus**: `reverb` (big) + feedback `delay` for desert space.
- Master: soft-clip limiter so fuzz stack never blows the DAC.
- Alt engine: **`knyst`** (real-time *dynamic* graph — add/remove nodes live) if we want arrangement changes to rewire the graph at runtime. Recommend **fundsp first** (more mature/documented); keep knyst as an escape hatch.

### Layer B — CORE (pure music theory, deterministic, `no_std`-friendly, 100% testable)
- Types: `Note`, `Scale` (MinorPentatonic, Blues, Phrygian), `Tuning`, `Bar`, `RiffCell`, `Groove`, `Arrangement`, `Section`.
- **Riff generator**: rule-based / Markov / L-system walk over scale degrees, biased to root + low register + rhythmic space (chug pattern with rests). Generates a 1–2 bar cell.
- **Variation**: mutate a cell (octave, added blue note, displaced accent) for hypnotic-but-evolving repetition.
- **Bass** derives from riff root; **drums** derive from `Groove` (kick on the pocket, backbeat snare, swing offset, velocity accents + humanized timing).
- **Arrangement**: grammar/Markov over sections → a song form. Emits an **event timeline** (note-on/off, velocity, param automation) with beat-relative timestamps.
- **Seeded** (`rand::StdRng` from a `u64` seed) → a seed reproduces a whole track. Shareable "track hash".

### Layer C — BRAIN (optional LLM director via Osaurus)
- **Osaurus** = native Apple-Silicon local LLM server, MLX-based, **OpenAI-compatible** REST at `http://127.0.0.1:8080/v1`, supports **function calling / JSON**. ~7 MB, offline, no cloud. ([repo](https://github.com/dinoki-ai/osaurus), [overview](https://www.blog.brightcoding.dev/2025/09/09/osaurus-a-local-llm-server-for-apple-silicon-with-openai-compatible-endpoints/))
- Role: **high-level composer/arranger only — never touches samples.** Given a mood prompt ("slow, heavy, Kyuss, 74 BPM, drop C, endless desert"), it returns a **strict JSON `TrackSpec`**: tempo, tuning, scale, section list, per-section riff seeds (scale-degree motifs), fuzz/reverb/space amounts, transition rules.
- Rust validates the JSON against a `serde` schema, repairs/rejects, and feeds CORE. CORE does the deterministic heavy lifting — the LLM only *seeds and directs*.
- **Graceful degrade**: if Osaurus isn't running, the pure-algorithmic director produces the same `TrackSpec` shape. LLM is a bonus, not a requirement → stays terminal-native and offline-first.
- Client: `reqwest` + `tokio` on a separate thread; async, never blocks audio or TUI.

### Layer D — TUI (`ratatui` + `crossterm`)
- Transport: play / pause / **regen** / next-section / seed entry.
- Live panels: current section, riff as scale-degrees/mini-tab, **VU meters + spectrum**, param "knobs" (fuzz, reverb size, tempo, tuning, drive, swing).
- Param edits → control events → audio thread (live tweak while playing).
- Aesthetic slots into the entheai **viz pillar** (this repo already ships a ratatui Canvas swarm viz) — treat music as an **ambient audio-identity layer**.

---

## 3. Threading / real-time model

| Thread | Job | Rule |
|---|---|---|
| TUI/main | render, input | 30–60 fps, never touch audio buffers |
| Audio (cpal) | fill sample buffer via fundsp | **RT-safe**: no alloc/lock/syscall; read ctrl ring only |
| Scheduler | wall-clock → beat clock; emit note-on/off + automation at the right instant | push into lock-free ring to audio |
| Brain | Osaurus calls, spec generation | async, best-effort, hot-swaps next `TrackSpec` |

Data path: `TrackSpec → CORE timeline → Scheduler → rtrb ring → SYNTH → cpal`. Meters/state flow back to TUI via a second ring or `arc-swap`.

---

## 4. Crate / workspace layout

```
stonegen/                      (own workspace, OR crates under entheai/)
├── stonegen-core     # theory, scales, riff/arr generators, seed. pure, unit-tested
├── stonegen-synth    # cpal + fundsp engine: voices, FX, RT-safe ctrl ring
├── stonegen-brain    # Osaurus client, TrackSpec schema, validation, algo fallback
└── stonegen-tui      # ratatui frontend + main binary (the terminal app)
```

**Dependencies (picks):**
`cpal` (audio I/O) · `fundsp` (synth/DSP; `knyst` as alt) · `rtrb`/`ringbuf` (lock-free ctrl) · `ratatui` + `crossterm` (TUI) · `rand` (seed) · `serde` + `serde_json` (TrackSpec) · `reqwest` + `tokio` (Osaurus) · `hound` (WAV export) · `anyhow`/`thiserror` · `arc-swap` (state snapshot).

Sources for stack: [FunDSP](https://github.com/SamiPerttu/fundsp), [cpal / Rust Audio](https://rust.audio/), [dasp](https://github.com/RustAudio/dasp), [knyst](https://lib.rs/crates/knyst), plus prior-art [`tunes`](https://github.com/sqrew/tunes) (50+ algorithmic sequences: Markov, L-systems) worth mining for riff-generator ideas.

---

## 5. Delivery slices (walking skeleton first)

- **S0 — Audio path skeleton.** `cpal` sine → speakers + `ratatui` play/stop. Proves RT-safe path end to end. *Exit: a tone plays, key toggles it, no xruns.*
- **S1 — The Tone.** One detuned fuzz guitar voice through waveshaper + LP + reverb. Hardcoded 2-bar drop-D riff loop. *Exit: it sounds unmistakably stoner.*
- **S2 — Procedural jam.** `stonegen-core`: scales, seeded riff generator, groove, bass, synth drums, scheduler. Endless seeded instrumental. *Exit: a `u64` seed reproduces a full jam; riffs vary but stay hypnotic.*
- **S3 — Arrangement + space.** Section grammar (intro/drone/riff/breakdown/out), transitions, delay+reverb ambience, analog drift. *Exit: coherent song forms, not just a loop.*
- **S4 — Osaurus brain.** `TrackSpec` JSON schema, Osaurus client, prompt→spec→validate→render, algo fallback. *Exit: a mood prompt steers a track; works with Osaurus off.*
- **S5 — TUI polish + capture.** Knobs, meters, spectrum, seed sharing, `hound` WAV/stem export, entheai viz integration. *Exit: playable, tweakable, recordable, shareable.*

---

## 6. Top risks & mitigations

1. **RT-safety** — any alloc/lock/`println!` in the cpal callback = clicks/xruns. *Mitigation:* preallocate; only lock-free rings cross the boundary; assert with a debug allocator guard.
2. **fundsp learning curve** — graph notation + AudioUnit vs AudioNode. *Mitigation:* S0/S1 spike small graphs before the generator lands.
3. **Clock/timing jitter** — groove needs tight scheduling. *Mitigation:* sample-accurate scheduling (schedule events by sample index, not wall time).
4. **LLM JSON reliability** — models emit invalid JSON. *Mitigation:* strict `serde` schema + repair pass + always-available algo fallback; LLM only seeds.
5. **"Sounds bad" risk** — procedural ≠ musical. *Mitigation:* genre constraints are hard rules (scale/tempo/register locked), randomness only inside the pocket; tune by ear each slice.

---

## 7. Locked decisions

- **Home:** crates **inside entheai** — `stonegen-*` become the ambient-audio pillar of the viz layer. Reuse entheai's ratatui conventions + aesthetic. Add to the existing workspace `Cargo.toml` members.
- **Osaurus:** **algo-first.** Build the full algorithmic director + generator across S0–S3 with no model dependency; Osaurus lands in S4 as a bonus `TrackSpec` seed source with the algo director as permanent fallback.
- **Generation shape:** **infinite live jam** — a desert-radio stream that never repeats. Emphasis on live synthesis + continuous variation, not finite track rendering. Seed still exists (reproducible starting point) but the loop runs open-ended. WAV capture (S5) records the live stream rather than rendering offline.
- **Drums:** **embedded WAV one-shots** — bundle punchy kick/snare/hat/tom samples, `include_bytes!`'d into the binary (no runtime file deps). Adds a small asset pipeline: load → decode (`hound`) → preallocated sample buffers → RT-safe voice triggers from the scheduler. Keep a synth-drum fallback path for anything a sample is missing.

### Consequences for the slices
- **S1** uses a sampled kick alongside the fuzz guitar (asset pipeline enters here, minimal).
- **S2** the groove engine triggers embedded drum one-shots; generator runs open-ended (infinite jam) rather than fixed-length.
- **S5** capture = record the live output stream to WAV, plus stem buses if cheap.
```
