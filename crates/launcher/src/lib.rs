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
    let shader_abs = materialize_shader(home)?;
    let config = CONFIG_TMPL.replace("{{SHADER}}", &shader_abs.display().to_string());
    let config_path = home.join("ghostty-minimal.conf");
    std::fs::write(&config_path, config)?;

    Ok((config_path, shader_abs))
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
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|p| p.join("ghostty"))
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
    std::process::Command::new(&ghostty).args(&args).spawn()?;
    Ok(())
}

// ── `entheai doctor` (viz Slice 2b) ──────────────────────────────────────────
// Install the rain-on-glass shader into a user's OWN Ghostty config (Path A —
// Ghostty `custom-shader`). The Path-C ANSI ambient fallback for non-Ghostty
// terminals is a separate follow-up.

const BLOCK_BEGIN: &str = "# >>> entheai raindrop shader — managed by `entheai doctor` >>>";
const BLOCK_END: &str = "# <<< entheai raindrop shader <<<";

/// What [`run_doctor`] did to the Ghostty config.
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
}

/// Summary of one `entheai doctor` run, for display.
#[derive(Debug, Clone)]
pub struct DoctorReport {
    pub is_ghostty_term: bool,
    pub ghostty_installed: bool,
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

/// Materialize the shader and merge it into `config_path` (the user's Ghostty
/// config), reusing the launcher's bundled shader. Only writes when something
/// changed. Returns a report for display.
pub fn run_doctor(entheai_home: &Path, config_path: &Path) -> anyhow::Result<DoctorReport> {
    let shader_path = materialize_shader(entheai_home)?;
    let existed = config_path.exists();
    let existing = std::fs::read_to_string(config_path).unwrap_or_default();
    let (new_text, mut action) = merge_shader_block(&existing, &shader_path.display().to_string());
    if !existed && action != ConfigAction::AlreadyCurrent {
        action = ConfigAction::Created;
    }
    if action != ConfigAction::AlreadyCurrent {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(config_path, new_text)?;
    }
    Ok(DoctorReport {
        is_ghostty_term: std::env::var("TERM_PROGRAM").ok().as_deref() == Some("ghostty"),
        ghostty_installed: resolve_ghostty().is_some(),
        shader_path,
        config_path: config_path.to_path_buf(),
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
        assert!(t1.contains("font-family = Berkeley Mono"), "user config kept");
        assert!(t1.contains("custom-shader = /s/rain.glsl"));
        // re-run with the same shader path → no change
        let (t2, a2) = merge_shader_block(&t1, "/s/rain.glsl");
        assert_eq!(a2, ConfigAction::AlreadyCurrent);
        assert_eq!(t1, t2);
        assert_eq!(t2.matches(BLOCK_BEGIN).count(), 1, "exactly one managed block");
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
        let r = run_doctor(&home, &cfg).unwrap();
        assert_eq!(r.action, ConfigAction::Created);
        assert!(cfg.is_file());
        assert!(r.shader_path.ends_with("shaders/rain_on_glass.glsl"));
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert!(text.contains("custom-shader = "));
        assert!(text.contains(&r.shader_path.display().to_string()));
        // second run changes nothing
        let r2 = run_doctor(&home, &cfg).unwrap();
        assert_eq!(r2.action, ConfigAction::AlreadyCurrent);
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), text);
    }
}
