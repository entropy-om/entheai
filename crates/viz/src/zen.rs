//! Zen view — the full-canvas living field. The operator's vision: one message
//! box, and the rest is entheai alive. This renders the whole [`BrainState`] as
//! a breathing composition across the entire content area — no panels, no
//! chrome — reusing the brain module's 3D projection so the field shares one
//! coherent rotation and depth.
//!
//! Layers (painted back to front):
//!   1. current-awareness motes — a drifting particle field that brightens
//!      when fresh world knowledge lands (`BrainState::current_glow`);
//!   2. the singularity core — a breathing centre star;
//!   3. faculties — luminous orbiting bodies (Model · Tools · Context), each
//!      sized and brightened by its activity, tethered to the core;
//!   4. the frozen constellation — doctrine nodes on a counter-rotating ring,
//!      brightness = their live wake glow, the awake ones labelled.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::canvas::{Canvas, Context, Line as CanvasLine};
use ratatui::widgets::Widget;
use ratatui::{buffer::Buffer, symbols::Marker};

use crate::brain::{depth_brightness, project, pulse, BrainState, FacultyKind, OMEGA};

/// A short human-facing label for a faculty.
fn faculty_label(kind: FacultyKind) -> &'static str {
    match kind {
        FacultyKind::Model => "model",
        FacultyKind::Tools => "tools",
        FacultyKind::Context => "context",
    }
}

/// Deterministic mote seed — a cheap hash so the particle field is stable
/// across frames (a mote keeps its identity) without storing per-mote state.
fn mote_seed(i: usize) -> f64 {
    // Fractional part of a large irrational multiple — a classic hash-free PRNG.
    let x = (i as f64 + 1.0) * 0.618_033_988_749_895;
    x.fract()
}

/// How many current-awareness motes to draw at a given glow. Bounded so a huge
/// canvas can't spawn an unbounded particle count.
fn mote_count(glow: f32) -> usize {
    (glow.clamp(0.0, 1.0) as f64 * 90.0) as usize
}

