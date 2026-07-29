use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Widget;

/// Ternary matrix surface visualization.
/// Renders a weight matrix {-1, 0, +1} as colored blocks in the terminal.
#[derive(Clone)]
pub struct TernarySurface {
    pub weights: Vec<f32>,
    pub rows: usize,
    pub cols: usize,
    pub elevation: f32,
    pub palette: TernaryPalette,
}

#[derive(Clone, Copy)]
pub enum TernaryPalette {
    Quantum,
    Diablo,
    Frost,
}

impl TernarySurface {
    pub fn new(weights: Vec<f32>, rows: usize, cols: usize) -> Self {
        TernarySurface { weights, rows, cols, elevation: 1.0, palette: TernaryPalette::Quantum }
    }

    fn color_for(&self, weight: f32) -> Color {
        match self.palette {
            TernaryPalette::Quantum => {
                if weight < -0.1 { Color::Rgb(108, 92, 231) }
                else if weight > 0.1 { Color::Rgb(162, 155, 254) }
                else { Color::Rgb(30, 30, 34) }
            }
            TernaryPalette::Diablo => {
                if weight < -0.1 { Color::Rgb(255, 71, 71) }
                else if weight > 0.1 { Color::Rgb(255, 215, 0) }
                else { Color::Rgb(10, 10, 15) }
            }
            TernaryPalette::Frost => {
                if weight < -0.1 { Color::Rgb(100, 149, 237) }
                else if weight > 0.1 { Color::Rgb(220, 220, 230) }
                else { Color::Rgb(60, 60, 70) }
            }
        }
    }
}

impl Widget for &TernarySurface {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let px_w = (area.width / 2).max(1) as usize;
        let px_h = area.height.max(1) as usize;
        let scale_x = (self.cols as f64) / (px_w as f64);
        let scale_y = (self.rows as f64) / (px_h as f64);

        for py in 0..px_h {
            for px in 0..px_w {
                let mx = (px as f64 * scale_x) as usize;
                let my = (py as f64 * scale_y) as usize;
                if mx < self.cols && my < self.rows {
                    let idx = my * self.cols + mx;
                    if let Some(&w) = self.weights.get(idx) {
                        let color = self.color_for(w);
                        let x = area.x + (px * 2) as u16;
                        let y = area.y + py as u16;
                        if x < area.right() {
                            buf.get_mut(x, y).set_char(' ').set_bg(color);
                            buf.get_mut(x + 1, y).set_char(' ').set_bg(color);
                        }
                    }
                }
            }
        }
    }
}
