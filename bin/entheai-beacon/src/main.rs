//! entheai-beacon — the local NATS→POST bridge that lights the entheai.com
//! entropy beacon (roadmap Phase 4.1, "Light the beacon").
//!
//! The TUI publishes an [`EntropySnapshot`] to `entheai.entropy.v1.<session>` on
//! the local NATS bus at ~1 Hz. This bridge subscribes to that stream and POSTs
//! the freshest snapshot to the site's `/api/entropy` write path, so the docs
//! header beacon reflects real activity.
//!
//! Honest by construction:
//!   * It forwards the snapshot the TUI actually produced — the shared
//!     `entheai-bus` wire type, not a re-derived copy — so the site's `at_ms`
//!     staleness check tracks real work. The beacon can only ever be as live as
//!     the TUI is; there is no code path that fabricates a heartbeat.
//!   * It throttles to one POST per interval (default 30 s). The TUI emits at
//!     ~1 Hz, but the site treats a snapshot as live for 15 min and CF KV writes
//!     are neither free nor unlimited — POSTing every second would be waste
//!     dressed up as liveness, so we send only the freshest and only when new.
//!   * No token → it refuses to start. A beacon that cannot authenticate cannot
//!     write, and the site would answer 401; better to say so at startup than
//!     to spin pretending.
//!
//! Config (env):
//!   ENTHEAI_ENTROPY_TOKEN         (required)  bearer for POST /api/entropy;
//!                                             must equal the CF `ENTROPY_TOKEN`.
//!   ENTHEAI_BEACON_URL            (= https://entheai.com/api/entropy)
//!   ENTHEAI_BEACON_NATS           (= nats://127.0.0.1:4222)
//!   ENTHEAI_BEACON_INTERVAL_SECS  (= 30, floored at 5)

use std::time::Duration;

use entheai_bus::{EntropySnapshot, ENTROPY_SUBJECT_PREFIX};
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = match std::env::var("ENTHEAI_ENTROPY_TOKEN") {
        Ok(t) if !t.trim().is_empty() => t,
        _ => {
            eprintln!(
                "entheai-beacon: ENTHEAI_ENTROPY_TOKEN is required (the bearer the site \
                 checks on POST /api/entropy). Refusing to start."
            );
            std::process::exit(2);
        }
    };
    let url = env_or("ENTHEAI_BEACON_URL", "https://entheai.com/api/entropy");
    let nats = env_or("ENTHEAI_BEACON_NATS", "nats://127.0.0.1:4222");
    let interval_secs = std::env::var("ENTHEAI_BEACON_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(30)
        .max(5);

    let client = async_nats::connect(&nats)
        .await
        .map_err(|e| format!("entheai-beacon: NATS connect to {nats} failed: {e}"))?;
    let subject = format!("{ENTROPY_SUBJECT_PREFIX}.>");
    let mut sub = client.subscribe(subject.clone()).await?;
    let http = reqwest::Client::new();
    eprintln!("entheai-beacon: watching {subject} -> {url} (≤1 POST / {interval_secs}s)");

    let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut latest: Option<EntropySnapshot> = None;
    let mut last_posted_at: i64 = i64::MIN;

    loop {
        tokio::select! {
            msg = sub.next() => match msg {
                Some(msg) => match serde_json::from_slice::<EntropySnapshot>(&msg.payload) {
                    Ok(snap) => latest = Some(snap),
                    Err(e) => eprintln!(
                        "entheai-beacon: skip malformed snapshot on {}: {e}",
                        msg.subject
                    ),
                },
                // Subscription ended — the bus is gone. Exit non-zero so a
                // supervisor restarts us (and async-nats reconnects) rather than
                // spinning on a dead stream while the site quietly goes stale.
                None => {
                    eprintln!("entheai-beacon: NATS subscription ended (bus down). Exiting for restart.");
                    std::process::exit(1);
                }
            },
            _ = ticker.tick() => {
                let Some(snap) = latest.as_ref() else { continue };
                if snap.at_ms <= last_posted_at {
                    continue; // nothing newer than the last successful POST
                }
                match http.post(&url).bearer_auth(&token).json(snap).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        last_posted_at = snap.at_ms;
                        eprintln!(
                            "entheai-beacon: lit {} (session {}, {} faculties, {} wk) [{}]",
                            snap.at_ms,
                            snap.session,
                            snap.glow.len(),
                            snap.workers,
                            resp.status()
                        );
                    }
                    Ok(resp) => eprintln!(
                        "entheai-beacon: site refused the snapshot [{}] — check the token / KV binding",
                        resp.status()
                    ),
                    Err(e) => eprintln!("entheai-beacon: POST failed: {e}"),
                }
            }
        }
    }
}

/// The env var's value, or `default` when unset or blank.
fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}
