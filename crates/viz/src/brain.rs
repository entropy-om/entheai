//! Always-on "brain state" panel: a small living graph (the agent's faculties —
//! model, tools, context — the remote fleet, frozen-node ring, a rotation frame,
//! and readouts: worker count, NATS up, context %, compression ratio). State is
//! pure + terminal-agnostic, fed by the TUI from event arms it already has;
//! `render()` below draws it onto a ratatui `Canvas`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Context, Line as CanvasLine};
use ratatui::widgets::Widget;

/// Rotation speed, radians per animation tick.
pub(crate) const OMEGA: f64 = 0.06;
/// Per-tick activity decay factor (a flare eases back to a dim glow).
const DECAY: f32 = 0.90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FacultyKind {
    Model,
    Tools,
    Context,
}

#[derive(Debug, Clone)]
pub struct Faculty {
    pub kind: FacultyKind,
    /// 0..1, set to 1.0 on a `flare`, multiplied by `DECAY` each `tick`.
    pub activity: f32,
}

/// A remote worker as a graph node (snapshot of the last fleet poll).
#[derive(Debug, Clone)]
pub struct FleetNode {
    pub node_id: String,
    pub working: bool,
}

#[derive(Debug, Clone)]
pub struct FrozenGlow {
    pub name: String,
    pub awake: f32,
}

#[derive(Debug, Clone)]
pub struct BrainState {
    pub faculties: Vec<Faculty>,
    pub fleet: Vec<FleetNode>,
    pub frozen: Vec<FrozenGlow>,
    pub nats_up: bool,
    pub worker_count: usize,
    pub ctx_pct: u16,
    pub frame: u64,
    /// Fractional carry toward the next `frame` advance — lets rotation speed
    /// vary continuously (via `rotation_speed_factor`) while `frame` itself
    /// stays a plain integer for `project`/`pulse`.
    frame_carry: f64,
    /// Seconds since the user's last keyboard/mouse input, from a direct
    /// `user-idle` poll (not MCP — see `crates/tui`'s idle-poll timer).
    /// `None` when the sensor is unavailable; rotation then runs at full speed.
    idle_seconds: Option<u64>,
    /// Last compression cycle's output/input ratio, e.g. 0.42 == kept 42% of tokens.
    /// `compression_tokens == (0, 0)` means never compressed yet — `footer_line`
    /// checks that before rendering the readout.
    pub compression_ratio: f64,
    pub compression_tokens: (usize, usize),
    /// Current-awareness glow 0..=1: flares to 1.0 when fresh world knowledge
    /// lands in the soil (a `/current` pulse), decays each tick. Drives the
    /// overall intensity of the Zen mote field.
    pub current_glow: f32,
    /// Per-source current glow — which soil source fed the brain most recently
    /// (`"valyu"`, `"worldmonitor"`, `"dogfood"`, …), each 0..=1, decaying like
    /// `current_glow`. Lets the Zen field COLOR the motes by origin so the
    /// human can read what she's drinking. Kept small (one entry per source).
    pub current_by_source: Vec<(String, f32)>,
}

impl Default for BrainState {
    fn default() -> Self {
        Self::new()
    }
}

impl BrainState {
    pub fn new() -> Self {
        BrainState {
            faculties: vec![
                Faculty {
                    kind: FacultyKind::Model,
                    activity: 0.0,
                },
                Faculty {
                    kind: FacultyKind::Tools,
                    activity: 0.0,
                },
                Faculty {
                    kind: FacultyKind::Context,
                    activity: 0.0,
                },
            ],
            fleet: Vec::new(),
            frozen: Vec::new(),
            nats_up: false,
            worker_count: 0,
            ctx_pct: 0,
            frame: 0,
            frame_carry: 0.0,
            idle_seconds: None,
            compression_ratio: 0.0,
            compression_tokens: (0, 0),
            current_glow: 0.0,
            current_by_source: Vec::new(),
        }
    }

    pub fn set_frozen(&mut self, names: &[String]) {
        self.frozen = names
            .iter()
            .map(|n| FrozenGlow {
                name: n.clone(),
                awake: 0.0,
            })
            .collect();
    }

    pub fn wake_frozen(&mut self, name: &str) {
        if let Some(f) = self.frozen.iter_mut().find(|f| f.name == name) {
            f.awake = 1.0;
        }
    }

