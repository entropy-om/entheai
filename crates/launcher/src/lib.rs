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

    let shader_abs = shader_path
        .canonicalize()
        .unwrap_or_else(|_| shader_path.clone());
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
}
