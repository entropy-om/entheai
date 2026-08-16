use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to parse config TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid config: {0}")]
    Invalid(String),
}

/// The accepted `[fanout].executor` values.
pub const FANOUT_EXECUTORS: [&str; 4] = ["auto", "local", "agy", "copilot"];

/// Built-in configuration used by the CLI and the MCP server when no
/// `entheai.toml` is found in the working directory or `~/.config/entheai/`.
/// DeepSeek V4 all the way down: V4 Flash for the interactive default, V4 Pro
/// for the fan-out orchestrator (per-role tiers and their Gemini / OpenRouter
/// fallbacks come from `entheai_router`'s built-in chains), with the deepseek /
/// gemini / openrouter / vaked providers injected by [`Config::from_toml_str`].
/// Needs `DEEPSEEK_API_KEY` in the environment / `.env`; without it the
/// interactive run errors loudly, while fan-out degrades to the free public
/// vaked node (`vaked/qwen3-coder:30b`, keyless, CPU-slow). Pass
/// `--model vaked/qwen3-coder:30b` to run keyless interactively. Deliberately
/// omits user-specific MCP servers/paths.
pub const BUILTIN_CONFIG_TOML: &str = r#"default_model = "deepseek/deepseek-v4-flash"

# Local Osaurus (MLX) inference, keyless — declared so `osaurus/<model>` ids
# resolve out of the box; deepseek / gemini / openrouter / vaked are built in.
[providers.osaurus]
base_url = "http://127.0.0.1:1337/v1"

[router]
orchestrator = "deepseek/deepseek-v4-pro"
max_parallel = 4
"#;

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
    /// The Oracle — the fused-fleet adjudication seam (step 1: advisory, off).
    #[serde(default)]
    pub oracle: OracleConfig,
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
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub obsidian: ObsidianConfig,
    #[serde(default)]
    pub nats: NatsConfig,
    #[serde(default)]
    pub federation: FederationConfig,
    #[serde(default)]
    pub frozen: FrozenConfig,
    #[serde(default)]
    pub current: CurrentConfig,
    #[serde(default)]
    pub chenno: ChennoConfig,
    #[serde(default)]
    pub kin: KinConfig,
}

/// `[kin]` — the family constellation: sibling nodes of the wider organism
/// (riva.vaked.dev & co) polled for liveness and rendered as the outermost
/// ring of the Zen field. Status only — one tiny GET per node per interval,
/// no auth, no data ingestion. Empty list = ring absent.
#[derive(Debug, Clone, Deserialize)]
pub struct KinConfig {
    /// Status URLs of kin nodes. The display name is the host's first label
    /// (`https://riva.vaked.dev/` → "riva").
    #[serde(default)]
    pub nodes: Vec<String>,
    /// Seconds between liveness polls. Default 120.
    #[serde(default = "default_kin_poll_secs")]
    pub poll_secs: u64,
}

impl Default for KinConfig {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            poll_secs: default_kin_poll_secs(),
        }
    }
}

fn default_kin_poll_secs() -> u64 {
    120
}

/// `[chenno]` — the call home: on `/freeze`, entheai publishes the checkpoint
/// plus a human-readable context report into a NEW folder of a central git
/// repo (one folder per context) and commits + pushes it herself. The repo's
/// `origin` remote is the destination — no URL lives in config or code.
/// The operator hand-picks folder links to share; no other integration.
#[derive(Debug, Clone, Deserialize)]
pub struct ChennoConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Local clone of the central repo (a plain git clone with a pushable
    /// `origin`). `~` expands at use.
    #[serde(default = "default_chenno_dir")]
    pub dir: String,
}

impl Default for ChennoConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            dir: default_chenno_dir(),
        }
    }
}

fn default_chenno_dir() -> String {
    "~/.entheai/karmapa-chenno".to_string()
}

/// Built-in keyless free-tier provider name — coder.vaked.dev's OpenAI-compatible
/// endpoint (Qwen3-Coder-30B on CPU, no API key). Injected into every parsed
/// config (unless the user declares their own `[providers.vaked]`) so the
/// fan-out level always has a working model when nothing else is available.
pub const VAKED_PROVIDER: &str = "vaked";
/// Base URL for the built-in [`VAKED_PROVIDER`] free tier.
pub const VAKED_BASE_URL: &str = "https://coder.vaked.dev/v1";

