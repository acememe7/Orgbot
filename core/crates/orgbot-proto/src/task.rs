//! The task lifecycle.
//!
//! A task moves through a single explicit state machine. Every mutation in the
//! engine goes through [`TaskState::apply`], so an illegal move (submitting
//! work that was never inspected, two bots starting the same task) is a typed
//! error at the boundary rather than corrupt state discovered three steps later.
//!
//! ```text
//!   Draft ──open──> Ready ──claim──> Claimed ──start──> InProgress
//!                     ^                 |                 |   ^
//!                     └──── release ────┴─────────────────┘   │
//!                                                  |          │
//!                             block │  ^ unblock   │          │ request_changes
//!                                   v  |           │          │
//!                                 Blocked          │          │
//!                                                  v          │
//!                                      submit_for_review      │
//!                                                  |          │
//!                                                  v          │
//!                                              InReview ──────┘
//!                                                  |
//!                                               approve
//!                                                  v
//!                                              Approved ──submit──> Submitted
//!
//!   any non-terminal ──fail/cancel──> Failed / Cancelled
//!   any terminal ──reopen──> Ready
//! ```

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, ArtifactId, HandoffId, RoomId, TaskId};
use crate::time::Timestamp;

/// Where a task sits in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Created but not yet visible to bots looking for work.
    Draft,
    /// Open and unassigned; any bot with a matching role may claim it.
    Ready,
    /// Assigned to a bot that has not begun work.
    Claimed,
    /// A bot is actively working the task.
    InProgress,
    /// Work is suspended pending a handoff, another task, or a human.
    Blocked,
    /// Work is finished and awaiting inspection.
    InReview,
    /// Inspection passed; the task may be submitted.
    Approved,
    /// Terminal: delivered.
    Submitted,
    /// Terminal: abandoned after an unrecoverable error.
    Failed,
    /// Terminal: withdrawn before completion.
    Cancelled,
}

impl TaskState {
    /// Whether no further work will happen without an explicit reopen.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Submitted | Self::Failed | Self::Cancelled)
    }

    /// Whether a bot currently holds this task.
    pub const fn is_assigned(self) -> bool {
        matches!(self, Self::Claimed | Self::InProgress | Self::Blocked | Self::InReview | Self::Approved)
    }

    /// Whether the task is waiting on something outside itself.
    pub const fn is_waiting(self) -> bool {
        matches!(self, Self::Blocked | Self::InReview)
    }

    /// Whether an unassigned bot may claim this task.
    pub const fn is_claimable(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// Stable lowercase name, matching the serde representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Ready => "ready",
            Self::Claimed => "claimed",
            Self::InProgress => "in_progress",
            Self::Blocked => "blocked",
            Self::InReview => "in_review",
            Self::Approved => "approved",
            Self::Submitted => "submitted",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Apply a transition, yielding the next state.
    ///
    /// This is the single source of truth for what is legal; the engine never
    /// assigns a [`TaskState`] directly.
    pub fn apply(self, transition: &TaskTransition) -> Result<TaskState, TransitionError> {
        use TaskState::*;
        use TaskTransition as T;

        let next = match (self, transition) {
            (Draft, T::Open) => Ready,
            (Ready, T::Claim { .. }) => Claimed,
            (Claimed, T::Start) => InProgress,

            // A blocked task may be released or re-blocked on a new reason
            // without first unblocking, which is what happens when a bot
            // discovers a second missing dependency.
            (InProgress | Blocked, T::Block { .. }) => Blocked,
            (Blocked, T::Unblock) => InProgress,

            (Claimed | InProgress | Blocked, T::Release { .. }) => Ready,

            (InProgress, T::SubmitForReview { .. }) => InReview,
            (InReview, T::Approve { .. }) => Approved,
            (InReview, T::RequestChanges { .. }) => InProgress,

            (Approved, T::Submit { .. }) => Submitted,

            (state, T::Fail { .. }) if !state.is_terminal() => Failed,
            (state, T::Cancel { .. }) if !state.is_terminal() => Cancelled,
            (state, T::Reopen) if state.is_terminal() => Ready,

            (from, transition) => {
                return Err(TransitionError {
                    from,
                    transition: transition.name().to_string(),
                })
            }
        };
        Ok(next)
    }

    /// Whether `transition` is legal from this state, without applying it.
    pub fn can_apply(self, transition: &TaskTransition) -> bool {
        self.apply(transition).is_ok()
    }
}

