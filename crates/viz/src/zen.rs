//! Zen view — the full-canvas living field. The operator's vision: one message
//! box, and the rest is entheai alive. This renders the whole [`BrainState`] as
//! a breathing composition across the entire content area — no panels, no
//! chrome — reusing the brain module's 3D projection so the field shares one
//! coherent rotation and depth.
//!
//! Layers (painted back to front):
//!   1. current-awareness motes — a drifting particle field, coloured by the
//!      soil source that fed her (global identity colours — see [`palette`]);
//!   2. the singularity core — a breathing centre star whose heartbeat and
//!      aura surge with [`BrainState::vitality`];
//!   3. faculties — luminous orbiting bodies (Model · Tools · Context), each
//!      sized and brightened by its activity, tethered to the core;
//!   4. the frozen constellation — doctrine nodes on a counter-rotating ring,
//!      brightness = their live wake glow, the awake ones labelled;
//!   5. response-as-light — her reply ignites character by character in the
//!      field, holds, fades to ember, then dissolves into motes (the words
//!      become soil).
//!
//! All ambient colour comes from a [`Palette`] theme; source identity colours
//! never change with the theme (the entity rule).

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::canvas::{Canvas, Context, Line as CanvasLine};
use ratatui::widgets::Widget;
use ratatui::{buffer::Buffer, symbols::Marker};

use crate::brain::{depth_brightness, project, pulse, BrainState, FacultyKind, OMEGA};
use crate::palette::{self, lerp, Palette, Rgb};

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

/// The GLOBAL identity colour of a soil source — deliberately not a theme
/// slot: a theme swap must never repaint what lineage/search/world are. The
/// set is machine-validated for CVD + normal-vision separation (all pairs;
/// provenance in [`palette`]'s module doc).
fn source_hue(source: &str) -> Rgb {
    match source {
        "dogfood" => palette::SOURCE_LINEAGE,
        "valyu" => palette::SOURCE_SEARCH,
        "worldmonitor" => palette::SOURCE_WORLD,
        _ => palette::SOURCE_UNKNOWN,
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
/// distinct drifts). Returns `(x, y, twinkle)` with x/y in [-1, 1]; twinkle is
/// a positive brightness multiplier (~[0.55, 1.45]) clamped after glow-scaling.
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
fn mote_span(hue: Rgb, b: f64) -> Span<'static> {
    Span::styled("·", Style::default().fg(scale(hue, b)))
}

/// Scale a full-brightness colour by factor `f` (clamped to [0, 1]).
fn scale(c: Rgb, f: f64) -> Color {
    let f = f.clamp(0.0, 1.0);
    Color::Rgb(
        (c.0 as f64 * f) as u8,
        (c.1 as f64 * f) as u8,
        (c.2 as f64 * f) as u8,
    )
}

// ── Response-as-light: the reveal envelope ──────────────────────────────────
//
// Tuned for the default viz tick (~11/s at `tick_ms = 90`): ignition sweeps
// ~88 chars/s, the full text holds ~8 s, fades to ember over ~5 s, then
// dissolves into motes over ~3 s. All phases are pure functions of age.

/// Characters ignited per tick.
const REVEAL_CHARS_PER_TICK: u64 = 8;
/// Ticks the fully-ignited text holds at full brightness.
const REVEAL_HOLD_TICKS: u64 = 90;
/// Ticks of the fade from full brightness down to the ember floor.
const REVEAL_FADE_TICKS: u64 = 55;
/// Ticks of the final dissolve (text → motes → gone).
const REVEAL_DISSOLVE_TICKS: u64 = 33;
/// Brightness floor the fade lands on before the dissolve.
const REVEAL_EMBER: f64 = 0.18;

/// One frame of the reveal ceremony, derived purely from `age` (in viz ticks)
/// and the reply's total character count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RevealFrame {
    /// How many characters are ignited (shown) so far.
    pub visible: usize,
    /// Text brightness 0..=1.
    pub brightness: f64,
    /// 0..=1 — fraction of characters already dissolved into motes.
    pub dissolve: f64,
    /// The ceremony is over; draw nothing.
    pub done: bool,
}

