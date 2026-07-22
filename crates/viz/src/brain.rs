//! Always-on "brain state" panel model (Slice A). Rendering is added in a later task.
//!
//! A small living graph: the agent's faculties (model, tools, context), the remote
//! fleet, a rotation frame, and readouts (worker count, NATS up, context %). Pure +
//! terminal-agnostic; fed by the TUI from event arms it already has.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Context, Line as CanvasLine};
use ratatui::widgets::Widget;

/// Rotation speed, radians per animation tick.
const OMEGA: f64 = 0.06;
/// Per-tick activity decay factor (a flare eases back to a dim glow).
const DECAY: f32 = 0.90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FacultyKind { Model, Tools, Context }

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
pub struct BrainState {
    pub faculties: Vec<Faculty>,
    pub fleet: Vec<FleetNode>,
    pub nats_up: bool,
    pub worker_count: usize,
    pub ctx_pct: u16,
    pub frame: u64,
}

impl Default for BrainState {
    fn default() -> Self { Self::new() }
}

impl BrainState {
    pub fn new() -> Self {
        BrainState {
            faculties: vec![
                Faculty { kind: FacultyKind::Model, activity: 0.0 },
                Faculty { kind: FacultyKind::Tools, activity: 0.0 },
                Faculty { kind: FacultyKind::Context, activity: 0.0 },
            ],
            fleet: Vec::new(),
            nats_up: false,
            worker_count: 0,
            ctx_pct: 0,
            frame: 0,
        }
    }

    pub fn faculty(&self, kind: FacultyKind) -> &Faculty {
        self.faculties.iter().find(|f| f.kind == kind).expect("faculty exists")
    }

    pub fn flare(&mut self, kind: FacultyKind) {
        if let Some(f) = self.faculties.iter_mut().find(|f| f.kind == kind) {
            f.activity = 1.0;
        }
    }

    /// Advance rotation + decay every faculty's activity toward 0.
    pub fn tick(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        for f in &mut self.faculties {
            f.activity = (f.activity * DECAY).max(0.0);
        }
    }

    pub fn set_fleet(&mut self, workers: &[(String, bool)]) {
        self.fleet = workers
            .iter()
            .map(|(id, working)| FleetNode { node_id: id.clone(), working: *working })
            .collect();
        self.worker_count = self.fleet.len();
    }

    pub fn set_nats(&mut self, up: bool) { self.nats_up = up; }
    pub fn set_ctx_pct(&mut self, pct: u16) { self.ctx_pct = pct; }
}

/// Project a node on a ring (radius `r`, vertical offset `y_off`) rotating about the
/// vertical axis by `frame`. Returns (screen_x, screen_y, depth); x/y land ~[-1,1].
fn project(angle: f64, r: f64, y_off: f64, frame: u64) -> (f64, f64, f64) {
    let theta = angle + frame as f64 * OMEGA;
    let wx = r * theta.cos();
    let wz = r * theta.sin();
    let sx = wx;
    let sy = y_off - wz * 0.35;
    (sx, sy, wz)
}

/// Nearer nodes (larger z) are brighter; result in [0.35, 1.0], monotonic in z.
fn depth_brightness(wz: f64, r: f64) -> f32 {
    let t = ((wz / r.max(1e-6)) + 1.0) / 2.0;
    (0.35 + 0.65 * t) as f32
}

/// Draw the brain panel into `area`: a rotating canvas (all rows but the last) +
/// a `wk N · nats ●/○ · ctx P%` footer on the bottom row.
pub fn render(state: &BrainState, area: Rect, buf: &mut Buffer, marker: Marker) {
    if area.width < 4 || area.height < 2 {
        return;
    }
    let canvas_area = Rect { height: area.height - 1, ..area };
    let n_fac = state.faculties.len().max(1);
    let n_fleet = state.fleet.len().max(1);

    let canvas = Canvas::default()
        .marker(marker)
        .x_bounds([-1.0, 1.0])
        .y_bounds([-1.0, 1.0])
        .paint(|ctx: &mut Context| {
            for (i, _f) in state.faculties.iter().enumerate() {
                let a = i as f64 / n_fac as f64 * std::f64::consts::TAU;
                let (x, y, wz) = project(a, 0.45, 0.10, state.frame);
                let g = (depth_brightness(wz, 0.45) * 90.0) as u8;
                ctx.draw(&CanvasLine { x1: 0.0, y1: 0.0, x2: x, y2: y, color: Color::Rgb(0, g, g) });
            }
            ctx.layer();
            ctx.print(0.0, 0.0, Span::styled("✦", Style::default().fg(Color::Rgb(120, 200, 220))));
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
                ctx.print(x, y, Span::styled(glyph, Style::default().fg(Color::Rgb(0, v, v))));
            }
            for (i, node) in state.fleet.iter().enumerate() {
                let a = i as f64 / n_fleet as f64 * std::f64::consts::TAU;
                let (x, y, wz) = project(a, 0.85, -0.12, state.frame);
                let db = depth_brightness(wz, 0.85);
                let base: (u8, u8, u8) = if node.working { (0, 220, 90) } else { (90, 90, 90) };
                let col = Color::Rgb(
                    (base.0 as f32 * db) as u8,
                    (base.1 as f32 * db) as u8,
                    (base.2 as f32 * db) as u8,
                );
                ctx.print(x, y, Span::styled("•", Style::default().fg(col)));
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
    Line::from(vec![
        Span::styled(format!("wk {}", state.worker_count), Style::default().fg(Color::Gray)),
        Span::raw(" · nats "),
        Span::styled(nats_glyph, Style::default().fg(nats_col)),
        Span::raw(" · "),
        Span::styled(format!("ctx {}%", state.ctx_pct), Style::default().fg(ctx_col)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flare_sets_full_activity_then_decays_bounded() {
        let mut b = BrainState::new();
        assert_eq!(b.faculty(FacultyKind::Model).activity, 0.0);
        b.flare(FacultyKind::Model);
        assert_eq!(b.faculty(FacultyKind::Model).activity, 1.0);
        b.tick();
        let a1 = b.faculty(FacultyKind::Model).activity;
        assert!((0.0..1.0).contains(&a1), "decays and stays non-negative: {a1}");
        for _ in 0..200 { b.tick(); }
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
    fn projection_periodic_and_depth_monotonic() {
        let period = (2.0 * std::f64::consts::PI / OMEGA).round() as u64;
        let (x0, y0, _) = project(0.0, 0.5, 0.0, 0);
        let (xp, yp, _) = project(0.0, 0.5, 0.0, period);
        assert!((x0 - xp).abs() < 2e-2 && (y0 - yp).abs() < 2e-2, "one rotation returns near start");
        assert!(depth_brightness(0.4, 0.5) > depth_brightness(-0.4, 0.5), "nearer = brighter");
        let db = depth_brightness(0.0, 0.5);
        assert!((0.0..=1.0).contains(&db));
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
