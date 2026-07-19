//! The macOS `.app` executable: open the branded entheai Ghostty window.
fn main() -> anyhow::Result<()> {
    entheai_launcher::launch()
}
