use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to parse config TOML: {0}")]
    Toml(#[from] toml::de::Error),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub companion: CompanionConfig,
    #[serde(default)]
    pub router: RouterConfig,
    #[serde(default)]
    pub agents: HashMap<String, AgentConfig>,
    #[serde(default)]
    pub fanout: FanoutConfig,
    #[serde(default)]
    pub mcp: std::collections::HashMap<String, McpServerConfig>,
    #[serde(default)]
    pub skills: SkillsConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub viz: VizConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub base_url: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RouterConfig {
    /// Model id ("<provider>/<model>") for the orchestrator role. Falls back
    /// to `default_model` when unset.
    #[serde(default)]
    pub orchestrator: Option<String>,
    /// Max number of sub-agents that may run concurrently during fan-out.
    #[serde(default = "default_max_parallel")]
    pub max_parallel: usize,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            orchestrator: None,
            max_parallel: default_max_parallel(),
        }
    }
}

fn default_max_parallel() -> usize {
    8
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AgentConfig {
    /// Preference-ordered model ids ("<provider>/<model>") for this role.
    #[serde(default)]
    pub model: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FanoutConfig {
    /// Shell command run inside each coder's worktree to decide whether its
    /// changes are integrated (e.g. "cargo test"). Unset = integrate all
    /// changed branches without verifying.
    #[serde(default)]
    pub verify: Option<String>,
    /// Per-coder timeout in seconds before it's force-aborted — a hung coder
    /// must not block the rest of the fan-out batch. Default: 600 (10 min).
    #[serde(default = "default_coder_timeout_secs")]
    pub coder_timeout_secs: u64,
}

impl Default for FanoutConfig {
    fn default() -> Self {
        Self {
            verify: None,
            coder_timeout_secs: default_coder_timeout_secs(),
        }
    }
}

fn default_coder_timeout_secs() -> u64 {
    600
}

/// One MCP server entheai spawns at startup; its tools are exposed to the agent.
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompanionConfig {
    /// Whether to spawn the companion window. Default: true.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Whether the companion floats above other windows. Default: true.
    #[serde(default = "default_true")]
    pub always_on_top: bool,
}

fn default_true() -> bool {
    true
}

impl Default for CompanionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            always_on_top: true,
        }
    }
}

/// Skill discovery directories (relative to the working root).
#[derive(Debug, Clone, Deserialize)]
pub struct SkillsConfig {
    #[serde(default = "default_skill_dirs")]
    pub dirs: Vec<String>,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            dirs: default_skill_dirs(),
        }
    }
}

fn default_skill_dirs() -> Vec<String> {
    vec!["skills".to_string()]
}

/// Visualization settings (viz pillar).
#[derive(Debug, Clone, Deserialize)]
pub struct VizConfig {
    /// Show the live fan-out swarm (inline pane + Ctrl-V full view).
    #[serde(default = "default_viz_swarm")]
    pub swarm: bool,
}

fn default_viz_swarm() -> bool {
    true
}

impl Default for VizConfig {
    fn default() -> Self {
        Self {
            swarm: default_viz_swarm(),
        }
    }
}

