//! One real pulse against the live APIs, metered through the real ledger.
//! Operator smoke tool: `source ~/.entheai/current.env && cargo run -p entheai-current --example live-pulse`

#[tokio::main]
async fn main() {
    let cfg = entheai_config::CurrentConfig {
        topics: vec!["Rust language and AI coding agents this week".to_string()],
        ..Default::default()
    };
    let ledger = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".entheai")
        .join("current-budget.json");
    let engine = entheai_current::CurrentEngine::from_config(&cfg, ledger);
    println!("budgets before: {:?}", engine.budget_status());
    let report = engine.pulse().await;
    println!("{}", report.summary());
    for (src, err) in &report.errors {
        println!("  error {src}: {err}");
    }
    for item in report.items.iter().take(8) {
        println!(
            "  [{}/{}] {} ({})",
            item.source,
            item.kind,
            &item.title[..item.title.len().min(80)],
            item.published_at.as_deref().unwrap_or("no date")
        );
    }
    println!("budgets after: {:?}", engine.budget_status());
}