    pub fn frozen_awake(&self, name: &str) -> f32 {
        self.frozen
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.awake)
            .unwrap_or(0.0)
    }

    /// How many frozen nodes are currently "visibly awake" (awake > 0.05).
    pub fn frozen_awake_count(&self) -> usize {
        self.frozen.iter().filter(|f| f.awake > 0.05).count()
    }

    pub fn faculty(&self, kind: FacultyKind) -> &Faculty {
        self.faculties
            .iter()
            .find(|f| f.kind == kind)
            .expect("faculty exists")
    }

    /// How alive she is *right now*, 0..=1 — the aggregate the Zen field uses
    /// to make the whole composition surge when she thinks/acts/recalls/drinks
    /// and settle to a calm breath when idle. The loudest live signal wins
    /// (max, not sum) so a single strong pulse reads as vitality without three
    /// weak ones faking it.
    pub fn vitality(&self) -> f32 {
        let fac = self
            .faculties
            .iter()
            .map(|f| f.activity)
            .fold(0.0_f32, f32::max);
        let frozen = self.frozen.iter().map(|f| f.awake).fold(0.0_f32, f32::max);
        fac.max(frozen).max(self.current_glow).clamp(0.0, 1.0)
    }

    pub fn flare(&mut self, kind: FacultyKind) {
        if let Some(f) = self.faculties.iter_mut().find(|f| f.kind == kind) {
            f.activity = 1.0;
        }
    }

    /// Fresh world knowledge landed in the soil — light the current-awareness
    /// mote field (overall intensity, no source attribution). Decays each `tick`.
    pub fn flare_current(&mut self) {
        self.current_glow = 1.0;
    }

    /// Fresh knowledge from a NAMED source landed — light both the overall
    /// glow and that source's own glow, so the Zen field can colour its motes
    /// by origin. Repeated sources reuse their entry (no unbounded growth).
    pub fn flare_current_source(&mut self, source: &str) {
        self.current_glow = 1.0;
        if let Some(e) = self.current_by_source.iter_mut().find(|(s, _)| s == source) {
            e.1 = 1.0;
        } else {
            self.current_by_source.push((source.to_string(), 1.0));
        }
    }

    /// Per-source current glows, brightest first — read by the Zen renderer to
    /// colour motes and draw the legend. Only sources still visibly glowing.
    pub fn current_sources(&self) -> Vec<(String, f32)> {
        let mut v: Vec<(String, f32)> = self
            .current_by_source
            .iter()
            .filter(|(_, g)| *g > 0.05)
            .cloned()
            .collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        v
    }

    /// Advance rotation + decay every faculty's activity toward 0. Rotation
    /// speed scales with `rotation_speed_factor` (idle presence), so the
    /// panel visibly slows when the user's away and speeds back up when they
    /// return — `frame` itself stays a plain per-tick integer for
    /// `project`/`pulse`, driven by a fractional carry.
    pub fn tick(&mut self) {
        self.frame_carry += self.rotation_speed_factor();
        while self.frame_carry >= 1.0 {
            self.frame = self.frame.wrapping_add(1);
            self.frame_carry -= 1.0;
        }
        for f in &mut self.faculties {
            f.activity = (f.activity * DECAY).max(0.0);
        }
        for f in &mut self.frozen {
            f.awake = (f.awake * DECAY).max(0.0);
        }
        // Current glow decays slower than a faculty flare — fresh world
        // knowledge lingers as a soft shimmer, not a blink.
        self.current_glow = (self.current_glow * 0.97).max(0.0);
        for (_, g) in &mut self.current_by_source {
            *g = (*g * 0.97).max(0.0);
        }
        // Reap fully-faded sources so the vec stays bounded by live sources.
        self.current_by_source.retain(|(_, g)| *g > 0.001);
    }

    /// Report the user's idle time (seconds since last input), from a direct
    /// sensor poll — `None` if the sensor is unavailable, in which case
    /// rotation runs at full speed (today's behavior, unchanged).
    pub fn set_idle_seconds(&mut self, secs: Option<u64>) {
        self.idle_seconds = secs;
    }

    /// 1.0 at full speed (active, or idle state unknown); falls linearly to a
    /// 0.15 floor as idle time grows from 30s to 5 minutes, so the panel
    /// visibly slows when the user steps away but never fully stops.
    fn rotation_speed_factor(&self) -> f64 {
        match self.idle_seconds {
            None => 1.0,
            Some(s) if s < 30 => 1.0,
            Some(s) => {
                let t = ((s - 30) as f64 / 270.0).min(1.0);
                1.0 - 0.85 * t
            }
        }
    }

    pub fn set_fleet(&mut self, workers: &[(String, bool)]) {
        self.fleet = workers
            .iter()
            .map(|(id, working)| FleetNode {
                node_id: id.clone(),
                working: *working,
            })
            .collect();
        self.worker_count = self.fleet.len();
    }

    pub fn set_nats(&mut self, up: bool) {
        self.nats_up = up;
    }
    pub fn set_ctx_pct(&mut self, pct: u16) {
        self.ctx_pct = pct;
    }

    pub fn set_compression(&mut self, ratio: f64, input_tokens: usize, output_tokens: usize) {
        self.compression_ratio = ratio;
        self.compression_tokens = (input_tokens, output_tokens);
    }
}

