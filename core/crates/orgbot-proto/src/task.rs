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

use crate::graph::{AuthorizationState, DependencyState, Lineage, Readiness, TaskKind};
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
    /// Terminal and permanent: replaced by another task, which now carries the
    /// work. Kept for audit; never actionable again.
    Superseded,
    /// Terminal and permanent: abandoned as no longer relevant. Kept for audit.
    Retired,
    /// Terminal and permanent: folded away after the room finished with it.
    Archived,
}

impl TaskState {
    /// Whether no further work will happen without an explicit reopen.
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Submitted
                | Self::Failed
                | Self::Cancelled
                | Self::Superseded
                | Self::Retired
                | Self::Archived
        )
    }

    /// Whether this task is closed for good and cannot be reopened.
    ///
    /// A superseded task's work lives on the record that replaced it, and a
    /// retired one was judged irrelevant. Letting either come back turns an
    /// old decision into live work again — which is exactly how a settled
    /// incident task kept resurfacing on a real board.
    pub const fn is_permanent(self) -> bool {
        matches!(self, Self::Superseded | Self::Retired | Self::Archived)
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
            Self::Superseded => "superseded",
            Self::Retired => "retired",
            Self::Archived => "archived",
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
            (state, T::Reopen) if state.is_terminal() && !state.is_permanent() => Ready,

            // Superseding and retiring are one-way. A task may be replaced
            // from any live state; the replacement carries the work forward.
            (state, T::Supersede { .. }) if !state.is_terminal() => Superseded,
            (state, T::Retire { .. }) if !state.is_permanent() => Retired,
            (state, T::Archive) if state.is_terminal() && !state.is_permanent() => Archived,

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
    /// Replace this task with another, which takes over its work.
    Supersede {
        /// The task that now carries this one's work.
        by: TaskId,
        /// Why it was replaced.
        reason: String,
    },
    /// Close this task as no longer relevant, permanently.
    Retire {
        /// Why it is no longer relevant.
        reason: String,
    },
    /// Fold a finished task away from the active board, keeping its history.
    Archive,
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
            Self::Supersede { .. } => "supersede",
            Self::Retire { .. } => "retire",
            Self::Archive => "archive",
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
    /// What kind of record this is. Only an objective is a top-level card;
    /// reviews and remediations hang off one.
    #[serde(default)]
    pub kind: TaskKind,
    /// Where this came from and what it replaces. Every link defaults to
    /// unknown, so a room written before lineage existed does not gain
    /// relationships that were never recorded.
    #[serde(default)]
    pub lineage: Lineage,
    /// Whether the work this needs has landed. Independent of the workflow
    /// state: a healthy task can simply not be its turn yet.
    #[serde(default)]
    pub dependency: DependencyState,
    /// Whether this is permitted to run. Independent of both the others:
    /// approval, security clearance and engineering progress move separately.
    #[serde(default)]
    pub authorization: AuthorizationState,
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
            kind: TaskKind::Task,
            lineage: Lineage::default(),
            dependency: DependencyState::Satisfied,
            authorization: AuthorizationState::Authorized,
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

    /// Whether this task can be worked right now, and if not, why.
    ///
    /// This replaces "is there an unclaimed record?" as the definition of
    /// ready. A record is only [`Readiness::ReadyNow`] once its workflow
    /// state, its dependencies and its authorization all agree — which is the
    /// difference between a board showing eleven ready tasks and a board
    /// showing the one that can actually start.
    ///
    /// The order the gates are resolved in is the order the scheduler must
    /// respect: terminal records leave the graph first, then dependencies,
    /// then policy and security, then the operator. Ranking before validating
    /// is what let superseded work be claimed.
    pub fn readiness(&self) -> Readiness {
        // 1. Terminal and superseded records leave the graph entirely. Checked
        //    first so nothing downstream can resurrect them.
        if self.state.is_terminal() || self.lineage.is_superseded() {
            return Readiness::Closed {
                state: if self.lineage.is_superseded() && !self.state.is_terminal() {
                    TaskState::Superseded.as_str()
                } else {
                    self.state.as_str()
                },
            };
        }

        if self.state == TaskState::Draft {
            return Readiness::NotYetOpen;
        }

        // 2. Dependencies. A task waiting its turn is not blocked in any sense
        //    that should reach a person.
        if let DependencyState::Waiting { on, reason, owner } = &self.dependency {
            return Readiness::WaitingDependency {
                on: *on,
                reason: reason.clone(),
                owner: *owner,
            };
        }

        // 3. Policy and security holds, before operator gates: a hold the room
        //    can clear itself must never be presented as a human decision.
        match &self.authorization {
            AuthorizationState::HeldByPolicy { policy, reason } => {
                return Readiness::HeldByPolicy {
                    policy: policy.clone(),
                    reason: reason.clone(),
                }
            }
            AuthorizationState::SecurityHold { finding, material } => {
                return if *material {
                    Readiness::BlockedOperator {
                        because: finding.clone(),
                        options: Vec::new(),
                    }
                } else {
                    // Missing proof is work for the room, not a question.
                    Readiness::BlockedInternal {
                        reason: finding.clone(),
                    }
                }
            }
            AuthorizationState::AwaitingOperator { because, options } => {
                return Readiness::BlockedOperator {
                    because: because.clone(),
                    options: options.clone(),
                }
            }
            AuthorizationState::Authorized => {}
        }

        // 4. Whatever the workflow state says last.
        if let Some(by) = self.assignee {
            if self.state != TaskState::Ready {
                return Readiness::Underway { by };
            }
        }
        match self.state {
            TaskState::Ready => Readiness::ReadyNow,
            TaskState::Blocked => Readiness::BlockedInternal {
                reason: self
                    .blocked_on
                    .first()
                    .map_or_else(|| "an unnamed blocker".to_string(), describe_block),
            },
            // Assigned states with no assignee recorded: still not claimable.
            other => Readiness::Closed {
                state: other.as_str(),
            },
        }
    }

    /// Whether a bot may claim this task. Derived, never stored.
    pub fn is_claimable_now(&self) -> bool {
        self.readiness().is_claimable()
    }

    /// Whether this task needs a person before it can move.
    pub fn needs_operator(&self) -> bool {
        self.readiness().needs_operator()
    }
}

