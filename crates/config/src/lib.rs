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
    #[serde(default)]
    pub inference: InferenceConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub permission: PermissionConfig,
    #[serde(default)]
    pub mcp_defaults: McpDefaultsConfig,
    #[serde(default)]
    pub radio: RadioConfig,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub obsidian: ObsidianConfig,
    #[serde(default)]
    pub nats: NatsConfig,
    #[serde(default)]
    pub federation: FederationConfig,
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
    /// Max number of turns the orchestrator may take before it's cut off.
    #[serde(default = "default_max_turns")]
    pub max_turns: usize,
    /// Override for the orchestrator's system prompt.
    #[serde(default)]
    pub orchestrator_prompt: Option<String>,
    /// Text appended to the orchestrator's system prompt.
    #[serde(default)]
    pub orchestrator_prompt_append: Option<String>,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            orchestrator: None,
            max_parallel: default_max_parallel(),
            max_turns: default_max_turns(),
            orchestrator_prompt: None,
            orchestrator_prompt_append: None,
        }
    }
}

fn default_max_parallel() -> usize {
    8
}

fn default_max_turns() -> usize {
    50
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
    /// TCP port the companion server listens on. Default: 9876.
    #[serde(default = "default_companion_port")]
    pub port: u16,
    /// Target render frame rate for the companion window. Default: 24.0.
    #[serde(default = "default_companion_fps")]
    pub fps: f64,
}

fn default_true() -> bool {
    true
}

fn default_companion_port() -> u16 {
    9876
}

fn default_companion_fps() -> f64 {
    24.0
}

impl Default for CompanionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            always_on_top: true,
            port: default_companion_port(),
            fps: default_companion_fps(),
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
    /// Viz render tick interval in milliseconds. Default: 90.
    #[serde(default = "default_viz_tick_ms")]
    pub tick_ms: u64,
    /// Max rows shown in the plan pane. Default: 8.
    #[serde(default = "default_viz_plan_rows_cap")]
    pub plan_rows_cap: u16,
    /// Max rows shown in the swarm pane. Default: 8.
    #[serde(default = "default_viz_swarm_rows_cap")]
    pub swarm_rows_cap: u16,
}

fn default_viz_swarm() -> bool {
    true
}

fn default_viz_tick_ms() -> u64 {
    90
}

fn default_viz_plan_rows_cap() -> u16 {
    8
}

fn default_viz_swarm_rows_cap() -> u16 {
    8
}

impl Default for VizConfig {
    fn default() -> Self {
        Self {
            swarm: default_viz_swarm(),
            tick_ms: default_viz_tick_ms(),
            plan_rows_cap: default_viz_plan_rows_cap(),
            swarm_rows_cap: default_viz_swarm_rows_cap(),
        }
    }
}

/// Provider request defaults (applied to every LLM call).
#[derive(Debug, Clone, Deserialize)]
pub struct InferenceConfig {
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default = "default_retries")]
    pub retries: u32,
}
fn default_request_timeout_secs() -> u64 {
    120
}
fn default_retries() -> u32 {
    2
}
impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            request_timeout_secs: default_request_timeout_secs(),
            max_tokens: None,
            temperature: None,
            retries: default_retries(),
        }
    }
}

/// Built-in tool caps.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolsConfig {
    #[serde(default = "default_shell_timeout_secs")]
    pub shell_timeout_secs: u64,
    #[serde(default = "default_shell_output_cap")]
    pub shell_output_cap: usize,
    #[serde(default = "default_search_max_results")]
    pub search_max_results: usize,
}
fn default_shell_timeout_secs() -> u64 {
    120
}
fn default_shell_output_cap() -> usize {
    100_000
}
fn default_search_max_results() -> usize {
    200
}
impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            shell_timeout_secs: default_shell_timeout_secs(),
            shell_output_cap: default_shell_output_cap(),
            search_max_results: default_search_max_results(),
        }
    }
}

/// Permission policy defaults.
#[derive(Debug, Clone, Deserialize)]
pub struct PermissionConfig {
    #[serde(default)]
    pub yolo: bool,
    #[serde(default)]
    pub allowlist: Vec<String>,
    #[serde(default = "default_fanout_auto_approve")]
    pub fanout_auto_approve: bool,
}
fn default_fanout_auto_approve() -> bool {
    true
}
impl Default for PermissionConfig {
    fn default() -> Self {
        Self {
            yolo: false,
            allowlist: Vec::new(),
            fanout_auto_approve: default_fanout_auto_approve(),
        }
    }
}