/// A subtle sinusoidal pulse that modulates brightness by ~±15% over the
/// animation cycle, so nodes breathe even at rest.
pub(crate) fn pulse(frame: u64, magnitude: f64) -> f64 {
    1.0 + magnitude * (frame as f64 * 0.04).sin()
}

/// Project a node on a ring (radius `r`, vertical offset `y_off`) rotating about the
/// vertical axis by `frame`. Returns (screen_x, screen_y, depth); x/y land ~[-1,1].
pub(crate) fn project(angle: f64, r: f64, y_off: f64, frame: u64) -> (f64, f64, f64) {
    let theta = angle + frame as f64 * OMEGA;
    let wx = r * theta.cos();
    let wz = r * theta.sin();
    let sx = wx;
    let sy = y_off - wz * 0.35;
    (sx, sy, wz)
}

/// Nearer nodes (larger z) are brighter; result in [0.35, 1.0], monotonic in z.
pub(crate) fn depth_brightness(wz: f64, r: f64) -> f32 {
    let t = ((wz / r.max(1e-6)) + 1.0) / 2.0;
    (0.35 + 0.65 * t) as f32
}

/// Faculty connection-line color: teal at rest, warm-cyan when active.
fn faculty_line_color(activity: f32, db: f32) -> Color {
    if activity > 0.01 {
        // Blend from teal (0,90,90) toward warm cyan (0,180,220) as activity rises.
        let g = (90.0 + 90.0 * activity.min(1.0)) * db;
        let b = (90.0 + 130.0 * activity.min(1.0)) * db;
        Color::Rgb(
            0,
            (g.min(255.0) as u8).max(10),
            (b.min(255.0) as u8).max(10),
        )
    } else {
        // Resting teal.
        let g = (90.0 * db) as u8;
        Color::Rgb(0, g.max(10), g.max(10))
    }
}