/// Paint the Zen field into `area`. `title` is the breathing header line (e.g.
/// "entheai · idle"); when `area` is too small to be meaningful, nothing is
/// drawn (never panics).
pub fn render(state: &BrainState, title: &str, area: Rect, buf: &mut Buffer, marker: Marker) {
    if area.width < 8 || area.height < 4 {
        return;
    }
    let frame = state.frame;
    let n_fac = state.faculties.len().max(1);
    let n_frozen = state.frozen.len();

    // Snapshot the fields the closure needs (Canvas' paint closure is 'static-ish
    // over borrows; keep it to Copy data + short slices).
    let faculties: Vec<(FacultyKind, f32)> = state
        .faculties
        .iter()
        .map(|f| (f.kind, f.activity))
        .collect();
    let frozen: Vec<(String, f32)> = state
        .frozen
        .iter()
        .map(|f| (f.name.clone(), f.awake))
        .collect();
    let current_glow = state.current_glow;

    let canvas = Canvas::default()
        .marker(marker)
        .x_bounds([-1.0, 1.0])
        .y_bounds([-1.0, 1.0])
        .paint(move |ctx: &mut Context| {
            // ── Layer 1: current-awareness motes ─────────────────────────────
            // Drifting points on lazy diagonal paths; density + brightness rise
            // with the glow. Positions are frame-animated but seed-stable.
            let motes = mote_count(current_glow);
            for i in 0..motes {
                let s = mote_seed(i);
                let s2 = mote_seed(i.wrapping_mul(2_654_435_761).rotate_left(3));
                // Drift: wrap across [-1,1] on both axes at mote-specific speeds.
                let px = ((s * 2.0 - 1.0) + frame as f64 * 0.004 * (0.5 + s)).rem_euclid(2.0) - 1.0;
                let py =
                    ((s2 * 2.0 - 1.0) - frame as f64 * 0.003 * (0.5 + s2)).rem_euclid(2.0) - 1.0;
                let tw = 0.5 + 0.5 * pulse(frame + i as u64 * 7, 0.9);
                let b = (current_glow as f64 * tw).clamp(0.0, 1.0);
                ctx.print(
                    px,
                    py,
                    Span::styled(
                        "·",
                        Style::default().fg(Color::Rgb(
                            (60.0 * b) as u8,
                            (150.0 * b) as u8,
                            (120.0 * b) as u8,
                        )),
                    ),
                );
            }
            ctx.layer();

            // ── Layer 2: the singularity core ────────────────────────────────
            let core = (0.5 + 0.5 * pulse(frame, 0.30)) as f32;
            ctx.print(
                0.0,
                0.0,
                Span::styled(
                    "✦",
                    Style::default().fg(Color::Rgb(
                        (130.0 * core) as u8,
                        (210.0 * core) as u8,
                        (230.0 * core) as u8,
                    )),
                ),
            );
            ctx.layer();

            // ── Layer 3: faculties as orbiting bodies ────────────────────────
            for (i, (kind, activity)) in faculties.iter().enumerate() {
                let a = i as f64 / n_fac as f64 * std::f64::consts::TAU;
                // Radius breathes outward a touch when the faculty is active.
                let r = 0.42 + 0.10 * *activity as f64;
                let (x, y, wz) = project(a, r, 0.12, frame);
                let db = depth_brightness(wz, r);
                // Tether to the core.
                let line_b = (0.25 + 0.55 * *activity) * db;
                ctx.draw(&CanvasLine {
                    x1: 0.0,
                    y1: 0.0,
                    x2: x,
                    y2: y,
                    color: Color::Rgb(
                        0,
                        (110.0 * line_b).min(255.0) as u8,
                        (150.0 * line_b).min(255.0) as u8,
                    ),
                });
                // The body: brighter + warmer with activity; a ✷ when lit, ○ at rest.
                let body_b = (0.45 + 0.55 * *activity) * db;
                let glyph = if *activity > 0.25 { "✷" } else { "◦" };
                ctx.print(
                    x,
                    y,
                    Span::styled(
                        glyph,
                        Style::default().fg(Color::Rgb(
                            (120.0 * body_b * *activity) as u8,
                            (200.0 * body_b) as u8,
                            (220.0 * body_b) as u8,
                        )),
                    ),
                );
                // Label just outside the body, only when the field is roomy in
                // the projection (avoid clutter near the far side).
                if wz > -0.1 {
                    ctx.print(
                        x,
                        y - 0.09,
                        Span::styled(
                            faculty_label(*kind),
                            Style::default().fg(Color::Rgb(
                                (60.0 * db) as u8,
                                (120.0 * db) as u8,
                                (140.0 * db) as u8,
                            )),
                        ),
                    );
                }
            }
            ctx.layer();

            // ── Layer 4: the frozen constellation ────────────────────────────
            // A wider, counter-rotating ring so doctrine reads as a distinct
            // sphere around the faculties. Awake nodes flare and label.
            if n_frozen > 0 {
                for (i, (name, awake)) in frozen.iter().enumerate() {
                    let a = i as f64 / n_frozen as f64 * std::f64::consts::TAU;
                    // Counter-rotate: negate the frame's angular contribution.
                    let theta = a - frame as f64 * OMEGA * 0.6;
                    let r = 0.80;
                    let wx = r * theta.cos();
                    let wz = r * theta.sin();
                    let x = wx;
                    let y = -0.28 - wz * 0.30;
                    let db = depth_brightness(wz, r);
                    let lit = *awake as f64;
                    let b = ((0.20 + 0.80 * lit) * db as f64).clamp(0.0, 1.0);
                    let glyph = if *awake > 0.5 {
                        "❄"
                    } else if *awake > 0.05 {
                        "✧"
                    } else {
                        "·"
                    };
                    ctx.print(
                        x,
                        y,
                        Span::styled(
                            glyph,
                            Style::default().fg(Color::Rgb(
                                (150.0 * b * lit) as u8,
                                (170.0 * b) as u8,
                                (210.0 * b) as u8,
                            )),
                        ),
                    );
                    if *awake > 0.4 && wz > -0.2 {
                        ctx.print(
                            x,
                            y - 0.08,
                            Span::styled(
                                name.clone(),
                                Style::default().fg(Color::Rgb(120, 150, 200)),
                            ),
                        );
                    }
                }
            }
        });

    canvas.render(area, buf);

    // The breathing title, centred on the top row — the one bit of text the
    // Zen field keeps, dimmed so it never competes with the message box.
    let breath = (0.45 + 0.55 * pulse(frame, 0.5)) as f32;
    let tcol = Color::Rgb(
        (110.0 * breath) as u8,
        (150.0 * breath) as u8,
        (170.0 * breath) as u8,
    );
    let tx = area.x + area.width.saturating_sub(title.chars().count() as u16) / 2;
    buf.set_string(tx, area.y, title, Style::default().fg(tcol));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::BrainState;

    fn scene() -> BrainState {
        let mut b = BrainState::new();
        b.flare(FacultyKind::Model);
        b.set_frozen(&["verification".to_string(), "docker".to_string()]);
        b.wake_frozen("verification");
        b.flare_current();
        b
    }

    fn render_to(w: u16, h: u16, state: &BrainState) -> Buffer {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        render(state, "entheai · alive", area, &mut buf, Marker::Braille);
        buf
    }

    #[test]
    fn tiny_and_huge_areas_never_panic() {
        for (w, h) in [(0, 0), (1, 1), (7, 3), (8, 4), (300, 120)] {
            let _ = render_to(w, h, &scene());
        }
    }

    #[test]
    fn title_is_centred_on_the_top_row() {
        let buf = render_to(60, 24, &scene());
        let top: String = (0..60)
            .map(|x| buf.cell((x, 0)).map(|c| c.symbol()).unwrap_or(" "))
            .collect();
        assert!(top.contains("entheai · alive"), "title row: {top:?}");
        // It's centred, not flush-left.
        assert!(
            top.starts_with(' '),
            "title should be indented, got {top:?}"
        );
    }

    #[test]
    fn mote_count_scales_with_glow_and_is_bounded() {
        assert_eq!(mote_count(0.0), 0);
        assert!(mote_count(1.0) <= 90);
        assert!(mote_count(0.5) > 0 && mote_count(0.5) < mote_count(1.0));
        // Out-of-range glow is clamped, never a huge allocation.
        assert!(mote_count(9.9) <= 90);
    }

    #[test]
    fn empty_state_still_renders_the_core_without_panicking() {
        let buf = render_to(40, 16, &BrainState::new());
        // Some non-space cell exists (the breathing core / title).
        let any = (0..40)
            .any(|x| (0..16).any(|y| buf.cell((x, y)).map(|c| c.symbol() != " ").unwrap_or(false)));
        assert!(any, "zen field drew nothing at all");
    }
}
