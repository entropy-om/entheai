//! entheai-bus: the F1 federation event bus. Publishes the fan-out
//! orchestrator's `FanoutEvent` lifecycle to NATS (`entheai.fanout.<session>.*`)
//! so any tailnet subscriber can watch runs live. Fully fail-safe: with the
//! `[nats]` feature off or the hub unreachable, every entry point is a no-op and
//! the caller runs entirely locally.

mod event;
pub use event::BusEvent;

use entheai_orchestrator::FanoutEvent;

/// Connection options resolved from the `[nats]` config + environment.
#[derive(Debug, Clone, Default)]
pub struct BusOptions {
    pub enabled: bool,
    pub url: Option<String>,
    pub token: Option<String>,
}

impl BusOptions {
    /// Resolve from the config block, reading the named env vars for URL/token.
    /// An unset or empty env var resolves to `None`, which makes `Bus::connect`
    /// a no-op (feature stays off) — the tracked config never inlines secrets.
    pub fn from_config(cfg: &entheai_config::NatsConfig) -> Self {
        let non_empty = |name: &str| std::env::var(name).ok().filter(|s| !s.is_empty());
        Self {
            enabled: cfg.enabled,
            url: non_empty(&cfg.url_env),
            token: non_empty(&cfg.token_env),
        }
    }
}

/// A connected NATS client for publishing fan-out events. Cheap to clone
/// (`async_nats::Client` is internally reference-counted).
#[derive(Clone)]
pub struct Bus {
    client: async_nats::Client,
}

impl Bus {
    /// Connect using the resolved options. Fail-safe: returns `None` when the
    /// feature is disabled, the URL is missing, or the connection/auth fails, so
    /// the caller runs entirely locally. `async_nats` returns an error
    /// immediately on an unreachable server (5s connection timeout, no initial
    /// retry), so a dead hub never stalls startup.
    pub async fn connect(opts: &BusOptions) -> Option<Bus> {
        if !opts.enabled {
            return None;
        }
        let Some(url) = opts.url.clone() else {
            log::warn!("nats: [nats].enabled but URL env is unset/empty — federation off");
            return None;
        };
        let connect = match &opts.token {
            Some(t) => async_nats::ConnectOptions::with_token(t.clone()),
            None => async_nats::ConnectOptions::new(),
        };
        match connect.connect(url.clone()).await {
            Ok(client) => {
                log::info!("nats: federation bus connected to {url}");
                Some(Bus { client })
            }
            Err(e) => {
                log::warn!("nats: connect to {url} failed ({e}) — federation off");
                None
            }
        }
    }

    /// Publish one fan-out event as JSON to `entheai.fanout.<session>.<suffix>`.
    /// Best-effort fire-and-forget (core NATS): any error is logged, never
    /// propagated (federation must never break a run).
    pub async fn publish_event(&self, session: &str, event: &FanoutEvent) {
        let dto = BusEvent::from(event);
        let subject = format!("entheai.fanout.{session}.{}", dto.subject_suffix());
        let payload = match serde_json::to_vec(&dto) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("nats: serialize event failed: {e}");
                return;
            }
        };
        if let Err(e) = self.client.publish(subject, payload.into()).await {
            log::warn!("nats: publish failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_returns_none_when_disabled() {
        let opts = BusOptions { enabled: false, url: Some("nats://127.0.0.1:4222".into()), token: None };
        assert!(Bus::connect(&opts).await.is_none());
    }

    #[tokio::test]
    async fn connect_returns_none_when_url_missing() {
        let opts = BusOptions { enabled: true, url: None, token: None };
        assert!(Bus::connect(&opts).await.is_none());
    }

    #[test]
    fn from_config_reads_named_env_vars() {
        // SAFETY: single-threaded test; unique var names avoid cross-test races.
        std::env::set_var("BUS_TEST_URL_F1", "nats://example:4222");
        std::env::set_var("BUS_TEST_TOKEN_F1", "s3cr3t");
        let cfg = entheai_config::NatsConfig {
            enabled: true,
            url_env: "BUS_TEST_URL_F1".into(),
            token_env: "BUS_TEST_TOKEN_F1".into(),
        };
        let opts = BusOptions::from_config(&cfg);
        assert!(opts.enabled);
        assert_eq!(opts.url.as_deref(), Some("nats://example:4222"));
        assert_eq!(opts.token.as_deref(), Some("s3cr3t"));
    }
}
