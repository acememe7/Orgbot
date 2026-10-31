//! Commands: the only way to change a room.
//!
//! A command is an *intent* ("claim this task"); the engine validates it and,
//! if it holds, appends one or more events. Bots issue commands through their
//! tool calls, the UI issues them through the WebSocket, and tests issue them
//! directly — all three go down the same validation path.

use orgbot_proto::decision::Answer;
use orgbot_proto::graph::{AuthorizationState, TaskOrigin};
use orgbot_proto::ids::DecisionId;
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
        /// What kind of record this is, and what it descends from.
        origin: TaskOrigin,
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
        /// Whether the content carries authority that cannot be re-derived.
        /// Such an artifact may only be re-emitted by whoever produced it.
        #[serde(default)]
        authority_bearing: bool,
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

    /// Ask the operator a question without granting or changing authority.
    AskOwner {
        /// The bot asking.
        by: AgentId,
        /// A task the asker owns, when the question concerns active work.
        task: Option<TaskId>,
        /// The question shown to the operator.
        question: String,
        /// Suggested answers. An empty list leaves the answer free-form.
        options: Vec<String>,
    },

    /// Ask the owner to let a bot use the bot server key for a task it holds.
    /// The task waits on the answer like any question; once allowed, the key
    /// is mounted for that bot only while the task is under way and clear to
    /// proceed (see `Engine::server_access_for`).
    RequestServerAccess {
        /// The bot asking, which must hold the task.
        by: AgentId,
        /// The task that needs a server.
        task: TaskId,
        /// Why, shown to the owner.
        reason: String,
    },

    /// Ask to use some of what the owner lends (the bot server key and the
    /// sign-ins they list) for a task the bot holds. Depending on the room's
    /// access mode and how the request is rated, it is approved on the spot or
    /// asked like any question (see `Engine::request_access`).
    RequestAccess {
        /// The bot asking, which must hold the task.
        by: AgentId,
        /// The task the access is for.
        task: TaskId,
        /// What it asks for: `server` and the names of lent sign-ins.
        items: Vec<String>,
        /// What it says it will do, which can only raise the rating.
        action: orgbot_proto::decision::AccessAction,
        /// Why, shown to the owner.
        reason: String,
    },

    /// Ask the owner to land commits a bot made in its copy onto a branch of
    /// the real repository, as a fast-forward. The workspace has already
    /// checked the bundle; the owner sees the commits and answers once.
    RequestLanding {
        /// The bot asking.
        by: AgentId,
        /// The task it holds that the commits are for.
        task: TaskId,
        /// The branch to move.
        branch: String,
        /// Where the branch stands now.
        base: String,
        /// The commit it would move to.
        head: String,
        /// One line per commit, newest first.
        commits: Vec<String>,
        /// What the commits change, in git's short words.
        changes: String,
        /// An artifact on the task recording the checks run on these commits.
        #[serde(default)]
        checks: Option<ArtifactId>,
        /// Why, shown to the owner.
        reason: String,
    },

    /// Ask the owner to push a branch of their repository to one of its
    /// remotes, as a fast-forward. Always asked, whatever the access mode:
    /// it leaves the owner's computer.
    RequestPush {
        /// The bot asking.
        by: AgentId,
        /// The task it holds that the push is for.
        task: TaskId,
        /// The remote, such as origin.
        remote: String,
        /// Where that remote points, shown to the owner.
        url: String,
        /// The branch to push.
        branch: String,
        /// Where the remote's branch stands now.
        base: String,
        /// The commit it would move to.
        head: String,
        /// One line per commit, newest first.
        commits: Vec<String>,
        /// Why, shown to the owner.
        reason: String,
    },

    /// Propose to the owner that a task nobody should work on be retired. The
    /// owner's "Retire this task" answer retires it.
    ProposeRetire {
        /// The coordinating bot proposing it.
        by: AgentId,
        /// The task to retire.
        task: TaskId,
        /// Why it is obsolete, shown to the owner.
        reason: String,
    },

    /// Change what a task asks for, on a coordinator's decision: its brief,
    /// its acceptance criteria, or both, with the reason recorded.
    AmendTask {
        /// The coordinating bot amending it.
        by: AgentId,
        /// The task to amend.
        task: TaskId,
        /// A new brief, if it changes.
        brief: Option<String>,
        /// The full new acceptance criteria, if they change.
        acceptance_criteria: Option<Vec<String>>,
        /// Every task it must wait for, if that changes.
        #[serde(default)]
        depends_on: Option<Vec<TaskId>>,
        /// Why, recorded on the task and shown to the room.
        reason: String,
    },

    /// Put a task aside until the owner resumes it, on the owner's own word or
    /// a coordinator's acting on it. Nobody holds, works on, claims or asks
    /// about a parked task, and questions waiting on it are withdrawn.
    ParkTask {
        /// The coordinating bot parking it on the owner's instruction, or
        /// `None` for the owner.
        by: Option<AgentId>,
        /// The task to put aside.
        task: TaskId,
        /// Why, shown to the room.
        reason: String,
    },

    /// Record how many file removals in one of `agent`'s copies wait for the
    /// owner's review, as the workspace last reported them.
    RecordPendingRemovals {
        /// The bot.
        agent: AgentId,
        /// Which copy: "room tools" or "its own CLI".
        copy: String,
        /// Removals waiting.
        count: u32,
    },

    /// The owner resumes a parked task, which returns to the pool. No bot
    /// tool sends this: only the owner resumes what they parked.
    ResumeTask {
        /// The parked task.
        task: TaskId,
    },

    /// Record the operator's answer to a question, releasing whatever it
    /// unblocks. One answer clears every task waiting on that same question.
    AnswerDecision {
        /// The question being answered.
        decision: DecisionId,
        /// What they said.
        answer: Answer,
    },

    // ---- engine housekeeping ----
    /// A bot is in a turn right now, or has something to do and is waiting
    /// only for a free slot: renew its claims, and hold open the handoffs
    /// waiting on it that came to it since its latest turn began — offers it
    /// has not yet been shown, and handoffs it accepted in that turn. A turn
    /// can run far longer than a claim's window (an agent running tests, a
    /// slow command) without touching the board, and its work must not be
    /// taken away while it is still doing it. Nor should an offer made to it
    /// meanwhile run out before it has had a turn in which to read it, or a
    /// handoff it took on run out while it is doing the work.
    KeepAlive { agent: AgentId },
    /// A bot's turn is starting and it is being shown the board, including
    /// every offer it could accept. From here the timeouts of those offers,
    /// and of handoffs it accepted in earlier turns, run, and
    /// [`Command::KeepAlive`] no longer holds them open.
    BeginTurn { agent: AgentId },
    /// Charge one model turn against the room budget.
    RecordTurn {
        agent: AgentId,
        /// What the turn cost. Default when a backend does not report it.
        #[serde(default)]
        usage: orgbot_proto::spend::TokenUsage,
    },
    /// Report a failed bot turn.
    ReportError {
        agent: AgentId,
        task: Option<TaskId>,
        error: String,
        /// Whether the same request could never succeed — a model that does
        /// not exist, a credential the backend refuses, a configuration this
        /// build cannot honour.
        ///
        /// Retrying one of those is not resilience, it is three wasted turns
        /// followed by the same answer, and in between the bot looks available
        /// while being incapable of doing anything.
        #[serde(default)]
        permanent: bool,
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
            Self::AskOwner { .. } => "ask_owner",
            Self::RequestServerAccess { .. } => "request_server_access",
            Self::RequestAccess { .. } => "request_access",
            Self::AmendTask { .. } => "amend_task",
            Self::ProposeRetire { .. } => "propose_retire",
            Self::RequestLanding { .. } => "request_landing",
            Self::RequestPush { .. } => "request_push",
            Self::RecordPendingRemovals { .. } => "record_pending_removals",
            Self::ParkTask { .. } => "park_task",
            Self::ResumeTask { .. } => "resume_task",
            Self::AnswerDecision { .. } => "answer_decision",
            Self::KeepAlive { .. } => "keep_alive",
            Self::BeginTurn { .. } => "begin_turn",
            Self::RecordTurn { .. } => "record_turn",
            Self::ReportError { .. } => "report_error",
            Self::Tick { .. } => "tick",
        }
    }

    /// The task this command acts on, when it acts on one.
    ///
    /// Used as a heartbeat: a bot doing anything to its task is a bot that has
    /// not stopped, and its claim should not lapse underneath it.
    pub const fn task(&self) -> Option<TaskId> {
        match self {
            Self::ClaimTask { task, .. }
            | Self::StartTask { task, .. }
            | Self::BlockTask { task, .. }
            | Self::UnblockTask { task }
            | Self::SubmitForReview { task, .. }
            | Self::AddArtifact { task, .. }
            | Self::RecordReview { task, .. } => Some(*task),
            Self::AskOwner { task: Some(task), .. } => Some(*task),
            Self::RequestServerAccess { task, .. } => Some(*task),
            Self::RequestAccess { task, .. } => Some(*task),
            Self::RequestLanding { task, .. } => Some(*task),
            Self::RequestPush { task, .. } => Some(*task),
            Self::AmendTask { task, .. } => Some(*task),
            _ => None,
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
                // The owner may put work aside or take it back while paused.
                | Self::ParkTask { by: None, .. }
                | Self::ResumeTask { .. }
                | Self::RecordPendingRemovals { .. }
                | Self::Tick { .. }
                | Self::KeepAlive { .. }
                | Self::BeginTurn { .. }
                // Speaking is always allowed. A paused or finished room still
                // has a transcript, and a message that is silently refused
                // leaves the person staring at a chat that swallowed it.
            | Self::PostMessage { .. }
        )
    }
}
