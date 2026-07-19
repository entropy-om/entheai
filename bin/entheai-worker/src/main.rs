use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use entheai_config::Config;

#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Path to the entheai TOML config (same format as the main binary's).
    #[arg(long, default_value = "entheai.toml")]
    config: String,
    /// The sub-agent role (routes to a model via `[agents.<role>]`).
    #[arg(long)]
    role: String,
    /// The sub-agent's task description.
    #[arg(long)]
    task: String,
    /// Path to the isolated git worktree this coder should run against.
    #[arg(long)]
    worktree: PathBuf,
}

/// Whether `output` (a coder's captured result text) indicates the sub-agent
/// failed, mirroring `entheai_orchestrator::run_coder_once`'s error-capture
/// convention (`"error: coder failed: {e}"`).
fn is_error_output(output: &str) -> bool {
    output.starts_with("error:")
}

/// Render a coder's result as the single JSON line printed to stdout.
fn render_result(role: &str, task: &str, output: &str) -> String {
    serde_json::json!({ "role": role, "task": task, "output": output }).to_string()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cfg_text = std::fs::read_to_string(&cli.config)
        .with_context(|| format!("reading config {}", cli.config))?;
    let config = Config::from_toml_str(&cfg_text)?;

    let output =
        entheai_orchestrator::run_coder_once(&config, &cli.role, &cli.task, &cli.worktree).await;
    println!("{}", render_result(&cli.role, &cli.task, &output));
    if is_error_output(&output) {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_error_output_detects_the_capture_convention() {
        assert!(is_error_output("error: coder failed: boom"));
        assert!(!is_error_output("added a null check"));
    }

    #[test]
    fn render_result_produces_valid_json_with_expected_fields() {
        let json = render_result("coder", "add x", "did the thing");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["role"], "coder");
        assert_eq!(parsed["task"], "add x");
        assert_eq!(parsed["output"], "did the thing");
    }
}
