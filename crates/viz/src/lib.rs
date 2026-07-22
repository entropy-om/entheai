//! entheai visualization — Slice 1: the fan-out swarm.
//!
//! A pure, terminal-agnostic model of a fan-out run plus a ratatui renderer.
//! Fed by the TUI from the existing `FanoutEvent` stream (mapped to mutators).

pub mod brain;
pub mod model;
pub mod swarm;
pub mod term;

pub use model::{NodeStatus, Phase, SwarmModel, SwarmNode};
pub use brain::{BrainState, Faculty, FacultyKind, FleetNode};
