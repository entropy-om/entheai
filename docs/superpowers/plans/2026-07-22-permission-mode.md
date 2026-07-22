# Permission + Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Shift+Tab-cycled permission posture `[plan · auto · yolo · ask]` that governs the whole run — tool risk **tiers**, a runtime **mode** with a mode×tier decision matrix, per-tool **pins**, and **subagent** propagation via an inherited tier ceiling.

**Architecture:** Extend `crates/permission::Policy` with `Tier`, `Mode`, `Pin`, and `decide_tiered(tool, tier)`; keep `decide(tool)` as a compatibility shim so existing callers are untouched until each phase wires them. Tools self-declare a tier; the core execution gate (`crates/core`) routes through `decide_tiered`; subagents inherit a tier ceiling via `ENTHEAI_MODE`; the TUI toggles the shared runtime mode.

**Tech Stack:** Rust, `Arc<Mutex<..>>` interior mutability (matching the existing `session` set), `#[tokio::test]`, ratatui/crossterm for the TUI.

Spec: `docs/superpowers/specs/2026-07-22-permission-mode-design.md`.

---

## Phase 1 — Core Policy + matrix (`crates/permission`)

Inert foundation: new types + `decide_tiered`, fully unit-tested. No behavior change until Phase 2 routes through it.

### Task 1: `Tier` enum

**Files:**
- Modify: `crates/permission/src/lib.rs` (add near the top, after the `Decision` enum)

- [ ] **Step 1: Write the failing test** — append to the existing `#[cfg(test)] mod tests` (create one if absent):

```rust
#[test]
fn tier_orders_by_autonomy() {
    assert!(Tier::Read < Tier::Write);
    assert!(Tier::Write < Tier::Exec);
    assert!(Tier::Exec < Tier::Network);
    assert!(Tier::Network < Tier::Spawn);
}
```

- [ ] **Step 2: Run it, verify it fails**

Run: `cargo test -p entheai-permission tier_orders_by_autonomy`
Expected: FAIL to compile — `cannot find type Tier`.

- [ ] **Step 3: Minimal implementation** — add above `Policy`:

```rust
/// Tool risk tier, ordered by how much autonomy the tool exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Read,
    Write,
    Exec,
    Network,
    Spawn,
}
```

- [ ] **Step 4: Run test, verify pass** — `cargo test -p entheai-permission tier_orders_by_autonomy` → PASS.
- [ ] **Step 5: Commit** — `git add crates/permission/src/lib.rs && git commit -m "feat(permission): Tier enum ordered by autonomy"`

### Task 2: `Mode` enum + `next()` + `ceiling()`

**Files:**
- Modify: `crates/permission/src/lib.rs`

- [ ] **Step 1: Failing test**

```rust
#[test]
fn mode_cycles_and_maps_to_ceiling() {
    assert_eq!(Mode::Plan.next(), Mode::Auto);
    assert_eq!(Mode::Auto.next(), Mode::Yolo);
    assert_eq!(Mode::Yolo.next(), Mode::Ask);
    assert_eq!(Mode::Ask.next(), Mode::Plan);
    // subagent ceiling: highest auto-approved tier for an unattended child
    assert_eq!(Mode::Plan.ceiling(), Tier::Read);
    assert_eq!(Mode::Auto.ceiling(), Tier::Exec);
    assert_eq!(Mode::Ask.ceiling(), Tier::Exec);
    assert_eq!(Mode::Yolo.ceiling(), Tier::Spawn);
}

#[test]
fn mode_parse_is_fail_safe() {
    assert_eq!(Mode::parse("plan"), Mode::Plan);
    assert_eq!(Mode::parse("YOLO"), Mode::Yolo);
    assert_eq!(Mode::parse("bogus"), Mode::Ask, "unknown → ask (safe default)");
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p entheai-permission mode_` → FAIL (no `Mode`).
- [ ] **Step 3: Implement**

