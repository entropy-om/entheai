//! Draw a [`SwarmModel`] onto a ratatui `Canvas`. The same function serves the
//! small inline pane and the full-screen viz-mode — only `area` changes.

use ratatui::layout::Rect;
use ratatui::prelude::Buffer;
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::text::Span;
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine, Points};
use ratatui::widgets::Widget;

use crate::model::{NodeStatus, SwarmModel};

/// Glyph for a node status.
fn glyph(status: NodeStatus, frame: u64) -> char {
    match status {
        NodeStatus::Pending => '◻',
        NodeStatus::Running => {
            // `frame` advances once per animation tick (90ms by default), so
            // toggling every frame reads as an ~11Hz strobe rather than a pulse.
            // Holding each glyph for 4 ticks (~360ms) gives a calm alternation
            // that still reads as "active" without flickering.
            if (frame / 4).is_multiple_of(2) {
                '◑'
            } else {
                '◐'
            }
        }
        NodeStatus::Done => '✓',
        NodeStatus::Failed => '✗',
    }
}

fn status_color(status: NodeStatus) -> Color {
    match status {
        NodeStatus::Pending => Color::DarkGray,
        NodeStatus::Running => Color::Cyan,
        NodeStatus::Done => Color::Green,
        NodeStatus::Failed => Color::Red,
    }
}

/// Render the swarm. `marker` picks the Canvas resolution (Braille inline,
/// HalfBlock for a chunkier full view); `frame` drives the running pulse.
pub fn render(model: &SwarmModel, area: Rect, buf: &mut Buffer, marker: Marker, frame: u64) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let w = f64::from(area.width);
    let h = f64::from(area.height);

    // Orchestrator at top-center; nodes fanned across a lower row.
    let orch = (w / 2.0, h * 0.85);
    let node_xy = |i: usize| -> (f64, f64) {
        let x = if model.nodes.len() <= 1 {
            w / 2.0
        } else {
            w * (0.10 + 0.80 * (i as f64) / ((model.nodes.len() - 1) as f64))
        };
        (x, h * 0.20)
    };

    // Clone only what the paint closure reads (status + role prefix), NOT each
    // node's full `task` instruction string — the render never touches `task`,
    // so cloning it every frame was pure churn.
    let nodes: Vec<(NodeStatus, String)> = model
        .nodes
        .iter()
        .map(|n| (n.status, n.role.chars().take(6).collect()))
        .collect();
    Canvas::default()
        .marker(marker)
        .x_bounds([0.0, w])
        .y_bounds([0.0, h])
        .paint(move |ctx| {
            for i in 0..nodes.len() {
                let (nx, ny) = node_xy(i);
                ctx.draw(&CanvasLine {
                    x1: orch.0,
                    y1: orch.1,
                    x2: nx,
                    y2: ny,
                    color: Color::DarkGray,
                });
            }
            ctx.layer();
            ctx.draw(&Points {
                coords: &[orch],
                color: Color::Blue,
            });
            // Style labels explicitly rather than leaning on the incidental color
            // left behind by the `Points` draw at the same cell: `Context::print`
            // uses the label's own style (defaulting to none), so an unstyled
            // string only picks up a color where its first character happens to
            // overlap a previously-painted point — every other character renders
            // in the terminal's default foreground. Coloring the whole span keeps
            // "orch" and each node's status+role text uniformly legible.
            ctx.print(
                orch.0,
                orch.1,
                Span::styled("orch", Style::default().fg(Color::Blue)),
            );
            if nodes.is_empty() {
                ctx.print(
                    w / 2.0,
                    h / 2.0,
                    Span::styled(
                        "(idle — no fan-out running)",
                        Style::default().fg(Color::DarkGray),
                    ),
                );
            }
            for (i, (status, label)) in nodes.iter().enumerate() {
                let (nx, ny) = node_xy(i);
                ctx.draw(&Points {
                    coords: &[(nx, ny)],
                    color: status_color(*status),
                });
                ctx.print(
                    nx,
                    ny,
                    Span::styled(
                        format!("{} {label}", glyph(*status, frame)),
                        Style::default().fg(status_color(*status)),
                    ),
                );
            }
        })
        .render(area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SwarmModel;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::symbols::Marker;

    fn buf_text(buf: &Buffer) -> String {
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn empty_model_draws_no_status_glyphs() {
        let m = SwarmModel::new();
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        render(&m, area, &mut buf, Marker::Braille, 0);
        let text = buf_text(&buf);
        assert!(!text.contains('✓') && !text.contains('◐') && !text.contains('◻'));
    }

    #[test]
    fn empty_model_shows_idle_hint_instead_of_a_bare_canvas() {
        // The full-screen swarm view (Ctrl-V) renders even when no fan-out is
        // running, so a model with zero nodes used to draw nothing but the
        // orchestrator hub — an uninformative, sparse-looking screen. It should
        // say plainly that nothing is running.
        let m = SwarmModel::new();
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        render(&m, area, &mut buf, Marker::Braille, 0);
        let text = buf_text(&buf);
        assert!(
            text.contains("idle"),
            "idle hint shown when no nodes: {text:?}"
        );
    }

    #[test]
    fn active_model_omits_idle_hint() {
        let mut m = SwarmModel::new();
        m.decompose(&[("coder".into(), "t".into())]);
        let area = Rect::new(0, 0, 60, 16);
        let mut buf = Buffer::empty(area);
        render(&m, area, &mut buf, Marker::Braille, 0);
        let text = buf_text(&buf);
        assert!(
            !text.contains("idle"),
            "idle hint should not show once nodes exist: {text:?}"
        );
    }

    #[test]
    fn nodes_render_status_glyphs() {
        let mut m = SwarmModel::new();
        m.decompose(&[("coder".into(), "t".into()), ("test".into(), "t".into())]);
        m.coder_started(0, "coder", "t");
        m.coder_finished(1, true, "verified");
        let area = Rect::new(0, 0, 60, 16);
        let mut buf = Buffer::empty(area);
        render(&m, area, &mut buf, Marker::Braille, 1);
        let text = buf_text(&buf);
        // The running glyph now holds for 4 ticks before alternating (see
        // `glyph`'s doc comment) instead of flipping every tick, so frame 1 still
        // shows the first half of the pulse ('◑'), not '◐'.
        assert!(text.contains('◑'), "running node glyph present");
        assert!(text.contains('✓'), "done node glyph present");
        assert!(
            text.contains('o') || text.contains('c'),
            "a role label rendered"
        );
    }

    #[test]
    fn tiny_and_large_areas_do_not_panic() {
        let mut m = SwarmModel::new();
        m.decompose(&[("a".into(), "t".into())]);
        for (w, h) in [(10u16, 3u16), (120, 40)] {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            render(&m, area, &mut buf, Marker::Braille, 7);
        }
    }
}
