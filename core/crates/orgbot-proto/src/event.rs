//! The event log.
//!
//! The engine is event-sourced: every mutation is appended here first, and all
//! views (the task board, the roster, the transcript) are folds over this
//! stream. That buys three things the group chat needs — a Flutter client can
//! attach mid-run and catch up by replaying from a sequence number, a crashed
//! room can be rebuilt from disk, and a run is reproducible after the fact.

use serde::{Deserialize, Serialize};

use crate::graph::TaskKind;
use crate::decision::{Answer, Decision};
use crate::agent::{AgentSpec, AgentStatus};
use crate::artifact::Artifact;
use crate::chat::ChatMessage;
use crate::handoff::{Handoff, HandoffState, HandoffTransition};
use crate::ids::{AgentId, ArtifactId, DecisionId, EventId, HandoffId, RoomId, TaskId};
use crate::review::Review;
use crate::task::{Task, TaskState, TaskTransition};
use crate::time::Timestamp;

/// A monotonically increasing position in a room's log.
///
/// Clients resume with "everything after N", so gaps and reordering are not
/// permitted: the engine assigns these under the same lock that appends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Seq(pub u64);

impl Seq {
    /// The position before any event has been written.
    pub const ZERO: Seq = Seq(0);

    /// The next position after this one.
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// Something that happened in a room.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// A room was opened.
    RoomOpened { room: RoomId, name: String, goal: String },
    /// The room finished, successfully or otherwise.
    RoomClosed { room: RoomId, reason: String },

    /// A bot joined the roster.
    AgentJoined { spec: Box<AgentSpec> },
    /// A bot left.
    AgentLeft { agent: AgentId, reason: String },
    /// A bot's availability changed.
    AgentStatusChanged {
        agent: AgentId,
        from: AgentStatus,
        to: AgentStatus,
    },

    /// A message was posted to the transcript.
    MessagePosted { message: Box<ChatMessage> },

    /// A task was added to the board.
    TaskCreated { task: Box<Task> },
    /// A task was let through by authority the operator had already given,
    /// rather than by asking them again.
    TaskAuthorized {
        /// The task released.
        task: TaskId,
        /// The standing decision that covered it.
        decision: DecisionId,
    },
    /// A question was put to the operator. Carries the whole decision, so a
    /// room rebuilt from its log knows what it has already asked.
    DecisionAsked {
        /// The question, with the authority an approval would grant.
        decision: Box<Decision>,
    },
    /// Another bot joined an already-pending owner question.
    DecisionQuestionJoined {
        /// The pending question.
        decision: DecisionId,
        /// The newly joined asker.
        by: AgentId,
        /// Active task that will wait for the answer, if any.
        task: Option<TaskId>,
    },
    /// The operator answered.
    DecisionAnswered {
        /// The question answered.
        decision: DecisionId,
        /// What they said.
        answer: Answer,
    },
    /// A task changed state. Carries both ends so a client that missed earlier
    /// events can still render the change coherently.
    TaskTransitioned {
        task: TaskId,
        /// The task's title, for the same reason `from` and `to` are both
        /// here: a reader should not need the rest of the log to understand
        /// one event.
        #[serde(default)]
        title: String,
        /// What kind of record moved. A delivered objective is a milestone; a
        /// delivered subtask is the room working.
        #[serde(default)]
        kind: TaskKind,
        from: TaskState,
        to: TaskState,
        transition: Box<TaskTransition>,
        /// The bot responsible, or `None` for engine-driven changes such as an
        /// expiry sweep.
        actor: Option<AgentId>,
    },

    /// A handoff was offered.
    HandoffOffered { handoff: Box<Handoff> },
    /// A handoff changed state.
    HandoffTransitioned {
        handoff: HandoffId,
        from: HandoffState,
        to: HandoffState,
        transition: Box<HandoffTransition>,
    },

