# entheai Native App Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `entheai.app` (+ `entheai --app`) opens one branded, minimalist Ghostty window running the entheai CLI with the raindrop shader baked in.

**Architecture:** A new pure `crates/launcher` lib embeds the shader + a minimalist Ghostty config template (`include_str!`), materializes them to `~/.config/entheai/`, resolves the installed Ghostty, and launches an isolated window (`ghostty --config-default-files=false --config-file=… -e entheai`). Two thin consumers call it: `bin/entheai-launch` (the `.app`'s executable) and the `entheai --app` flag. `build-release.sh` assembles + ad-hoc-signs the `.app`; a Homebrew cask installs it.

**Tech Stack:** Rust (`std::process::Command`), Ghostty 1.3.1 (installed, MIT), macOS `codesign`/`iconutil`, Homebrew cask.

---

> ## Scope & hazards — READ FIRST
> - **In scope:** `crates/launcher`, `bin/entheai-launch`, the `entheai --app` flag, the shader + config assets, `build-release.sh` `.app` assembly, `Casks/entheai.rb`, a README section.
> - **Out of scope (separate follow-up):** the standalone `entheai doctor` (writing the shader into a user's *own* `~/.config/ghostty/config`) and the Path-C Perlin fallback.
> - **Collision-free:** Tasks 1–3 (new `crates/launcher` + `bin/entheai-launch`) and Tasks 5–6 (build script, cask, README). **Task 4 edits the HOT `bin/entheai/src/main.rs`** — scoped explicit-pathspec commit, push-immediately, rebase-on-non-FF (re-read on conflict; abort+BLOCKED if irreconcilable).
> - **Every commit:** `git add <exact paths>` (new files first), `git commit -m "…" -- <paths>`, push immediately, rebase on non-FF (stash only `.repowise/wiki.db` if it blocks). Never `-A`/`.`, never `git reset --hard`.
> - **Ghostty option/flag names** (`--config-default-files`, `macos-titlebar-style`, `window-decoration`) are verified against the installed Ghostty 1.3.1 in Task 3/5 via `ghostty --help` / `ghostty +show-config --default`. `custom-shader`, `custom-shader-animation`, `minimum-contrast`, `background`, `font-thicken` are already confirmed real keys (from the research report). If a chrome key differs, use the 1.3.1 equivalent — the *behavior* (hidden titlebar, padding, shader) is the contract.

## File structure

| File | Responsibility |
|---|---|
| `crates/launcher/Cargo.toml` | new lib crate manifest |
| `crates/launcher/src/lib.rs` | `materialize_assets`, `resolve_ghostty`, `build_args`, `launch` + tests |
| `crates/launcher/assets/rain_on_glass.glsl` | the shader (embedded via `include_str!`) |
| `crates/launcher/assets/ghostty-minimal.conf.tmpl` | minimalist config template (`{{SHADER}}` placeholder) |
| `bin/entheai-launch/Cargo.toml` + `src/main.rs` | thin `.app` executable → `launcher::launch()` |
| `bin/entheai/src/main.rs` | `--app` flag → `launcher::launch()` (HOT file) |
| `bin/entheai/Cargo.toml` | add `entheai-launcher` dep |
| `Cargo.toml` (workspace) | add `crates/launcher`, `bin/entheai-launch` to `members` |
| `scripts/build-release.sh` | assemble `entheai.app`, ad-hoc codesign, zip |
| `Casks/entheai.rb` | Homebrew cask |
| `README.md` | "Native app" section |

---

## Task 1: `crates/launcher` scaffold + assets + `materialize_assets`

**Files:**
- Create: `crates/launcher/Cargo.toml`, `crates/launcher/src/lib.rs`, `crates/launcher/assets/rain_on_glass.glsl`, `crates/launcher/assets/ghostty-minimal.conf.tmpl`
- Modify: `Cargo.toml` (workspace `members`)

- [ ] **Step 1: Create the crate manifest** `crates/launcher/Cargo.toml`:
```toml
[package]
name = "entheai-launcher"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[dependencies]
anyhow = { workspace = true }

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Add to the workspace** — in root `Cargo.toml`, add `"crates/launcher"` and `"bin/entheai-launch"` to `members` (before `"bin/entheai"`).

- [ ] **Step 3: Create the shader asset** `crates/launcher/assets/rain_on_glass.glsl` (verbatim — the ldSBWW 3-iteration port from the research report §4.3):
```glsl
// rain_on_glass.glsl — Ghostty custom-shader for entheai
// Based on ldSBWW by Élie Michel (CC BY 3.0)
// Adapted for low-contrast terminal use (refraction 0.3->0.08)

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 u = fragCoord.xy / iResolution.xy;

    // Sample terminal texture at coarse scale as displacement noise
    vec2 n = texture(iChannel0, u * .1).rg;

    // Start with terminal content (text remains as-is in non-drop pixels)
    fragColor = texture(iChannel0, u);

    // Three scale passes: r=3 (many small drops), r=2, r=1 (few large drops)
    for (float r = 3.; r > 0.; r--) {
        vec2 x = iResolution.xy * r * .015;  // grid dimensions
        vec2 p = 6.28 * u * x + (n - .5) * 2.;  // UV modulation
        vec2 s = sin(p);
        // Quantized noise lookup — consistent properties per grid cell
        vec4 d = texture(iChannel0, round(u * x - .25) / x);
        // Drop lifecycle: sine magnitude x fade envelope
        float t = (s.x + s.y) * max(0., 1. - fract(iTime * (d.b + .1) + d.g) * 2.);

        if (d.r < (5. - r) * .08 && t > .5) {
            // Refraction normal: cos gives lateral curvature; z gives depth
            vec3 v = normalize(-vec3(cos(p), mix(.2, 2., t - .5)));
            // Refraction 0.08 (reduced from 0.3) preserves text legibility
            vec4 refracted = texture(iChannel0, u - v.xy * 0.08);
            // Blend: 70% original terminal, 30% refracted
            fragColor = mix(fragColor, refracted, 0.3);
        }
    }

    // Luminance-based masking: subtle darkening in dark/empty areas only
    float lum = dot(fragColor.rgb, vec3(0.299, 0.587, 0.114));
    float darkMask = 1.0 - smoothstep(0.03, 0.18, lum);
    fragColor = mix(fragColor, fragColor * 0.97, darkMask);
}
```

- [ ] **Step 4: Create the config template** `crates/launcher/assets/ghostty-minimal.conf.tmpl`:
```
# entheai — minimalist branded window (generated by the launcher; do not edit)
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

- [ ] **Step 5: Write the failing test** in `crates/launcher/src/lib.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materialize_writes_shader_and_renders_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path(); // stands in for ~/.config/entheai
        let (config, shader) = materialize_assets(home).unwrap();

        assert!(shader.ends_with("shaders/rain_on_glass.glsl"));
        assert!(shader.is_absolute());
        assert!(std::fs::read_to_string(&shader).unwrap().contains("void mainImage"));

        let conf = std::fs::read_to_string(&config).unwrap();
        assert!(!conf.contains("{{SHADER}}"), "placeholder was rendered");
        assert!(conf.contains(&format!("custom-shader = {}", shader.display())));
        assert!(conf.contains("macos-titlebar-style = hidden"));
    }

    #[test]
    fn materialize_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let (c1, s1) = materialize_assets(dir.path()).unwrap();
        let (c2, s2) = materialize_assets(dir.path()).unwrap();
        assert_eq!(c1, c2);
        assert_eq!(s1, s2);
        // second run still yields valid content
        assert!(std::fs::read_to_string(&s2).unwrap().contains("void mainImage"));
    }
}
```

- [ ] **Step 6: Run to verify it fails**

Run: `cargo test -p entheai-launcher materialize`
Expected: FAIL — `cannot find function materialize_assets`.

- [ ] **Step 7: Implement** (top of `crates/launcher/src/lib.rs`, above the tests):
```rust
//! entheai native-app launcher: materialize the bundled shader + minimalist
//! Ghostty config, resolve the installed Ghostty, and open one branded window.

use std::path::{Path, PathBuf};

const SHADER_SRC: &str = include_str!("../assets/rain_on_glass.glsl");
const CONFIG_TMPL: &str = include_str!("../assets/ghostty-minimal.conf.tmpl");

/// Write the shader + rendered config under `home` (the entheai config dir,
/// normally `~/.config/entheai`). Idempotent — always (re)writes the bundled
/// copies so a version bump refreshes them. Returns `(config_path, shader_path)`,
/// both absolute.
pub fn materialize_assets(home: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let shaders_dir = home.join("shaders");
    std::fs::create_dir_all(&shaders_dir)?;
    let shader_path = shaders_dir.join("rain_on_glass.glsl");
    std::fs::write(&shader_path, SHADER_SRC)?;

    let shader_abs = shader_path.canonicalize().unwrap_or(shader_path.clone());
    let config = CONFIG_TMPL.replace("{{SHADER}}", &shader_abs.display().to_string());
    let config_path = home.join("ghostty-minimal.conf");
    std::fs::write(&config_path, config)?;

    Ok((config_path, shader_abs))
}

/// The entheai config dir: `$HOME/.config/entheai`.
pub fn entheai_config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config").join("entheai")
}
```

- [ ] **Step 8: Run to verify it passes**

Run: `cargo test -p entheai-launcher`
Expected: PASS (2 tests). Note: the test asserts `shader.is_absolute()` — a tempdir path is absolute, and `canonicalize` keeps it absolute; if `canonicalize` fails on a not-yet-existing path in some CI, the `unwrap_or(shader_path)` keeps the (absolute) tempdir path.

- [ ] **Step 9: Gate + commit**

Run: `cargo clippy -p entheai-launcher -- -D warnings` → clean. `cargo fmt -p entheai-launcher`.
```bash
git add crates/launcher/Cargo.toml crates/launcher/src/lib.rs crates/launcher/assets/rain_on_glass.glsl crates/launcher/assets/ghostty-minimal.conf.tmpl Cargo.toml Cargo.lock
git commit -m "feat(launcher): new crate — embed shader/config + materialize_assets" -- crates/launcher/Cargo.toml crates/launcher/src/lib.rs crates/launcher/assets/rain_on_glass.glsl crates/launcher/assets/ghostty-minimal.conf.tmpl Cargo.toml Cargo.lock
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 2: `resolve_ghostty` + `build_args`

**Files:** Modify `crates/launcher/src/lib.rs`

- [ ] **Step 1: Write the failing tests** (add to `mod tests`):
```rust
#[test]
fn build_args_is_exact() {
    let cfg = Path::new("/Users/x/.config/entheai/ghostty-minimal.conf");
    let entheai = Path::new("/usr/local/bin/entheai");
    let args = build_args(cfg, entheai);
    assert_eq!(
        args,
        vec![
            "--config-default-files=false".to_string(),
            "--config-file=/Users/x/.config/entheai/ghostty-minimal.conf".to_string(),
            "-e".to_string(),
            "/usr/local/bin/entheai".to_string(),
        ]
    );
}

#[test]
fn resolve_ghostty_prefers_app_bundle_then_path() {
    let dir = tempfile::tempdir().unwrap();
    // Simulate the /Applications bundle path existing.
    let app = dir.path().join("Ghostty.app/Contents/MacOS");
    std::fs::create_dir_all(&app).unwrap();
    let bin = app.join("ghostty");
    std::fs::write(&bin, "#!/bin/sh\n").unwrap();
    assert_eq!(resolve_ghostty_in(&[dir.path().join("Ghostty.app")]).as_deref(), Some(bin.as_path()));
    // Nonexistent candidates → None (PATH lookup not exercised in the unit test).
    assert!(resolve_ghostty_in(&[dir.path().join("nope.app")]).is_none());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p entheai-launcher build_args resolve_ghostty`
Expected: FAIL — `cannot find function build_args` / `resolve_ghostty_in`.

- [ ] **Step 3: Implement** (add to `crates/launcher/src/lib.rs`):
```rust
/// The `ghostty` argument vector for an isolated, branded window running
/// `entheai`. `--config-default-files=false` keeps the user's own Ghostty
/// config from bleeding in.
pub fn build_args(config_path: &Path, entheai_path: &Path) -> Vec<String> {
    vec![
        "--config-default-files=false".to_string(),
        format!("--config-file={}", config_path.display()),
        "-e".to_string(),
        entheai_path.display().to_string(),
    ]
}

/// Testable core: return the first `<app>/Contents/MacOS/ghostty` that exists
/// among `candidates`.
fn resolve_ghostty_in(candidates: &[PathBuf]) -> Option<PathBuf> {
    for app in candidates {
        let bin = app.join("Contents/MacOS/ghostty");
        if bin.exists() {
            return Some(bin);
        }
    }
    None
}

/// Locate the installed Ghostty: the standard `/Applications/Ghostty.app`, else
/// `ghostty` on `PATH`.
pub fn resolve_ghostty() -> Option<PathBuf> {
    if let Some(bin) = resolve_ghostty_in(&[PathBuf::from("/Applications/Ghostty.app")]) {
        return Some(bin);
    }
    // PATH lookup.
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|p| p.join("ghostty"))
        .find(|p| p.exists())
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p entheai-launcher`
Expected: PASS (4 tests).

- [ ] **Step 5: Gate + commit**

Run: `cargo clippy -p entheai-launcher -- -D warnings` → clean. `cargo fmt -p entheai-launcher`.
```bash
git add crates/launcher/src/lib.rs
git commit -m "feat(launcher): resolve_ghostty + build_args (exact arg vector)" -- crates/launcher/src/lib.rs
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 3: `launch()` + `bin/entheai-launch`

**Files:**
- Modify: `crates/launcher/src/lib.rs`
- Create: `bin/entheai-launch/Cargo.toml`, `bin/entheai-launch/src/main.rs`

- [ ] **Step 1: Implement `launch()`** (add to `crates/launcher/src/lib.rs` — the impure glue; the pure parts are already tested, so `launch` needs no unit test, only that it compiles + is used):
```rust
use std::process::Command;

/// Resolve the entheai CLI to run in the window: prefer a sibling of the current
/// executable (the `.app` Resources layout / same dir), else `entheai` on PATH.
pub fn resolve_entheai() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sib = dir.join("entheai");
            if sib.exists() {
                return sib;
            }
        }
    }
    PathBuf::from("entheai")
}

/// Materialize assets, find Ghostty, and open one branded window running entheai.
/// Errors clearly if Ghostty isn't installed.
pub fn launch() -> anyhow::Result<()> {
    let home = entheai_config_dir();
    let (config_path, _shader) = materialize_assets(&home)?;
    let ghostty = resolve_ghostty().ok_or_else(|| {
        anyhow::anyhow!("Ghostty is required. Install it: brew install --cask ghostty")
    })?;
    let entheai = resolve_entheai();
    let args = build_args(&config_path, &entheai);
    // Spawn detached — Ghostty runs as its own window; the launcher can exit.
    Command::new(&ghostty).args(&args).spawn()?;
    Ok(())
}
```

- [ ] **Step 2: Create `bin/entheai-launch/Cargo.toml`:**
```toml
[package]
name = "entheai-launch"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[[bin]]
name = "entheai-launch"
path = "src/main.rs"

[dependencies]
entheai-launcher = { path = "../../crates/launcher" }
anyhow = { workspace = true }
```

- [ ] **Step 3: Create `bin/entheai-launch/src/main.rs`:**
```rust
//! The macOS `.app` executable: open the branded entheai Ghostty window.
fn main() -> anyhow::Result<()> {
    entheai_launcher::launch()
}
```

- [ ] **Step 4: Build + verify**

Run: `cargo build -p entheai-launch` → compiles.
Run: `cargo test -p entheai-launcher` → the 4 tests still pass (launch() added, no new test needed — its parts are covered).
Manual smoke (optional, on-device): `cargo run -p entheai-launch` should open a Ghostty window (it materializes `~/.config/entheai/` + runs `ghostty … -e entheai`; if `entheai` isn't on PATH it'll error inside the window — fine for the smoke).

- [ ] **Step 5: Gate + commit**

Run: `cargo clippy -p entheai-launcher -p entheai-launch -- -D warnings` → clean. `cargo fmt -p entheai-launcher -p entheai-launch`.
```bash
git add crates/launcher/src/lib.rs bin/entheai-launch/Cargo.toml bin/entheai-launch/src/main.rs Cargo.lock
git commit -m "feat(launcher): launch() + entheai-launch bin (.app executable)" -- crates/launcher/src/lib.rs bin/entheai-launch/Cargo.toml bin/entheai-launch/src/main.rs Cargo.lock
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 4: `entheai --app` flag  ⚠ HOT file

**Files:** Modify `bin/entheai/src/main.rs`, `bin/entheai/Cargo.toml`

**RE-READ `bin/entheai/src/main.rs` before editing** (shared/churny). The `Cli` struct is flag-based with a positional `prompt` + `--config/--model/--yolo/--fanout/--no-companion`. A subcommand would clash with the positional, so use an **`--app` flag** (same effect: "start entheai → one Ghostty window").

- [ ] **Step 1: Add the dependency** — `bin/entheai/Cargo.toml` `[dependencies]`: `entheai-launcher = { path = "../../crates/launcher" }`.

- [ ] **Step 2: Add the flag to `Cli`:**
```rust
    /// Open entheai in a dedicated minimalist Ghostty window (the native-app experience).
    #[arg(long)]
    app: bool,
```

- [ ] **Step 3: Handle it first in `main`** — right after `let cli = Cli::parse();` (before reading config / building anything), short-circuit:
```rust
    if cli.app {
        return entheai_launcher::launch();
    }
```
This runs the launcher (which opens Ghostty running plain `entheai` — NOT `entheai --app`, so no recursion) and exits. `main` returns `anyhow::Result<()>`, and `launch()` returns `anyhow::Result<()>`, so `return entheai_launcher::launch();` type-checks. Place it before `init_telemetry`/config so `--app` needs no config file.

- [ ] **Step 4: Build + verify**

Run: `cargo build -p entheai` → compiles.
Run: `entheai --help` shows `--app`. `cargo test -p entheai` → existing tests pass.
(Do NOT actually run `entheai --app` in CI — it spawns Ghostty. The recursion-safety is: the launcher's `build_args` runs `entheai` with NO `--app`, so the in-window entheai opens the TUI.)

- [ ] **Step 5: Gate + commit** (explicit pathspec — HOT file)

Run: `cargo clippy -p entheai -- -D warnings` → clean. `cargo fmt -p entheai`.
```bash
git add bin/entheai/src/main.rs bin/entheai/Cargo.toml Cargo.lock
git commit -m "feat(bin): entheai --app opens the native minimalist Ghostty window" -- bin/entheai/src/main.rs bin/entheai/Cargo.toml Cargo.lock
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 5: `build-release.sh` — assemble + ad-hoc-sign the `.app`

**Files:** Modify `scripts/build-release.sh`; Create `bin/entheai/resources/Info.plist` (the plist template)

`build-release.sh` currently PGO-builds `entheai` (+ `entheai-companion`) for `aarch64-apple-darwin` and tars them. Add an `.app` assembly step after the binaries are built.

- [ ] **Step 1: Verify Ghostty option/flag names on the build machine** (a manual guard, not a code step — record the result in the commit message):
```bash
ghostty --help 2>&1 | grep -iE "config-default-files|config-file"
ghostty +show-config --default 2>&1 | grep -iE "macos-titlebar-style|window-decoration|window-padding|custom-shader"
```
Expected: `--config-default-files` and `--config-file` are listed; `macos-titlebar-style`, `window-decoration`, `window-padding-x/y`, `custom-shader` appear in the default config. **If any chrome key name differs in 1.3.1, update `crates/launcher/assets/ghostty-minimal.conf.tmpl` to the real name** (behavior is the contract) and re-commit Task 1's asset.

- [ ] **Step 2: Create the Info.plist template** `bin/entheai/resources/Info.plist`:
```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>entheai</string>
  <key>CFBundleDisplayName</key><string>entheai</string>
  <key>CFBundleIdentifier</key><string>com.entheai.app</string>
  <key>CFBundleExecutable</key><string>entheai-launch</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
```

- [ ] **Step 3: Add the `.app` assembly to `scripts/build-release.sh`** (after the release binaries + the existing tarball step; use the actual built binary paths from the script — `target/${TARGET}/release/{entheai,entheai-companion}` and `entheai-launch`):
```bash
echo "==> assembling entheai.app"
APP="dist/entheai.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/shaders"
# launcher is the app executable; the CLI binaries sit ALONGSIDE it in MacOS/
# so resolve_entheai()'s sibling lookup (same dir as current_exe) finds them.
cp "target/${TARGET}/release/entheai-launch"     "$APP/Contents/MacOS/entheai-launch"
cp "target/${TARGET}/release/entheai"            "$APP/Contents/MacOS/entheai"
cp "target/${TARGET}/release/entheai-companion"  "$APP/Contents/MacOS/entheai-companion"
cp crates/launcher/assets/ghostty-minimal.conf.tmpl "$APP/Contents/Resources/ghostty-minimal.conf.tmpl"
cp crates/launcher/assets/rain_on_glass.glsl        "$APP/Contents/Resources/shaders/rain_on_glass.glsl"
cp bin/entheai/resources/Info.plist "$APP/Contents/Info.plist"
# icon (best-effort): generate AppIcon.icns from an existing image if available, else skip
if [ -f docs/images/hero.jpg ]; then
  ICONSET="$(mktemp -d)/AppIcon.iconset"; mkdir -p "$ICONSET"
  for s in 16 32 64 128 256 512; do
    sips -z $s $s docs/images/hero.jpg --out "$ICONSET/icon_${s}x${s}.png" >/dev/null 2>&1 || true
    sips -z $((s*2)) $((s*2)) docs/images/hero.jpg --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null 2>&1 || true
  done
  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns" 2>/dev/null || echo "   (icon generation skipped)"
fi
echo "==> ad-hoc codesigning entheai.app"
codesign --force --deep --sign - "$APP"
echo "==> zipping entheai-app-macos-arm64.zip"
( cd dist && ditto -c -k --keepParent entheai.app entheai-app-macos-arm64.zip )
echo "    built: dist/entheai-app-macos-arm64.zip"
```
(Layout rationale: `entheai` + `entheai-companion` are in `Contents/MacOS/` alongside `entheai-launch` so `resolve_entheai()`'s sibling lookup — same dir as `current_exe()` — finds the bundled CLI; the config template, shader, and icon go in `Contents/Resources/`.)

- [ ] **Step 4: Verify (on the build machine)**

Run: `bash scripts/build-release.sh` (or just the `.app` assembly section against pre-built binaries) → produces `dist/entheai.app` + the zip. `codesign -dv dist/entheai.app` shows an ad-hoc signature. Double-clicking (or `open dist/entheai.app`) opens the branded window (first launch may need right-click→Open under Gatekeeper).

- [ ] **Step 5: Commit**
```bash
git add scripts/build-release.sh bin/entheai/resources/Info.plist
git commit -m "build: assemble + ad-hoc-sign entheai.app (bundled Ghostty launcher)" -- scripts/build-release.sh bin/entheai/resources/Info.plist
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Task 6: Homebrew cask + README

**Files:** Create `Casks/entheai.rb`; Modify `README.md`

- [ ] **Step 1: Create the cask** `Casks/entheai.rb`:
```ruby
cask "entheai" do
  version "0.1.0"
  sha256 :no_check # ad-hoc-signed self-built zip; pin the sha on a real release

  url "https://github.com/entropy-om/entheai/releases/download/v#{version}/entheai-app-macos-arm64.zip"
  name "entheai"
  desc "Native minimalist Ghostty window running the entheai coding agent"
  homepage "https://entheai.com"

  depends_on cask: "ghostty"
  depends_on macos: ">= :ventura"
  depends_on arch: :arm64

  app "entheai.app"

  caveats <<~EOS
    entheai.app is ad-hoc signed. On first launch: right-click the app → Open,
    then confirm. (Notarized signing is planned.)
  EOS
end
```
(On a real release, replace `sha256 :no_check` with the zip's actual `shasum -a 256`.)

- [ ] **Step 2: README "Native app" section** — add under Quick start in `README.md`:
```markdown
### Native app (minimalist Ghostty window)

Prefer a dedicated, branded window? Install the app (needs Ghostty):

```bash
brew install --cask ghostty
brew install --cask entropy-om/entheai/entheai
```

Launch `entheai.app` (first time: right-click → Open — it's ad-hoc signed), or from a terminal: `entheai --app`. It opens one minimalist Ghostty window — hidden titlebar, entheai's theme, and an ambient raindrop shader behind the text — running the agent. Your own Ghostty config is untouched.
```

- [ ] **Step 3: Commit**
```bash
git add Casks/entheai.rb README.md
git commit -m "docs: Homebrew cask + README for the entheai native app" -- Casks/entheai.rb README.md
git push origin main || { git pull --rebase origin main && git push origin main; }
```

---

## Final verification

- [ ] `cargo build -p entheai-launcher -p entheai-launch -p entheai` → compiles.
- [ ] `cargo test -p entheai-launcher` → 4 tests green; `cargo clippy --workspace --all-targets -- -D warnings` → clean.
- [ ] **On-device (the settled Slice 2a spike):** `entheai --app` (or `open dist/entheai.app`) opens ONE minimalist window; rain renders behind text; the **swarm graph stays legible**; closing entheai closes the window; the user's own `~/.config/ghostty/config` is unaffected (`--config-default-files=false`). Tune `refraction`/blend in `rain_on_glass.glsl` only if legibility needs it.

## Notes for the executor

- **Recursion safety:** `--app`/`entheai-launch` runs `ghostty -e entheai` (plain, no `--app`) — the in-window entheai is the normal TUI. Never pass `--app` in `build_args`.
- **`.app` binary layout:** `entheai` + `entheai-companion` go in `Contents/MacOS/` (sibling to `entheai-launch`) so `resolve_entheai`'s sibling lookup finds the bundled CLI; config/shader/icon go in `Contents/Resources/`.
- **Ghostty flag names** are verified in Task 5 Step 1 against the installed 1.3.1; adjust the config template if a chrome key differs — behavior is the contract.
- The launcher writes to `~/.config/entheai/` — the same home the future standalone `entheai doctor` will reuse (one shader, one location).
