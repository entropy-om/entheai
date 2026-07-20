//! entheai-bus: the F1 federation event bus. Publishes the fan-out
//! orchestrator's `FanoutEvent` lifecycle to NATS (`entheai.fanout.<session>.*`)
//! so any tailnet subscriber can watch runs live. Fully fail-safe: with the
//! `[nats]` feature off or the hub unreachable, every entry point is a no-op and
//! the caller runs entirely locally.

mod event;
pub use event::BusEvent;