/// The reveal envelope: ignite → hold → fade → dissolve → done.
pub fn reveal_envelope(age: u64, total_chars: usize) -> RevealFrame {
    if total_chars == 0 {
        return RevealFrame {
            visible: 0,
            brightness: 0.0,
            dissolve: 1.0,
            done: true,
        };
    }
    let ignite = (total_chars as u64).div_ceil(REVEAL_CHARS_PER_TICK);
    let hold_end = ignite + REVEAL_HOLD_TICKS;
    let fade_end = hold_end + REVEAL_FADE_TICKS;
    let end = fade_end + REVEAL_DISSOLVE_TICKS;
    if age >= end {
        return RevealFrame {
            visible: total_chars,
            brightness: 0.0,
            dissolve: 1.0,
            done: true,
        };
    }
    let visible =
        ((age.saturating_add(1)) * REVEAL_CHARS_PER_TICK).min(total_chars as u64) as usize;
    if age < hold_end {
        // Ignition + hold: full brightness while characters sweep in and rest.
        RevealFrame {
            visible,
            brightness: 1.0,
            dissolve: 0.0,
            done: false,
        }
    } else if age < fade_end {
        // Fade: full → ember.
        let t = (age - hold_end) as f64 / REVEAL_FADE_TICKS as f64;
        RevealFrame {
            visible,
            brightness: 1.0 - (1.0 - REVEAL_EMBER) * t,
            dissolve: 0.0,
            done: false,
        }
    } else {
        // Dissolve: ember → dark while characters crumble into motes.
        let t = (age - fade_end) as f64 / REVEAL_DISSOLVE_TICKS as f64;
        RevealFrame {
            visible,
            brightness: REVEAL_EMBER * (1.0 - t),
            dissolve: t,
            done: false,
        }
    }
}

