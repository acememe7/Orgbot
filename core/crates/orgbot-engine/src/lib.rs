//! # orgbot-engine
//!
//! Orchestration for a room of bots: the task board, handoff routing,
//! inspection gating, and deadlock detection.
//!
//! The engine is pure — no I/O, no async, no model calls. Everything that
//! decides *whether a bot may do something* lives here and is unit-testable
//! without a network. Layers above supply the model turns and the transport.
//!
//! ```no_run
//! use orgbot_engine::{Command, Engine};
//! use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
//!
//! let mut engine = Engine::new(RoomSpec {
//!     name: "release".into(),
//!     goal: "ship the notes".into(),
//!     roster: vec![],
//!     completion: CompletionRule::AllTasksTerminal,
//!     limits: RoomLimits::default(),
//! });
//! engine.apply(Command::StartRoom).unwrap();
//! ```

#![forbid(unsafe_code)]
// Every module, type, method, and enum *variant* in this crate carries docs.
// `missing_docs` is not enabled because it also fires on the fields inside
// serde data-carrier variants — `Claim { by: AgentId }`, `Fail { reason: String }` —
// where a per-field comment restates the field name and adds nothing. Those
// variants are documented one level up, where the meaning actually lives.
#![warn(missing_debug_implementations, rust_2018_idioms, unreachable_pub)]

pub mod command;
pub mod analytics;
pub mod consistency;
pub mod deadlock;
pub mod digest;
pub mod eligibility;
pub mod health;
pub mod engine;
pub mod notify;
pub mod error;

pub use command::Command;
pub use analytics::{retrospective, Retrospective};
pub use consistency::Violation;
pub use deadlock::WaitGraph;
pub use digest::{digest, BoardCounts, ObjectiveCard, Progress, RoomDigest};
pub use eligibility::{Eligibility, LeaseState, Reason};
pub use health::Health;
pub use engine::Engine;
pub use notify::{notification_for, Notification};
pub use error::{EngineError, Result};
