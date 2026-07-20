# `entheai --skills add <url>` — Design

**Status:** approved (brainstorm, 2026-07-20). **Scope:** add a skill to entheai from a URL via
layered well-known discovery, so `entheai --skills add https://docs.stripe.com` installs a usable
skill that `SkillRegistry::discover` picks up immediately.

## Goal

One command fetches a skill from a domain and writes it as `skills/<slug>/SKILL.md` (Claude
Agent-Skills format — the format entheai already discovers). Works against real docs sites **today**
(Stripe et al. publish `/llms.txt`) and supports a richer entheai-native manifest for sites that opt in.

## Existing model (unchanged)

- A **skill** = a dir containing `SKILL.md`: frontmatter `name`/`description` + a markdown body of
  instructions (`crates/skills/src/lib.rs::parse_skill_md`).
- `SkillRegistry::discover(dirs)` scans each `[skills].dirs` entry (default `["skills"]`, resolved
  under the repo root) for immediate sub-dirs with a `SKILL.md`.
- The CLI is all flag-verbs that "do X then exit" (`--memory stats`, `--doctor`); `reqwest` (rustls)
  is already a workspace dependency.

## CLI surface

```
entheai --skills add <url>
```

`#[arg(long = "skills", num_args = 1.., value_name = "ARGS")] skills: Option<Vec<String>>`. The
handler runs **before** TUI/companion setup (like `--memory`/`--doctor`), prints what it did, exits.
- `--skills add <url>` → install. Leaves room for `--skills list` later.
- Any other/empty args → a usage error listing supported verbs.

## Discovery — layered (first tier that yields content wins)

For `add <url>`, parse with `reqwest::Url` (reject non-`http`/`https`), derive the **origin**
(`scheme://host[:port]`), and try:

1. **`GET <origin>/.well-known/skills.json`** — entheai-native manifest (multi-skill):
   ```json
   { "skills": [
       { "name": "stripe-payments", "description": "…",
         "body": "…markdown…"            // inline instructions,  OR
         "skill_md_url": "https://…/SKILL.md" }   // fetch a SKILL.md (parsed via parse_skill_md)
   ] }
   ```
   Each entry needs `name` + `description` + exactly one of `body`/`skill_md_url`. Entries missing
   both are skipped with a warning. A 404/parse-failure falls through to tier 2.

2. **`GET <origin>/llms.txt`** — the de-facto docs convention (Stripe serves this today). Synthesize
   **one** skill:
   - `name`: the first `# <title>` line's text if present, else the host (e.g. `docs.stripe.com`).
   - `description`: the first `> blockquote` line (stripped), else the first non-heading paragraph,
     trimmed to ≤200 chars.
   - `body`: the `llms.txt` content verbatim (a curated index of doc links), prefixed with a
     one-line source note (and a pointer that `/llms-full.txt` may hold full text).

3. **`GET <url>`** (the original URL, last resort) — if `Content-Type` is `text/markdown`/`text/plain`,
   use the body as-is; if HTML, a conservative dependency-light strip (`<script>`/`<style>` blocks
   removed, tags stripped, basic entities decoded, whitespace collapsed), **body clearly labeled
   "auto-extracted from <url> — best-effort, may be noisy."** One skill, `name` = host.

If no tier yields content, exit non-zero with a message naming the URLs tried.

## Install

- Target: the first `[skills].dirs` entry resolved under the repo root (default `skills/`), creating
  it if absent → `skills/<slug>/SKILL.md`.
- Generated file:
  ```
  ---
  name: <name>
  description: <description>
  source: <url>
  ---
  <body>
  ```
  (`parse_skill_md` reads `name`/`description`; `source` is extra provenance, preserved for humans.)
- **Skip-if-exists:** if `skills/<slug>/` already exists, print the path + "skipping (delete to
  re-add)" and do not clobber. (A `--force` is a later addition.)
- Print a summary: which tier matched, each skill written (name → path), and
  `added from <url> — its instructions will be advisory to the agent`.

## Security (fetches remote content + writes files)

- **Strict slug (path-traversal guard):** the possibly-remote-controlled `name` is slugified to
  `[a-z0-9-]+` (lowercase; non-alnum runs → `-`; trim/collapse `-`; cap 64 chars). Empty slug →
  error. This structurally prevents `../`, `/`, or absolute paths from a malicious manifest. After
  joining, assert the canonical target stays inside the skills dir.
- **Bounded external input:** 15s request timeout; response body capped at **1 MiB** (reject via
  `Content-Length` when present, and truncate defensively while reading); `http`/`https` only; rustls
  (no new system libs).
- **Trust note:** a skill body becomes agent instructions, so `add` is explicit about provenance
  (printed + stamped `source:` in the file). User-initiated with a user-chosen URL — no auto-add from
  untrusted tool output.

## Code structure

- **New `crates/skills/src/remote.rs`** — owns fetch + discovery + synthesis + install:
  `pub async fn add_from_url(url: &str, skills_dir: &Path) -> anyhow::Result<Vec<AddedSkill>>`,
  the `Manifest`/`ManifestSkill` serde types, `slugify`, `synthesize_from_llms_txt`,
  `html_to_text`, and `write_skill`. `lib.rs` gains `pub mod remote;`.
- **`crates/skills/Cargo.toml`** — add `reqwest` (workspace, rustls), `serde` (derive), `tokio`
  (workspace: `rt-multi-thread`,`macros`), `anyhow` (workspace), `regex`; `wiremock` (workspace) +
  `tempfile` as dev-deps.
- **`bin/entheai/src/main.rs`** — add the `--skills` arg; a `run_skills_cmd(args, skills_dir).await`
  dispatcher; resolve `skills_dir = root.join(cfg.skills.dirs.first())` (fallback `"skills"`);
  short-circuit like `--memory`/`--doctor` before the interactive path.

## Testing (TDD, hermetic via `wiremock`)

- `slugify`: `"Stripe Payments"` → `stripe-payments`; `"../../etc/passwd"` → safe slug (no `/`/`.`);
  empty → error.
- Tier 1: mock `/.well-known/skills.json` (inline `body` + a `skill_md_url` that the mock also
  serves) → asserts two `SKILL.md`s with correct name/description/body.
- Tier 2: mock `/llms.txt` (with `# Title` + `>` blurb) → asserts synthesized name/description/body.
- Tier 3: mock `/` returning HTML → asserts tags stripped + "auto-extracted" label; and a
  `text/plain` variant used as-is.
- Precedence: when tiers 1 and 2 both exist, tier 1 wins.
- Install: writes under a tempdir; skip-if-exists returns the existing path without overwriting.
- **Live smoke (manual, not a unit test):** `entheai --skills add https://docs.stripe.com` from this
  Mac → tier 2 (`llms.txt`) writes `skills/<slug>/SKILL.md`; verify + delete the throwaway skill dir.

## Out of scope (later)

`--skills list`/`remove`/`update`, `--force` overwrite, auth'd/private manifests, non-HTTP sources,
recursive multi-page crawling, and turning `llms-full.txt` into a full-text skill (v1 links to it).