```rust
/// Runtime permission posture, cycled with Shift+Tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    Plan,
    Auto,
    Yolo,
    #[default]
    Ask,
}

impl Mode {
    /// Shift+Tab order: plan → auto → yolo → ask → plan.
    pub fn next(self) -> Mode {
        match self {
            Mode::Plan => Mode::Auto,
            Mode::Auto => Mode::Yolo,
            Mode::Yolo => Mode::Ask,
            Mode::Ask => Mode::Plan,
        }
    }

    /// The highest tier an unattended subagent auto-approves under this mode.
    pub fn ceiling(self) -> Tier {
        match self {
            Mode::Plan => Tier::Read,
            Mode::Auto | Mode::Ask => Tier::Exec,
            Mode::Yolo => Tier::Spawn,
        }
    }

    /// Parse the config string; unknown values warn and fall back to `Ask`.
    pub fn parse(s: &str) -> Mode {
        match s.trim().to_ascii_lowercase().as_str() {
            "plan" => Mode::Plan,
            "auto" => Mode::Auto,
            "yolo" => Mode::Yolo,
            "ask" | "" => Mode::Ask,
            other => {
                log::warn!("unknown permission mode {other:?}; defaulting to ask");
                Mode::Ask
            }
        }
    }
}
```

Add `log = "0.4"` to `crates/permission/Cargo.toml` `[dependencies]` if not already present.

- [ ] **Step 4: Run, verify pass** — `cargo test -p entheai-permission mode_` → PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(permission): Mode enum (cycle + subagent ceiling + fail-safe parse)"`

### Task 3: `Pin` + the mode×tier matrix (`decide_tiered`)

**Files:**
- Modify: `crates/permission/src/lib.rs`

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn matrix_matches_the_spec() {
    use Decision::*;
    use Tier::*;
    let cases = [
        (Mode::Plan, [Allow, Deny, Deny, Deny, Deny]),
        (Mode::Auto, [Allow, Allow, Ask, Ask, Ask]),
        (Mode::Yolo, [Allow, Allow, Allow, Allow, Allow]),
        (Mode::Ask,  [Allow, Ask, Ask, Ask, Ask]),
    ];
    let tiers = [Read, Write, Exec, Network, Spawn];
    for (mode, row) in cases {
        let p = Policy::with_mode(mode);
        for (t, want) in tiers.iter().zip(row) {
            assert_eq!(p.decide_tiered("some_tool", *t), want, "{mode:?} × {t:?}");
        }
    }
}

