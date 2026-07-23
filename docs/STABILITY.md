# Stability contract (1.0)

What `entheai 1.0.0` commits to, and — just as deliberately — what it does not.
SemVer discipline per [VERSIONING.md](../VERSIONING.md): breaking any **stable**
surface below bumps MAJOR; additive changes bump MINOR.

## Stable surfaces

### 1. CLI

The `entheai` binary's documented flags and subcommand shapes (`--fanout`,
`--yolo`, one-shot prompt mode), `entheai-worker`'s `--serve` / `--dispatch` /
one-shot contract, and `entheai-launch`'s launch behaviour. Removing or
renaming a documented flag is MAJOR.

### 2. Configuration schema (`entheai.toml`)

Every documented `[section]` key keeps its name, type, and default. New keys
may appear (MINOR); existing keys never change meaning silently. A default may
only change when the changelog calls it out as **Changed** (e.g.
`[fanout].verify_required`).

### 3. Wire & on-disk schemas (versioned independently, `…vN`)

| Schema | Carrier |
|---|---|
| `entheai.fanout.<session>.*` | NATS fan-out event stream (`BusEvent`) |
| `entheai.entropy.v1` | NATS entropy telemetry + `/api/entropy` body (`EntropySnapshot`) |
| `entheai.checkpoint.v1` | `.entheai/checkpoints/<id>.json` (`EntropyState`) |
| `entheai.learning.v1` | second-brain learning/trajectory sync |
| `entheai.repro.v1` | `dist/repro-manifest.json` |

A breaking layout change bumps the schema's `vN` **and** the crate version;
readers reject unknown schema tags rather than guessing (see
`EntropyState::load`).

### 4. Verification invariants

These behaviours are load-bearing for trust and will not weaken without MAJOR:

- Fan-out integration requires a passing empirical verify gate by default
  (`verify_required = true`); unverifiable branches are never silently merged.
- A sealed merge's `MergeSeal` stays deterministic: same diff + same verify
  log ⇒ same SHA-256 seal.
- Recursive development keeps its depth guard (`ENTHEAI_FANOUT_DEPTH`,
  `MAX_DEPTH = 3`), its append-only `.entheai/recursion.log` ledger, and its
  post-integration self-audit (which may *skip honestly*, never pass silently).
- The site's `/api/entropy` never fakes liveness: absent or stale snapshots
  report `live: false`.

### 5. Frozen-node doctrine format

The `+++` TOML front-matter (`name`, `domain`, `triggers`, `mcp`, `rank`) +
markdown body format of `frozen/*.md`, and the rule that experience re-ranking
lives in the `frozen-ranks.json` overlay — doctrine files are never rewritten
by the system.

## Explicitly NOT stable

- **Rust crate APIs.** The workspace crates (`entheai-core`, `-orchestrator`,
  `-memory-pp`, …) are internal implementation of the binaries. Their `pub`
  items may change in any release; do not build on them as a library.
- **The TUI's visual layout**, slash-command output text, and report wording
  (the report's *structure* — status per coder, seal presence — is stable; its
  prose is not).
- **Experience-delta magnitudes** (`FAIL_RANK_DELTA`, `SUCCESS_RANK_DELTA`) —
  tunable priors, may be recalibrated in MINOR releases.
- **The PGO release pipeline** (`build-release.sh`) — machine-tuned by design
  (`target-cpu=native`); byte-reproducibility is only promised by the
  deterministic sibling (`build-repro.sh`) under an identical toolchain, and
  only as verified empirically by its `--verify` mode.