    /// A deliverable was recorded.
    ArtifactAdded { artifact: Box<Artifact> },
    /// An inspection was completed.
    ReviewRecorded { review: Box<Review> },

    /// The engine found a cycle in the wait-for graph. Carries the cycle so
    /// the UI can show exactly who is waiting on whom.
    DeadlockDetected {
        /// Tasks forming the cycle, in wait order.
        cycle: Vec<TaskId>,
        /// The bots holding those tasks, positionally aligned with `cycle`.
        agents: Vec<AgentId>,
        /// What the engine did about it.
        resolution: DeadlockResolution,
    },

    /// The room's own assessment of whether it can carry on changed.
    ///
    /// Raised when a condition appears or clears, not on every check: a room
    /// that repeats itself trains its operator to ignore it, and then the new
    /// thing goes unread with the rest.
    RoomHealthChanged {
        /// Whether the room can carry on without anyone.
        ok: bool,
        /// One line naming what is wrong.
        summary: String,
        /// The whole picture, for someone reading deliberately.
        detail: String,
    },

    /// A bot's turn failed. Repeated failures fault the bot.
    AgentErrored {
        agent: AgentId,
        task: Option<TaskId>,
        error: String,
        /// Which retry this was.
        attempt: u32,
    },
}

/// What the engine did after detecting a deadlock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "resolution", rename_all = "snake_case")]
pub enum DeadlockResolution {
    /// One task in the cycle was released back to the pool to break it.
    ReleasedTask { task: TaskId },
    /// The blocking handoff was withdrawn.
    WithdrewHandoff { handoff: HandoffId },
    /// The engine could not break it; a human must intervene.
    EscalatedToHuman,
}

impl Event {
    /// A stable name for metrics and log filtering.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::RoomOpened { .. } => "room_opened",
            Self::RoomClosed { .. } => "room_closed",
            Self::AgentJoined { .. } => "agent_joined",
            Self::AgentLeft { .. } => "agent_left",
            Self::AgentStatusChanged { .. } => "agent_status_changed",
            Self::MessagePosted { .. } => "message_posted",
            Self::TaskCreated { .. } => "task_created",
            Self::TaskAuthorized { .. } => "task_authorized",
            Self::DecisionAsked { .. } => "decision_asked",
            Self::DecisionQuestionJoined { .. } => "decision_question_joined",
            Self::DecisionAnswered { .. } => "decision_answered",
            Self::TaskTransitioned { .. } => "task_transitioned",
            Self::HandoffOffered { .. } => "handoff_offered",
            Self::HandoffTransitioned { .. } => "handoff_transitioned",
            Self::ArtifactAdded { .. } => "artifact_added",
            Self::ReviewRecorded { .. } => "review_recorded",
            Self::DeadlockDetected { .. } => "deadlock_detected",
            Self::AgentErrored { .. } => "agent_errored",
            Self::RoomHealthChanged { .. } => "room_health_changed",
        }
    }

    /// The task this event concerns, for per-task filtering in the UI.
    pub fn task(&self) -> Option<TaskId> {
        match self {
            Self::TaskCreated { task } => Some(task.id),
            Self::TaskTransitioned { task, .. } => Some(*task),
            Self::HandoffOffered { handoff } => Some(handoff.task),
            Self::ArtifactAdded { artifact } => Some(artifact.task),
            Self::ReviewRecorded { review } => Some(review.task),
            Self::MessagePosted { message } => message.task,
            Self::AgentErrored { task, .. } => *task,
            _ => None,
        }
    }

    /// The bot that caused this event, where one is identifiable.
    pub fn actor(&self) -> Option<AgentId> {
        match self {
            Self::AgentJoined { spec } => Some(spec.id),
            Self::AgentLeft { agent, .. } => Some(*agent),
            Self::AgentStatusChanged { agent, .. } => Some(*agent),
            Self::MessagePosted { message } => message.author.agent(),
            Self::TaskTransitioned { actor, .. } => *actor,
            Self::HandoffOffered { handoff } => Some(handoff.from),
            Self::HandoffTransitioned { transition, .. } => transition.actor(),
            Self::ArtifactAdded { artifact } => Some(artifact.author),
            Self::ReviewRecorded { review } => Some(review.inspector),
            Self::AgentErrored { agent, .. } => Some(*agent),
            _ => None,
        }
    }

    /// Whether this event ends the room.
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::RoomClosed { .. })
    }
}