/// Built-in keyed providers, injected into every parsed config (never
/// overriding a user-declared `[providers.<name>]`) so the DeepSeek V4 defaults
/// (`deepseek/deepseek-v4-pro|flash`) and the gemini / openrouter fallbacks
/// resolve in a bare config as soon as the matching key is in the environment:
/// `(name, base_url, api_key_env, kind)`. Gemini rides adk-rust's native
/// client (`kind = "gemini"`): Gemini 3.x tool-call turns need the
/// `thought_signature` round-trip the OpenAI-compatible endpoint path drops.
pub const BUILTIN_KEYED_PROVIDERS: &[(&str, &str, &str, Option<&str>)] = &[
    (
        "deepseek",
        "https://api.deepseek.com/v1",
        "DEEPSEEK_API_KEY",
        None,
    ),
    (
        "gemini",
        "https://generativelanguage.googleapis.com/v1beta/openai",
        "GEMINI_API_KEY",
        Some("gemini"),
    ),
    (
        "openrouter",
        "https://openrouter.ai/api/v1",
        "OPENROUTER_API_KEY",
        None,
    ),
];

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProviderConfig {
    pub base_url: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Directory of a self-contained native model (ternary/quantal): the
    /// ayeOS `m*.json` + `index.json` matrices, `embeddings.f16`, `norms.f32`
    /// and a vendored `tokenizer.json`. Only read when `kind = "ternary"`.
    /// Deliberately NOT overloading `base_url` as a directory (oracle review
    /// correction #4).
    #[serde(default)]
    pub model_dir: Option<String>,
    /// Provider backend: `"openai"` (default; `base_url` + `api_key_env`),
    /// `"gemini"` (adk-rust's native Gemini client over `api_key_env`;
    /// `base_url` ignored) or `"ternary"` (native `model_dir` runner).
    #[serde(default)]
    pub kind: Option<String>,
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
    200
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
    /// changes are integrated (e.g. "cargo test"). Unset = auto-detect
    /// `./scripts/check.sh` at the repo root (see [`FanoutConfig::resolve_verify`]).
    #[serde(default)]
    pub verify: Option<String>,
    /// Whether a passing verify run is REQUIRED before a coder's branch is
    /// integrated (frozen/verification.md: never trust self-reported success).
    /// When `true` (default) and no verify command resolves — neither `verify`
    /// set nor `./scripts/check.sh` present — changed branches are left
    /// unmerged for human review instead of being integrated unverified.
    /// Set `false` to restore the legacy integrate-as-is behaviour.
    #[serde(default = "default_verify_required")]
    pub verify_required: bool,
    /// Per-coder timeout in seconds before it's force-aborted — a hung coder
    /// must not block the rest of the fan-out batch. Default: 600 (10 min).
    #[serde(default = "default_coder_timeout_secs")]
    pub coder_timeout_secs: u64,
    /// Coder execution backend: "auto" (federation if `[federation]` is on and a
    /// worker answers, else local) | "local" (always in-process, on the
    /// `[agents.<role>]` models — never federates) | "agy" (run each coder via
    /// the Antigravity CLI on `agy_model` — recursive dev; bypasses
    /// `[agents.coder]`) | "copilot" (run each coder via the GitHub Copilot CLI).
    #[serde(default = "default_fanout_executor")]
    pub executor: String,
    /// Model the "agy" executor runs fan-out coders on.
    #[serde(default = "default_agy_model")]
    pub agy_model: String,
    /// Model the "copilot" executor runs fan-out coders on, passed to
    /// `copilot --model` (empty = the Copilot CLI's own default model).
    #[serde(default = "default_copilot_model")]
    pub copilot_model: String,
    /// Per-run mode override for fan-out sub-agents ("" = inherit parent ceiling).
    #[serde(default)]
    pub mode: String,
}

/// The Oracle — entheai's single adjudication seam over the fused fleet.
/// Step 1 skeleton: advisory-only, disabled by default, darwin-safe.
#[derive(Debug, Clone, Deserialize)]
pub struct OracleConfig {
    /// Master switch. Default OFF — today's fan-out behavior is unchanged.
    #[serde(default)]
    pub enabled: bool,
    /// Where coders run: "local" (darwin, today's path) | "fleet" (Linux host —
    /// required for the eBPF sphere to attest). Default "local".
    #[serde(default = "default_oracle_coders")]
    pub coders: String,
    /// Gate mode: "advisory" (log + record, never blocks) | "block" (block on
    /// high-confidence Reject/Rework). Default "advisory".
    #[serde(default = "default_oracle_gate")]
    pub gate: String,
    /// Confidence threshold (0..1) above which Reject/Rework may block when
    /// gate = "block". Default 0.8.
    #[serde(default = "default_oracle_block_confidence")]
    pub block_confidence: f32,
    /// The Oracle's adjudication model (router-resolved; degrades to the free
    /// tier when its provider is unavailable). Defaults to DeepSeek V4 Pro.
    #[serde(default = "default_oracle_model")]
    pub model: String,
}

impl Default for OracleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            coders: default_oracle_coders(),
            gate: default_oracle_gate(),
            block_confidence: default_oracle_block_confidence(),
            model: default_oracle_model(),
        }
    }
}

fn default_oracle_coders() -> String {
    "local".to_string()
}
fn default_oracle_gate() -> String {
    "advisory".to_string()
}
fn default_oracle_block_confidence() -> f32 {
    0.8
}
fn default_oracle_model() -> String {
    "deepseek/deepseek-v4-pro".to_string()
}