/// Draw the brain panel into `area`: a rotating canvas (all rows but the last) +
/// a `wk N · nats ●/○ · ctx P%` footer on the bottom row.
pub fn render(state: &BrainState, area: Rect, buf: &mut Buffer, marker: Marker) {
    if area.width < 4 || area.height < 2 {
        return;
    }
    let canvas_area = Rect {
        height: area.height - 1,
        ..area
    };
    let n_fac = state.faculties.len().max(1);
    let n_fleet = state.fleet.len().max(1);

    let canvas = Canvas::default()
        .marker(marker)
        .x_bounds([-1.0, 1.0])
        .y_bounds([-1.0, 1.0])
        .paint(|ctx: &mut Context| {
            // Faculty-to-centre connection lines — colour shifts with activity.
            for (i, f) in state.faculties.iter().enumerate() {
                let a = i as f64 / n_fac as f64 * std::f64::consts::TAU;
                let (x, y, wz) = project(a, 0.45, 0.10, state.frame);
                let db = depth_brightness(wz, 0.45);
                ctx.draw(&CanvasLine {
                    x1: 0.0,
                    y1: 0.0,
                    x2: x,
                    y2: y,
                    color: faculty_line_color(f.activity, db),
                });
            }
            ctx.layer();
            // Centre glyph — a subtle pulsing star.
            let centre_bright = (0.47 + 0.53 * pulse(state.frame, 0.25)) as f32;
            ctx.print(
                0.0,
                0.0,
                Span::styled(
                    "✦",
                    Style::default().fg(Color::Rgb(
                        (120.0 * centre_bright) as u8,
                        (200.0 * centre_bright) as u8,
                        (220.0 * centre_bright) as u8,
                    )),
                ),
            );
            for (i, f) in state.faculties.iter().enumerate() {
                let a = i as f64 / n_fac as f64 * std::f64::consts::TAU;
                let (x, y, wz) = project(a, 0.45, 0.10, state.frame);
                let db = depth_brightness(wz, 0.45);
                let v = ((0.30 + 0.70 * f.activity) * db * 255.0) as u8;
                let glyph = match f.kind {
                    FacultyKind::Model => "M",
                    FacultyKind::Tools => "T",
                    FacultyKind::Context => "C",
                };
                ctx.print(
                    x,
                    y,
                    Span::styled(glyph, Style::default().fg(Color::Rgb(0, v, v))),
                );
            }
            for (i, node) in state.fleet.iter().enumerate() {
                let a = i as f64 / n_fleet as f64 * std::f64::consts::TAU;
                let (x, y, wz) = project(a, 0.85, -0.12, state.frame);
                let db = depth_brightness(wz, 0.85);
                let base: (u8, u8, u8) = if node.working {
                    (0, 220, 90)
                } else {
                    (90, 90, 90)
                };
                let col = Color::Rgb(
                    (base.0 as f32 * db) as u8,
                    (base.1 as f32 * db) as u8,
                    (base.2 as f32 * db) as u8,
                );
                ctx.print(x, y, Span::styled("•", Style::default().fg(col)));
            }
            // Frozen node ring — the dyad partner to `FrozenStore::wake`.
            // Each node sits on the outermost ring and glows (green-cyan)
            // proportionally to `awake`, with a subtle breath pulsing on top.
            // A freshly-woken node (awake > 0.85) briefly flashes brighter.
            let n_frozen = state.frozen.len().max(1);
            for (i, node) in state.frozen.iter().enumerate() {
                let a = i as f64 / n_frozen as f64 * std::f64::consts::TAU;
                let (x, y, wz) = project(a, 1.05, 0.05, state.frame);
                let db = depth_brightness(wz, 1.05);
                let p = pulse(state.frame, 0.15);
                // Combine: depth brightness * (awake level + breath) * wake-flash boost.
                let raw_v = (0.20 + 0.80 * node.awake) * p as f32;
                let v = (raw_v * db * 255.0) as u8;
                // Freshly woken → white-hot centre; settled → green-cyan glow.
                let (r, g, b) = if node.awake > 0.85 {
                    // White-hot flash: blends toward (200, 240, 255).
                    let flash = ((node.awake - 0.85) / 0.15).min(1.0);
                    (
                        (160.0 * flash * db) as u8,
                        ((200.0 + 40.0 * flash) * db) as u8,
                        ((220.0 + 35.0 * flash) * db) as u8,
                    )
                } else {
                    // Green-cyan resting glow: (0, g, v).
                    (0, (v as u16 * 200 / 255) as u8, v)
                };
                let ch = node
                    .name
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_else(|| "*".to_string());
                ctx.print(
                    x,
                    y,
                    Span::styled(ch, Style::default().fg(Color::Rgb(r, g, b))),
                );
            }
        });
    Widget::render(canvas, canvas_area, buf);

    let footer = footer_line(state);
    let _ = buf.set_line(area.x, area.bottom() - 1, &footer, area.width);
}