/// Cross-cutting MCP settings (siblings of the per-server `[mcp.<name>]` map).
#[derive(Debug, Clone, Deserialize)]
pub struct McpDefaultsConfig {
    #[serde(default = "default_mcp_spawn_timeout_secs")]
    pub spawn_timeout_secs: u64,
}
fn default_mcp_spawn_timeout_secs() -> u64 {
    10
}
impl Default for McpDefaultsConfig {
    fn default() -> Self {
        Self {
            spawn_timeout_secs: default_mcp_spawn_timeout_secs(),
        }
    }
}

/// Radio (background music) settings.
#[derive(Debug, Clone, Deserialize)]
pub struct RadioConfig {
    #[serde(default = "default_radio_download_timeout_secs")]
    pub download_timeout_secs: u64,
}
fn default_radio_download_timeout_secs() -> u64 {
    300
}
impl Default for RadioConfig {
    fn default() -> Self {
        Self {
            download_timeout_secs: default_radio_download_timeout_secs(),
        }
    }
}

/// Telemetry / crash reporting.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TelemetryConfig {
    #[serde(default)]
    pub sentry_dsn: Option<String>,
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
    fn obsidian_config_defaults() {
        let cfg = Config::from_toml_str("").unwrap();
        assert!(
            cfg.obsidian.enabled,
            "obsidian on by default (no-op unless a vault resolves)"
        );
        assert_eq!(cfg.obsidian.vault_path, "");
        assert_eq!(cfg.obsidian.subtree, "entheai-sync");
        assert_eq!(cfg.obsidian.debounce_ms, 500);
        assert!(cfg.obsidian.mcp_nudge);
        assert_eq!(cfg.obsidian.mcp_port, 22360);
        assert!(cfg.obsidian.include_architecture);
        assert!(cfg.obsidian.include_sessions);
        assert_eq!(
            cfg.obsidian.watch,
            vec![
                "docs",
                ".remember",
                "README.md",
                "AGENTS.md",
                "CHANGELOG.md",
                "VERSIONING.md"
            ]
        );
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

    #[test]
    fn refactor_config_defaults() {
        let cfg = Config::from_toml_str("").unwrap();
        assert_eq!(cfg.router.max_turns, 50);
        assert!(cfg.router.orchestrator_prompt.is_none());
        assert!(cfg.router.orchestrator_prompt_append.is_none());
        assert_eq!(cfg.inference.request_timeout_secs, 120);
        assert!(cfg.inference.max_tokens.is_none());
        assert!(cfg.inference.temperature.is_none());
        assert_eq!(cfg.inference.retries, 2);
        assert_eq!(cfg.tools.shell_timeout_secs, 120);
        assert_eq!(cfg.tools.shell_output_cap, 100_000);
        assert_eq!(cfg.tools.search_max_results, 200);
        assert!(!cfg.permission.yolo);
        assert!(cfg.permission.allowlist.is_empty());
        assert!(cfg.permission.fanout_auto_approve);
        assert_eq!(cfg.mcp_defaults.spawn_timeout_secs, 10);
        assert_eq!(cfg.memory.embed_timeout_secs, 30);
        assert_eq!(cfg.viz.tick_ms, 90);
        assert_eq!(cfg.viz.plan_rows_cap, 8);
        assert_eq!(cfg.viz.swarm_rows_cap, 8);
        assert_eq!(cfg.companion.port, 9876);
        assert_eq!(cfg.companion.fps, 24.0);
        assert_eq!(cfg.radio.download_timeout_secs, 300);
        assert!(cfg.telemetry.sentry_dsn.is_none());
    }

    #[test]
    fn refactor_config_overrides_parse() {
        let cfg = Config::from_toml_str(
            "[router]\nmax_turns = 10\n[inference]\nrequest_timeout_secs = 5\nmax_tokens = 2048\ntemperature = 0.2\n[permission]\nfanout_auto_approve = false\n[viz]\ntick_ms = 33\n",
        )
        .unwrap();
        assert_eq!(cfg.router.max_turns, 10);
        assert_eq!(cfg.inference.request_timeout_secs, 5);
        assert_eq!(cfg.inference.max_tokens, Some(2048));
        assert_eq!(cfg.inference.temperature, Some(0.2));
        assert!(!cfg.permission.fanout_auto_approve);
        assert_eq!(cfg.viz.tick_ms, 33);
    }

    #[test]
    fn nats_defaults_off_with_standard_env_names() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(!cfg.nats.enabled);
        assert_eq!(cfg.nats.url_env, "NATS_URL");
        assert_eq!(cfg.nats.token_env, "NATS_TOKEN");
    }

    #[test]
    fn nats_block_parses_and_overrides() {
        let cfg: Config = toml::from_str(
            r#"
            [nats]
            enabled = true
            url_env = "MY_NATS_URL"
            token_env = "MY_NATS_TOKEN"
            "#,
        )
        .unwrap();
        assert!(cfg.nats.enabled);
        assert_eq!(cfg.nats.url_env, "MY_NATS_URL");
        assert_eq!(cfg.nats.token_env, "MY_NATS_TOKEN");
    }

    #[test]
    fn federation_defaults_off() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(!cfg.federation.enabled);
        assert_eq!(cfg.federation.role, "auto");
        assert_eq!(cfg.federation.deadline_secs, 600);
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
    #[serde(default = "default_embed_timeout_secs")]
    pub embed_timeout_secs: u64,
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
fn default_embed_timeout_secs() -> u64 {
    30
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
            embed_timeout_secs: default_embed_timeout_secs(),
        }
    }
}

