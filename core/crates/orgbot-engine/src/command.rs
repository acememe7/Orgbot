//! Commands: the only way to change a room.
//!
//! A command is an *intent* ("claim this task"); the engine validates it and,
//! if it holds, appends one or more events. Bots issue commands through their
//! tool calls, the UI issues them through the WebSocket, and tests issue them
//! directly — all three go down the same validation path.

use orgbot_proto::graph::AuthorizationState;
use orgbot_proto::artifact::ArtifactBody;
use orgbot_proto::handoff::{HandoffKind, Recipient};
use orgbot_proto::review::Finding;
use orgbot_proto::task::{BlockReason, Priority};
use orgbot_proto::{AgentId, AgentSpec, ArtifactId, ChatMessage, HandoffId, TaskId, Timestamp};
use serde::{Deserialize, Serialize};

/// An intent to change room state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    // ---- room lifecycle ----
    /// Seat a bot.
    JoinAgent { spec: Box<AgentSpec> },
    /// Remove a bot, releasing anything it holds.
    LeaveAgent { agent: AgentId, reason: String },
    /// Move the room from assembling to running.
    StartRoom,
    /// Suspend turn-taking without discarding state.
    PauseRoom { reason: String },
    /// Resume a paused room.
    ResumeRoom,
    /// End the room.
    CloseRoom { reason: String },

    // ---- conversation ----
    /// Post to the transcript.
    PostMessage { message: Box<ChatMessage> },

    // ---- task board ----
    /// Add a task as a draft.
    CreateTask {
        title: String,
        brief: String,
        /// The bot creating it, or `None` for a human/seed task.
        by: Option<AgentId>,
        required_role: Option<String>,
        priority: Priority,
        acceptance_criteria: Vec<String>,
        depends_on: Vec<TaskId>,
        parent: Option<TaskId>,
    },
    /// Publish a draft so bots can claim it.
    OpenTask { task: TaskId },
    /// Take ownership of an open task.
    ClaimTask { task: TaskId, by: AgentId },
    /// Begin work on a claimed task.
    StartTask { task: TaskId, by: AgentId },
    /// Suspend work.
    BlockTask {
        task: TaskId,
        by: AgentId,
        reason: BlockReason,
    },
    /// Resume after a block clears.
    UnblockTask { task: TaskId },
    /// Return a held task to the pool.
    ReleaseTask {
        task: TaskId,
        by: AgentId,
        reason: String,
    },
    /// Declare work finished and request inspection.
    SubmitForReview {
        task: TaskId,
        by: AgentId,
        artifacts: Vec<ArtifactId>,
    },
    /// Deliver approved work.
    SubmitTask { task: TaskId, by: AgentId },
    /// Abandon a task.
    FailTask { task: TaskId, reason: String },
    /// Withdraw a task.
    CancelTask { task: TaskId, reason: String },

    // ---- handoffs ----
    /// Offer work to another bot.
    OfferHandoff {
        task: TaskId,
        from: AgentId,
        to: Recipient,
        kind: HandoffKind,
        brief: String,
        expects: Vec<String>,
        context: Vec<ArtifactId>,
        /// Override the kind's default blocking behaviour.
        blocks_sender: Option<bool>,
        /// Milliseconds until the offer expires. `None` uses the room default.
        timeout_ms: Option<i64>,
    },
    /// Take up an offered handoff.
    AcceptHandoff { handoff: HandoffId, by: AgentId },
    /// Decline an offered handoff.
    RejectHandoff {
        handoff: HandoffId,
        by: AgentId,
        reason: String,
    },
    /// Deliver on an accepted handoff.
    CompleteHandoff {
        handoff: HandoffId,
        by: AgentId,
        artifacts: Vec<ArtifactId>,
        summary: String,
    },
    /// Report that an accepted handoff cannot be finished.
    FailHandoff {
        handoff: HandoffId,
        by: AgentId,
        reason: String,
    },
    /// Pull back an offer.
    WithdrawHandoff {
        handoff: HandoffId,
        by: AgentId,
        reason: String,
    },

    // ---- deliverables and inspection ----
    /// Record a deliverable.
    AddArtifact {
        task: TaskId,
        author: AgentId,
        name: String,
        body: ArtifactBody,
    },
    /// Record an inspection. The verdict is derived from the findings.
    RecordReview {
        task: TaskId,
        inspector: AgentId,
        findings: Vec<Finding>,
        summary: String,
        criteria_met: Vec<String>,
        criteria_failed: Vec<String>,
    },

    /// Replace `task` with `replacement`, which takes over its work.
    ///
    /// The original becomes permanently non-actionable: unclaimable, absent
    /// from ready scans, and excluded from anything counted for the operator.
    /// Its history is kept.
    SupersedeTask {
        /// The task being replaced.
        task: TaskId,
        /// The task that now carries the work.
        replacement: TaskId,
        /// Why it was replaced.
        reason: String,
    },
    /// Close `task` as no longer relevant, permanently.
    RetireTask {
        /// The task to retire.
        task: TaskId,
        /// Why it is no longer relevant.
        reason: String,
    },
    /// Put an authorization hold on a task, or lift one.
    ///
    /// Holds are how policy, security and operator gates reach the scheduler.
    /// Setting [`AuthorizationState::Authorized`] lifts whatever was there.
    SetAuthorization {
        /// The task to hold or release.
        task: TaskId,
        /// The state to put it in.
        authorization: AuthorizationState,
    },

    // ---- engine housekeeping ----
    /// Charge one model turn against the room budget.
    RecordTurn { agent: AgentId },
    /// Report a failed bot turn.
    ReportError {
        agent: AgentId,
        task: Option<TaskId>,
        error: String,
    },
    /// Advance the clock: expire stale handoffs and sweep for deadlocks.
    ///
    /// `now` is a parameter rather than read from the system clock so that
    /// tests can drive expiry deterministically.
    Tick { now: Timestamp },
}

