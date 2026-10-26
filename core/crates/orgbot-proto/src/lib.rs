//! # orgbot-proto
//!
//! The wire protocol shared by every OrgBot surface: the Rust engine, the
//! Flutter app, the TypeScript SDK, and the Python tool workers.
//!
//! Two rules keep the four implementations honest:
//!
//! 1. **State machines live here, not in the engine.** [`task::TaskState`] and
//!    [`handoff::HandoffState`] own every legal transition, so a client can
//!    grey out an impossible button using the same rules the server enforces.
//! 2. **The log is the truth.** [`event::Event`] is the only way state changes.
//!    Views are folds over that stream.

#![forbid(unsafe_code)]
// Every module, type, method, and enum *variant* in this crate carries docs.
// `missing_docs` is not enabled because it also fires on the fields inside
// serde data-carrier variants — `Claim { by: AgentId }`, `Fail { reason: String }` —
// where a per-field comment restates the field name and adds nothing. Those
// variants are documented one level up, where the meaning actually lives.
#![warn(missing_debug_implementations, rust_2018_idioms, unreachable_pub)]

pub mod agent;
pub mod artifact;
pub mod chat;
pub mod convergence;
pub mod decision;
pub mod event;
pub mod graph;
pub mod handoff;
pub mod ids;
pub mod review;
pub mod roles;
pub mod room;
pub mod skill;
pub mod spend;
pub mod task;
pub mod text;
pub mod time;

pub use agent::{AgentPolicy, AgentSpec, AgentStatus, Autonomy, CliKind, ProviderRef};
pub use artifact::{Artifact, ArtifactBody};
pub use chat::{is_broadcast_tag, Audience, Author, ChatMessage, MessageIntent};
pub use event::{Event, EventEnvelope, Seq};
pub use handoff::{Handoff, HandoffKind, HandoffState, HandoffTransition, Recipient};
pub use ids::{
    AgentId, ArtifactId, DecisionId, EventId, HandoffId, MessageId, ReviewId, RoomId, TaskId,
};
pub use review::{Finding, Review, Severity, Verdict};
pub use room::{Room, RoomPhase, RoomSpec};
pub use task::{BlockReason, Priority, Task, TaskState, TaskTransition};
pub use time::Timestamp;

/// The protocol version this build speaks.
///
/// Clients send it on connect; the server refuses a mismatched major version
/// rather than failing later on an unknown enum variant.
pub const PROTOCOL_VERSION: u32 = 1;