#[test]
fn pins_override_the_matrix() {
    let mut p = Policy::with_mode(Mode::Yolo); // matrix would Allow everything
    p.pin("run_shell", Pin::AlwaysAsk);
    p.pin("rm", Pin::Never);
    p.pin("read_file", Pin::AlwaysAllow);
    assert_eq!(p.decide_tiered("run_shell", Tier::Exec), Decision::Ask);
    assert_eq!(p.decide_tiered("rm", Tier::Exec), Decision::Deny);
    assert_eq!(p.decide_tiered("read_file", Tier::Read), Decision::Allow);
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p entheai-permission matrix_ pins_` → FAIL (no `Pin`/`with_mode`/`decide_tiered`).
- [ ] **Step 3: Implement** — add `Pin`, extend `Policy` with `mode` + `pins`, add the constructors/methods:

```rust
/// A per-tool override that wins over the mode×tier matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pin {
    AlwaysAllow,
    AlwaysAsk,
    Never, // always Deny
}
```

In `Policy` add fields `mode: Arc<Mutex<Mode>>` and `pins: std::collections::HashMap<String, Pin>`, initialise them in `new` (`Mode::default()`, empty map), and add:

```rust
impl Policy {
    pub fn with_mode(mode: Mode) -> Self {
        let mut p = Policy::new(false, Vec::new());
        p.set_mode(mode);
        p
    }
    pub fn mode(&self) -> Mode {
        *self.mode.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn set_mode(&self, mode: Mode) {
        *self.mode.lock().unwrap_or_else(|e| e.into_inner()) = mode;
    }
    pub fn pin(&mut self, tool: &str, pin: Pin) {
        self.pins.insert(tool.to_string(), pin);
    }

    /// Tiered decision: pin first, else the mode×tier matrix.
    pub fn decide_tiered(&self, tool: &str, tier: Tier) -> Decision {
        if let Some(pin) = self.pins.get(tool) {
            return match pin {
                Pin::AlwaysAllow => Decision::Allow,
                Pin::AlwaysAsk => Decision::Ask,
                Pin::Never => Decision::Deny,
            };
        }
        match (self.mode(), tier) {
            (Mode::Yolo, _) => Decision::Allow,
            (_, Tier::Read) => Decision::Allow,
            (Mode::Plan, _) => Decision::Deny,
            (Mode::Auto, Tier::Write) => Decision::Allow,
            (Mode::Auto, _) => Decision::Ask,
            (Mode::Ask, _) => Decision::Ask,
        }
    }
}
```

Add `use std::collections::HashMap;` if not present. Derive `Default` on `Mode` is already done (Task 2); ensure `Policy::default()` still works (the added `mode`/`pins` fields need Default — `Arc<Mutex<Mode>>` and `HashMap` are both `Default`, so `#[derive(Default)]` on `Policy` remains valid; if `Policy` is hand-`Default`ed, add the two fields there).

- [ ] **Step 4: Run, verify pass** — `cargo test -p entheai-permission` (whole crate) → all PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(permission): Pin + mode×tier decide_tiered matrix"`

### Task 4: `decide()` shim + `yolo` compatibility

**Files:**
- Modify: `crates/permission/src/lib.rs`

- [ ] **Step 1: Failing test**

```rust
#[test]
fn decide_shim_preserves_legacy_semantics() {
    // yolo policy → Allow (legacy); allowlist → Allow; else Ask (Exec-tier default).
    let yolo = Policy::new(true, vec![]);
    assert_eq!(yolo.decide("anything"), Decision::Allow);
    let allow = Policy::new(false, vec!["echo".into()]);
    assert_eq!(allow.decide("echo"), Decision::Allow);
    assert_eq!(allow.decide("rm"), Decision::Ask);
}
```

- [ ] **Step 2: Run, verify fail** — the existing `decide` still returns Allow/Ask but this test also asserts the `yolo` path maps through the mode; run `cargo test -p entheai-permission decide_shim` → confirm it passes or fails, then align: make `Policy::new(true, ..)` set `mode = Yolo`.
- [ ] **Step 3: Implement** — in `Policy::new`, when `yolo` is true set the mode to `Yolo`; keep `allowlist`/`session` handled as today. Re-express `decide(tool)` as: allowlist/session Allow first (legacy fast-path), else `decide_tiered(tool, Tier::Exec)` (unknown tools default to Exec).

```rust
pub fn decide(&self, tool_name: &str) -> Decision {
    if self.allowlist.iter().any(|t| t == tool_name)
        || self.session.lock().unwrap_or_else(|e| e.into_inner()).contains(tool_name)
    {
        return Decision::Allow;
    }
    self.decide_tiered(tool_name, Tier::Exec)
}
```

- [ ] **Step 4: Run, verify pass** — `cargo test -p entheai-permission` → all PASS (existing `decide` tests still green).
- [ ] **Step 5: Commit** — `git commit -am "feat(permission): decide() shim over decide_tiered + yolo→Mode::Yolo compat"`

---

## Phase 2 — Tool tiering (`crates/tools`, `crates/core`)

### Task 5: `Tool::tier()` self-declaration

**Files:**
- Modify: `crates/tools/src/lib.rs` (the `Tool` trait at line 30 + each built-in tool impl)
- Add dep: `entheai-permission` to `crates/tools/Cargo.toml` (if not present)

- [ ] **Step 1: Failing test** — in the tools test module:

```rust
#[test]
fn builtin_tools_declare_expected_tiers() {
    use entheai_permission::Tier;
    // adjust the constructors to however these tools are built in this crate:
    assert_eq!(ReadFile::new(root()).tier(), Tier::Read);
    assert_eq!(WriteFile::new(root()).tier(), Tier::Write);
    assert_eq!(RunShell::new(root()).tier(), Tier::Exec);
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p entheai-tools builtin_tools_declare` → FAIL (no `tier`).
- [ ] **Step 3: Implement** — add a defaulted method to the trait, then override per tool:

```rust
// in `trait Tool`:
fn tier(&self) -> entheai_permission::Tier {
    entheai_permission::Tier::Exec // conservative default (unknown/MCP)
}
```

Override in each built-in: read/search/list → `Tier::Read`; write/edit → `Tier::Write`; run_shell → `Tier::Exec`; any fetch/HTTP tool → `Tier::Network`. (Read each tool struct in this file and add a one-line `fn tier`.)

- [ ] **Step 4: Run, verify pass** — `cargo test -p entheai-tools` → PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(tools): Tool::tier() self-declaration (Exec default)"`

### Task 6: Route the execution gate through `decide_tiered`

**Files:**
- Modify: `crates/core/src/lib.rs:298` (the `policy.decide(name)` gate)

- [ ] **Step 1: Failing test** — in `crates/core` tests, add a plan-mode run that must NOT execute a write tool:

```rust
#[tokio::test]
async fn plan_mode_denies_writes_but_allows_reads() {
    // Build a registry with a read tool and a write tool, a Policy in Plan mode,
    // and assert the write tool is denied (Decision::Deny path) while a read runs.
    // Mirror the existing run_task test harness in this module.
}
```

(Model it on the existing `AllowAll`/`DenyAll` prompter tests around line 515/722.)

- [ ] **Step 2: Run, verify fail** — `cargo test -p entheai-core plan_mode_denies` → FAIL.
- [ ] **Step 3: Implement** — at the gate, look up the tool's tier and call `decide_tiered`:

```rust
let tier = registry.get(name).map(|t| t.tier()).unwrap_or(entheai_permission::Tier::Exec);
let allowed = match policy.decide_tiered(name, tier) {
    Decision::Allow => true,
    Decision::Deny => false,
    Decision::Ask => matches!(prompter.confirm(name, &call.function.arguments).await, Grant::Allow | Grant::AllowSession),
};
```

Preserve the existing `Grant::AllowSession` → `policy.grant_session(name)` side effect if present.

- [ ] **Step 4: Run, verify pass** — `cargo test -p entheai-core` → all PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(core): route tool gate through decide_tiered(tier)"`

---

## Phase 3 — Subagent ceiling (`crates/orchestrator`)

### Task 7: `ENTHEAI_MODE` propagation + ceiling policy

**Files:**
- Modify: `crates/orchestrator/src/lib.rs` (`fanout_policy`, ~line 121) and `crates/orchestrator/src/agy.rs` (~line 118)

- [ ] **Step 1: Failing test** — in orchestrator tests:

```rust
#[test]
fn ceiling_policy_denies_above_the_parent_ceiling() {
    use entheai_permission::{Decision, Tier};
    let p = ceiling_policy(entheai_permission::Mode::Plan); // Read ceiling
    assert_eq!(p.decide_tiered("read_file", Tier::Read), Decision::Allow);
    assert_eq!(p.decide_tiered("run_shell", Tier::Exec), Decision::Deny);
    let a = ceiling_policy(entheai_permission::Mode::Auto); // Exec ceiling
    assert_eq!(a.decide_tiered("run_shell", Tier::Exec), Decision::Allow);
    assert_eq!(a.decide_tiered("fetch", Tier::Network), Decision::Deny);
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p entheai-orchestrator ceiling_policy` → FAIL.
- [ ] **Step 3: Implement** — add a `ceiling_policy(mode)` that returns a `Policy` whose `decide_tiered` allows `tier <= mode.ceiling()` and denies above. Simplest: a thin wrapper mode — set the policy to `Yolo` but cap via a dedicated method, OR build the decision directly:

```rust
/// A Policy for an unattended subagent: auto-approve tools at or below the parent's
/// tier ceiling, deny above (never Ask — subagents can't prompt).
pub fn ceiling_policy(parent: entheai_permission::Mode) -> entheai_permission::Policy {
    entheai_permission::Policy::with_ceiling(parent.ceiling())
}
```

Add `Policy::with_ceiling(tier)` to `crates/permission` (a mode-independent policy whose `decide_tiered` returns `Allow` iff `tier <= ceiling` else `Deny`; store `Option<Tier> ceiling` and check it first in `decide_tiered`). Wire `fanout_policy` to read `std::env::var("ENTHEAI_MODE")` → `Mode::parse` (default `Auto` when unset) and build `ceiling_policy(mode)`, unless `[fanout] mode` overrides. In `agy.rs`, only pass `--dangerously-skip-permissions` when the resolved ceiling is `Tier::Spawn`; otherwise pass the bounded policy. Set `ENTHEAI_MODE` in the spawned child env alongside `ENTHEAI_FANOUT_DEPTH`.

- [ ] **Step 4: Run, verify pass** — `cargo test -p entheai-orchestrator -p entheai-permission` → PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(orchestrator): subagent tier-ceiling policy + ENTHEAI_MODE propagation"`

---

## Phase 4 — TUI toggle (`crates/tui`)

### Task 8: Shift+Tab cycles the shared mode + status segment

**Files:**
- Modify: `crates/tui/src/lib.rs` (App field, `handle_key`, `status_line`, and the `Policy` handed to the run)

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn backtab_cycles_mode() {
    let mut app = test_app(); // mode starts at Ask
    handle_key(&mut app, KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
    assert_eq!(app.mode, entheai_permission::Mode::Plan); // Ask.next() == Plan
}

#[test]
fn status_line_shows_mode() {
    let app = test_app();
    let line = status_line(&app);
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("mode:"), "status shows the mode: {text:?}");
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p entheai-tui backtab_ status_line_shows_mode` → FAIL.
- [ ] **Step 3: Implement** — add `mode: entheai_permission::Mode` to `App` (init from config `[permission] mode`; update every `App { .. }` literal — the compiler lists them). In `handle_key`, before the normal arms: `KeyCode::BackTab => { app.mode = app.mode.next(); app.policy.set_mode(app.mode); app.notice = Some(format!("mode: {}", label(app.mode))); return Action::None; }` (thread the shared `Policy` onto `App`, or a `set_mode` callback). Add a colored `mode: <label>` span to `status_line` (plan=Cyan, auto=Green, yolo=Red, ask=Yellow), mirroring the pomodoro segment.

- [ ] **Step 4: Run, verify pass** — `cargo test -p entheai-tui` → PASS; `cargo clippy -p entheai-tui --all-targets -- -D warnings` clean.
- [ ] **Step 5: Commit** — `git commit -am "feat(tui): Shift+Tab mode cycle + status segment, writes shared Policy"`

---

## Phase 5 — Config (`crates/config`, `entheai.toml`)

### Task 9: `[permission] mode` + `pins`, `[fanout] mode`, docs

**Files:**
- Modify: `crates/config/src/lib.rs` (`PermissionConfig`, `FanoutConfig`), `entheai.toml`, `CHANGELOG.md`

- [ ] **Step 1: Failing test**

```rust
#[test]
fn permission_mode_and_pins_parse() {
    let cfg = Config::from_toml_str(
        "[permission]\nmode = \"auto\"\npins = { run_shell = \"always_ask\" }\n",
    ).unwrap();
    assert_eq!(cfg.permission.mode, "auto");
    assert_eq!(cfg.permission.pins.get("run_shell").map(String::as_str), Some("always_ask"));
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p entheai-config permission_mode_and_pins` → FAIL.
- [ ] **Step 3: Implement** — add `#[serde(default = "default_permission_mode")] pub mode: String` (default `"ask"`) and `#[serde(default)] pub pins: std::collections::HashMap<String, String>` to `PermissionConfig`; add `#[serde(default)] pub mode: String` to `FanoutConfig`. Wire the bin: build the main `Policy` with `Mode::parse(&cfg.permission.mode)` and apply pins (`Pin` parsed from the string: `always_allow`/`always_ask`/`never`). Document the block in `entheai.toml` and add a CHANGELOG entry.

- [ ] **Step 4: Run, verify pass** — `cargo test -p entheai-config` → PASS; `cargo build -p entheai` → OK.
- [ ] **Step 5: Commit** — `git commit -am "feat(config): [permission] mode + pins, [fanout] mode; docs + CHANGELOG"`

---

## Final verification

- [ ] `cargo clippy --workspace --all-targets -- -D warnings` → clean
- [ ] `cargo test --workspace` → all green
- [ ] Manual: launch the TUI, Shift+Tab through `plan → auto → yolo → ask`, confirm the status segment updates and a write tool is denied in `plan`.

## Self-review notes
- **Spec coverage:** tiers (T1, T5), mode+matrix (T2–T3), pins (T3), decide shim/compat (T4), tool routing (T6), subagent ceiling (T7), TUI toggle (T8), config (T9) — all spec sections mapped.
- **Type consistency:** `decide_tiered(&str, Tier) -> Decision`, `Mode::{next,ceiling,parse}`, `Pin::{AlwaysAllow,AlwaysAsk,Never}`, `Policy::{with_mode,with_ceiling,set_mode,pin}` used consistently across tasks.
- **Known read-first points:** the exact tool structs in `crates/tools` (T5) and the `run_task` test harness shape (T6) — the implementer reads those files; signatures above are fixed.
