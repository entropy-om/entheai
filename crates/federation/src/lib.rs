//! entheai-federation (F2): dispatch coder sub-tasks to worker nodes over NATS
//! JetStream. A WorkQueue stream delivers each `WorkItem` to exactly one worker;
//! git bundles travel through the JetStream object store; results return over
//! core NATS. Fail-safe: any NATS failure leaves the caller to run locally.

pub mod executor;
pub mod repo;
pub mod types;

use std::time::Duration;

use futures::StreamExt;
use tokio::io::AsyncReadExt;

pub use executor::FederationExecutor;
pub use types::{WorkItem, WorkResult};

const WORK_STREAM: &str = "ENTHEAI_WORK";
const WORK_SUBJECT: &str = "entheai.work.coder";
const BUNDLES_BUCKET: &str = "entheai-bundles";
const DURABLE: &str = "coder-workers";
const PRESENCE_SUBJECT: &str = "entheai.presence.coder";
const PRESENCE_PING: &str = "entheai.presence.ping";
/// Cap on a downloaded bundle — a malicious dispatcher (huge base bundle) or
/// worker (huge result bundle) must not be able to OOM the node reading it.
const BUNDLE_CAP: usize = 128 * 1024 * 1024; // 128 MiB

/// Resolved federation options (reuses the `[nats]` connection).
#[derive(Debug, Clone, Default)]
pub struct FedOptions {
    pub enabled: bool,
    pub url: Option<String>,
    pub token: Option<String>,
    pub deadline: Duration,
}

impl FedOptions {
    pub fn from_config(
        nats: &entheai_config::NatsConfig,
        fed: &entheai_config::FederationConfig,
    ) -> Self {
        let bus = entheai_bus::BusOptions::from_config(nats);
        Self {
            enabled: fed.enabled,
            url: bus.url,
            token: bus.token,
            deadline: Duration::from_secs(fed.deadline_secs),
        }
    }
}

/// A connected federation client (JetStream + core NATS). Cheap to clone.
#[derive(Clone)]
pub struct Federation {
    js: async_nats::jetstream::Context,
    client: async_nats::Client,
    deadline: Duration,
}

/// A claimed work item with its ack handle.
pub struct Claimed {
    pub item: WorkItem,
    msg: async_nats::jetstream::Message,
}

impl Claimed {
    pub async fn ack(&self) {
        let _ = self.msg.ack().await;
    }
}

impl Federation {
    /// Connect + ensure infra. Fail-safe: `None` on disabled/unreachable/error,
    /// so the caller degrades to running locally.
    pub async fn connect(opts: &FedOptions) -> Option<Federation> {
        if !opts.enabled {
            return None;
        }
        let url = opts.url.clone()?;
        let connect = match &opts.token {
            Some(t) => async_nats::ConnectOptions::with_token(t.clone()),
            None => async_nats::ConnectOptions::new(),
        };
        let client = match connect.connect(url.clone()).await {
            Ok(c) => c,
            Err(e) => {
                log::warn!(
                    "federation: connect {} failed ({e}) — off",
                    entheai_bus::redact_url(&url)
                );
                return None;
            }
        };
        let js = async_nats::jetstream::new(client.clone());
        let fed = Federation {
            js,
            client,
            deadline: opts.deadline,
        };
        if let Err(e) = fed.ensure_infra().await {
            log::warn!("federation: ensure_infra failed ({e}) — off");
            return None;
        }
        Some(fed)
    }

    async fn ensure_infra(&self) -> anyhow::Result<()> {
        self.js
            .get_or_create_stream(async_nats::jetstream::stream::Config {
                name: WORK_STREAM.into(),
                subjects: vec!["entheai.work.>".into()],
                retention: async_nats::jetstream::stream::RetentionPolicy::WorkQueue,
                ..Default::default()
            })
            .await?;
        // Idempotent-ish: ignore an already-exists error on the bucket.
        let _ = self
            .js
            .create_object_store(async_nats::jetstream::object_store::Config {
                bucket: BUNDLES_BUCKET.into(),
                ..Default::default()
            })
            .await;
        Ok(())
    }

    pub async fn put_bundle(&self, key: &str, bytes: &[u8]) -> anyhow::Result<()> {
        let store = self.js.get_object_store(BUNDLES_BUCKET).await?;
        let mut src = bytes;
        store.put(key, &mut src).await?;
        Ok(())
    }

    pub async fn get_bundle(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        let store = self.js.get_object_store(BUNDLES_BUCKET).await?;
        let mut obj = store.get(key).await?;
        let mut buf = Vec::new();
        // Bounded read (cap + 1 to detect overflow) so an oversized object can't OOM us.
        (&mut obj)
            .take((BUNDLE_CAP + 1) as u64)
            .read_to_end(&mut buf)
            .await?;
        if buf.len() > BUNDLE_CAP {
            anyhow::bail!("bundle {key} exceeds the {BUNDLE_CAP}-byte cap");
        }
        Ok(buf)
    }