impl Default for FanoutConfig {
    fn default() -> Self {
        Self {
            verify: None,
            verify_required: default_verify_required(),
            coder_timeout_secs: default_coder_timeout_secs(),
            executor: default_fanout_executor(),
            agy_model: default_agy_model(),
            copilot_model: default_copilot_model(),
            mode: String::new(),
        }
    }
}

impl FanoutConfig {
    /// Whether fan-out coders may be dispatched to the federation: only the
    /// "auto" executor with `[federation].enabled`. `"local"` (and the CLI
    /// executors) never federate, so an operator pinning `executor = "local"`
    /// gets in-process coders even with `[federation]` on.
    pub fn federates(&self, federation: &FederationConfig) -> bool {
        self.executor == "auto" && federation.enabled
    }

    /// The effective verify command for a repo rooted at `root`: the configured
    /// `[fanout].verify` when set, else `./scripts/check.sh` when that script
    /// exists at the root (the repo's own empirical gate), else `None`.
    pub fn resolve_verify(&self, root: &std::path::Path) -> Option<String> {
        if let Some(cmd) = &self.verify {
            return Some(cmd.clone());
        }
        let script = root.join("scripts").join("check.sh");
        script.is_file().then(|| "./scripts/check.sh".to_string())
    }
}

fn default_verify_required() -> bool {
    true
}