/// Char-based wrap (no word awareness — the ceremony favours even blocks) to
/// `width` columns; empty input yields no lines.
fn wrap_chars(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for raw in text.lines() {
        let chars: Vec<char> = raw.chars().collect();
        if chars.is_empty() {
            lines.push(String::new());
            continue;
        }
        for chunk in chars.chunks(width) {
            lines.push(chunk.iter().collect());
        }
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines
}

/// Deterministic per-character dissolve lot: char `i` crumbles once `dissolve`
/// crosses its fixed threshold — holes accumulate monotonically, no flicker.
fn dissolved(i: usize, dissolve: f64) -> bool {
    let lot = ((i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 33) % 100;
    (lot as f64) < dissolve * 100.0
}

/// Draw the reveal ceremony over the field (cell space — crisper than canvas
/// for text). The block sits just below the core, centred; if the reply is
/// taller than the field allows, the head is shown with an honest pointer to
/// the full text in chat view.
fn render_reveal(text: &str, age: u64, p: &Palette, area: Rect, buf: &mut Buffer) {
    let env = reveal_envelope(age, text.chars().count());
    if env.done || area.width < 16 || area.height < 8 {
        return;
    }
    let width = (area.width.saturating_sub(10) as usize).clamp(12, 76);
    let mut lines = wrap_chars(text, width);
    let max_lines = ((area.height / 3) as usize).max(3);
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        lines.push("… full reply lives in chat (Ctrl-G)".to_string());
    }
    // Anchor just below the core; clamp so the block never spills off-field.
    let block_h = lines.len() as u16;
    let mut y = area.y + area.height / 2 + 2;
    let bottom = area.y + area.height.saturating_sub(1);
    if y + block_h > bottom {
        y = bottom.saturating_sub(block_h).max(area.y + 1);
    }
    let mut idx = 0usize; // running char index across the whole block
    for (li, line) in lines.iter().enumerate() {
        let row = y + li as u16;
        if row >= bottom {
            break;
        }
        let x0 = area.x + area.width.saturating_sub(line.chars().count() as u16) / 2;
        for (ci, ch) in line.chars().enumerate() {
            if idx >= env.visible {
                return; // ignition frontier — the rest is still unlit
            }
            let (glyph, b) = if dissolved(idx, env.dissolve) {
                ('·', env.brightness * 0.6) // this character has become a mote
            } else {
                (ch, env.brightness)
            };
            let x = x0 + ci as u16;
            if x < area.x + area.width {
                buf.set_string(
                    x,
                    row,
                    glyph.to_string(),
                    Style::default().fg(scale(p.reveal, b)),
                );
            }
            idx += 1;
        }
    }
}

/// Paint the Zen field into `area`. `title` is the breathing header line;
/// `reveal` is the in-flight reply ceremony as `(text, age_in_ticks)`; all
/// ambient colour comes from `p`. Too-small areas draw nothing (never panics).
pub fn render(
    state: &BrainState,
    title: &str,
    reveal: Option<(&str, u64)>,
    p: &Palette,
    area: Rect,
    buf: &mut Buffer,
    marker: Marker,
) {
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
    // How alive she is right now — drives the core's brightness + heartbeat
    // rate and the aura ring. Idle → a slow calm breath; thinking/acting → the
    // core burns brighter and beats faster, and the aura swells outward.
    let vitality = state.vitality();
    // Per-source glows, brightest first, with their GLOBAL identity hues.
    let sources: Vec<(Rgb, f32)> = state
        .current_sources()
        .into_iter()
        .map(|(name, glow)| (source_hue(&name), glow))
        .collect();
    // Palette slots copied into the paint closure (it must be self-contained).
    let (core_c, aura_c, fac_rest, fac_active, label_c) =
        (p.core, p.aura, p.faculty_rest, p.faculty_active, p.label);
    let (froz_dim, froz_lit, froz_label, mote_fb) =
        (p.frozen_dim, p.frozen_lit, p.frozen_label, p.mote_fallback);

    let canvas = Canvas::default()
        .marker(marker)
        .x_bounds([-1.0, 1.0])
        .y_bounds([-1.0, 1.0])
        .paint(move |ctx: &mut Context| {
            // ── Layer 1: current-awareness motes ─────────────────────────────
            // Drifting points on lazy diagonal paths; density + brightness rise
            // with the glow. Positions are frame-animated but seed-stable. When
            // sources are known, each paints its own identity hue in its own
            // seed band; otherwise the theme's fallback field.
            if sources.is_empty() {
                for i in 0..mote_count(current_glow) {
                    let (px, py, tw) = mote_pos(i, 0, frame);
                    let b = (current_glow as f64 * tw).clamp(0.0, 1.0);
                    ctx.print(px, py, mote_span(mote_fb, b));
                }
            } else {
                for (si, (hue, glow)) in sources.iter().enumerate() {
                    for i in 0..motes_for_source(*glow) {
                        let (px, py, tw) = mote_pos(i, si + 1, frame);
                        let b = (*glow as f64 * tw).clamp(0.0, 1.0);
                        ctx.print(px, py, mote_span(*hue, b));
                    }
                }
            }
            ctx.layer();

            // ── Layer 2: the singularity core + vitality aura ────────────────
            // The heartbeat speeds up with vitality (idle ~0.30, alive faster)
            // and the core brightens; a faint aura ring swells outward when
            // she's working, so the whole centre visibly surges.
            let beat_rate = 0.30 + 1.10 * vitality as f64;
            let pulse_v = pulse(frame, beat_rate);
            let core = (0.45 + 0.35 * vitality as f64 + 0.20 * pulse_v).clamp(0.0, 1.0);
            if vitality > 0.02 {
                let aura_r = 0.06 + 0.14 * vitality as f64 * pulse_v;
                let aura_b = (0.35 * vitality as f64 * pulse_v).clamp(0.0, 1.0);
                let n = 12;
                for k in 0..n {
                    let a = k as f64 / n as f64 * std::f64::consts::TAU + frame as f64 * 0.03;
                    ctx.print(
                        aura_r * a.cos(),
                        aura_r * a.sin() * 0.6,
                        mote_span(aura_c, aura_b),
                    );
                }
            }
            ctx.print(
                0.0,
                0.0,
                Span::styled("✦", Style::default().fg(scale(core_c, core))),
            );
            ctx.layer();

            // ── Layer 3: faculties as orbiting bodies ────────────────────────
            for (i, (kind, activity)) in faculties.iter().enumerate() {
                let a = i as f64 / n_fac as f64 * std::f64::consts::TAU;
                // Radius breathes outward a touch when the faculty is active.
                let r = 0.42 + 0.10 * *activity as f64;
                let (x, y, wz) = project(a, r, 0.12, frame);
                let db = depth_brightness(wz, r) as f64;
                let body_hue = lerp(fac_rest, fac_active, *activity);
                // Tether to the core.
                ctx.draw(&CanvasLine {
                    x1: 0.0,
                    y1: 0.0,
                    x2: x,
                    y2: y,
                    color: scale(body_hue, (0.30 + 0.50 * *activity as f64) * db),
                });
                // The body: brighter with activity; a ✷ when lit, ◦ at rest.
                let glyph = if *activity > 0.25 { "✷" } else { "◦" };
                ctx.print(
                    x,
                    y,
                    Span::styled(
                        glyph,
                        Style::default().fg(scale(body_hue, (0.45 + 0.55 * *activity as f64) * db)),
                    ),
                );
                // Label just outside the body, only when the node is on the
                // near side of the projection (avoid clutter at the far side).
                if wz > -0.1 {
                    ctx.print(
                        x,
                        y - 0.09,
                        Span::styled(
                            faculty_label(*kind),
                            Style::default().fg(scale(label_c, db)),
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
                    let db = depth_brightness(wz, r) as f64;
                    let b = ((0.20 + 0.80 * *awake as f64) * db).clamp(0.0, 1.0);
                    let glyph = if *awake > 0.5 {
                        "❄"
                    } else if *awake > 0.05 {
                        "✧"
                    } else {
                        "·"
                    };
                    let hue = lerp(froz_dim, froz_lit, *awake);
                    ctx.print(
                        x,
                        y,
                        Span::styled(glyph, Style::default().fg(scale(hue, b))),
                    );
                    if *awake > 0.4 && wz > -0.2 {
                        ctx.print(
                            x,
                            y - 0.08,
                            Span::styled(
                                name.clone(),
                                Style::default().fg(Color::Rgb(
                                    froz_label.0,
                                    froz_label.1,
                                    froz_label.2,
                                )),
                            ),
                        );
                    }
                }
            }
        });

    canvas.render(area, buf);

    // ── Layer 5: response-as-light ──────────────────────────────────────────
    // Her reply ignites over the field, holds, fades to ember, and dissolves
    // into motes — the words become soil. Cell space, over the canvas.
    if let Some((text, age)) = reveal {
        render_reveal(text, age, p, area, buf);
    }

    // The breathing title, centred on the top row — the one bit of text the
    // Zen field keeps, dimmed so it never competes with the message box.
    let breath = 0.45 + 0.55 * pulse(frame, 0.5);
    let tx = area.x + area.width.saturating_sub(title.chars().count() as u16) / 2;
    buf.set_string(
        tx,
        area.y,
        title,
        Style::default().fg(scale(p.title, breath)),
    );

    // Legend, bottom-left: one coloured dot + label per source she's drinking
    // from right now, so the field is READABLE — "● lineage  ● world" tells you
    // the gold motes are her own corpus and the green ones are the live world.
    let legend = state.current_sources();
    if !legend.is_empty() && area.height >= 2 {
        let row = area.y + area.height - 1;
        let mut cx = area.x + 1;
        for (name, glow) in legend.iter() {
            let br = (0.4 + 0.6 * *glow) as f64;
            let label = source_label(name);
            if cx + label.len() as u16 + 3 > area.x + area.width {
                break; // out of room — legend never spills off the field
            }
            buf.set_string(
                cx,
                row,
                "●",
                Style::default().fg(scale(source_hue(name), br)),
            );
            buf.set_string(
                cx + 2,
                row,
                label,
                Style::default().fg(scale(p.legend_label, 1.0)),
            );
            cx += label.len() as u16 + 4;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::BrainState;
    use crate::palette::{by_name, EMBER, ENTHEIA};

    fn scene() -> BrainState {
        let mut b = BrainState::new();
        b.flare(FacultyKind::Model);
        b.set_frozen(&["verification".to_string(), "docker".to_string()]);
        b.wake_frozen("verification");
        b.flare_current();
        b
    }

    fn render_to(w: u16, h: u16, state: &BrainState) -> Buffer {
        render_themed(w, h, state, None, &ENTHEIA)
    }

    fn render_themed(
        w: u16,
        h: u16,
        state: &BrainState,
        reveal: Option<(&str, u64)>,
        p: &Palette,
    ) -> Buffer {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        render(
            state,
            "entheai · alive",
            reveal,
            p,
            area,
            &mut buf,
            Marker::Braille,
        );
        buf
    }

    fn buf_text(buf: &Buffer, w: u16, h: u16) -> String {
        let mut s = String::new();
        for y in 0..h {
            for x in 0..w {
                s.push_str(buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn tiny_and_huge_areas_never_panic() {
        for (w, h) in [(0, 0), (1, 1), (7, 3), (8, 4), (300, 120)] {
            let _ = render_to(w, h, &scene());
            let _ = render_themed(w, h, &scene(), Some(("hello field", 5)), by_name("void"));
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
    fn source_hues_are_global_distinct_and_lineage_is_warm() {
        let d = source_hue("dogfood");
        let v = source_hue("valyu");
        let w = source_hue("worldmonitor");
        let u = source_hue("???");
        assert!(d.0 > d.2 && d.0 > 150, "dogfood burns warm gold: {d:?}");
        assert!(v.2 >= v.0, "valyu reads cool: {v:?}");
        assert!(w.1 > w.0 && w.1 > w.2, "worldmonitor reads green: {w:?}");
        let hues = [d, v, w, u];
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

    fn max_cell_brightness(buf: &Buffer, w: u16, h: u16) -> u16 {
        let mut m = 0u16;
        for x in 0..w {
            for y in 0..h {
                if let Some(Color::Rgb(r, g, b)) = buf
                    .cell((x, y))
                    .map(|c| c.style().fg.unwrap_or(Color::Reset))
                {
                    m = m.max(r as u16 + g as u16 + b as u16);
                }
            }
        }
        m
    }

    #[test]
    fn a_thinking_field_burns_brighter_than_an_idle_one() {
        // Same frame, same everything — only vitality differs. The active
        // field's brightest cell (core + aura surging) must exceed idle's.
        let idle = BrainState::new();
        let mut alive = BrainState::new();
        alive.flare(FacultyKind::Model); // vitality → ~1.0
        let (w, h) = (60u16, 24u16);
        let bi = render_to(w, h, &idle);
        let ba = render_to(w, h, &alive);
        assert!(
            max_cell_brightness(&ba, w, h) > max_cell_brightness(&bi, w, h),
            "the field must visibly surge when she thinks"
        );
    }

    #[test]
    fn field_colours_motes_and_draws_legend_per_source() {
        let mut b = BrainState::new();
        b.flare_current_source("dogfood");
        b.flare_current_source("worldmonitor");
        let buf = render_themed(80, 30, &b, None, &ENTHEIA);

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

    #[test]
    fn reveal_envelope_walks_ignite_hold_fade_dissolve_done() {
        let total = 80; // → ignite 10 ticks at 8 chars/tick
                        // Ignition: partial text at full brightness.
        let early = reveal_envelope(3, total);
        assert!(early.visible < total && early.visible >= 8, "{early:?}");
        assert_eq!(early.brightness, 1.0);
        assert!(!early.done);
        // Hold: everything lit, still bright.
        let hold = reveal_envelope(50, total);
        assert_eq!(hold.visible, total);
        assert_eq!(hold.brightness, 1.0);
        // Fade: dimmer than hold, no dissolve yet.
        let fade = reveal_envelope(10 + 90 + 30, total);
        assert!(fade.brightness < 1.0 && fade.brightness > REVEAL_EMBER * 0.9);
        assert_eq!(fade.dissolve, 0.0);
        // Dissolve: holes accumulate, brightness sinks below ember.
        let diss = reveal_envelope(10 + 90 + 55 + 20, total);
        assert!(diss.dissolve > 0.3 && diss.dissolve < 1.0, "{diss:?}");
        assert!(diss.brightness < REVEAL_EMBER);
        // Done: past the end, and for empty text immediately.
        assert!(reveal_envelope(10 + 90 + 55 + 33, total).done);
        assert!(reveal_envelope(0, 0).done);
    }

    #[test]
    fn dissolve_lots_are_deterministic_and_monotonic() {
        for i in 0..200 {
            assert!(!dissolved(i, 0.0), "nothing dissolves at 0");
            assert!(dissolved(i, 1.0), "everything dissolves at 1");
            // Monotone: once crumbled at t, still crumbled at t' > t.
            if dissolved(i, 0.4) {
                assert!(dissolved(i, 0.7));
            }
        }
    }

    #[test]
    fn wrap_chars_respects_width_and_survives_empties() {
        assert!(wrap_chars("", 10).is_empty());
        let lines = wrap_chars("abcdefghij", 4);
        assert_eq!(lines, vec!["abcd", "efgh", "ij"]);
        assert!(
            wrap_chars("a\n\nb", 10).len() >= 3,
            "blank lines kept mid-text"
        );
        for l in wrap_chars("x".repeat(500).as_str(), 7) {
            assert!(l.chars().count() <= 7);
        }
    }

    #[test]
    fn a_young_reveal_shows_her_words_and_a_done_one_leaves_no_trace() {
        let b = BrainState::new();
        // Young: the first characters are ignited and readable in the buffer.
        let young = render_themed(70, 24, &b, Some(("hello field of light", 40)), &ENTHEIA);
        let txt = buf_text(&young, 70, 24);
        assert!(txt.contains("hello"), "ignited text missing:\n{txt}");
        // Done (way past the ceremony): no trace of the words remains.
        let done = render_themed(70, 24, &b, Some(("hello field of light", 10_000)), &ENTHEIA);
        let txt = buf_text(&done, 70, 24);
        assert!(!txt.contains("hello"), "ceremony must end cleanly:\n{txt}");
    }

    #[test]
    fn long_replies_truncate_with_an_honest_pointer_to_chat() {
        let b = BrainState::new();
        let long = "word ".repeat(600); // far taller than the field allows
                                        // Age 100: everything displayed is ignited, ceremony still burning.
        let buf = render_themed(60, 18, &b, Some((long.as_str(), 100)), &ENTHEIA);
        let txt = buf_text(&buf, 60, 18);
        assert!(
            txt.contains("full reply lives in chat"),
            "truncation must say so:\n{txt}"
        );
    }

    #[test]
    fn ember_theme_turns_the_idle_field_warm() {
        // Idle, no sources: every lit ambient cell comes from the palette. The
        // entheia field is cool (blue ≥ red everywhere); ember must produce at
        // least one warm-dominant cell (its core/title/aura are r > b).
        let b = BrainState::new();
        let warm_cell = |buf: &Buffer| {
            (0..60).any(|x| {
                (0..20).any(|y| {
                    matches!(
                        buf.cell((x, y)).and_then(|c| c.style().fg),
                        Some(Color::Rgb(r, _, bl)) if r > bl && r > 60
                    )
                })
            })
        };
        let cool = render_themed(60, 20, &b, None, &ENTHEIA);
        let warm = render_themed(60, 20, &b, None, &EMBER);
        assert!(!warm_cell(&cool), "idle entheia should have no warm cells");
        assert!(warm_cell(&warm), "idle ember should glow warm");
    }
}