fn footer_line(state: &BrainState) -> Line<'static> {
    let (nats_glyph, nats_col) = if state.nats_up {
        ("●", Color::Green)
    } else {
        ("○", Color::DarkGray)
    };
    let ctx_col = if state.ctx_pct >= 85 {
        Color::Red
    } else if state.ctx_pct >= 60 {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    let mut spans = vec![
        Span::styled(
            format!("wk {}", state.worker_count),
            Style::default().fg(Color::Gray),
        ),
        Span::raw(" · nats "),
        Span::styled(nats_glyph, Style::default().fg(nats_col)),
        Span::raw(" · "),
        Span::styled(
            format!("ctx {}%", state.ctx_pct),
            Style::default().fg(ctx_col),
        ),
    ];
    if state.compression_tokens.0 > 0 || state.compression_tokens.1 > 0 {
        let pct = (state.compression_ratio * 100.0).round() as i64;
        let (inp, out) = state.compression_tokens;
        spans.push(Span::raw(" · "));
        spans.push(Span::styled(
            format!("kx {pct}%"),
            Style::default().fg(Color::Magenta),
        ));
        // Show input→output token counts when both are known: e.g.
        // "kx 42% · 1.2k→500t" so the operator sees the absolute reduction,
        // not only the ratio.
        if inp > 0 && out > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("{}→{}t", fmt_tokens(inp), fmt_tokens(out)),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ));
        }
    }
    Line::from(spans)
}

