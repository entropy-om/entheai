# entheai Native App — Bundled Minimalist Ghostty Launcher — Design

**Date:** 2026-07-19 · **Status:** approved design, pre-plan
**Scope:** make entheai feel like a **native macOS app**: an `entheai.app` (+ an `entheai app` CLI verb) that opens **one branded, minimalist Ghostty window** running the entheai CLI, with the **raindrop shader** (viz Slice 2) baked in. Delivers the Slice 2 shader *for the bundled window*. macOS / Apple Silicon.

**Decisions locked (2026-07-19):**
- **Launcher model** — `entheai.app` launches the user's **installed** Ghostty with a bundled config; it does NOT embed Ghostty. (True self-contained bundle is a later non-goal.)
- **Ad-hoc codesigning** for now (works on the user's machines, first-launch right-click→Open); Developer-ID signing + notarization is a later upgrade.
- Ghostty is **MIT-licensed** — bundling its config/shader + depending on the installed app is fine.

## 1. Purpose

Starting entheai should feel like opening an app, not typing a command into whatever terminal you happen to have. `entheai.app` opens exactly **one** dedicated, minimalist, entheai-branded Ghostty window — hidden titlebar, no tab chrome, entheai's font/theme, and the ambient **raindrop shader behind the text** — running the agent. Because entheai *owns* that window, the shader + minimalist look are **guaranteed** there (no per-user Ghostty config required).

## 2. Scope

**In:** the `entheai.app` launcher bundle; the `bin/entheai-launch` binary; an `entheai app` CLI subcommand; the bundled minimalist Ghostty config; shipping + installing the settled `rain_on_glass.glsl`; the `build-release.sh` `.app` assembly + ad-hoc codesign; a Homebrew **cask**.

**Out (explicit, related follow-ups):**
- **Standalone-CLI viz Slice 2** — `entheai doctor` writing the shader into a *user's own* `~/.config/ghostty/config`, plus the **Path-C** ANSI-Perlin fallback for non-Ghostty terminals. These serve people running `entheai` in their own terminal (not the app) and get their own spec/plan. This app spec only ships the shader *for the bundled window*.
- A **true self-contained** Ghostty bundle; **notarization** + a public signed cask; **Linux** packaging.

## 3. Architecture

`entheai.app` is a **thin launcher** — no embedded terminal emulator. On launch it materializes the shader + config into a per-user home, locates the installed Ghostty, and opens a single isolated window running the entheai CLI.

```
entheai.app/Contents/
  Info.plist                     # CFBundleIdentifier=com.entheai.app, icon, version, no LSUIElement
  MacOS/entheai-launch           # the launcher (CFBundleExecutable) — new `bin/entheai-launch`
  Resources/
    entheai                      # the CLI binary (self-contained except Ghostty)
    entheai-companion            # the beacon binary
    ghostty-minimal.conf.tmpl    # minimalist config template ({{SHADER}} placeholder)
    shaders/rain_on_glass.glsl   # the settled Slice 2 shader (ldSBWW 3-iter port)
    AppIcon.icns
```

Two entry points, one behavior:
- **`entheai.app`** (Finder / Spotlight / Dock) → runs `entheai-launch`.
- **`entheai app`** (CLI subcommand, from any terminal) → runs the same launcher logic. "Start entheai → one branded Ghostty window."

The plain **`entheai`** CLI is unchanged — it runs the agent in the current terminal for people who prefer that.

## 4. The launcher — `bin/entheai-launch` (new Rust bin)

A small binary. On run:

1. **Materialize assets** into `~/.config/entheai/` (idempotent — copy only if missing or older than the bundled copy):
   - `~/.config/entheai/shaders/rain_on_glass.glsl` ← bundled shader.
   - `~/.config/entheai/ghostty-minimal.conf` ← rendered from `ghostty-minimal.conf.tmpl` with `{{SHADER}}` replaced by the **absolute** `~/.config/entheai/shaders/rain_on_glass.glsl` path (expanded to `$HOME`, since Ghostty config paths are absolute).
   - This per-user home is **shared** with the standalone `entheai doctor` follow-up — one shader, one canonical location.
2. **Resolve the entheai CLI path** — prefer the sibling in the app bundle (`../Resources/entheai` relative to the launcher executable via `std::env::current_exe()`); fall back to `entheai` on `PATH` (for the `entheai app` subcommand case where the launcher is the CLI itself).
3. **Locate Ghostty** — `/Applications/Ghostty.app/Contents/MacOS/ghostty`, else `ghostty` on `PATH`. If absent → a clear message ("Ghostty is required: `brew install --cask ghostty`") on stderr / a macOS alert, exit non-zero.
4. **Launch one isolated window:**
   ```
   ghostty --config-default-files=false \
           --config-file=$HOME/.config/entheai/ghostty-minimal.conf \
           -e <abs entheai path>
   ```
   `--config-default-files=false` prevents the user's own `~/.config/ghostty/config` from bleeding in — the minimalist look is guaranteed. `-e <entheai>` runs the agent as the window's program; closing entheai closes the window. (The exact Ghostty CLI flags are confirmed against the installed Ghostty 1.3.1 via `ghostty --help` / `+show-config` during implementation; if `--config-default-files` differs, use the documented equivalent.)

**Code sharing:** the launcher logic lives in a small new **`crates/launcher`** library — `launch() -> Result<()>` plus the testable helpers (`materialize_assets(home)`, `resolve_ghostty()`, `build_args(config, entheai)`). Two thin consumers call it: `bin/entheai-launch` (the `.app`'s `MacOS` executable — a ~5-line `main`) and `bin/entheai`'s new **`app`** subcommand. (A shared lib is required because Rust binaries can't import each other.) Keep the pure parts — asset materialization + arg-vector construction — separated from the actual `Command::spawn` so they're unit-testable without launching Ghostty. The bundled asset bytes are embedded into `crates/launcher` via `include_bytes!` (shader + config template) so `entheai-launch` can materialize them without needing the `.app` Resources on a specific path — the `.app` Resources copies are for the CLI binaries + icon, not required by the launcher for the shader/config.

## 5. The minimalist config (`ghostty-minimal.conf.tmpl`)

```
# entheai — minimalist branded window
macos-titlebar-style = hidden
window-decoration = true
window-padding-x = 14
window-padding-y = 10
window-padding-balance = true
font-family = JetBrainsMono Nerd Font
font-thicken = true
background = #0f0e1d
foreground = #e1cba6
custom-shader = {{SHADER}}
custom-shader-animation = true
minimum-contrast = 1.3
background-opacity = 1.0
confirm-close-surface = false
```
- `custom-shader-animation = true` (focused-only) per the settled decision — honors idle-frugal.
- `minimum-contrast = 1.3` — the legibility floor from the research (keep ≤ 1.3).
- The chrome keys (`macos-titlebar-style`, `window-decoration`, `window-padding-*`) are **verified against Ghostty 1.3.1's real option names** during implementation (`ghostty +show-config --default` lists them); the template is adjusted if any differ. A missing/renamed key is a build-time fix, not a design change.

## 6. Distribution

- **`scripts/build-release.sh`** gains an `.app` assembly step: build the release binaries, lay out `entheai.app/Contents/{MacOS,Resources}`, drop in `Info.plist`, the binaries, the config template, the shader, and the icon; **ad-hoc codesign**: `codesign --force --deep --sign - entheai.app`; zip it (`entheai-app-macos-arm64.zip`) and attach to the GitHub release next to the existing CLI tarball.
- **Homebrew cask** (`Casks/entheai.rb`): `app "entheai.app"`, `depends_on cask: "ghostty"`, installs to `/Applications`. Because it's ad-hoc-signed, **first launch is right-click → Open** (Gatekeeper) — documented in the README. Notarization + a clean cask is the later upgrade.
- The existing **CLI formula (`Formula/entheai.rb`) stays** for terminal-only users. `README` gets a short "Native app" section (cask install + the first-launch note).

## 7. Testing

- **`bin/entheai-launch` unit tests** (pure logic, no real spawn):
  - Asset materialization writes `rain_on_glass.glsl` + a `ghostty-minimal.conf` whose `custom-shader` line is the **absolute** shader path; a second run is a no-op (idempotent); an older on-disk copy is refreshed.
  - Ghostty resolution: returns the `/Applications` path when present; falls back to a PATH lookup; returns a clear error when neither exists.
  - The constructed `ghostty` **arg vector** is exactly `[--config-default-files=false, --config-file=<home>/.config/entheai/ghostty-minimal.conf, -e, <entheai>]` (assert the vector; don't spawn).
- **Manual (on-device spike — the settled Slice 2a check):** `entheai app` opens **one** minimalist window; rain renders **behind** the text; the **swarm graph stays legible** under refraction 0.08; closing entheai closes the window; the user's own `~/.config/ghostty/config` is unaffected.

## 8. Success criteria

- Double-clicking `entheai.app` (or running `entheai app`) opens exactly **one** entheai-branded Ghostty window — hidden titlebar, minimalist padding, JetBrainsMono, the raindrop shader animating behind the text — running the agent, with no dependence on the user's own Ghostty config.
- The shader + config live in `~/.config/entheai/` and are re-used, not re-derived, by the future standalone `entheai doctor`.
- The `.app` builds in `build-release.sh` (ad-hoc signed) and installs via the Homebrew cask; `brew install --cask ghostty` is its only external dependency.
- The plain `entheai` CLI is unchanged for terminal users.

## 9. Non-goals (restated)

Embedding Ghostty (true bundle) · notarization + public signed cask · Linux/`.deb` · the standalone `entheai doctor` shader-into-user-config + Path-C Perlin fallback (separate viz-Slice-2 follow-up) · live shader toggling (Ghostty config-level shaders can't toggle at runtime — a known Path-A limitation).