/// A transition was attempted from a state that does not permit it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[error("cannot `{transition}` a task that is `{}`", from.as_str())]
pub struct TransitionError {
    /// The state the task was actually in.
    pub from: TaskState,
    /// The name of the rejected transition.
    pub transition: String,
}

/// Why a task stopped making progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BlockReason {
    /// Waiting for a specific handoff to be accepted and completed.
    AwaitingHandoff { handoff: HandoffId },
    /// Waiting for another task to reach a terminal success.
    AwaitingTask { task: TaskId },
    /// Waiting for a person to answer.
    AwaitingHuman { prompt: String },
    /// Waiting on something the engine does not model.
    Other { detail: String },
}

impl BlockReason {
    /// The task this reason depends on, if the dependency is a task or a
    /// handoff's task. Used to build the wait-for graph for deadlock detection.
    pub const fn depends_on_task(&self) -> Option<TaskId> {
        match self {
            Self::AwaitingTask { task } => Some(*task),
            _ => None,
        }
    }

    /// Whether only a human can clear this block.
    pub const fn needs_human(&self) -> bool {
        matches!(self, Self::AwaitingHuman { .. })
    }
}

/// An event that moves a task from one state to the next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transition", rename_all = "snake_case")]
pub enum TaskTransition {
    /// Publish a draft so bots can claim it.
    Open,
    /// Assign the task to a bot.
    Claim { by: AgentId },
    /// Begin work.
    Start,
    /// Suspend work pending `reason`.
    Block { reason: BlockReason },
    /// Resume after the block clears.
    Unblock,
    /// Return the task to the pool, dropping the assignee.
    Release { reason: String },
    /// Declare the work finished and request inspection.
    SubmitForReview { artifacts: Vec<ArtifactId> },
    /// Inspection passed.
    Approve { by: AgentId },
    /// Inspection failed; send it back for rework.
    RequestChanges { by: AgentId, reason: String },
    /// Deliver the approved work.
    Submit { by: AgentId },
    /// Abandon after an unrecoverable error.
    Fail { reason: String },
    /// Withdraw the task.
    Cancel { reason: String },
    /// Return a terminal task to the pool.
    Reopen,
}

impl TaskTransition {
    /// A stable name for logs and error messages.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Claim { .. } => "claim",
            Self::Start => "start",
            Self::Block { .. } => "block",
            Self::Unblock => "unblock",
            Self::Release { .. } => "release",
            Self::SubmitForReview { .. } => "submit_for_review",
            Self::Approve { .. } => "approve",
            Self::RequestChanges { .. } => "request_changes",
            Self::Submit { .. } => "submit",
            Self::Fail { .. } => "fail",
            Self::Cancel { .. } => "cancel",
            Self::Reopen => "reopen",
        }
    }
}

/// How urgently a task should be picked up. Higher sorts first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    Normal,
    High,
    Urgent,
}

impl Default for Priority {
    fn default() -> Self {
        Self::Normal
    }
}