fn default_fanout_executor() -> String {
    "auto".into()
}
fn default_agy_model() -> String {
    "gemini-3.6-flash-high".into()
}
fn default_copilot_model() -> String {
    // Empty → defer to the Copilot CLI's own configured default model.
    String::new()
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
    /// Show the brain panel. Default: true.
    #[serde(default = "default_viz_brain")]
    pub brain: bool,
    /// Width of the brain panel in columns. Default: 26.
    #[serde(default = "default_viz_brain_width")]
    pub brain_width: u16,
    /// Zen field theme: "entheia" | "ember" | "verdant" | "void". Unknown
    /// names fall back to "entheia". Default: "entheia".
    #[serde(default = "default_viz_theme")]
    pub theme: String,
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

fn default_viz_brain() -> bool {
    true
}

fn default_viz_brain_width() -> u16 {
    26
}

impl Default for VizConfig {
    fn default() -> Self {
        Self {
            swarm: default_viz_swarm(),
            tick_ms: default_viz_tick_ms(),
            plan_rows_cap: default_viz_plan_rows_cap(),
            swarm_rows_cap: default_viz_swarm_rows_cap(),
            brain: default_viz_brain(),
            brain_width: default_viz_brain_width(),
            theme: default_viz_theme(),
        }
    }
}

fn default_viz_theme() -> String {
    "entheia".to_string()
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
    #[serde(default = "default_permission_mode")]
    pub mode: String,
    #[serde(default)]
    pub pins: HashMap<String, String>,
}
fn default_fanout_auto_approve() -> bool {
    true
}
fn default_permission_mode() -> String {
    "ask".into()
}
impl Default for PermissionConfig {
    fn default() -> Self {
        Self {
            yolo: false,
            allowlist: Vec::new(),
            fanout_auto_approve: default_fanout_auto_approve(),
            mode: default_permission_mode(),
            pins: HashMap::new(),
        }
    }
}

/// Cross-cutting MCP settings (siblings of the per-server `[mcp.<name>]` map).
#[derive(Debug, Clone, Deserialize)]
pub struct McpDefaultsConfig {
    /// Bound on spawning + the initialize handshake + `tools/list` per server.
    #[serde(default = "default_mcp_spawn_timeout_secs")]
    pub spawn_timeout_secs: u64,
    /// Bound on a single `tools/call` (tool calls legitimately outlive the
    /// spawn bound: web fetch, research, build tools). Default 300.
    #[serde(default = "default_mcp_call_timeout_secs")]
    pub call_timeout_secs: u64,
}
fn default_mcp_spawn_timeout_secs() -> u64 {
    10
}
fn default_mcp_call_timeout_secs() -> u64 {
    300
}
impl Default for McpDefaultsConfig {
    fn default() -> Self {
        Self {
            spawn_timeout_secs: default_mcp_spawn_timeout_secs(),
            call_timeout_secs: default_mcp_call_timeout_secs(),
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
        let mut cfg: Config = toml::from_str(s)?;
        if !FANOUT_EXECUTORS.contains(&cfg.fanout.executor.as_str()) {
            return Err(ConfigError::Invalid(format!(
                "[fanout].executor = {:?} is not one of {:?}",
                cfg.fanout.executor, FANOUT_EXECUTORS
            )));
        }
        cfg.inject_builtin_providers();
        Ok(cfg)
    }

    /// Ensure the built-in providers are present: the keyless free tier
    /// ([`VAKED_PROVIDER`] → coder.vaked.dev) so the fan-out level can always
    /// fall back to a working model, plus the keyed [`BUILTIN_KEYED_PROVIDERS`]
    /// (deepseek / gemini / openrouter) so the DeepSeek V4 defaults resolve
    /// without a `[providers.*]` block. Never overrides a user-declared entry.
    fn inject_builtin_providers(&mut self) {
        self.providers
            .entry(VAKED_PROVIDER.to_string())
            .or_insert_with(|| ProviderConfig {
                base_url: VAKED_BASE_URL.to_string(),
                api_key_env: None,
                model_dir: None,
                kind: None,
            });
        for (name, base_url, api_key_env, kind) in BUILTIN_KEYED_PROVIDERS {
            self.providers
                .entry((*name).to_string())
                .or_insert_with(|| ProviderConfig {
                    base_url: (*base_url).to_string(),
                    api_key_env: Some((*api_key_env).to_string()),
                    model_dir: None,
                    kind: kind.map(str::to_string),
                });
        }
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
    fn injects_builtin_vaked_free_tier_provider() {
        // Even an empty config carries a working, keyless free-tier provider.
        let cfg = Config::from_toml_str("").unwrap();
        let vaked = cfg
            .providers
            .get(VAKED_PROVIDER)
            .expect("built-in vaked provider injected");
        assert_eq!(vaked.base_url, VAKED_BASE_URL);
        assert_eq!(vaked.api_key_env, None, "free tier is keyless");
    }

    #[test]
    fn user_declared_vaked_provider_is_not_overridden() {
        let cfg = Config::from_toml_str(
            r#"
            [providers.vaked]
            base_url = "https://my-own-node/v1"
            api_key_env = "MY_KEY"
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg.providers[VAKED_PROVIDER].base_url,
            "https://my-own-node/v1"
        );
        assert_eq!(
            cfg.providers[VAKED_PROVIDER].api_key_env.as_deref(),
            Some("MY_KEY")
        );
    }

    #[test]
    fn injects_builtin_keyed_providers_deepseek_gemini_openrouter() {
        let cfg = Config::from_toml_str("").unwrap();
        for (name, base_url, api_key_env, kind) in BUILTIN_KEYED_PROVIDERS {
            let p = cfg
                .providers
                .get(*name)
                .unwrap_or_else(|| panic!("built-in provider {name} injected"));
            assert_eq!(p.base_url, *base_url);
            assert_eq!(p.api_key_env.as_deref(), Some(*api_key_env));
            assert_eq!(p.kind.as_deref(), *kind);
        }
        assert_eq!(cfg.providers["gemini"].kind.as_deref(), Some("gemini"));
        assert_eq!(
            cfg.providers["deepseek"].base_url,
            "https://api.deepseek.com/v1"
        );
    }

    #[test]
    fn user_declared_keyed_provider_is_not_overridden() {
        let cfg = Config::from_toml_str(
            r#"
            [providers.deepseek]
            base_url = "https://my-proxy/v1"
            api_key_env = "MY_DEEPSEEK_KEY"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.providers["deepseek"].base_url, "https://my-proxy/v1");
        assert_eq!(
            cfg.providers["deepseek"].api_key_env.as_deref(),
            Some("MY_DEEPSEEK_KEY")
        );
    }

    #[test]
    fn oracle_model_defaults_to_deepseek_v4_pro() {
        let cfg = Config::from_toml_str("[oracle]\nenabled = true\n").unwrap();
        assert_eq!(cfg.oracle.model, "deepseek/deepseek-v4-pro");
    }

    #[test]
    fn unknown_fanout_executor_is_a_config_error() {
        let err = Config::from_toml_str("[fanout]\nexecutor = \"federation\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)), "{err}");
        assert!(err.to_string().contains("[fanout].executor"));
        for ok in FANOUT_EXECUTORS {
            Config::from_toml_str(&format!("[fanout]\nexecutor = \"{ok}\"\n"))
                .unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
    }

    #[test]
    fn builtin_config_parses_and_is_deepseek_first() {
        let cfg = Config::from_toml_str(BUILTIN_CONFIG_TOML).unwrap();
        assert_eq!(
            cfg.default_model.as_deref(),
            Some("deepseek/deepseek-v4-flash")
        );
        assert_eq!(
            cfg.router.orchestrator.as_deref(),
            Some("deepseek/deepseek-v4-pro")
        );
        assert_eq!(cfg.router.max_parallel, 4);
        for p in [
            "deepseek",
            "gemini",
            "openrouter",
            VAKED_PROVIDER,
            "osaurus",
        ] {
            assert!(
                cfg.providers.contains_key(p),
                "{p} missing from built-in config"
            );
        }
    }

    #[test]
    fn fanout_federates_only_under_auto_with_federation_on() {
        let auto_on = Config::from_toml_str("[federation]\nenabled = true\n").unwrap();
        assert_eq!(auto_on.fanout.executor, "auto");
        assert!(auto_on.fanout.federates(&auto_on.federation));

        let local_on =
            Config::from_toml_str("[fanout]\nexecutor = \"local\"\n[federation]\nenabled = true\n")
                .unwrap();
        assert!(!local_on.fanout.federates(&local_on.federation));

        let auto_off = Config::from_toml_str("").unwrap();
        assert!(!auto_off.fanout.federates(&auto_off.federation));
    }

    #[test]
    fn parses_ternary_provider_kind_and_model_dir() {
        let cfg = Config::from_toml_str(
            r#"
            default_model = "quantal/qwen2.5-0.5b-quantal"

            [providers.quantal]
            base_url = ""
            model_dir = "/Users/lodripeter/workspace/peterlodri-sec/pocoo.vaked.dev/demos/quantal"
            kind = "ternary"
            "#,
        )
        .unwrap();
        let q = &cfg.providers["quantal"];
        assert_eq!(q.kind.as_deref(), Some("ternary"));
        assert_eq!(
            q.model_dir.as_deref(),
            Some("/Users/lodripeter/workspace/peterlodri-sec/pocoo.vaked.dev/demos/quantal")
        );
        assert_eq!(
            cfg.default_model.as_deref(),
            Some("quantal/qwen2.5-0.5b-quantal")
        );
    }

    #[test]
    fn openai_provider_defaults_to_no_kind() {
        let cfg = Config::from_toml_str(
            r#"
            [providers.osaurus]
            base_url = "http://127.0.0.1:1337/v1"
            "#,
        )
        .unwrap();
        let o = &cfg.providers["osaurus"];
        assert_eq!(o.kind, None, "plain providers default to the openai path");
        assert_eq!(o.model_dir, None);
    }

    #[test]
    fn parses_router_and_agents_when_present() {
        let cfg = Config::from_toml_str(
            r#"
            [router]
            orchestrator = "deepseek/deepseek-v4-pro"
            max_parallel = 4

            [agents.coder]
            model = ["deepseek/deepseek-v4-flash", "gemini/gemini-3.6-flash"]
            "#,
        )
        .unwrap();

        assert_eq!(
            cfg.router.orchestrator.as_deref(),
            Some("deepseek/deepseek-v4-pro")
        );
        assert_eq!(cfg.router.max_parallel, 4);
        assert_eq!(
            cfg.agents["coder"].model,
            vec![
                "deepseek/deepseek-v4-flash".to_string(),
                "gemini/gemini-3.6-flash".to_string()
            ]
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
    fn fanout_verify_required_defaults_to_true_and_parses_false() {
        let default = Config::from_toml_str(r#"default_model = "osaurus/qwen3-coder""#).unwrap();
        assert!(default.fanout.verify_required);

        let lax = Config::from_toml_str(
            r#"
            default_model = "osaurus/qwen3-coder"
            [fanout]
            verify_required = false
            "#,
        )
        .unwrap();
        assert!(!lax.fanout.verify_required);
    }

    #[test]
    fn resolve_verify_prefers_explicit_command_over_autodetect() {
        let cfg = Config::from_toml_str(
            r#"
            default_model = "osaurus/qwen3-coder"
            [fanout]
            verify = "cargo test"
            "#,
        )
        .unwrap();
        // Explicit command wins regardless of what exists on disk.
        assert_eq!(
            cfg.fanout
                .resolve_verify(std::path::Path::new("/nonexistent")),
            Some("cargo test".to_string())
        );
    }

    #[test]
    fn resolve_verify_autodetects_check_sh_else_none() {
        let cfg = Config::from_toml_str(r#"default_model = "osaurus/qwen3-coder""#).unwrap();

        let root = std::env::temp_dir().join(format!("entheai-cfg-test-{}", std::process::id()));
        let scripts = root.join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();

        // No check.sh yet → no resolved command.
        assert_eq!(cfg.fanout.resolve_verify(&root), None);

        std::fs::write(scripts.join("check.sh"), "#!/bin/sh\nexit 0\n").unwrap();
        assert_eq!(
            cfg.fanout.resolve_verify(&root),
            Some("./scripts/check.sh".to_string())
        );

        std::fs::remove_dir_all(&root).ok();
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
    fn memory_mode_defaults_to_topk() {
        let cfg = Config::from_toml_str("[memory]\nenabled = true\n").unwrap();
        assert_eq!(cfg.memory.mode, "topk");
        assert!(cfg.memory.prompt_processing.is_none());
    }

    #[test]
    fn memory_prompt_processing_parses() {
        let cfg = Config::from_toml_str(
            "[memory]\nmode = \"prompt-processing\"\n\
             [memory.prompt_processing]\nsearch_deadline_ms = 800\nrecall_k = 32\n",
        )
        .unwrap();
        assert_eq!(cfg.memory.mode, "prompt-processing");
        let pp = cfg.memory.prompt_processing.unwrap();
        assert_eq!(pp.search_deadline_ms, 800);
        assert_eq!(pp.recall_k, 32);
        assert_eq!(
            pp.marqant_cmd, "mq",
            "absent sub-fields take their defaults"
        );
        assert_eq!(
            pp.marqant_backend, "subprocess",
            "marqant defaults to subprocess backend"
        );
        assert_eq!(pp.raw_retention_days, 90);
        assert_eq!(
            pp.mesh_backend, "native",
            "mesh defaults to the in-process native backend"
        );
        assert_eq!(
            pp.native_model, "",
            "no .ugm reranker by default → lexical scorer"
        );
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
    fn viz_brain_defaults_on() {
        let cfg = Config::from_toml_str("").unwrap();
        assert!(cfg.viz.brain);
        assert_eq!(cfg.viz.brain_width, 26);
    }

    #[test]
    fn viz_brain_overrides() {
        let cfg = Config::from_toml_str("[viz]\nbrain = false\nbrain_width = 30\n").unwrap();
        assert!(!cfg.viz.brain);
        assert_eq!(cfg.viz.brain_width, 30);
    }

    #[test]
    fn refactor_config_defaults() {
        let cfg = Config::from_toml_str("").unwrap();
        assert_eq!(cfg.router.max_turns, 200);
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
        assert_eq!(cfg.mcp_defaults.call_timeout_secs, 300);
        assert_eq!(cfg.memory.embed_timeout_secs, 30);
        assert_eq!(cfg.viz.tick_ms, 90);
        assert_eq!(cfg.viz.plan_rows_cap, 8);
        assert_eq!(cfg.viz.swarm_rows_cap, 8);
        assert_eq!(cfg.companion.port, 9876);
        assert_eq!(cfg.companion.fps, 24.0);
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

    #[test]
    fn federation_sandbox_defaults_permissive_and_parses() {
        use entheai_sandbox::SandboxMode;
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.federation.sandbox, SandboxMode::Permissive); // default

        let cfg: Config = toml::from_str("[federation]\nsandbox = \"strict\"\n").unwrap();
        assert_eq!(cfg.federation.sandbox, SandboxMode::Strict);

        assert!(toml::from_str::<Config>("[federation]\nsandbox = \"bogus\"\n").is_err());
    }

    #[test]
    fn federation_max_concurrent_coders_defaults_to_4_and_parses() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.federation.max_concurrent_coders, 4);

        let cfg: Config = toml::from_str("[federation]\nmax_concurrent_coders = 3\n").unwrap();
        assert_eq!(cfg.federation.max_concurrent_coders, 3);
    }

    #[test]
    fn base_cache_count_stays_above_concurrency() {
        // The cache cap must always exceed max_concurrent_coders so an in-use base
        // is never the least-recent eviction target (belt to the in-use guard).
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.federation.base_cache_count(), 4 * 2 + 4); // 12 by default
        assert!(cfg.federation.base_cache_count() > cfg.federation.max_concurrent_coders);

        let cfg: Config = toml::from_str("[federation]\nmax_concurrent_coders = 16\n").unwrap();
        assert_eq!(cfg.federation.base_cache_count(), 16 * 2 + 4);
        assert!(cfg.federation.base_cache_count() > cfg.federation.max_concurrent_coders);
    }

    #[test]
    fn permission_mode_and_pins_parse() {
        let cfg = Config::from_toml_str(
            "[permission]\nmode = \"auto\"\npins = { run_shell = \"always_ask\" }\n",
        )
        .unwrap();
        assert_eq!(cfg.permission.mode, "auto");
        assert_eq!(
            cfg.permission.pins.get("run_shell").map(String::as_str),
            Some("always_ask")
        );
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
    /// Retrieval mode: "topk" (today's behaviour, default) | "prompt-processing".
    #[serde(default = "default_memory_mode")]
    pub mode: String,
    /// Prompt-processing sub-table; only read when `mode = "prompt-processing"`.
    #[serde(default)]
    pub prompt_processing: Option<PromptProcessingConfig>,
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
fn default_memory_mode() -> String {
    "topk".into()
}

/// Prompt-processing configuration (spec §Configuration). All fields default,
/// so `[memory.prompt_processing]` can be omitted entirely.
#[derive(Debug, Clone, Deserialize)]
pub struct PromptProcessingConfig {
    /// The mesh sidecar command (Slice 2; unused by the Slice-1 stub).
    #[serde(default = "default_pp_sidecar_cmd")]
    pub sidecar_cmd: String,
    /// Ternary models in the mesh (Slice 2).
    #[serde(default = "default_pp_mesh_size")]
    pub mesh_size: usize,
    /// Fail fast to fallback past this deadline (bounds the whole pipeline).
    #[serde(default = "default_pp_search_deadline_ms")]
    pub search_deadline_ms: u64,
    /// The compression subprocess (Slice 2; unused by the Slice-1 stub).
    #[serde(default = "default_pp_marqant_cmd")]
    pub marqant_cmd: String,
    /// Raw-store retention window; pruned on startup.
    #[serde(default = "default_pp_raw_retention_days")]
    pub raw_retention_days: u64,
    /// Stage-1 lexical recall breadth.
    #[serde(default = "default_pp_recall_k")]
    pub recall_k: usize,
    /// Raw-store DB path (separate file from memory.db).
    #[serde(default = "default_pp_raw_path")]
    pub raw_path: String,
    /// Per-ingest byte cap (adversarial-review correction #4): bound unbounded
    /// tool/transcript payloads so a huge output can't balloon raw.db in one run.
    #[serde(default = "default_pp_max_ingest_bytes")]
    pub max_ingest_bytes: usize,
    /// Stage-2 mesh backend: "native" (in-process, default — no Python), "sidecar"
    /// (the stdio-JSON-RPC `sidecar_cmd`), or "stub" (always fall back to top-K).
    #[serde(default = "default_pp_mesh_backend")]
    pub mesh_backend: String,
    /// Optional `.ugm` reranker for the native mesh (a FEATURE_DIM-input dense model);
    /// empty → the deterministic lexical scorer.
    #[serde(default)]
    pub native_model: String,
    /// Stage-3 marqant backend: "subprocess" (default — spawn `marqant_cmd`),
    /// "kompress" (in-process kompress-core pipeline, no subprocess), or "stub"
    /// (identity passthrough, always falls back to top-K).
    #[serde(default = "default_pp_marqant_backend")]
    pub marqant_backend: String,
}

impl Default for PromptProcessingConfig {
    fn default() -> Self {
        Self {
            sidecar_cmd: default_pp_sidecar_cmd(),
            mesh_size: default_pp_mesh_size(),
            search_deadline_ms: default_pp_search_deadline_ms(),
            marqant_cmd: default_pp_marqant_cmd(),
            raw_retention_days: default_pp_raw_retention_days(),
            recall_k: default_pp_recall_k(),
            raw_path: default_pp_raw_path(),
            max_ingest_bytes: default_pp_max_ingest_bytes(),
            mesh_backend: default_pp_mesh_backend(),
            native_model: String::new(),
            marqant_backend: default_pp_marqant_backend(),
        }
    }
}

fn default_pp_marqant_backend() -> String {
    "subprocess".into()
}

fn default_pp_mesh_backend() -> String {
    "native".into()
}

fn default_pp_sidecar_cmd() -> String {
    // NOTE: the published `ultragraph-1bit` ships no stdio `rerank` module; the
    // Slice-2 sidecar is a new in-repo script (uses ultragraph if importable, else
    // a lexical reference scorer). Relative path resolves from the run cwd; set to
    // "" to force the in-process stub (disable the mesh).
    "python3 sidecars/ultragraph/serve.py".into()
}
fn default_pp_mesh_size() -> usize {
    8
}
fn default_pp_search_deadline_ms() -> u64 {
    1500
}
fn default_pp_marqant_cmd() -> String {
    "mq".into()
}
fn default_pp_raw_retention_days() -> u64 {
    90
}
fn default_pp_recall_k() -> usize {
    64
}
fn default_pp_raw_path() -> String {
    "~/.cache/entheai/raw.db".into()
}
fn default_pp_max_ingest_bytes() -> usize {
    262_144 // 256 KiB
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
            mode: default_memory_mode(),
            prompt_processing: None,
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

/// Distributed swarm (F2). Opt-in; reuses `[nats]` for the connection.
///
/// `role` is **reserved** and currently unused by the runtime: worker vs.
/// dispatcher mode is chosen by the `--serve` / `--dispatch` CLI flags on
/// `entheai-worker`, not by this field. `sandbox` sets the coder confinement
/// posture on a serving worker (see `crates/sandbox`).
#[derive(Debug, Clone, Deserialize)]
pub struct FederationConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Reserved; worker mode is selected by `--serve` / `--dispatch`, not this field.
    #[serde(default = "default_fed_role")]
    pub role: String, // "auto" | "worker" | "dispatch"
    #[serde(default = "default_fed_deadline_secs")]
    pub deadline_secs: u64,
    /// Coder confinement posture on this worker (see crates/sandbox).
    #[serde(default)]
    pub sandbox: entheai_sandbox::SandboxMode,
    /// How many coders a serving worker runs concurrently. Coders are
    /// model-wait-bound, so several share one node cheaply — each in its own
    /// detached worktree off a single cached base repo. Default: 4.
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_coders: usize,
}

impl FederationConfig {
    /// LRU capacity for the per-node base-repo cache. Deliberately kept
    /// comfortably above `max_concurrent_coders` so a base a live coder is still
    /// attached to is never the least-recent eviction target; combined with the
    /// in-use guard on `BaseCache`, eviction of an in-use base is impossible.
    pub fn base_cache_count(&self) -> usize {
        self.max_concurrent_coders * 2 + 4
    }
}

impl Default for FederationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            role: default_fed_role(),
            deadline_secs: default_fed_deadline_secs(),
            sandbox: entheai_sandbox::SandboxMode::default(),
            max_concurrent_coders: default_max_concurrent(),
        }
    }
}

fn default_fed_role() -> String {
    "auto".to_string()
}
fn default_fed_deadline_secs() -> u64 {
    600
}
fn default_max_concurrent() -> usize {
    4
}

#[derive(Debug, Clone, Deserialize)]
pub struct FrozenConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_frozen_dir")]
    pub dir: String,
    #[serde(default = "default_frozen_top_k")]
    pub top_k: usize,
    #[serde(default = "default_frozen_max_bytes")]
    pub max_inject_bytes: usize,
}

impl Default for FrozenConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            dir: default_frozen_dir(),
            top_k: default_frozen_top_k(),
            max_inject_bytes: default_frozen_max_bytes(),
        }
    }
}

fn default_frozen_dir() -> String {
    "frozen".to_string()
}
fn default_frozen_top_k() -> usize {
    1
}
fn default_frozen_max_bytes() -> usize {
    4096
}

/// `[current]` — current-awareness ingestion (Valyu + WorldMonitor → the raw
/// memory soil, under hard daily request budgets). Off by default; requires
/// prompt-processing memory to be on (the brain IS where current lands).
#[derive(Debug, Clone, Deserialize)]
pub struct CurrentConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Env var holding the Valyu API key. Missing/empty key disables Valyu.
    #[serde(default = "default_valyu_key_env")]
    pub valyu_api_key_env: String,
    /// Env var holding the WorldMonitor API key (X-WorldMonitor-Key).
    #[serde(default = "default_worldmonitor_key_env")]
    pub worldmonitor_api_key_env: String,
    /// Daily request cap for Valyu (budget honesty; requests, not dollars —
    /// per-query dollars are bounded by `valyu_max_price`).
    #[serde(default = "default_current_daily_cap")]
    pub valyu_daily_cap: u32,
    /// Daily request cap for WorldMonitor. Clamped to ≤ 50 at engine build —
    /// the operator's mandate, not negotiable via config.
    #[serde(default = "default_current_daily_cap")]
    pub worldmonitor_daily_cap: u32,
    /// Minutes between automatic pulses in the TUI. Default 120 keeps a full
    /// day of WorldMonitor pulses (3 req each) at 36 ≤ 50.
    #[serde(default = "default_current_refresh_minutes")]
    pub refresh_minutes: u64,
    /// Valyu news topics — one request per topic per pulse. Empty = Valyu idle.
    #[serde(default)]
    pub topics: Vec<String>,
    /// Results per Valyu query (1–20).
    #[serde(default = "default_valyu_max_results")]
    pub valyu_max_results: u32,
    /// CPM price ceiling per Valyu query, in dollars.
    #[serde(default = "default_valyu_max_price")]
    pub valyu_max_price: f64,
    /// The gated HuggingFace dogfood dataset — the genetic corpus entheai was
    /// born from (the ultrawhale dogfeed loop's Q&A pairs). Empty = disabled.
    #[serde(default)]
    pub dogfood_repo: String,
    /// Env var holding the HuggingFace token (dataset is gated). Missing/empty
    /// disables dogfood even when a repo is set.
    #[serde(default = "default_hf_token_env")]
    pub hf_token_env: String,
    /// Daily request cap for dogfood (2 requests per pulse: list + newest batch).
    #[serde(default = "default_current_daily_cap")]
    pub dogfood_daily_cap: u32,
}

impl Default for CurrentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            valyu_api_key_env: default_valyu_key_env(),
            worldmonitor_api_key_env: default_worldmonitor_key_env(),
            valyu_daily_cap: default_current_daily_cap(),
            worldmonitor_daily_cap: default_current_daily_cap(),
            refresh_minutes: default_current_refresh_minutes(),
            topics: Vec::new(),
            valyu_max_results: default_valyu_max_results(),
            valyu_max_price: default_valyu_max_price(),
            dogfood_repo: String::new(),
            hf_token_env: default_hf_token_env(),
            dogfood_daily_cap: default_current_daily_cap(),
        }
    }
}

fn default_hf_token_env() -> String {
    "HF_TOKEN".to_string()
}

fn default_valyu_key_env() -> String {
    "VALYU_API_KEY".to_string()
}
fn default_worldmonitor_key_env() -> String {
    "WORLDMONITOR_API_KEY".to_string()
}
fn default_current_daily_cap() -> u32 {
    50
}
fn default_current_refresh_minutes() -> u64 {
    120
}
fn default_valyu_max_results() -> u32 {
    5
}
fn default_valyu_max_price() -> f64 {
    30.0
}

#[cfg(test)]
mod frozen_tests {
    use super::*;

    #[test]
    fn frozen_config_defaults_off() {
        let cfg = Config::from_toml_str("").unwrap();
        assert!(!cfg.frozen.enabled);
        assert_eq!(cfg.frozen.dir, "frozen");
        assert_eq!(cfg.frozen.top_k, 1);
        assert_eq!(cfg.frozen.max_inject_bytes, 4096);
        let on = Config::from_toml_str("[frozen]\nenabled = true\ntop_k = 2\n").unwrap();
        assert!(on.frozen.enabled);
        assert_eq!(on.frozen.top_k, 2);
    }
}
