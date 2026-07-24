//! One real pulse against the live APIs, metered through the real ledger.
//! Operator smoke tool: `source ~/.entheai/current.env && cargo run -p entheai-current --example live-pulse`

#[tokio::main]
async fn main() {
    let cfg = entheai_config::CurrentConfig {
        topics: vec!["Rust language and AI coding agents this week".to_string()],
        dogfood_repo: "PeetPedro/ultrawhale-dogfood".to_string(),
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
    // Source breakdown, then a couple of items per source (so dogfood — which
    // sorts after worldmonitor/valyu — is actually visible).
    let mut by_source: std::collections::BTreeMap<&str, usize> = Default::default();
    for i in &report.items {
        *by_source.entry(i.source.as_str()).or_default() += 1;
    }
    println!("by source: {by_source:?}");
    for src in ["dogfood", "valyu", "worldmonitor"] {
        for item in report.items.iter().filter(|i| i.source == src).take(2) {
            println!(
                "  [{}/{}] {} ({})",
                item.source,
                item.kind,
                &item.title[..item.title.len().min(80)],
                item.published_at.as_deref().unwrap_or("no date")
            );
        }
    }
    println!("budgets after: {:?}", engine.budget_status());
}
