//! entheai companion — session beacon window binary.
//!
//! The GUI implementation (winit/softbuffer/arboard) lives in [`app`] and is
//! compiled only under the default `window` feature. Without it, this binary
//! degrades to a stub, so the crate — and anything that links only its
//! lightweight `state` protocol (the entheai bin, the TUI) — still builds with
//! no windowing system libraries.

#[cfg(feature = "window")]
mod app;

#[cfg(feature = "window")]
fn main() -> anyhow::Result<()> {
    app::run()
}

#[cfg(not(feature = "window"))]
fn main() {
    eprintln!("entheai-companion was built without the `window` feature — no GUI available.");
    std::process::exit(1);
}
