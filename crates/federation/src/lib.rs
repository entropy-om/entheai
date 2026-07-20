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
                log::warn!("federation: connect {url} failed ({e}) — off");
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
        obj.read_to_end(&mut buf).await?;
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

    /// Announce this worker is alive (core NATS, fire-and-forget).
    pub async fn heartbeat(&self) {
        let _ = self.client.publish(PRESENCE_SUBJECT, "1".into()).await;
        let _ = self.client.flush().await;
    }

    /// A subscription to presence pings — a `--serve` worker heartbeats in
    /// response so `count_workers` gets a prompt answer.
    pub async fn subscribe_ping(&self) -> anyhow::Result<async_nats::Subscriber> {
        Ok(self.client.subscribe(PRESENCE_PING).await?)
    }

    /// Cheap "are there workers?" — ping, then count heartbeats seen within
    /// `window`. Returns the number of heartbeats (≈ live workers).
    pub async fn count_workers(&self, window: Duration) -> usize {
        let Ok(mut sub) = self.client.subscribe(PRESENCE_SUBJECT).await else {
            return 0;
        };
        let _ = self.client.publish(PRESENCE_PING, "?".into()).await;
        let _ = self.client.flush().await;
        let mut n = 0usize;
        let deadline = tokio::time::Instant::now() + window;
        while let Ok(Some(_)) = tokio::time::timeout_at(deadline, sub.next()).await {
            n += 1;
        }
        n
    }
}
