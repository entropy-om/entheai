//! entheai native-app launcher: materialize the bundled rain-on-glass shader +
//! minimalist terminal configs (NasTTY primary — the 8b-is WezTerm fork,
//! installed terminal, and open one branded window.

use std::path::{Path, PathBuf};

const SHADER_SRC: &str = include_str!("../assets/rain_on_glass.glsl");
const CONFIG_TMPL: &str = include_str!("../assets/ghostty-minimal.conf.tmpl");
const WEZTERM_FRAG_SRC: &str = include_str!("../assets/rain_on_glass_wezterm.frag");
const WEZTERM_CONFIG_TMPL: &str = include_str!("../assets/wezterm-minimal.lua.tmpl");

/// Write the shader + rendered config under `home` (the entheai config dir,
/// normally `~/.config/entheai`). Idempotent — always (re)writes the bundled
/// copies so a version bump refreshes them. Returns `(config_path, shader_path)`,
/// both absolute.
pub fn materialize_assets(home: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let shader_abs = materialize_shader(home)?;
    let config = CONFIG_TMPL.replace("{{SHADER}}", &shader_abs.display().to_string());
    let config_path = home.join("ghostty-minimal.conf");
    std::fs::write(&config_path, config)?;

    Ok((config_path, shader_abs))
}

/// Write the WezTerm fragment shader + rendered Lua config under `home` (the
/// entheai config dir, normally `~/.config/entheai`). Idempotent — always
/// (re)writes the bundled copies so a version bump refreshes them. Returns
/// `(lua_config_path, frag_shader_path)`, both absolute.
pub fn materialize_wezterm_assets(home: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let shader_abs = materialize_wezterm_shader(home)?;
    let lua = WEZTERM_CONFIG_TMPL.replace("{{SHADER}}", &shader_abs.display().to_string());
    let lua_path = home.join("wezterm-minimal.lua");
    std::fs::write(&lua_path, lua)?;

    Ok((lua_path, shader_abs))
}

/// Write the bundled WezTerm fragment shader under `home/shaders/` and return
/// its absolute path. WezTerm's shader contract (GLSL `main` + `ocolor`, with
/// `glyph_texture`/`pixel_coord` uniforms) differs from Ghostty's
/// Shadertoy-style `mainImage`, so the same rain-on-glass effect ships as a
/// separate `rain_on_glass_wezterm.frag` alongside the Ghostty asset. Shared by
/// the native-app launcher and `entheai doctor` — one shader, one location.
pub fn materialize_wezterm_shader(home: &Path) -> anyhow::Result<PathBuf> {
    let shaders_dir = home.join("shaders");
    std::fs::create_dir_all(&shaders_dir)?;
    let shader_path = shaders_dir.join("rain_on_glass_wezterm.frag");
    std::fs::write(&shader_path, WEZTERM_FRAG_SRC)?;
    Ok(shader_path
        .canonicalize()
        .unwrap_or_else(|_| shader_path.clone()))
}

/// Write the bundled shader under `home/shaders/` and return its absolute path.
/// Shared by the native-app launcher and `entheai doctor` — one shader, one
/// canonical location. Idempotent (always rewrites, so a version bump refreshes).
pub fn materialize_shader(home: &Path) -> anyhow::Result<PathBuf> {
    let shaders_dir = home.join("shaders");
    std::fs::create_dir_all(&shaders_dir)?;
    let shader_path = shaders_dir.join("rain_on_glass.glsl");
    std::fs::write(&shader_path, SHADER_SRC)?;
    Ok(shader_path
        .canonicalize()
        .unwrap_or_else(|_| shader_path.clone()))
}

/// The entheai config dir: `$HOME/.config/entheai`.
pub fn entheai_config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config").join("entheai")
}