/// Describe a blocker in words a person can read.
fn describe_block(reason: &BlockReason) -> String {
    match reason {
        BlockReason::AwaitingHandoff { .. } => "a handoff to be accepted".into(),
        BlockReason::AwaitingTask { .. } => "another task to finish".into(),
        BlockReason::AwaitingHuman { prompt } => prompt.clone(),
        BlockReason::Other { detail } => detail.clone(),
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

#[cfg(test)]
mod graph_tests {
    use super::*;
    use crate::graph::{AuthorizationState, DependencyState, Lineage, TaskKind};

    fn task() -> Task {
        let mut task = Task::draft(RoomId::new(), "Repair the harness", "do the thing");
        task.state = TaskState::Ready;
        task
    }

    /// The bug the whole model exists to fix: eleven future graph nodes all
    /// advertising themselves as ready because each was an unclaimed record.
    #[test]
    fn a_task_waiting_on_another_is_not_ready() {
        let upstream = TaskId::new();
        let mut waiting = task();
        waiting.dependency = DependencyState::Waiting {
            on: upstream,
            reason: "source recovery".into(),
            owner: None,
        };

        assert!(!waiting.is_claimable_now());
        assert!(!waiting.needs_operator());
        match waiting.readiness() {
            Readiness::WaitingDependency { on, reason, .. } => {
                assert_eq!(on, upstream);
                assert_eq!(reason, "source recovery");
            }
            other => panic!("expected a dependency wait, got {other:?}"),
        }
    }

    /// Five downstream tasks pointing at one recovery node must all name that
    /// same node, so the board can show one blocker instead of five.
    #[test]
    fn dependents_all_name_the_same_upstream_blocker() {
        let recovery = TaskId::new();
        let waits: Vec<Task> = (0..5)
            .map(|_| {
                let mut task = task();
                task.dependency = DependencyState::Waiting {
                    on: recovery,
                    reason: "B3-C source recovery".into(),
                    owner: None,
                };
                task
            })
            .collect();

        let blockers: std::collections::HashSet<TaskId> = waits
            .iter()
            .filter_map(|task| task.dependency.waiting_on())
            .collect();
        assert_eq!(blockers.len(), 1, "one missing source is one blocker");
        assert!(waits.iter().all(|task| !task.needs_operator()));
    }

    #[test]
    fn a_superseded_task_is_closed_and_unclaimable() {
        let replacement = TaskId::new();
        let mut original = task();
        original.lineage = Lineage {
            superseded_by: Some(replacement),
            ..Default::default()
        };

        assert!(!original.is_claimable_now());
        assert!(!original.needs_operator());
        assert!(matches!(original.readiness(), Readiness::Closed { .. }));
    }

    /// A settled task that comes back as live work is how an old incident kept
    /// reappearing on a real board.
    #[test]
    fn a_permanently_closed_task_cannot_be_reopened() {
        for state in [TaskState::Superseded, TaskState::Retired, TaskState::Archived] {
            assert!(state.is_terminal());
            assert!(state.is_permanent());
            assert!(
                state.apply(&TaskTransition::Reopen).is_err(),
                "{state:?} must not be reopenable"
            );
        }
        // Failing or cancelling is recoverable; being replaced is not.
        assert_eq!(
            TaskState::Failed.apply(&TaskTransition::Reopen).unwrap(),
            TaskState::Ready
        );
    }

    #[test]
    fn superseding_is_legal_from_live_states_only() {
        let supersede = TaskTransition::Supersede {
            by: TaskId::new(),
            reason: "merged into the canonical task".into(),
        };
        assert_eq!(
            TaskState::InProgress.apply(&supersede).unwrap(),
            TaskState::Superseded
        );
        assert!(TaskState::Submitted.apply(&supersede).is_err());
    }

    /// Missing evidence is work for the room. It used to reach the operator.
    #[test]
    fn a_non_material_security_hold_stays_inside_the_room() {
        let mut held = task();
        held.authorization = AuthorizationState::SecurityHold {
            finding: "need merged-filesystem classification".into(),
            material: false,
        };

        assert!(!held.needs_operator());
        assert!(held.readiness().is_progressing());
        assert!(matches!(
            held.readiness(),
            Readiness::BlockedInternal { .. }
        ));
    }

    #[test]
    fn a_material_incident_does_reach_the_operator() {
        let mut held = task();
        held.authorization = AuthorizationState::SecurityHold {
            finding: "a live credential was published".into(),
            material: true,
        };
        assert!(held.needs_operator());
    }

    #[test]
    fn an_operator_decision_carries_its_reason_and_options() {
        let mut waiting = task();
        waiting.authorization = AuthorizationState::AwaitingOperator {
            because: "this provisions a paid VPS".into(),
            options: vec!["Approve".into(), "Decline".into()],
        };

        assert!(waiting.needs_operator());
        match waiting.readiness() {
            Readiness::BlockedOperator { because, options } => {
                assert_eq!(because, "this provisions a paid VPS");
                assert_eq!(options.len(), 2);
            }
            other => panic!("expected an operator block, got {other:?}"),
        }
    }

    /// A dependency wait outranks an operator gate: there is no point asking
    /// someone to approve work that could not start anyway.
    #[test]
    fn a_dependency_wait_is_reported_before_an_operator_gate() {
        let mut both = task();
        both.dependency = DependencyState::Waiting {
            on: TaskId::new(),
            reason: "upstream build".into(),
            owner: None,
        };
        both.authorization = AuthorizationState::AwaitingOperator {
            because: "it costs money".into(),
            options: Vec::new(),
        };

        assert!(matches!(
            both.readiness(),
            Readiness::WaitingDependency { .. }
        ));
        assert!(!both.needs_operator(), "do not ask until it could actually run");
    }

    #[test]
    fn a_clear_task_is_ready_now() {
        let clear = task();
        assert!(clear.is_claimable_now());
        assert_eq!(clear.readiness(), Readiness::ReadyNow);
        assert!(!clear.needs_operator());
    }

    #[test]
    fn a_draft_is_not_offered_to_bots() {
        let draft = Task::draft(RoomId::new(), "later", "not yet");
        assert!(!draft.is_claimable_now());
        assert_eq!(draft.readiness(), Readiness::NotYetOpen);
    }

    /// A room file written before any of this existed must load unchanged.
    #[test]
    fn a_legacy_task_decodes_with_conservative_defaults() {
        let legacy = serde_json::json!({
            "id": TaskId::new(),
            "room": RoomId::new(),
            "title": "Old work",
            "brief": "from before the graph existed",
            "acceptance_criteria": [],
            "required_role": null,
            "state": "ready",
            "priority": "normal",
            "assignee": null,
            "blocked_on": [],
            "depends_on": [],
            "parent": null,
            "artifacts": [],
            "revision": 0,
            "max_revisions": 3,
            "created_at": Timestamp::now(),
            "updated_at": Timestamp::now(),
        });
        let task: Task = serde_json::from_value(legacy).unwrap();

        assert_eq!(task.kind, TaskKind::Task, "an unlabelled record is a task");
        assert_eq!(task.lineage, Lineage::default(), "links stay unknown");
        assert!(task.dependency.is_satisfied());
        assert!(task.authorization.is_authorized());
        // And it behaves exactly as it did before: an open record is ready.
        assert!(task.is_claimable_now());
    }
}