impl Command {
    /// A stable name for logs and metrics.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::JoinAgent { .. } => "join_agent",
            Self::LeaveAgent { .. } => "leave_agent",
            Self::StartRoom => "start_room",
            Self::PauseRoom { .. } => "pause_room",
            Self::ResumeRoom => "resume_room",
            Self::CloseRoom { .. } => "close_room",
            Self::PostMessage { .. } => "post_message",
            Self::CreateTask { .. } => "create_task",
            Self::OpenTask { .. } => "open_task",
            Self::ClaimTask { .. } => "claim_task",
            Self::StartTask { .. } => "start_task",
            Self::BlockTask { .. } => "block_task",
            Self::UnblockTask { .. } => "unblock_task",
            Self::ReleaseTask { .. } => "release_task",
            Self::SubmitForReview { .. } => "submit_for_review",
            Self::SubmitTask { .. } => "submit_task",
            Self::FailTask { .. } => "fail_task",
            Self::CancelTask { .. } => "cancel_task",
            Self::OfferHandoff { .. } => "offer_handoff",
            Self::AcceptHandoff { .. } => "accept_handoff",
            Self::RejectHandoff { .. } => "reject_handoff",
            Self::CompleteHandoff { .. } => "complete_handoff",
            Self::FailHandoff { .. } => "fail_handoff",
            Self::WithdrawHandoff { .. } => "withdraw_handoff",
            Self::AddArtifact { .. } => "add_artifact",
            Self::RecordReview { .. } => "record_review",
            Self::SupersedeTask { .. } => "supersede_task",
            Self::RetireTask { .. } => "retire_task",
            Self::SetAuthorization { .. } => "set_authorization",
            Self::RecordTurn { .. } => "record_turn",
            Self::ReportError { .. } => "report_error",
            Self::Tick { .. } => "tick",
        }
    }

    /// Whether this command is rejected while the room is not running.
    ///
    /// Lifecycle and housekeeping commands are exempt — otherwise a paused
    /// room could never be resumed and a closing room could never drain.
    pub const fn requires_running_room(&self) -> bool {
        !matches!(
            self,
            Self::JoinAgent { .. }
                | Self::LeaveAgent { .. }
                | Self::StartRoom
                | Self::PauseRoom { .. }
                | Self::ResumeRoom
                | Self::CloseRoom { .. }
                | Self::CreateTask { .. }
                | Self::Tick { .. }
                // Speaking is always allowed. A paused or finished room still
                // has a transcript, and a message that is silently refused
                // leaves the person staring at a chat that swallowed it.
                | Self::PostMessage { .. }
        )
    }
}