    pub async fn dispatch(&self, item: &WorkItem) -> anyhow::Result<()> {
        let payload = serde_json::to_vec(item)?;
        self.js
            .publish(WORK_SUBJECT.to_string(), payload.into())
            .await?
            .await?;
        Ok(())
    }

    /// Block for the next work item (bounded by `expires`). `None` on timeout.
    pub async fn claim(&self, expires: Duration) -> anyhow::Result<Option<Claimed>> {
        let stream = self.js.get_stream(WORK_STREAM).await?;
        let consumer = stream
            .get_or_create_consumer(
                DURABLE,
                async_nats::jetstream::consumer::pull::Config {
                    durable_name: Some(DURABLE.into()),
                    filter_subject: WORK_SUBJECT.into(),
                    ack_policy: async_nats::jetstream::consumer::AckPolicy::Explicit,
                    ack_wait: self.deadline,
                    max_deliver: 3,
                    ..Default::default()
                },
            )
            .await?;
        let mut batch = consumer
            .batch()
            .max_messages(1)
            .expires(expires)
            .messages()
            .await?;
        match batch.next().await {
            Some(Ok(msg)) => {
                let item: WorkItem = serde_json::from_slice(&msg.payload)?;
                Ok(Some(Claimed { item, msg }))
            }
            _ => Ok(None),
        }
    }

    pub async fn publish_result(&self, r: &WorkResult) -> anyhow::Result<()> {
        let subject = types::result_subject(&r.session, r.index);
        self.client
            .publish(subject, serde_json::to_vec(r)?.into())
            .await?;
        self.client.flush().await?;
        Ok(())
    }

    /// Subscribe BEFORE dispatching so the core-NATS result isn't missed.
    pub async fn subscribe_result(
        &self,
        session: &str,
        index: usize,
    ) -> anyhow::Result<async_nats::Subscriber> {
        Ok(self
            .client
            .subscribe(types::result_subject(session, index))
            .await?)
    }

    /// Await one result on an existing subscription, bounded by `self.deadline`.
    pub async fn await_result(&self, sub: &mut async_nats::Subscriber) -> Option<WorkResult> {
        match tokio::time::timeout(self.deadline, sub.next()).await {
            Ok(Some(msg)) => serde_json::from_slice(&msg.payload).ok(),
            _ => None,
        }
    }

    /// Announce this worker is alive with its identity + current state. Publishes a
    /// JSON-serialized [`WorkerPresence`] to `PRESENCE_SUBJECT` (core NATS,
    /// fire-and-forget), matching the `WorkItem`/`WorkResult` on-the-wire JSON style.
    pub async fn heartbeat(&self, presence: &WorkerPresence) {
        let Ok(payload) = serde_json::to_vec(presence) else {
            return;
        };
        let _ = self.client.publish(PRESENCE_SUBJECT, payload.into()).await;
        let _ = self.client.flush().await;
    }

    /// A subscription to presence pings — a `--serve` worker heartbeats in
    /// response so `count_workers` gets a prompt answer.
    pub async fn subscribe_ping(&self) -> anyhow::Result<async_nats::Subscriber> {
        Ok(self.client.subscribe(PRESENCE_PING).await?)
    }

    /// The live worker roster: ping, collect presence heartbeats for `window`,
    /// deserialize each into a [`WorkerPresence`], and dedup to one per `node_id`
    /// (newest — see [`dedup_presence`]). This is the data behind the `/fleet` UI.
    /// Same ping/collect pattern as the old `count_workers`; non-JSON payloads
    /// (e.g. a pre-identity worker) simply don't deserialize and are skipped.
    pub async fn list_workers(&self, window: Duration) -> Vec<WorkerPresence> {
        let Ok(mut sub) = self.client.subscribe(PRESENCE_SUBJECT).await else {
            return Vec::new();
        };
        let _ = self.client.publish(PRESENCE_PING, "?".into()).await;
        let _ = self.client.flush().await;
        let mut msgs = Vec::new();
        let deadline = tokio::time::Instant::now() + window;
        while let Ok(Some(msg)) = tokio::time::timeout_at(deadline, sub.next()).await {
            if let Ok(p) = serde_json::from_slice::<WorkerPresence>(&msg.payload) {
                msgs.push(p);
            }
        }
        dedup_presence(msgs)
    }

    /// Cheap "are there workers?" — the number of DISTINCT live workers seen within
    /// `window`. Delegates to [`list_workers`] so both share the ping/collect path;
    /// `FederationExecutor::workers_available` gates dispatch on this being `> 0`.
    pub async fn count_workers(&self, window: Duration) -> usize {
        self.list_workers(window).await.len()
    }
}

