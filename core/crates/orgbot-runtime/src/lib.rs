//! # orgbot-runtime
//!
//! The layer that makes a room actually run: it gives bots turns, feeds them
//! what changed, executes their tool calls, and decides when the work is done.
//!
//! ```no_run
//! use std::sync::Arc;
//! use orgbot_runtime::Orchestrator;
//! use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
//! use orgbot_proto::{AgentSpec, agent::ProviderRef};
//! use orgbot_providers::ScriptedProvider;
//!
//! # async fn example() {
//! let mut room = Orchestrator::new(RoomSpec {
//!     name: "release".into(),
//!     goal: "ship the release notes".into(),
//!     roster: vec![],
//!     completion: CompletionRule::AllTasksTerminal,
//!     limits: RoomLimits::default(),
//! });
//!
//! let spec = AgentSpec::new("w1", "Worker", ProviderRef::Scripted { script: "s".into() });
//! room.seat(spec, Arc::new(ScriptedProvider::always_says("on it"))).unwrap();
//! room.start().unwrap();
//!
//! let outcome = room.run(None).await;
//! # }
//! ```

#![forbid(unsafe_code)]
// See the note in orgbot-proto on `missing_docs` and data-carrier variants.
#![warn(missing_debug_implementations, rust_2018_idioms, unreachable_pub)]

pub mod allowance;
pub mod attachments;
pub mod bot;
mod capabilities;
pub mod mcp;
mod mcp_approval;
pub mod mcp_client;
pub mod orchestrator;
pub mod prompt;
pub mod relay;
pub mod shared;
pub mod workspace;

pub use bot::{Bot, TurnReport};
pub use mcp::{room_tools_for, McpCall, McpSession};
pub use orchestrator::{Orchestrator, RunOutcome};
pub use shared::SharedEngine;
