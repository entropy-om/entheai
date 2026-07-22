//! Always-on "brain state" panel model (Slice A). Rendering is added in a later task.
//!
//! A small living graph: the agent's faculties (model, tools, context), the remote
//! fleet, a rotation frame, and readouts (worker count, NATS up, context %). Pure +
//! terminal-agnostic; fed by the TUI from event arms it already has.

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
#[allow(dead_code)] // consumed by the render task (next task in this slice)
fn project(angle: f64, r: f64, y_off: f64, frame: u64) -> (f64, f64, f64) {
    let theta = angle + frame as f64 * OMEGA;
    let wx = r * theta.cos();
    let wz = r * theta.sin();
    let sx = wx;
    let sy = y_off - wz * 0.35;
    (sx, sy, wz)
}

/// Nearer nodes (larger z) are brighter; result in [0.35, 1.0], monotonic in z.
#[allow(dead_code)] // consumed by the render task (next task in this slice)
fn depth_brightness(wz: f64, r: f64) -> f32 {
    let t = ((wz / r.max(1e-6)) + 1.0) / 2.0;
    (0.35 + 0.65 * t) as f32
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
        assert!(a1 < 1.0 && a1 >= 0.0, "decays and stays non-negative: {a1}");
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
}