impl Config {
    pub fn from_toml_str(s: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_providers_and_default_model() {
        let cfg = Config::from_toml_str(
            r#"
            default_model = "osaurus/qwen3-coder"

            [providers.osaurus]
            base_url = "http://127.0.0.1:1337/v1"

            [providers.zen]
            base_url = "https://opencode.ai/zen/v1"
            api_key_env = "OPENCODE_API_KEY"
            "#,
        )
        .unwrap();

        assert_eq!(cfg.default_model.as_deref(), Some("osaurus/qwen3-coder"));
        assert_eq!(
            cfg.providers["osaurus"].base_url,
            "http://127.0.0.1:1337/v1"
        );
        assert_eq!(cfg.providers["osaurus"].api_key_env, None);
        assert_eq!(
            cfg.providers["zen"].api_key_env.as_deref(),
            Some("OPENCODE_API_KEY")
        );
    }

    #[test]
    fn parses_router_and_agents_when_present() {
        let cfg = Config::from_toml_str(
            r#"
            [router]
            orchestrator = "zen/deepseek-v4-pro"
            max_parallel = 4

            [agents.coder]
            model = ["deepseek/deepseek-chat"]
            "#,
        )
        .unwrap();

        assert_eq!(
            cfg.router.orchestrator.as_deref(),
            Some("zen/deepseek-v4-pro")
        );
        assert_eq!(cfg.router.max_parallel, 4);
        assert_eq!(
            cfg.agents["coder"].model,
            vec!["deepseek/deepseek-chat".to_string()]
        );
    }

    #[test]
    fn router_and_agents_default_when_absent() {
        let cfg = Config::from_toml_str(
            r#"
            default_model = "osaurus/qwen3-coder"
            "#,
        )
        .unwrap();

        assert_eq!(cfg.router.orchestrator, None);
        assert_eq!(cfg.router.max_parallel, 8);
        assert!(cfg.agents.is_empty());
    }

    #[test]
    fn parses_fanout_verify_when_present() {
        let cfg = Config::from_toml_str(
            r#"
            [fanout]
            verify = "cargo test"
            "#,
        )
        .unwrap();

        assert_eq!(cfg.fanout.verify.as_deref(), Some("cargo test"));
    }

    #[test]
    fn fanout_verify_defaults_to_none() {
        let cfg = Config::from_toml_str(
            r#"
            default_model = "osaurus/qwen3-coder"
            "#,
        )
        .unwrap();

        assert_eq!(cfg.fanout.verify, None);
    }

    #[test]
    fn parses_fanout_coder_timeout_secs_when_present() {
        let cfg = Config::from_toml_str(
            r#"
            [fanout]
            coder_timeout_secs = 120
            "#,
        )
        .unwrap();

        assert_eq!(cfg.fanout.coder_timeout_secs, 120);
    }

    #[test]
    fn fanout_coder_timeout_secs_defaults_to_600() {
        let cfg = Config::from_toml_str(
            r#"
            default_model = "osaurus/qwen3-coder"
            "#,
        )
        .unwrap();

        assert_eq!(cfg.fanout.coder_timeout_secs, 600);
    }

    #[test]
    fn parses_mcp_servers_when_present() {
        let cfg = Config::from_toml_str(
            r#"
            [mcp.codebase]
            command = "codebase-memory-mcp"
            args = ["--root", "."]
            "#,
        )
        .unwrap();

        assert_eq!(cfg.mcp["codebase"].command, "codebase-memory-mcp");
        assert_eq!(
            cfg.mcp["codebase"].args,
            vec!["--root".to_string(), ".".to_string()]
        );
    }

    #[test]
    fn mcp_defaults_to_empty_map_when_absent() {
        let cfg = Config::from_toml_str(
            r#"
            default_model = "osaurus/qwen3-coder"
            "#,
        )
        .unwrap();

        assert!(cfg.mcp.is_empty());
    }

    #[test]
    fn parses_skills_dirs_when_present() {
        let cfg = Config::from_toml_str(
            r#"
            [skills]
            dirs = ["a", "b"]
            "#,
        )
        .unwrap();

        assert_eq!(cfg.skills.dirs, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn skills_dirs_defaults_to_skills_when_absent() {
        let cfg = Config::from_toml_str(
            r#"
            default_model = "osaurus/qwen3-coder"
            "#,
        )
        .unwrap();

        assert_eq!(cfg.skills.dirs, vec!["skills".to_string()]);
    }

    #[test]
    fn memory_config_defaults() {
        let cfg = Config::from_toml_str("").unwrap();
        assert!(cfg.memory.enabled, "memory is on by default in v1");
        assert_eq!(cfg.memory.path, "~/.cache/entheai/memory.db");
        assert!((cfg.memory.w_recency - 0.3).abs() < 1e-9);
        assert!((cfg.memory.half_life_days - 14.0).abs() < 1e-9);
        assert_eq!(cfg.memory.rrf_k, 60.0);
        assert_eq!(cfg.memory.recall_overfetch, 3);
    }

    #[test]
    fn viz_config_defaults() {
        let cfg = Config::from_toml_str("").unwrap();
        assert!(cfg.viz.swarm, "the swarm is on by default");
    }

    #[test]
    fn viz_swarm_can_be_disabled() {
        let cfg = Config::from_toml_str("[viz]\nswarm = false\n").unwrap();
        assert!(!cfg.viz.swarm);
    }
}

/// Memory configuration per the SOTA memory design spec.
#[derive(Debug, Clone, Deserialize)]
pub struct MemoryConfig {
    #[serde(default = "default_memory_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub strict: bool,
    #[serde(default = "default_memory_path")]
    pub path: String,
    #[serde(default)]
    pub embed_provider: Option<String>,
    #[serde(default = "default_embed_model")]
    pub embed_model: String,
    #[serde(default = "default_retrieve_codebase")]
    pub retrieve_codebase: usize,
    #[serde(default = "default_retrieve_learnings")]
    pub retrieve_learnings: usize,
    #[serde(default = "default_retrieve_trajectories")]
    pub retrieve_trajectories: usize,
    #[serde(default = "default_max_context_chars")]
    pub max_context_chars: usize,
    #[serde(default = "default_tool_spill_chars")]
    pub tool_spill_chars: usize,
    #[serde(default)]
    pub evidence_tools: Vec<String>,
    #[serde(default = "default_w_recency")]
    pub w_recency: f64,
    #[serde(default = "default_w_conf")]
    pub w_conf: f64,
    #[serde(default = "default_half_life_days")]
    pub half_life_days: f64,
    #[serde(default = "default_rrf_k")]
    pub rrf_k: f64,
    #[serde(default = "default_recall_overfetch")]
    pub recall_overfetch: usize,
}

fn default_memory_enabled() -> bool {
    true
}
fn default_memory_path() -> String {
    "~/.cache/entheai/memory.db".into()
}
fn default_embed_model() -> String {
    "nomic-embed-text".into()
}
fn default_retrieve_codebase() -> usize {
    4
}
fn default_retrieve_learnings() -> usize {
    6
}
fn default_retrieve_trajectories() -> usize {
    3
}
fn default_max_context_chars() -> usize {
    12_000
}
fn default_tool_spill_chars() -> usize {
    8_000
}
fn default_w_recency() -> f64 {
    0.3
}
fn default_w_conf() -> f64 {
    0.2
}
fn default_half_life_days() -> f64 {
    14.0
}
fn default_rrf_k() -> f64 {
    60.0
}
fn default_recall_overfetch() -> usize {
    3
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: default_memory_enabled(),
            strict: false,
            path: default_memory_path(),
            embed_provider: None,
            embed_model: default_embed_model(),
            retrieve_codebase: default_retrieve_codebase(),
            retrieve_learnings: default_retrieve_learnings(),
            retrieve_trajectories: default_retrieve_trajectories(),
            max_context_chars: default_max_context_chars(),
            tool_spill_chars: default_tool_spill_chars(),
            evidence_tools: vec!["run_shell".into(), "search".into()],
            w_recency: default_w_recency(),
            w_conf: default_w_conf(),
            half_life_days: default_half_life_days(),
            rrf_k: default_rrf_k(),
            recall_overfetch: default_recall_overfetch(),
        }
    }
}
