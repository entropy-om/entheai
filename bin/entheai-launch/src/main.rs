//! The macOS `.app` executable: open the branded entheai window (WezTerm when
//! installed — the 8b-is fork — else Ghostty).
fn main() -> anyhow::Result<()> {
    entheai_launcher::launch()
}
