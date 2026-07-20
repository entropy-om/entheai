//! entheai-federation (F2): dispatch coder sub-tasks to worker nodes over NATS
//! JetStream. A WorkQueue stream delivers each `WorkItem` to exactly one worker;
//! git bundles travel through the JetStream object store; results return over
//! core NATS. Fail-safe: any NATS failure leaves the caller to run locally.

pub mod repo;
pub mod types;