/// A unit of work on the board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub room: RoomId,
    /// One-line summary shown in the task board.
    pub title: String,
    /// The full instruction given to the working bot.
    pub brief: String,
    /// Conditions the inspector checks before approving. An empty list means
    /// the inspector uses its own judgement.
    pub acceptance_criteria: Vec<String>,
    /// The role a bot must have to claim this task. `None` means any role.
    pub required_role: Option<String>,
    pub state: TaskState,
    pub priority: Priority,
    /// The bot currently holding the task, if any.
    pub assignee: Option<AgentId>,
    /// Why the task is blocked. Non-empty only while `state == Blocked`.
    pub blocked_on: Vec<BlockReason>,
    /// Tasks that must reach `Submitted` before this one may start.
    pub depends_on: Vec<TaskId>,
    /// Set when this task was split out of a larger one.
    pub parent: Option<TaskId>,
    /// Deliverables produced so far.
    pub artifacts: Vec<ArtifactId>,
    /// How many times the task has come back from inspection.
    pub revision: u32,
    /// Rework budget. Once `revision` exceeds this, the engine fails the task
    /// instead of looping forever between a worker and an inspector.
    pub max_revisions: u32,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Task {
    /// A new draft task with sensible defaults.
    pub fn draft(room: RoomId, title: impl Into<String>, brief: impl Into<String>) -> Self {
        let now = Timestamp::now();
        Self {
            id: TaskId::new(),
            room,
            title: title.into(),
            brief: brief.into(),
            acceptance_criteria: Vec::new(),
            required_role: None,
            state: TaskState::Draft,
            priority: Priority::default(),
            assignee: None,
            blocked_on: Vec::new(),
            depends_on: Vec::new(),
            parent: None,
            artifacts: Vec::new(),
            revision: 0,
            max_revisions: DEFAULT_MAX_REVISIONS,
            created_at: now,
            updated_at: now,
        }
    }

    /// Whether the rework budget is spent.
    ///
    /// Matches the check the engine applies before sending work back: once
    /// `revision` reaches `max_revisions` the next rejection fails the task
    /// instead of queueing another round.
    pub const fn is_out_of_revisions(&self) -> bool {
        self.revision >= self.max_revisions
    }

    /// Whether `agent` may act on this task right now.
    pub fn is_held_by(&self, agent: AgentId) -> bool {
        self.assignee == Some(agent)
    }
}

/// Default rework budget: a task may come back from inspection three times
/// before the engine gives up on it.
pub const DEFAULT_MAX_REVISIONS: u32 = 3;

#[cfg(test)]
mod tests {
    use super::TaskState::*;
    use super::*;

    fn agent() -> AgentId {
        AgentId::new()
    }

    /// Walk the intended happy path end to end.
    #[test]
    fn happy_path_reaches_submitted() {
        let bot = agent();
        let states = [
            (Draft, TaskTransition::Open, Ready),
            (Ready, TaskTransition::Claim { by: bot }, Claimed),
            (Claimed, TaskTransition::Start, InProgress),
            (
                InProgress,
                TaskTransition::SubmitForReview { artifacts: vec![] },
                InReview,
            ),
            (InReview, TaskTransition::Approve { by: bot }, Approved),
            (Approved, TaskTransition::Submit { by: bot }, Submitted),
        ];
        for (from, transition, want) in states {
            assert_eq!(from.apply(&transition).unwrap(), want, "{from:?} -> {want:?}");
        }
    }

    #[test]
    fn work_cannot_be_submitted_without_inspection() {
        let err = InProgress
            .apply(&TaskTransition::Submit { by: agent() })
            .unwrap_err();
        assert_eq!(err.from, InProgress);
        assert_eq!(err.transition, "submit");
    }

    #[test]
    fn approved_work_cannot_skip_back_into_review() {
        assert!(Approved
            .apply(&TaskTransition::SubmitForReview { artifacts: vec![] })
            .is_err());
    }

    #[test]
    fn a_task_cannot_be_claimed_twice() {
        assert!(Claimed.apply(&TaskTransition::Claim { by: agent() }).is_err());
        assert!(InProgress.apply(&TaskTransition::Claim { by: agent() }).is_err());
    }