/// An event plus its position and provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub id: EventId,
    pub seq: Seq,
    pub room: RoomId,
    pub at: Timestamp,
    pub event: Event,
}

impl EventEnvelope {
    /// Stamp an event with its position in the log.
    pub fn new(seq: Seq, room: RoomId, event: Event) -> Self {
        Self {
            id: EventId::new(),
            seq,
            room,
            at: Timestamp::now(),
            event,
        }
    }

    /// Serialize as one JSON Lines record.
    ///
    /// The on-disk log is JSONL so it can be tailed, grepped, and truncated at
    /// a record boundary without a parser.
    pub fn to_jsonl(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Parse one JSON Lines record.
    pub fn from_jsonl(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }
}

/// Artifacts referenced by an event, for prefetching in a client.
pub fn referenced_artifacts(event: &Event) -> Vec<ArtifactId> {
    match event {
        Event::ArtifactAdded { artifact } => vec![artifact.id],
        Event::MessagePosted { message } => message.attachments.clone(),
        Event::HandoffOffered { handoff } => handoff.context.clone(),
        Event::ReviewRecorded { review } => review.inspected.clone(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::ChatMessage;
    use crate::task::Task;

    #[test]
    fn sequence_numbers_advance_by_one() {
        assert_eq!(Seq::ZERO.next(), Seq(1));
        assert_eq!(Seq(41).next(), Seq(42));
    }

    #[test]
    fn envelopes_round_trip_through_jsonl() {
        let room = RoomId::new();
        let task = Task::draft(room, "ship", "do it");
        let envelope = EventEnvelope::new(
            Seq(7),
            room,
            Event::TaskCreated {
                task: Box::new(task),
            },
        );
        let line = envelope.to_jsonl().unwrap();
        assert!(!line.contains('\n'), "a JSONL record must be one line");
        assert_eq!(EventEnvelope::from_jsonl(&line).unwrap(), envelope);
    }

    #[test]
    fn events_report_the_task_they_concern() {
        let room = RoomId::new();
        let task = Task::draft(room, "ship", "do it");
        let id = task.id;
        let created = Event::TaskCreated { task: Box::new(task) };
        assert_eq!(created.task(), Some(id));
        assert_eq!(
            Event::RoomClosed {
                room,
                reason: "done".into()
            }
            .task(),
            None
        );
    }

    #[test]
    fn events_report_their_actor() {
        let (room, bot) = (RoomId::new(), AgentId::new());
        let posted = Event::MessagePosted {
            message: Box::new(ChatMessage::say(room, bot, "hi")),
        };
        assert_eq!(posted.actor(), Some(bot));

        let system = Event::MessagePosted {
            message: Box::new(ChatMessage::system(room, "notice")),
        };
        assert_eq!(system.actor(), None);
    }

    #[test]
    fn only_room_closed_is_terminal() {
        let room = RoomId::new();
        assert!(Event::RoomClosed {
            room,
            reason: String::new()
        }
        .is_terminal());
        assert!(!Event::RoomOpened {
            room,
            name: String::new(),
            goal: String::new()
        }
        .is_terminal());
    }

    #[test]
    fn deadlock_events_carry_the_full_cycle() {
        let (a, b) = (TaskId::new(), TaskId::new());
        let event = Event::DeadlockDetected {
            cycle: vec![a, b],
            agents: vec![AgentId::new(), AgentId::new()],
            resolution: DeadlockResolution::ReleasedTask { task: a },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
        assert_eq!(event.name(), "deadlock_detected");
    }
}