/// The `ghostty` argument vector for an isolated, branded window running
/// `entheai`. `--config-default-files=false` keeps the user's own Ghostty
/// config from bleeding in.
///
/// The inner command is wrapped in `sh -c 'cd <cwd> && exec <entheai>'` so the
/// window roots in the directory `entheai --app` was invoked from. Ghostty's
/// macOS login-shell wrapper otherwise resets cwd to `$HOME`, which hides the
/// project's `.env` (provider keys → 401) and points the agent at the wrong tree.
pub fn build_args(config_path: &Path, entheai_path: &Path, cwd: &Path) -> Vec<String> {
    let inner = format!("cd {} && exec {}", sh_quote(cwd), sh_quote(entheai_path));
    vec![
        "--config-default-files=false".to_string(),
        format!("--config-file={}", config_path.display()),
        "-e".to_string(),
        "/bin/sh".to_string(),
        "-c".to_string(),
        inner,
    ]
}

/// POSIX single-quote a path for safe interpolation into an `sh -c` string.
fn sh_quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', r"'\''"))
}

/// The `wezterm start` argument vector for an isolated, branded window running
/// `entheai`. `--config-file` overrides the user's own config (so it can't
/// bleed in); `--cwd` roots the window in the directory `entheai --app` was
/// invoked from — WezTerm's native equivalent of the Ghostty `sh -c cd` wrapper,
/// so the project's `.env` and code are picked up.
pub fn build_wezterm_args(config_path: &Path, entheai_path: &Path, cwd: &Path) -> Vec<String> {
    vec![
        "start".to_string(),
        format!("--config-file={}", config_path.display()),
        format!("--cwd={}", cwd.display()),
        "--".to_string(),
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
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|p| p.join("ghostty"))
        .find(|p| p.exists())
}

/// The terminal backing the native-app window layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalKind {
    /// The 8b-is/wezterm fork — the primary window layer.
    WezTerm,
    /// The stock Ghostty fallback.
    Ghostty,
}

impl TerminalKind {
    /// Human-readable name for doctor output.
    pub fn name(&self) -> &'static str {
        match self {
            TerminalKind::WezTerm => "WezTerm",
            TerminalKind::Ghostty => "Ghostty",
        }
    }
}

/// Pick the window-layer terminal: WezTerm when its binary is on PATH (the
/// primary layer), else Ghostty. Pure so the chooser is unit-testable.
pub fn choose_terminal(wezterm_present: bool) -> TerminalKind {
    if wezterm_present {
        TerminalKind::WezTerm
    } else {
        TerminalKind::Ghostty
    }
}

/// Locate the installed WezTerm CLI: `wezterm` on `PATH` (both the stock cask
/// and the 8b-is fork install the `wezterm` binary — `wezterm start` is what we
/// spawn).
pub fn resolve_wezterm() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|p| p.join("wezterm"))
        .find(|p| p.exists())
}

/// Resolve the entheai CLI to run in the window: prefer a sibling of the current
/// executable (the `.app` MacOS layout / same dir), else `entheai` on PATH.
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

/// Materialize assets, find a terminal, and open one branded window running
/// entheai. Prefers WezTerm (the 8b-is fork) when its binary is on PATH, else
/// Ghostty. Errors clearly if neither is installed.
pub fn launch() -> anyhow::Result<()> {
    if let Some(wezterm) = resolve_wezterm() {
        return launch_wezterm(&wezterm);
    }
    launch_ghostty()
}

/// Open the branded window in WezTerm (`wezterm start --config-file <lua>`).
fn launch_wezterm(wezterm: &Path) -> anyhow::Result<()> {
    let home = entheai_config_dir();
    let (config_path, _shader) = materialize_wezterm_assets(&home)?;
    let entheai = resolve_entheai();
    // The directory `entheai --app` was invoked from — the window roots here so
    // it inherits the project's `.env` and code.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let args = build_wezterm_args(&config_path, &entheai, &cwd);
    // Spawn detached — WezTerm runs as its own window; the launcher can exit.
    std::process::Command::new(wezterm).args(&args).spawn()?;
    Ok(())
}

