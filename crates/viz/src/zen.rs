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

/// How many current-awareness motes to draw at a given glow, uniform fallback
/// (no source attribution). Bounded so a huge canvas can't spawn unbounded.
fn mote_count(glow: f32) -> usize {
    (glow.clamp(0.0, 1.0) as f64 * 90.0) as usize
}

/// Motes for ONE named source at a given glow — smaller cap so several sources
/// sum to a sane total (3 sources at full ≈ 120 motes).
fn motes_for_source(glow: f32) -> usize {
    (glow.clamp(0.0, 1.0) as f64 * 40.0) as usize
}

/// The colour a soil source paints its motes and legend dot. dogfood — her own
/// genetic lineage — burns warm gold, set apart from the world-sources; the
/// live world reads cool (valyu cyan, worldmonitor green); anything else a soft
/// violet. Base hue at full brightness; the renderer scales by glow.
fn source_hue(source: &str) -> (u8, u8, u8) {
    match source {
        "dogfood" => (230, 180, 70),      // gold — the corpus she grew from
        "valyu" => (70, 190, 210),        // cyan — AI-native search
        "worldmonitor" => (90, 200, 120), // green — the living world
        _ => (170, 130, 210),             // violet — unknown origin
    }
}

/// Short legend label for a source (the dot is drawn separately, coloured).
fn source_label(source: &str) -> &str {
    match source {
        "dogfood" => "lineage",
        "valyu" => "search",
        "worldmonitor" => "world",
        other => other,
    }
}

/// Frame-animated, seed-stable position + twinkle for mote `i` in seed `band`
/// (0 = uniform fallback; a source's index+1 otherwise, so sources occupy
/// distinct drifts). Returns `(x, y, twinkle)` with x/y in [-1, 1].
fn mote_pos(i: usize, band: usize, frame: u64) -> (f64, f64, f64) {
    let k = i + band * 10_000;
    let s = mote_seed(k);
    let s2 = mote_seed(k.wrapping_mul(2_654_435_761).rotate_left(3));
    let px = ((s * 2.0 - 1.0) + frame as f64 * 0.004 * (0.5 + s)).rem_euclid(2.0) - 1.0;
    let py = ((s2 * 2.0 - 1.0) - frame as f64 * 0.003 * (0.5 + s2)).rem_euclid(2.0) - 1.0;
    let tw = 0.5 + 0.5 * pulse(frame + k as u64 * 7, 0.9);
    (px, py, tw)
}