    #[test]
    fn blocking_and_unblocking_round_trips() {
        let reason = BlockReason::AwaitingTask { task: TaskId::new() };
        let blocked = InProgress
            .apply(&TaskTransition::Block { reason: reason.clone() })
            .unwrap();
        assert_eq!(blocked, Blocked);
        assert_eq!(blocked.apply(&TaskTransition::Unblock).unwrap(), InProgress);
    }

    #[test]
    fn a_blocked_task_can_gain_a_second_blocker() {
        let reason = BlockReason::AwaitingHuman {
            prompt: "which region?".into(),
        };
        assert_eq!(
            Blocked.apply(&TaskTransition::Block { reason }).unwrap(),
            Blocked
        );
    }

    #[test]
    fn rework_returns_to_in_progress() {
        let next = InReview
            .apply(&TaskTransition::RequestChanges {
                by: agent(),
                reason: "missing tests".into(),
            })
            .unwrap();
        assert_eq!(next, InProgress);
    }

    #[test]
    fn release_returns_the_task_to_the_pool_from_any_held_state() {
        for held in [Claimed, InProgress, Blocked] {
            let next = held
                .apply(&TaskTransition::Release {
                    reason: "bot offline".into(),
                })
                .unwrap();
            assert_eq!(next, Ready, "releasing from {held:?}");
        }
    }

    #[test]
    fn terminal_states_reject_every_transition_but_reopen() {
        let every = [
            TaskTransition::Open,
            TaskTransition::Claim { by: agent() },
            TaskTransition::Start,
            TaskTransition::Unblock,
            TaskTransition::Release { reason: String::new() },
            TaskTransition::SubmitForReview { artifacts: vec![] },
            TaskTransition::Approve { by: agent() },
            TaskTransition::Submit { by: agent() },
            TaskTransition::Fail { reason: String::new() },
            TaskTransition::Cancel { reason: String::new() },
        ];
        for terminal in [Submitted, Failed, Cancelled] {
            for transition in &every {
                assert!(
                    terminal.apply(transition).is_err(),
                    "{terminal:?} should reject {}",
                    transition.name()
                );
            }
            assert_eq!(terminal.apply(&TaskTransition::Reopen).unwrap(), Ready);
        }
    }

    #[test]
    fn reopen_is_rejected_on_live_tasks() {
        for live in [Draft, Ready, Claimed, InProgress, Blocked, InReview, Approved] {
            assert!(live.apply(&TaskTransition::Reopen).is_err(), "{live:?}");
        }
    }

    #[test]
    fn failure_is_reachable_from_every_live_state() {
        for live in [Draft, Ready, Claimed, InProgress, Blocked, InReview, Approved] {
            let next = live
                .apply(&TaskTransition::Fail { reason: "boom".into() })
                .unwrap();
            assert_eq!(next, Failed, "failing from {live:?}");
        }
    }

    #[test]
    fn state_predicates_agree_with_the_machine() {
        assert!(Submitted.is_terminal() && Failed.is_terminal() && Cancelled.is_terminal());
        assert!(!InProgress.is_terminal());
        assert!(Ready.is_claimable() && !Claimed.is_claimable());
        assert!(Blocked.is_waiting() && InReview.is_waiting());
        assert!(InProgress.is_assigned() && !Ready.is_assigned());
    }

    #[test]
    fn a_fresh_draft_has_no_assignee_and_full_rework_budget() {
        let task = Task::draft(RoomId::new(), "ship it", "do the thing");
        assert_eq!(task.state, Draft);
        assert!(task.assignee.is_none());
        assert!(!task.is_out_of_revisions());
    }

    #[test]
    fn transitions_survive_a_json_round_trip() {
        let original = TaskTransition::Block {
            reason: BlockReason::AwaitingHandoff { handoff: HandoffId::new() },
        };
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(serde_json::from_str::<TaskTransition>(&json).unwrap(), original);
    }
}