/// Open the branded window in Ghostty (the fallback when WezTerm is absent).
fn launch_ghostty() -> anyhow::Result<()> {
    let home = entheai_config_dir();
    let (config_path, _shader) = materialize_assets(&home)?;
    let ghostty = resolve_ghostty().ok_or_else(|| {
        anyhow::anyhow!(
            "neither WezTerm nor Ghostty is installed. \
             Install WezTerm: brew install --cask wezterm (or Ghostty: brew install --cask ghostty)"
        )
    })?;
    let entheai = resolve_entheai();
    // The directory `entheai --app` was invoked from — the window roots here so
    // it inherits the project's `.env` and code.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let args = build_args(&config_path, &entheai, &cwd);
    // Spawn detached — Ghostty runs as its own window; the launcher can exit.
    std::process::Command::new(&ghostty).args(&args).spawn()?;
    Ok(())
}

// ── `entheai doctor` (viz Slice 2b) ──────────────────────────────────────────
// Install the rain-on-glass shader into a user's OWN terminal config: WezTerm's
// `wezterm.lua` (the `shaders` key — the 8b-is fork is the primary layer) when
// WezTerm is installed, else Ghostty's `config` (`custom-shader`). The Path-C
// ANSI ambient fallback for terminals with no shader support is a follow-up.

const BLOCK_BEGIN: &str = "# >>> entheai raindrop shader — managed by `entheai doctor` >>>";
const BLOCK_END: &str = "# <<< entheai raindrop shader <<<";
const WEZ_BLOCK_BEGIN: &str =
    "-- >>> entheai raindrop shader (wezterm) — managed by `entheai doctor` >>>";
const WEZ_BLOCK_END: &str = "-- <<< entheai raindrop shader (wezterm) <<<";

/// What [`run_doctor`] did to the terminal config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigAction {
    /// The config file didn't exist; created it with the managed block.
    Created,
    /// Appended a new managed block to an existing config.
    Added,
    /// Replaced an existing managed block (e.g. the shader path changed).
    Updated,
    /// The managed block already pointed at this shader — nothing written.
    AlreadyCurrent,
    /// The target config is a Lua file we can't patch safely (e.g. an inline
    /// `return { ... }` table) — the doctor only appended an advisory comment.
    NeedsManual,
}

/// Summary of one `entheai doctor` run, for display.
#[derive(Debug, Clone)]
pub struct DoctorReport {
    /// The terminal the doctor configured (WezTerm when installed, else Ghostty).
    pub terminal: TerminalKind,
    pub wezterm_installed: bool,
    pub ghostty_installed: bool,
    /// True when we're running inside the configured terminal (its shader is
    /// visible here).
    pub is_active_term: bool,
    pub shader_path: PathBuf,
    pub config_path: PathBuf,
    pub action: ConfigAction,
}

/// The user's own Ghostty config path: `$XDG_CONFIG_HOME/ghostty/config`,
/// defaulting to `~/.config/ghostty/config` (read by Ghostty on macOS + Linux).
pub fn ghostty_config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config")
        });
    base.join("ghostty").join("config")
}

/// The user's own WezTerm config path: `$XDG_CONFIG_HOME/wezterm/wezterm.lua`,
/// defaulting to `~/.config/wezterm/wezterm.lua` (WezTerm's canonical macOS /
/// Linux search path; `~/.wezterm.lua` is the legacy alternative).
pub fn wezterm_config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config")
        });
    base.join("wezterm").join("wezterm.lua")
}

