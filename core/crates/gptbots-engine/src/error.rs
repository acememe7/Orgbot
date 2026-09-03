//! Errors returned when a command would violate a room invariant.
//!
//! Every variant here is a *rejected command*, not a crash: the engine leaves
//! its state untouched and the caller — usually a bot's tool call — gets the
//! message back so the model can correct itself and try again.

use gptbots_proto::handoff::HandoffError;
use gptbots_proto::task::TransitionError;
use gptbots_proto::{AgentId, ArtifactId, HandoffId, RoomPhase, TaskId};
use serde::{Deserialize, Serialize};

/// A command was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum EngineError {
    #[error("no task {task}")]
    UnknownTask { task: TaskId },

    #[error("no bot {agent} in this room")]
    UnknownAgent { agent: AgentId },

    #[error("no handoff {handoff}")]
    UnknownHandoff { handoff: HandoffId },

    #[error("no artifact {artifact}")]
    UnknownArtifact { artifact: ArtifactId },

    #[error(transparent)]
    Transition(#[from] TransitionError),

    #[error(transparent)]
    Handoff(#[from] HandoffError),

    #[error("task {task} is held by {holder}, not {agent}")]
    NotAssignee {
        task: TaskId,
        agent: AgentId,
        holder: String,
    },

    #[error("task {task} requires role `{required}`, which {agent} does not hold")]
    RoleMismatch {
        task: TaskId,
        agent: AgentId,
        required: String,
    },

    #[error("{agent} is not permitted to {action}")]
    NotPermitted { agent: AgentId, action: String },

    #[error("{agent} cannot inspect its own work on task {task}")]
    SelfInspection { task: TaskId, agent: AgentId },

    #[error("task {task} cannot start until {blocker} is submitted")]
    DependencyNotMet { task: TaskId, blocker: TaskId },

    #[error("{agent} already has {open} handoffs open, limit is {limit}")]
    TooManyOpenHandoffs {
        agent: AgentId,
        open: u32,
        limit: u32,
    },

    #[error("the room is {phase:?}, not running")]
    RoomNotRunning { phase: RoomPhase },

    #[error("task {task} has used its {budget} revisions and cannot be reworked again")]
    OutOfRevisions { task: TaskId, budget: u32 },

    #[error("handle `{handle}` is already taken in this room")]
    DuplicateHandle { handle: String },

    #[error("a bot cannot hand off to itself")]
    SelfHandoff,

    #[error("task {task} is not part of this room")]
    WrongRoom { task: TaskId },
}

/// Shorthand for engine results.
pub type Result<T> = std::result::Result<T, EngineError>;
