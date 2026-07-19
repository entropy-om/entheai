//! Draw a [`SwarmModel`] onto a ratatui `Canvas`. The same function serves the
//! small inline pane and the full-screen viz-mode — only `area` changes.

use ratatui::layout::Rect;
use ratatui::prelude::Buffer;
use ratatui::style::Color;
use ratatui::symbols::Marker;
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine, Points};
use ratatui::widgets::Widget;

use crate::model::{NodeStatus, SwarmModel};

/// Glyph for a node status.
fn glyph(status: NodeStatus, frame: u64) -> char {
    match status {
        NodeStatus::Pending => '◻',
        NodeStatus::Running => {
            if frame % 2 == 0 {
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

    let nodes = model.nodes.clone();
    Canvas::default()
        .marker(marker)
        .x_bounds([0.0, w])
        .y_bounds([0.0, h])
        .paint(move |ctx| {
            for (i, _node) in nodes.iter().enumerate() {
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
            ctx.print(orch.0, orch.1, "orch");
            for (i, node) in nodes.iter().enumerate() {
                let (nx, ny) = node_xy(i);
                ctx.draw(&Points {
                    coords: &[(nx, ny)],
                    color: status_color(node.status),
                });
                let label: String = node.role.chars().take(6).collect();
                ctx.print(nx, ny, format!("{} {label}", glyph(node.status, frame)));
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
    fn nodes_render_status_glyphs() {
        let mut m = SwarmModel::new();
        m.decompose(&[("coder".into(), "t".into()), ("test".into(), "t".into())]);
        m.coder_started(0, "coder", "t");
        m.coder_finished(1, true, "verified");
        let area = Rect::new(0, 0, 60, 16);
        let mut buf = Buffer::empty(area);
        render(&m, area, &mut buf, Marker::Braille, 1);
        let text = buf_text(&buf);
        assert!(text.contains('◐'), "running node glyph present");
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