/// Compact token count label: `950`, `18.4k`, `1.2M`.
fn fmt_tokens(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vitality_is_the_loudest_live_signal_and_settles_when_idle() {
        let mut b = BrainState::new();
        assert_eq!(b.vitality(), 0.0, "idle is calm");

        b.flare(FacultyKind::Model);
        assert!(b.vitality() > 0.9, "thinking surges");

        // Decays back toward calm over time.
        for _ in 0..200 {
            b.tick();
        }
        assert!(b.vitality() < 0.05, "settles when the work stops");

        // Max wins, not sum: a single strong signal reads as full vitality,
        // and three weak ones don't fake a surge past the strongest.
        let mut c = BrainState::new();
        c.set_frozen(&["v".to_string()]);
        c.wake_frozen("v");
        assert!(
            c.vitality() > 0.9,
            "a woken doctrine node alone is vitality"
        );

        let mut d = BrainState::new();
        d.flare_current_source("dogfood");
        assert!(d.vitality() > 0.9, "fresh soil alone is vitality");
        assert!(d.vitality() <= 1.0, "always clamped");
    }

    #[test]
    fn frozen_node_wakes_and_melts() {
        let mut b = BrainState::new();
        b.set_frozen(&["nixos".to_string(), "ngrok".to_string()]);
        assert_eq!(b.frozen_awake("nixos"), 0.0, "starts frozen");
        b.wake_frozen("nixos");
        assert_eq!(b.frozen_awake("nixos"), 1.0, "wakes fully");
        for _ in 0..200 {
            b.tick();
        }
        assert!(b.frozen_awake("nixos") < 0.02, "melts back toward frozen");
    }

    #[test]
    fn flare_sets_full_activity_then_decays_bounded() {
        let mut b = BrainState::new();
        assert_eq!(b.faculty(FacultyKind::Model).activity, 0.0);
        b.flare(FacultyKind::Model);
        assert_eq!(b.faculty(FacultyKind::Model).activity, 1.0);
        b.tick();
        let a1 = b.faculty(FacultyKind::Model).activity;
        assert!(
            (0.0..1.0).contains(&a1),
            "decays and stays non-negative: {a1}"
        );
        for _ in 0..200 {
            b.tick();
        }
        let a = b.faculty(FacultyKind::Model).activity;
        assert!((0.0..0.02).contains(&a), "eases to ~0: {a}");
    }

    #[test]
    fn tick_advances_frame() {
        let mut b = BrainState::new();
        assert_eq!(b.frame, 0);
        b.tick();
        assert_eq!(b.frame, 1);
    }

    #[test]
    fn set_fleet_maps_working_and_counts() {
        let mut b = BrainState::new();
        b.set_fleet(&[("aaa".to_string(), true), ("bbb".to_string(), false)]);
        assert_eq!(b.worker_count, 2);
        assert_eq!(b.fleet.len(), 2);
        assert!(b.fleet[0].working);
        assert!(!b.fleet[1].working);
        b.set_fleet(&[]);
        assert_eq!(b.worker_count, 0);
        assert!(b.fleet.is_empty());
    }

    #[test]
    fn nats_and_ctx_round_trip() {
        let mut b = BrainState::new();
        b.set_nats(true);
        b.set_ctx_pct(42);
        assert!(b.nats_up);
        assert_eq!(b.ctx_pct, 42);
    }

    #[test]
    fn compression_round_trip_and_footer_shows_ratio_and_token_arrow() {
        let mut b = BrainState::new();
        b.set_compression(0.42, 1000, 420);
        assert!((b.compression_ratio - 0.42).abs() < 1e-9);
        assert_eq!(b.compression_tokens, (1000, 420));

        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::symbols::Marker;
        let area = Rect::new(0, 0, 50, 12);
        let mut buf = Buffer::empty(area);
        render(&b, area, &mut buf, Marker::Braille);
        let y = area.bottom() - 1;
        let mut row = String::new();
        for x in area.left()..area.right() {
            row.push_str(buf[(x, y)].symbol());
        }
        assert!(
            row.contains("kx 42%"),
            "footer compression readout missing: {row:?}"
        );
        assert!(row.contains("→"), "token arrow should be present: {row:?}");
    }

    #[test]
    fn zero_compression_activity_omits_readout() {
        let b = BrainState::new();
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::symbols::Marker;
        let area = Rect::new(0, 0, 40, 12);
        let mut buf = Buffer::empty(area);
        render(&b, area, &mut buf, Marker::Braille);
        let y = area.bottom() - 1;
        let mut row = String::new();
        for x in area.left()..area.right() {
            row.push_str(buf[(x, y)].symbol());
        }
        assert!(
            !row.contains("kx"),
            "no compression activity yet, readout should be absent: {row:?}"
        );
    }

    #[test]
    fn projection_periodic_and_depth_monotonic() {
        let period = (2.0 * std::f64::consts::PI / OMEGA).round() as u64;
        let (x0, y0, _) = project(0.0, 0.5, 0.0, 0);
        let (xp, yp, _) = project(0.0, 0.5, 0.0, period);
        assert!(
            (x0 - xp).abs() < 2e-2 && (y0 - yp).abs() < 2e-2,
            "one rotation returns near start"
        );
        assert!(
            depth_brightness(0.4, 0.5) > depth_brightness(-0.4, 0.5),
            "nearer = brighter"
        );
        let db = depth_brightness(0.0, 0.5);
        assert!((0.0..=1.0).contains(&db));
    }

    #[test]
    fn tick_advances_frame_by_one_when_idle_unknown_or_active() {
        let mut b = BrainState::new();
        b.tick();
        assert_eq!(b.frame, 1);
        b.tick();
        assert_eq!(b.frame, 2);

        b.set_idle_seconds(Some(5)); // below the 30s slow-down threshold
        b.tick();
        assert_eq!(b.frame, 3);
    }

    #[test]
    fn tick_slows_rotation_when_idle_and_never_fully_stops() {
        let mut b = BrainState::new();
        b.set_idle_seconds(Some(300)); // >= 5 min idle -> speed floor (0.15x)
        for _ in 0..20 {
            b.tick();
        }
        // At 0.15x speed, 20 ticks accumulate 3.0 of frame_carry -> frame == 3.
        assert_eq!(b.frame, 3);
        assert!(
            b.frame > 0,
            "rotation must still advance, never freeze solid"
        );
    }

    #[test]
    fn tick_resumes_full_speed_when_idle_clears() {
        let mut b = BrainState::new();
        b.set_idle_seconds(Some(600));
        for _ in 0..10 {
            b.tick();
        }
        let slow_frame = b.frame;
        b.set_idle_seconds(None); // user returned
        for _ in 0..10 {
            b.tick();
        }
        assert_eq!(b.frame, slow_frame + 10, "full speed resumes immediately");
    }

    #[test]
    fn render_small_buffer_no_panic_and_footer() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::symbols::Marker;
        let mut b = BrainState::new();
        b.set_nats(true);
        b.set_ctx_pct(42);
        b.set_fleet(&[("n1".to_string(), true)]);
        b.flare(FacultyKind::Tools);
        let area = Rect::new(0, 0, 26, 12);
        let mut buf = Buffer::empty(area);
        render(&b, area, &mut buf, Marker::Braille);
        // read the footer (bottom) row into a string — use the SAME cell-access API swarm.rs / the TUI uses
        let y = area.bottom() - 1;
        let mut row = String::new();
        for x in area.left()..area.right() {
            row.push_str(buf[(x, y)].symbol());
        }
        assert!(row.contains("wk 1"), "footer worker count missing: {row:?}");
        assert!(row.contains("42%"), "footer ctx pct missing: {row:?}");
        assert!(row.contains('●'), "nats-up marker missing: {row:?}");
    }
}