/// Presence broadcast by each `--serve` worker on `PRESENCE_SUBJECT`: a stable node
/// identity plus what the worker is doing right now. This is the contract the
/// `/fleet` roster (task C2) consumes, so the shape stays `pub` and JSON-stable.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct WorkerPresence {
    /// FNV-1a of the hostname → 6 lowercase hex (see [`seeded_node_id`]).
    pub node_id: String,
    pub hostname: String,
    /// The worker build's `CARGO_PKG_VERSION`.
    pub version: String,
    pub state: WorkerState,
    /// Seconds since the Unix epoch when this worker started.
    pub started_at_unix: u64,
}

/// What a worker is doing at heartbeat time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum WorkerState {
    /// Between claims — available for work.
    Idle,
    /// Running a coder for `task`.
    Working { task: String },
}

/// A stable 6-hex node id SEEDED from the hostname (FNV-1a). Mirrors the TUI
/// env-banner's `seeded_machine_id` (`crates/tui/src/lib.rs`) — duplicated here so
/// federation need not depend on the tui crate. Same on a host every run; no
/// hardware PII.
pub fn seeded_node_id(host: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in host.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:06x}", h & 0xff_ffff)
}

/// Dedup presence messages to ONE per `node_id`, keeping the NEWEST: the entry with
/// the highest `started_at_unix` wins (a restarted worker supersedes its stale
/// self), and on a tie the last-seen message wins (so the freshest `state` — the
/// most recent heartbeat — survives). Pure, so it's unit-tested without a live NATS
/// hub; `list_workers` supplies the collected messages.
pub fn dedup_presence(msgs: Vec<WorkerPresence>) -> Vec<WorkerPresence> {
    use std::collections::HashMap;
    let mut by_node: HashMap<String, WorkerPresence> = HashMap::new();
    for m in msgs {
        match by_node.get(&m.node_id) {
            // Keep the existing entry only if it started strictly later than `m`.
            Some(prev) if prev.started_at_unix > m.started_at_unix => {}
            _ => {
                by_node.insert(m.node_id.clone(), m);
            }
        }
    }
    by_node.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_presence_json_round_trips() {
        let p = WorkerPresence {
            node_id: seeded_node_id("dev-cx53"),
            hostname: "dev-cx53".into(),
            version: "9.9.9".into(),
            state: WorkerState::Working {
                task: "add a null check".into(),
            },
            started_at_unix: 1_753_000_000,
        };
        let j = serde_json::to_vec(&p).unwrap();
        assert_eq!(serde_json::from_slice::<WorkerPresence>(&j).unwrap(), p);

        // The Idle variant survives the round trip too.
        let idle = WorkerPresence {
            state: WorkerState::Idle,
            ..p.clone()
        };
        let j = serde_json::to_vec(&idle).unwrap();
        assert_eq!(serde_json::from_slice::<WorkerPresence>(&j).unwrap(), idle);
    }

    #[test]
    fn seeded_node_id_is_six_lowercase_hex_and_stable() {
        let id = seeded_node_id("dev-cx53");
        assert_eq!(id.len(), 6, "expected 6 hex chars, got {id:?}");
        assert!(id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(id, seeded_node_id("dev-cx53"), "must be deterministic");
        assert_ne!(id, seeded_node_id("other-host"), "distinct hosts differ");
    }

    #[test]
    fn dedup_presence_keeps_one_newest_per_node() {
        let mk = |node: &str, host: &str, started: u64, state: WorkerState| WorkerPresence {
            node_id: node.into(),
            hostname: host.into(),
            version: "1.0.0".into(),
            state,
            started_at_unix: started,
        };
        // Two messages from node "aaaaaa" (older Idle, newer Working) + one other node.
        let msgs = vec![
            mk("aaaaaa", "host-a", 100, WorkerState::Idle),
            mk(
                "aaaaaa",
                "host-a",
                200,
                WorkerState::Working { task: "t".into() },
            ),
            mk("bbbbbb", "host-b", 150, WorkerState::Idle),
        ];
        let out = dedup_presence(msgs);
        assert_eq!(out.len(), 2, "one entry per node_id");
        let a = out
            .iter()
            .find(|p| p.node_id == "aaaaaa")
            .expect("node aaaaaa present");
        assert_eq!(a.started_at_unix, 200, "newest start survives");
        assert_eq!(
            a.state,
            WorkerState::Working { task: "t".into() },
            "newest state survives"
        );
        assert!(
            out.iter().any(|p| p.node_id == "bbbbbb"),
            "other node retained"
        );
    }
}