/// Idempotently insert/update the managed `custom-shader` block in `existing`
/// config text. Pure (no I/O) for easy testing. Ghostty stacks multiple
/// `custom-shader` lines, so this only ever touches OUR marked block — the
/// user's own config and any of their shaders are preserved.
fn merge_shader_block(existing: &str, shader_path: &str) -> (String, ConfigAction) {
    let block = format!("{BLOCK_BEGIN}\ncustom-shader = {shader_path}\n{BLOCK_END}");
    if let (Some(b), Some(e_start)) = (existing.find(BLOCK_BEGIN), existing.find(BLOCK_END)) {
        let end = e_start + BLOCK_END.len();
        if existing[b..end] == block {
            return (existing.to_string(), ConfigAction::AlreadyCurrent);
        }
        let mut out = String::with_capacity(existing.len() + block.len());
        out.push_str(&existing[..b]);
        out.push_str(&block);
        out.push_str(&existing[end..]);
        return (out, ConfigAction::Updated);
    }
    let mut out = existing.to_string();
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(&block);
    out.push('\n');
    (out, ConfigAction::Added)
}

/// The config variable name from a trailing `return <ident>` line (the common
/// `local config = ...; return config` shape). Returns `None` for inline
/// `return { ... }` tables, which can't be patched by variable.
fn trailing_return_var(existing: &str) -> Option<&str> {
    let last = existing
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with("--"))?;
    let ident = last
        .trim()
        .strip_prefix("return ")?
        .split_whitespace()
        .next()?;
    if ident == "{" {
        None
    } else {
        Some(ident)
    }
}

/// The full managed WezTerm block for `var`, e.g.
/// `-- >>> ... >>>\nconfig.shaders = { frag = '<path>' }\n-- <<< ... <<<`.
fn wez_block(var: &str, shader_path: &str) -> String {
    format!("{WEZ_BLOCK_BEGIN}\n{var}.shaders = {{ frag = '{shader_path}' }}\n{WEZ_BLOCK_END}")
}