/// A mote glyph at brightness `b` (0..=1) over base hue `(r, g, bl)`.
fn mote_span(hue: (u8, u8, u8), b: f64) -> Span<'static> {
    let (r, g, bl) = hue;
    Span::styled(
        "·",
        Style::default().fg(Color::Rgb(
            (r as f64 * b) as u8,
            (g as f64 * b) as u8,
            (bl as f64 * b) as u8,
        )),
    )
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
    // Per-source glows, brightest first, with their hues precomputed. When this
    // is non-empty the mote field is COLOURED by origin; when empty (a generic
    // `flare_current`) it falls back to the uniform teal below.
    let sources: Vec<((u8, u8, u8), f32)> = state
        .current_sources()
        .into_iter()
        .map(|(name, glow)| (source_hue(&name), glow))
        .collect();

    let canvas = Canvas::default()
        .marker(marker)
        .x_bounds([-1.0, 1.0])
        .y_bounds([-1.0, 1.0])
        .paint(move |ctx: &mut Context| {
            // ── Layer 1: current-awareness motes ─────────────────────────────
            // Drifting points on lazy diagonal paths; density + brightness rise
            // with the glow. Positions are frame-animated but seed-stable. When
            // sources are known, each paints its own hue in its own seed band;
            // otherwise a single teal field (the generic-flare fallback).
            if sources.is_empty() {
                for i in 0..mote_count(current_glow) {
                    let (px, py, tw) = mote_pos(i, 0, frame);
                    let b = (current_glow as f64 * tw).clamp(0.0, 1.0);
                    ctx.print(px, py, mote_span((60, 150, 120), b));
                }
            } else {
                for (si, ((r, g, bl), glow)) in sources.iter().enumerate() {
                    for i in 0..motes_for_source(*glow) {
                        let (px, py, tw) = mote_pos(i, si + 1, frame);
                        let b = (*glow as f64 * tw).clamp(0.0, 1.0);
                        ctx.print(px, py, mote_span((*r, *g, *bl), b));
                    }
                }
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

    // Legend, bottom-left: one coloured dot + label per source she's drinking
    // from right now, so the field is READABLE — "● lineage  ● world" tells you
    // the gold motes are her own corpus and the green ones are the live world.
    let legend = state.current_sources();
    if !legend.is_empty() && area.height >= 2 {
        let row = area.y + area.height - 1;
        let mut cx = area.x + 1;
        for (name, glow) in legend.iter() {
            let (r, g, b) = source_hue(name);
            let br = (0.4 + 0.6 * *glow).clamp(0.0, 1.0) as f64;
            let dot = Color::Rgb(
                (r as f64 * br) as u8,
                (g as f64 * br) as u8,
                (b as f64 * br) as u8,
            );
            let label = source_label(name);
            if cx + label.len() as u16 + 3 > area.x + area.width {
                break; // out of room — legend never spills off the field
            }
            buf.set_string(cx, row, "●", Style::default().fg(dot));
            buf.set_string(
                cx + 2,
                row,
                label,
                Style::default().fg(Color::Rgb(90, 110, 130)),
            );
            cx += label.len() as u16 + 4;
        }
    }
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

    #[test]
    fn source_hues_are_distinct_and_dogfood_is_the_warm_one() {
        let (dr, dg, db) = source_hue("dogfood");
        let (vr, vg, vb) = source_hue("valyu");
        let (wr, wg, wb) = source_hue("worldmonitor");
        // dogfood is warm (red dominates); the world-sources are cool (blue/green).
        assert!(
            dr > db && dr > 150,
            "dogfood should burn warm gold: {dr},{dg},{db}"
        );
        assert!(vb >= vr, "valyu should read cool/cyan: {vr},{vg},{vb}");
        assert!(
            wg > wr && wg > wb,
            "worldmonitor should read green: {wr},{wg},{wb}"
        );
        // All three are distinct, and the unknown fallback differs too.
        let hues = [(dr, dg, db), (vr, vg, vb), (wr, wg, wb), source_hue("???")];
        for i in 0..hues.len() {
            for j in (i + 1)..hues.len() {
                assert_ne!(hues[i], hues[j], "hue collision at {i},{j}");
            }
        }
    }

    #[test]
    fn per_source_motes_scale_and_stay_bounded() {
        assert_eq!(motes_for_source(0.0), 0);
        assert!(motes_for_source(1.0) <= 40, "per-source cap");
        assert!(motes_for_source(0.5) < motes_for_source(1.0));
        assert!(motes_for_source(9.9) <= 40, "out-of-range clamped");
    }

    #[test]
    fn mote_pos_is_deterministic_in_band_and_bounded_to_the_field() {
        // Same (i, band, frame) → same position (seed-stable across frames).
        assert_eq!(mote_pos(3, 1, 100), mote_pos(3, 1, 100));
        // Different bands place the same index differently (sources don't stack).
        assert_ne!(mote_pos(3, 1, 100), mote_pos(3, 2, 100));
        // Always inside the canvas bounds; twinkle is a positive multiplier
        // (~[0.55, 1.45]) that the renderer clamps after scaling by glow.
        for i in 0..50 {
            for band in 0..4 {
                let (x, y, tw) = mote_pos(i, band, 12_345);
                assert!((-1.0..=1.0).contains(&x) && (-1.0..=1.0).contains(&y));
                assert!(tw > 0.0 && tw < 2.0);
            }
        }
    }

    #[test]
    fn field_colours_motes_and_draws_legend_per_source() {
        let mut b = BrainState::new();
        b.flare_current_source("dogfood");
        b.flare_current_source("worldmonitor");
        let buf = render_to(80, 30, &b);

        // The legend row (bottom) carries the coloured dots + labels.
        let bottom: String = (0..80)
            .map(|x| buf.cell((x, 29)).map(|c| c.symbol()).unwrap_or(" "))
            .collect();
        assert!(bottom.contains('●'), "legend dot missing: {bottom:?}");
        assert!(
            bottom.contains("lineage"),
            "dogfood label missing: {bottom:?}"
        );
        assert!(
            bottom.contains("world"),
            "worldmonitor label missing: {bottom:?}"
        );

        // A gold mote (red-dominant, dogfood's hue) exists somewhere in the field.
        let gold = (0..80).any(|x| {
            (1..29).any(|y| {
                buf.cell((x, y))
                    .and_then(|c| match c.style().fg {
                        Some(Color::Rgb(r, g, bl)) => Some(r > g && r > bl && r > 40),
                        _ => None,
                    })
                    .unwrap_or(false)
            })
        });
        assert!(gold, "no gold (dogfood) mote painted in the field");
    }
}