/// Obsidian wiki-sync layer configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ObsidianConfig {
    #[serde(default = "default_obsidian_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub vault_path: String,
    #[serde(default = "default_obsidian_subtree")]
    pub subtree: String,
    #[serde(default = "default_obsidian_watch")]
    pub watch: Vec<String>,
    #[serde(default = "default_obsidian_debounce_ms")]
    pub debounce_ms: u64,
    #[serde(default = "default_true")]
    pub mcp_nudge: bool,
    #[serde(default = "default_obsidian_mcp_port")]
    pub mcp_port: u16,
    #[serde(default = "default_true")]
    pub include_architecture: bool,
    #[serde(default = "default_true")]
    pub include_sessions: bool,
}

fn default_obsidian_enabled() -> bool {
    true
}
fn default_obsidian_subtree() -> String {
    "entheai-sync".into()
}
fn default_obsidian_debounce_ms() -> u64 {
    500
}
fn default_obsidian_mcp_port() -> u16 {
    22360
}
fn default_obsidian_watch() -> Vec<String> {
    [
        "docs",
        ".remember",
        "README.md",
        "AGENTS.md",
        "CHANGELOG.md",
        "VERSIONING.md",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

impl Default for ObsidianConfig {
    fn default() -> Self {
        Self {
            enabled: default_obsidian_enabled(),
            vault_path: String::new(),
            subtree: default_obsidian_subtree(),
            watch: default_obsidian_watch(),
            debounce_ms: default_obsidian_debounce_ms(),
            mcp_nudge: true,
            mcp_port: default_obsidian_mcp_port(),
            include_architecture: true,
            include_sessions: true,
        }
    }
}

/// Federation event bus (`entheai-bus`, F1). Opt-in and fail-safe: with
/// `enabled = false` (the default) or an unreachable hub, entheai runs entirely
/// locally. The URL and token are read from the named environment variables
/// (populated from the gitignored `.env`), never inlined in the tracked config.
#[derive(Debug, Clone, Deserialize)]
pub struct NatsConfig {
    /// Master switch. When false, `Bus::connect` short-circuits to `None`.
    #[serde(default)]
    pub enabled: bool,
    /// Name of the env var holding the NATS URL (e.g. `nats://host:4222`).
    #[serde(default = "default_nats_url_env")]
    pub url_env: String,
    /// Name of the env var holding the NATS auth token.
    #[serde(default = "default_nats_token_env")]
    pub token_env: String,
}

impl Default for NatsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url_env: default_nats_url_env(),
            token_env: default_nats_token_env(),
        }
    }
}

fn default_nats_url_env() -> String {
    "NATS_URL".to_string()
}

fn default_nats_token_env() -> String {
    "NATS_TOKEN".to_string()
}

/// Distributed swarm (F2). Opt-in; reuses `[nats]` for the connection. `role`
/// selects whether this process dispatches work, serves as a worker, or both.
#[derive(Debug, Clone, Deserialize)]
pub struct FederationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_fed_role")]
    pub role: String, // "auto" | "worker" | "dispatch"
    #[serde(default = "default_fed_deadline_secs")]
    pub deadline_secs: u64,
}

impl Default for FederationConfig {
    fn default() -> Self {
        Self { enabled: false, role: default_fed_role(), deadline_secs: default_fed_deadline_secs() }
    }
}

fn default_fed_role() -> String { "auto".to_string() }
fn default_fed_deadline_secs() -> u64 { 600 }