/// Idempotently insert/update the managed `config.shaders` block in `existing`
/// WezTerm Lua config text. Pure (no I/O) for easy testing. Patches whatever
/// variable the user's config `return`s (`config`, `cfg`, …) by capturing it
/// from the trailing `return <ident>`; inline `return { ... }` configs can't be
/// patched safely and get an advisory comment instead
/// ([`ConfigAction::NeedsManual`]).
fn merge_wezterm_shader_block(existing: &str, shader_path: &str) -> (String, ConfigAction) {
    if let (Some(b), Some(e_start)) = (existing.find(WEZ_BLOCK_BEGIN), existing.find(WEZ_BLOCK_END))
    {
        let end = e_start + WEZ_BLOCK_END.len();
        // A managed block already exists — keep the current `return <ident>`
        // var so a user rename heals the block, else the block's own var.
        let var = trailing_return_var(existing).or_else(|| {
            existing[b..end]
                .lines()
                .nth(1)
                .and_then(|l| l.split_once('.').map(|(v, _)| v))
        });
        if let Some(var) = var {
            let block = wez_block(var, shader_path);
            if existing[b..end] == block {
                return (existing.to_string(), ConfigAction::AlreadyCurrent);
            }
            let mut out = String::with_capacity(existing.len() + block.len());
            out.push_str(&existing[..b]);
            out.push_str(&block);
            out.push_str(&existing[end..]);
            return (out, ConfigAction::Updated);
        }
    }

    if let Some(var) = trailing_return_var(existing) {
        let block = wez_block(var, shader_path);
        let needle = format!("return {var}");
        let pos = existing.rfind(&needle).unwrap_or(0);
        let line_start = existing[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let mut out = String::with_capacity(existing.len() + block.len() + 2);
        out.push_str(&existing[..line_start]);
        out.push_str(&block);
        out.push('\n');
        out.push('\n');
        out.push_str(&existing[line_start..]);
        return (out, ConfigAction::Added);
    }

    // Inline `return { ... }` (or no return at all): can't patch — append an
    // advisory comment only, and report NeedsManual.
    let mut out = existing.to_string();
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(&format!(
        "{WEZ_BLOCK_BEGIN}\n-- your config returns an inline table; add a shaders key by hand:\n--   shaders = {{ frag = '{shader_path}' }}\n{WEZ_BLOCK_END}\n"
    ));
    (out, ConfigAction::NeedsManual)
}

/// A fresh `wezterm.lua` written when the user has no WezTerm config yet: a
/// minimal config with the rain-on-glass shader wired in.
fn wezterm_created_config(shader_path: &Path) -> String {
    format!(
        "-- WezTerm config created by `entheai doctor` — rain-on-glass shader.\n\
         local wezterm = require 'wezterm'\n\
         local config = {{}}\n\
         \n\
         config.colors = {{ background = '#0f0e1d', foreground = '#e1cba6' }}\n\
         config.window_padding = {{ left = 14, right = 14, top = 10, bottom = 10 }}\n\
         \n\
         {WEZ_BLOCK_BEGIN}\n\
         config.shaders = {{ frag = '{}' }}\n\
         {WEZ_BLOCK_END}\n\
         return config\n",
        shader_path.display()
    )
}

/// True when the current terminal (by `$TERM_PROGRAM`) is `terminal`.
fn active_term(terminal: TerminalKind) -> bool {
    let tp = std::env::var("TERM_PROGRAM").unwrap_or_default();
    match terminal {
        TerminalKind::WezTerm => tp.eq_ignore_ascii_case("wezterm"),
        TerminalKind::Ghostty => tp.eq_ignore_ascii_case("ghostty"),
    }
}

/// Materialize the shader and merge it into the user's OWN terminal config,
/// reusing the launcher's bundled shader. The terminal is chosen by what's
/// installed: WezTerm (the primary layer) when its binary is on PATH, else
/// Ghostty. Only writes when something changed. Returns a report for display.
/// `ghostty_cfg` / `wezterm_cfg` are the two user-config paths to choose
/// between (injected so tests don't depend on the real env).
pub fn run_doctor(
    entheai_home: &Path,
    ghostty_cfg: &Path,
    wezterm_cfg: &Path,
) -> anyhow::Result<DoctorReport> {
    run_doctor_for(
        choose_terminal(resolve_wezterm().is_some()),
        entheai_home,
        ghostty_cfg,
        wezterm_cfg,
    )
}

/// [`run_doctor`] for an explicit terminal choice (testable without depending
/// on what's on `PATH`).
pub fn run_doctor_for(
    terminal: TerminalKind,
    entheai_home: &Path,
    ghostty_cfg: &Path,
    wezterm_cfg: &Path,
) -> anyhow::Result<DoctorReport> {
    let (shader_path, config_path, action) = match terminal {
        TerminalKind::WezTerm => {
            // Reuses the launcher's managed `wezterm-minimal.lua` + shader.
            let (_managed, shader_path) = materialize_wezterm_assets(entheai_home)?;
            let cfg = wezterm_cfg;
            if !cfg.exists() {
                if let Some(parent) = cfg.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(cfg, wezterm_created_config(&shader_path))?;
                (shader_path, cfg.to_path_buf(), ConfigAction::Created)
            } else {
                let existing = std::fs::read_to_string(cfg).unwrap_or_default();
                let (new_text, action) =
                    merge_wezterm_shader_block(&existing, &shader_path.display().to_string());
                if action != ConfigAction::AlreadyCurrent && action != ConfigAction::NeedsManual {
                    std::fs::write(cfg, new_text)?;
                }
                (shader_path, cfg.to_path_buf(), action)
            }
        }
        TerminalKind::Ghostty => {
            let shader_path = materialize_shader(entheai_home)?;
            let cfg = ghostty_cfg;
            let existed = cfg.exists();
            let existing = std::fs::read_to_string(cfg).unwrap_or_default();
            let (new_text, mut action) =
                merge_shader_block(&existing, &shader_path.display().to_string());
            if !existed && action != ConfigAction::AlreadyCurrent {
                action = ConfigAction::Created;
            }
            if action != ConfigAction::AlreadyCurrent {
                if let Some(parent) = cfg.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(cfg, new_text)?;
            }
            (shader_path, cfg.to_path_buf(), action)
        }
    };

    Ok(DoctorReport {
        terminal,
        wezterm_installed: resolve_wezterm().is_some(),
        ghostty_installed: resolve_ghostty().is_some(),
        is_active_term: active_term(terminal),
        shader_path,
        config_path,
        action,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materialize_writes_shader_and_renders_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let (config, shader) = materialize_assets(home).unwrap();

        assert!(shader.ends_with("shaders/rain_on_glass.glsl"));
        assert!(shader.is_absolute());
        assert!(std::fs::read_to_string(&shader)
            .unwrap()
            .contains("void mainImage"));

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
        assert!(std::fs::read_to_string(&s2)
            .unwrap()
            .contains("void mainImage"));
    }

    #[test]
    fn build_args_is_exact() {
        let cfg = Path::new("/Users/x/.config/entheai/ghostty-minimal.conf");
        let entheai = Path::new("/usr/local/bin/entheai");
        let cwd = Path::new("/Users/x/projects/demo");
        let args = build_args(cfg, entheai, cwd);
        assert_eq!(
            args,
            vec![
                "--config-default-files=false".to_string(),
                "--config-file=/Users/x/.config/entheai/ghostty-minimal.conf".to_string(),
                "-e".to_string(),
                "/bin/sh".to_string(),
                "-c".to_string(),
                "cd '/Users/x/projects/demo' && exec '/usr/local/bin/entheai'".to_string(),
            ]
        );
    }

    #[test]
    fn sh_quote_escapes_embedded_single_quotes() {
        // A directory whose name contains a single quote must not break the
        // `sh -c` string.
        assert_eq!(
            sh_quote(Path::new("/tmp/it's here")),
            r"'/tmp/it'\''s here'"
        );
    }

    #[test]
    fn resolve_ghostty_prefers_app_bundle_then_path() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Ghostty.app/Contents/MacOS");
        std::fs::create_dir_all(&app).unwrap();
        let bin = app.join("ghostty");
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();
        assert_eq!(
            resolve_ghostty_in(&[dir.path().join("Ghostty.app")]).as_deref(),
            Some(bin.as_path())
        );
        assert!(resolve_ghostty_in(&[dir.path().join("nope.app")]).is_none());
    }

    #[test]
    fn doctor_merge_appends_then_is_idempotent() {
        let cfg = "font-family = Berkeley Mono\nbackground = 05070d\n";
        let (t1, a1) = merge_shader_block(cfg, "/s/rain.glsl");
        assert_eq!(a1, ConfigAction::Added);
        assert!(
            t1.contains("font-family = Berkeley Mono"),
            "user config kept"
        );
        assert!(t1.contains("custom-shader = /s/rain.glsl"));
        // re-run with the same shader path → no change
        let (t2, a2) = merge_shader_block(&t1, "/s/rain.glsl");
        assert_eq!(a2, ConfigAction::AlreadyCurrent);
        assert_eq!(t1, t2);
        assert_eq!(
            t2.matches(BLOCK_BEGIN).count(),
            1,
            "exactly one managed block"
        );
    }

    #[test]
    fn doctor_merge_updates_on_path_change_preserving_surroundings() {
        let (t1, _) = merge_shader_block("keep = me\n", "/old/rain.glsl");
        let (t2, a2) = merge_shader_block(&t1, "/new/rain.glsl");
        assert_eq!(a2, ConfigAction::Updated);
        assert!(t2.contains("custom-shader = /new/rain.glsl"));
        assert!(!t2.contains("/old/rain.glsl"), "old path replaced");
        assert!(t2.contains("keep = me"), "surrounding config preserved");
        assert_eq!(t2.matches(BLOCK_BEGIN).count(), 1);
    }

    #[test]
    fn doctor_merge_into_empty_is_just_the_block() {
        let (t, a) = merge_shader_block("", "/s/rain.glsl");
        assert_eq!(a, ConfigAction::Added);
        assert!(t.starts_with(BLOCK_BEGIN) && t.contains("custom-shader = /s/rain.glsl"));
    }

    #[test]
    fn doctor_run_creates_config_then_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("entheai");
        let cfg = dir.path().join("ghostty/config");
        let wez = dir.path().join("wezterm/wezterm.lua");
        let r = run_doctor_for(TerminalKind::Ghostty, &home, &cfg, &wez).unwrap();
        assert_eq!(r.action, ConfigAction::Created);
        assert_eq!(r.terminal, TerminalKind::Ghostty);
        assert!(cfg.is_file());
        assert!(r.shader_path.ends_with("shaders/rain_on_glass.glsl"));
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert!(text.contains("custom-shader = "));
        assert!(text.contains(&r.shader_path.display().to_string()));
        // second run changes nothing
        let r2 = run_doctor_for(TerminalKind::Ghostty, &home, &cfg, &wez).unwrap();
        assert_eq!(r2.action, ConfigAction::AlreadyCurrent);
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), text);
    }

    // ── WezTerm (primary window layer, 8b-is fork) ──────────────────────────

    #[test]
    fn materialize_wezterm_writes_shader_and_renders_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let (config, shader) = materialize_wezterm_assets(home).unwrap();

        assert!(shader.ends_with("shaders/rain_on_glass_wezterm.frag"));
        assert!(shader.is_absolute());
        assert!(std::fs::read_to_string(&shader)
            .unwrap()
            .contains("out vec4 ocolor"));

        let lua = std::fs::read_to_string(&config).unwrap();
        assert!(
            lua.trim_start()
                .starts_with("local wezterm = require 'wezterm'"),
            "a WezTerm Lua config opens with the wezterm module"
        );
        assert!(!lua.contains("{{SHADER}}"), "placeholder was rendered");
        assert!(lua.contains(&format!("frag = '{}'", shader.display())));
        assert!(lua.contains("return config"));
    }

    #[test]
    fn materialize_wezterm_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let (c1, s1) = materialize_wezterm_assets(dir.path()).unwrap();
        let (c2, s2) = materialize_wezterm_assets(dir.path()).unwrap();
        assert_eq!(c1, c2);
        assert_eq!(s1, s2);
    }

    #[test]
    fn build_wezterm_args_is_exact() {
        let cfg = Path::new("/Users/x/.config/entheai/wezterm-minimal.lua");
        let entheai = Path::new("/usr/local/bin/entheai");
        let cwd = Path::new("/Users/x/projects/demo");
        let args = build_wezterm_args(cfg, entheai, cwd);
        assert_eq!(
            args,
            vec![
                "start".to_string(),
                "--config-file=/Users/x/.config/entheai/wezterm-minimal.lua".to_string(),
                "--cwd=/Users/x/projects/demo".to_string(),
                "--".to_string(),
                "/usr/local/bin/entheai".to_string(),
            ]
        );
    }

    #[test]
    fn choose_terminal_prefers_wezterm_when_present() {
        // WezTerm installed → WezTerm (the 8b-is fork is the primary layer).
        assert_eq!(choose_terminal(true), TerminalKind::WezTerm);
        // No WezTerm → Ghostty fallback.
        assert_eq!(choose_terminal(false), TerminalKind::Ghostty);
        assert_ne!(
            TerminalKind::WezTerm,
            TerminalKind::Ghostty,
            "distinct kinds"
        );
        assert_eq!(TerminalKind::WezTerm.name(), "WezTerm");
        assert_eq!(TerminalKind::Ghostty.name(), "Ghostty");
    }

    #[test]
    fn wezterm_doctor_merge_appends_then_is_idempotent() {
        let cfg = "local wezterm = require 'wezterm'\nlocal config = {}\nconfig.font_size = 13\nreturn config\n";
        let (t1, a1) = merge_wezterm_shader_block(cfg, "/s/rain_wez.frag");
        assert_eq!(a1, ConfigAction::Added);
        assert!(t1.contains("config.font_size = 13"), "user config kept");
        assert!(t1.contains("config.shaders = { frag = '/s/rain_wez.frag' }"));
        // the block lands before `return config`
        let ret = t1.rfind("return config").unwrap();
        let block = t1.rfind(WEZ_BLOCK_BEGIN).unwrap();
        assert!(block < ret, "managed block precedes the return");
        // re-run with the same shader path → no change
        let (t2, a2) = merge_wezterm_shader_block(&t1, "/s/rain_wez.frag");
        assert_eq!(a2, ConfigAction::AlreadyCurrent);
        assert_eq!(t1, t2);
        assert_eq!(t2.matches(WEZ_BLOCK_BEGIN).count(), 1);
    }

    #[test]
    fn wezterm_doctor_merge_patches_returned_var_name() {
        // `return cfg` — the block must set `cfg.shaders`, not `config`.
        let cfg = "local wezterm = require 'wezterm'\nlocal cfg = wezterm.config_builder()\ncfg.default_cwd = '/work'\nreturn cfg\n";
        let (t1, a1) = merge_wezterm_shader_block(cfg, "/s/rain_wez.frag");
        assert_eq!(a1, ConfigAction::Added);
        assert!(t1.contains("cfg.shaders = { frag = '/s/rain_wez.frag' }"));
        assert!(!t1.contains("config.shaders"));
        // path change → updated, still on the right var
        let (t2, a2) = merge_wezterm_shader_block(&t1, "/new/rain_wez.frag");
        assert_eq!(a2, ConfigAction::Updated);
        assert!(t2.contains("cfg.shaders = { frag = '/new/rain_wez.frag' }"));
        assert_eq!(t2.matches(WEZ_BLOCK_BEGIN).count(), 1);
    }

    #[test]
    fn wezterm_doctor_merge_inline_table_is_needs_manual() {
        // `return { ... }` inline table — can't patch by variable.
        let cfg = "local wezterm = require 'wezterm'\nreturn {\n  font_size = 13,\n}\n";
        let (t, a) = merge_wezterm_shader_block(cfg, "/s/rain_wez.frag");
        assert_eq!(a, ConfigAction::NeedsManual);
        assert!(t.contains("add a shaders key by hand"));
        assert!(t.contains("shaders = { frag = '/s/rain_wez.frag' }"));
    }

    #[test]
    fn wezterm_doctor_run_creates_config_then_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("entheai");
        let ghostty_cfg = dir.path().join("ghostty/config");
        let wez = dir.path().join("wezterm/wezterm.lua");
        let r = run_doctor_for(TerminalKind::WezTerm, &home, &ghostty_cfg, &wez).unwrap();
        assert_eq!(r.action, ConfigAction::Created);
        assert_eq!(r.terminal, TerminalKind::WezTerm);
        assert!(wez.is_file());
        assert!(r
            .shader_path
            .ends_with("shaders/rain_on_glass_wezterm.frag"));
        let text = std::fs::read_to_string(&wez).unwrap();
        assert!(text.contains("config.shaders = { frag = "));
        assert!(text.contains(&r.shader_path.display().to_string()));
        assert!(text.contains("return config"));
        // second run changes nothing
        let r2 = run_doctor_for(TerminalKind::WezTerm, &home, &ghostty_cfg, &wez).unwrap();
        assert_eq!(r2.action, ConfigAction::AlreadyCurrent);
        assert_eq!(std::fs::read_to_string(&wez).unwrap(), text);
    }
}
