//! `tantra-agent` — board-control agent for the tantric board
//! (https://mlxquantlovefrom.com/board), a GitHub-issues-backed kanban in
//! `peterlodri-sec/mlxquantlovefrom.com`.
//!
//! - [`board`] — the GitHub API client, lane/label mapping, collaborator
//!   token resolution, URL building and the `todo:` / `daily:` title
//!   conventions. Pure helpers are unit-tested.
//! - [`tools`] — the three `adk_rust::Tool` impls (`tantra_list`, `tantra_add`,
//!   `tantra_move`).
//! - [`agent`] — the minimal adk-rust agent wiring those tools to an
//!   OpenAI-compatible model (the free `coder.vaked.dev` node by default).
//!
//! The CLI (`tantra-agent list|add|move|todo|summary|whoami|agent`) in
//! `main.rs` is the primary surface; the agent is a thin wrapper over it.

pub mod agent;
pub mod board;
pub mod tools;
