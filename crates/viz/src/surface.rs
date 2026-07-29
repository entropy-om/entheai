use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;

/// Ternary matrix surface visualization.
/// Renders a weight matrix {-1, 0, +1} as a colored surface in the terminal.
/// Each cell is one "pixel" (2x1 terminal chars for square-ish aspect).
#[derive(Clone)]
pub struct TernarySurface {
    /// Weight values {-1.0, 0.0, +1.0} flattened [row * cols + col]
    pub weights: Vec<f32>,
    pub rows: usize,
    pub cols: usize,
    /// Surface elevation multiplier (higher = more contrast)
    pub elevation: f32,
    /// Time offset for animation
    pub time: f64,
    /// Palette for the surface
    pub palette: TernaryPalette,
}

#[derive(Clone, Copy)]
pub enum TernaryPalette {
    /// Purple (-1), dark (0), gold (+1)
    Quantum,
    /// Red (-1), black (0), green (+1)
    Diablo,
    /// Blue (-1), grey (0), white (+1)
    Frost,
}

impl TernarySurface {
    pub fn new(weights: Vec<f32>, rows: usize, cols: usize) -> Self {
        TernarySurface {
            weights,
            rows,
            cols,
            elevation: 1.0,
            time: 0.0,
            palette: TernaryPalette::Quantum,
        }
    }

    /// Generate a random ternary matrix seeded from a hash.
    pub fn from_seed(seed: &str, rows: usize, cols: usize, group_size: usize) -> Self {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        seed.hash(&mut hasher);
        let h = hasher.finish();

        // Simple xorshift PRNG from hash
        let mut state = h;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f64) / (u64::MAX as f64)
        };

        let n = rows * cols;
        let mut weights = Vec::with_capacity(n);
        let groups = n / group_size;

        for g in 0..groups {
            // Generate group of weights
            let mut group_weights = Vec::with_capacity(group_size);
            let mut abs_sum = 0.0f64;
            for _ in 0..group_size {
                // Box-Muller
                let u = next().max(1e-10);
                let v = next();
                let w = (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos();
                group_weights.push(w);
                abs_sum += w.abs();
            }
            let scale = (abs_sum / group_size as f64).max(1e-7);

            for w in group_weights {
                let shifted = ((w / scale).clamp(-1.0, 1.0).round() + 1.0) as i32;
                let code = shifted.clamp(0, 2);
                weights.push((code - 1) as f32); // {-1, 0, +1}
            }
        }

        TernarySurface {
            weights,
            rows,
            cols,
            elevation: 1.2,
            time: 0.0,
            palette: TernaryPalette::Quantum,
        }
    }

    fn color_for(&self, weight: f32) -> Color {
        let w = weight * self.elevation;
        match self.palette {
            TernaryPalette::Quantum => {
                if w < -0.1 {
                    Color::Rgb(108, 92, 231) // purple
                } else if w > 0.1 {
                    Color::Rgb(162, 155, 254) // light purple
                } else {
                    Color::Rgb(30, 30, 34) // dark
                }
            }
            TernaryPalette::Diablo => {
                if w < -0.1 {
                    Color::Rgb(255, 71, 71) // red
                } else if w > 0.1 {
                    Color::Rgb(255, 215, 0) // gold
                } else {
                    Color::Rgb(10, 10, 15) // near black
                }
            }
            TernaryPalette::Frost => {
                if w < -0.1 {
                    Color::Rgb(100, 149, 237) // cornflower blue
                } else if w > 0.1 {
                    Color::Rgb(220, 220, 230) // white-ish
                } else {
                    Color::Rgb(60, 60, 70) // grey
                }
            }
        }
    }
}

impl Widget for &TernarySurface {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Scale matrix to fit available area
        let px_w = (area.width / 2).max(1) as usize; // 2 chars per "pixel"
        let px_h = area.height.max(1) as usize;
        let scale_x = self.cols as f64 / px_w as f64;
        let scale_y = self.rows as f64 / px_h as f64;

        for py in 0..px_h {
            for px in 0..px_w {
                let mx = (px as f64 * scale_x) as usize;
                let my = (py as f64 * scale_y) as usize;
                if mx < self.cols && my < self.rows {
                    let idx = my * self.cols + mx;
                    if idx < self.weights.len() {
                        let w = self.weights[idx];
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

/// Live surface animator — generates frames over time.
pub struct SurfaceAnimator {
    surface: TernarySurface,
    frame: u64,
}

impl SurfaceAnimator {
    pub fn new(surface: TernarySurface) -> Self {
        SurfaceAnimator { surface, frame: 0 }
    }

    /// Advance one frame.
    pub fn tick(&mut self) {
        self.frame += 1;
        self.surface.time = self.frame as f64 * 0.05;
    }

    /// Get the current surface for rendering.
    pub fn surface(&self) -> &TernarySurface {
        &self.surface
    }

    pub fn surface_mut(&mut self) -> &mut TernarySurface {
        &mut self.surface
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_seed() {
        let surf = TernarySurface::from_seed("LINOSV", 16, 16, 64);
        assert_eq!(surf.weights.len(), 256);
        // All weights should be -1, 0, or +1
        for w in &surf.weights {
            assert!(*w == -1.0 || *w == 0.0 || *w == 1.0,
                "weight {} not in {{-1, 0, +1}}", w);
        }
    }

    #[test]
    fn test_from_seed_deterministic() {
        let s1 = TernarySurface::from_seed("test", 8, 8, 64);
        let s2 = TernarySurface::from_seed("test", 8, 8, 64);
        assert_eq!(s1.weights, s2.weights);
    }
}
