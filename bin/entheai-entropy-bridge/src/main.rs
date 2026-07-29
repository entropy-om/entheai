use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use reqwest::Client;
use futures::StreamExt;

#[derive(Parser)]
#[command(version, about = "Forward local NATS entropy telemetry to entheai.com's API")]
struct Cli {
    /// Path to entheai.toml (reads [nats] section).
    #[arg(long, default_value = "entheai.toml")]
    config: String,

    /// The target endpoint URL (default: entheai.com production).
    #[arg(long, default_value = "https://entheai.com/api/entropy")]
    endpoint: String,

    /// NATS subject pattern to subscribe to.
    #[arg(long, default_value = "entheai.entropy.v1.>")]
    subject: String,

    /// How often to forward the latest snapshot (seconds).
    #[arg(long, default_value = "10")]
    interval_secs: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();

    let config_toml = std::fs::read_to_string(&cli.config)
        .with_context(|| format!("reading {}", cli.config))?;
    let cfg: entheai_config::Config =
        entheai_config::Config::from_toml_str(&config_toml)?;
    let opts = entheai_bus::BusOptions::from_config(&cfg.nats);

    let token = std::env::var("ENTROPY_TOKEN")
        .context("ENTROPY_TOKEN env var required — set it to the site's API token")?;

    let bus = entheai_bus::Bus::connect(&opts)
        .await
        .context("connect to NATS — is the hub running?")?;

    // Subscribe to the entropy subject pattern.
    let subscription = bus.subscribe(&cli.subject).await
        .context("subscribe to subject")?;

    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    log::info!(
        "entropy bridge: listening on {subject} → {endpoint} (every {i}s)",
        subject = cli.subject,
        endpoint = cli.endpoint,
        i = cli.interval_secs,
    );

    let mut latest: Option<String> = None;

    // Background task: collect latest snapshot from NATS.
    let mut sub = subscription;
    tokio::spawn(async move {
        while let Some(msg) = sub.next().await {
            if let Ok(payload) = String::from_utf8(msg.payload.to_vec()) {
                latest = Some(payload);
            }
        }
    });

    // Foreground: periodically POST the latest snapshot to the site.
    let mut ticker = tokio::time::interval(Duration::from_secs(cli.interval_secs));
    loop {
        ticker.tick().await;
        if let Some(payload) = &latest {
            match client
                .post(&cli.endpoint)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(payload.clone())
                .send()
                .await
            {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        log::debug!("forwarded entropy snapshot ({} bytes)", payload.len());
                    } else {
                        log::warn!("forward failed: HTTP {status}");
                    }
                }
                Err(e) => log::warn!("forward error: {e}"),
            }
        }
    }
}
