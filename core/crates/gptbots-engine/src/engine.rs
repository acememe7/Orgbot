//! The room engine.
//!
//! Holds every projection of a room — roster, board, handoffs, transcript —
//! and is the only thing allowed to mutate them. [`Engine::apply`] takes a
//! [`Command`], validates it against the room's invariants, and appends the
//! resulting events.
//!
//! The engine is deliberately synchronous and free of I/O. Model calls, tool
//! execution, and the network all live in layers above it, so the rules that
//! decide whether a bot may submit work can be tested without a provider key.

use std::collections::{BTreeMap, BTreeSet};

use gptbots_proto::agent::AgentStatus;
use gptbots_proto::artifact::{Artifact, ArtifactBody};
use gptbots_proto::event::{DeadlockResolution, Event, EventEnvelope, Seq};
use gptbots_proto::handoff::{Handoff, HandoffKind, HandoffState, HandoffTransition, Recipient};
use gptbots_proto::review::{Finding, Review, Verdict};
use gptbots_proto::room::{Room, RoomPhase, RoomSpec};
use gptbots_proto::task::{BlockReason, Priority, Task, TaskState, TaskTransition};
use gptbots_proto::{
    AgentId, AgentSpec, ArtifactId, ChatMessage, HandoffId, RoomId, TaskId, Timestamp,
};

use crate::command::Command;
use crate::deadlock::{self, WaitGraph};
use crate::error::{EngineError, Result};

/// A live group chat and everything in it.
#[derive(Debug)]
pub struct Engine {
    room: Room,
    specs: BTreeMap<AgentId, AgentSpec>,
    statuses: BTreeMap<AgentId, AgentStatus>,
    handles: BTreeMap<String, AgentId>,
    tasks: BTreeMap<TaskId, Task>,
    handoffs: BTreeMap<HandoffId, Handoff>,
    artifacts: BTreeMap<ArtifactId, Artifact>,
    reviews: Vec<Review>,
    transcript: Vec<ChatMessage>,
    log: Vec<EventEnvelope>,
    seq: Seq,
    /// Consecutive failed turns per bot, reset by any success.
    error_streak: BTreeMap<AgentId, u32>,
}

impl Engine {
    /// Open a new room.
    pub fn new(spec: RoomSpec) -> Self {
        let room = Room::new(spec);
        let mut engine = Self {
            room,
            specs: BTreeMap::new(),
            statuses: BTreeMap::new(),
            handles: BTreeMap::new(),
            tasks: BTreeMap::new(),
            handoffs: BTreeMap::new(),
            artifacts: BTreeMap::new(),
            reviews: Vec::new(),
            transcript: Vec::new(),
            log: Vec::new(),
            seq: Seq::ZERO,
            error_streak: BTreeMap::new(),
        };
        let (id, name, goal) = (
            engine.room.id,
            engine.room.spec.name.clone(),
            engine.room.spec.goal.clone(),
        );
        engine.emit(Event::RoomOpened {
            room: id,
            name,
            goal,
        });
        engine
    }

    // ---------------------------------------------------------------- readers

    /// The room's identifier.
    pub fn room_id(&self) -> RoomId {
        self.room.id
    }

    /// The room record.
    pub fn room(&self) -> &Room {
        &self.room
    }

    /// A bot's configuration.
    pub fn agent(&self, id: AgentId) -> Option<&AgentSpec> {
        self.specs.get(&id)
    }

    /// Look a bot up by its chat handle.
    pub fn agent_by_handle(&self, handle: &str) -> Option<&AgentSpec> {
        self.handles.get(handle).and_then(|id| self.specs.get(id))
    }

    /// Every seated bot, in join order by id.
    pub fn agents(&self) -> impl Iterator<Item = &AgentSpec> {
        self.specs.values()
    }

    /// A bot's availability.
    pub fn status(&self, id: AgentId) -> AgentStatus {
        self.statuses.get(&id).copied().unwrap_or(AgentStatus::Offline)
    }

    /// A task on the board.
    pub fn task(&self, id: TaskId) -> Option<&Task> {
        self.tasks.get(&id)
    }

    /// Every task on the board.
    pub fn tasks(&self) -> impl Iterator<Item = &Task> {
        self.tasks.values()
    }

    /// A handoff.
    pub fn handoff(&self, id: HandoffId) -> Option<&Handoff> {
        self.handoffs.get(&id)
    }

    /// Every handoff.
    pub fn handoffs(&self) -> impl Iterator<Item = &Handoff> {
        self.handoffs.values()
    }

    /// A recorded deliverable.
    pub fn artifact(&self, id: ArtifactId) -> Option<&Artifact> {
        self.artifacts.get(&id)
    }

    /// Every inspection, oldest first.
    pub fn reviews(&self) -> &[Review] {
        &self.reviews
    }

    /// The room transcript, oldest first.
    pub fn transcript(&self) -> &[ChatMessage] {
        &self.transcript
    }

    /// The whole event log.
    pub fn log(&self) -> &[EventEnvelope] {
        &self.log
    }

    /// Events after `seq`, for a client catching up mid-run.
    pub fn events_since(&self, seq: Seq) -> impl Iterator<Item = &EventEnvelope> {
        self.log.iter().filter(move |e| e.seq > seq)
    }

    /// The most recently assigned sequence number.
    pub fn head(&self) -> Seq {
        self.seq
    }

    /// Tasks `agent` currently holds and has not finished with.
    pub fn active_tasks_of(&self, agent: AgentId) -> Vec<TaskId> {
        self.tasks
            .values()
            .filter(|t| t.assignee == Some(agent) && t.state.is_assigned())
            .map(|t| t.id)
            .collect()
    }

    /// Open tasks `agent` is eligible to claim, best first.
    ///
    /// Ordering is by priority then age, so a room drains urgent work before
    /// old work and old work before new.
    pub fn claimable_by(&self, agent: AgentId) -> Vec<TaskId> {
        let Some(spec) = self.specs.get(&agent) else {
            return Vec::new();
        };
        let mut open: Vec<&Task> = self
            .tasks
            .values()
            .filter(|t| t.state.is_claimable())
            .filter(|t| spec.can_take(t.required_role.as_deref()))
            .filter(|t| self.dependencies_met(t).is_ok())
            .collect();
        open.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then(a.created_at.cmp(&b.created_at))
                .then(a.id.cmp(&b.id))
        });
        open.into_iter().map(|t| t.id).collect()
    }

    /// Handoffs `agent` could accept right now.
    pub fn offers_for(&self, agent: AgentId) -> Vec<HandoffId> {
        let Some(spec) = self.specs.get(&agent) else {
            return Vec::new();
        };
        self.handoffs
            .values()
            .filter(|h| h.state == HandoffState::Offered)
            .filter(|h| h.from != agent)
            .filter(|h| h.to.admits(agent, &spec.roles))
            .map(|h| h.id)
            .collect()
    }

    /// How many handoffs `agent` has offered that are still unresolved.
    pub fn open_handoffs_from(&self, agent: AgentId) -> u32 {
        self.handoffs
            .values()
            .filter(|h| h.from == agent && h.state.is_pending())
            .count() as u32
    }

    /// Tasks awaiting inspection that `inspector` is allowed to review.
    ///
    /// Excludes anything the inspector wrote itself, so the no-self-review rule
    /// is visible in the queue rather than only enforced on submit.
    pub fn review_queue_for(&self, inspector: AgentId) -> Vec<TaskId> {
        let Some(spec) = self.specs.get(&inspector) else {
            return Vec::new();
        };
        if !spec.autonomy.may_inspect {
            return Vec::new();
        }
        self.tasks
            .values()
            .filter(|t| t.state == TaskState::InReview)
            .filter(|t| t.assignee != Some(inspector))
            .map(|t| t.id)
            .collect()
    }

    /// Tasks that have passed inspection and are waiting to be delivered.
    ///
    /// A coordinator holds no tasks of its own, so without this it has no way
    /// to discover that there is work to deliver and would never act.
    pub fn awaiting_delivery(&self) -> Vec<TaskId> {
        self.tasks
            .values()
            .filter(|task| task.state == TaskState::Approved)
            .map(|task| task.id)
            .collect()
    }

    /// Whether every dependency of `task` has been submitted.
    fn dependencies_met(&self, task: &Task) -> Result<()> {
        for dep in &task.depends_on {
            let met = self
                .tasks
                .get(dep)
                .is_some_and(|d| d.state == TaskState::Submitted);
            if !met {
                return Err(EngineError::DependencyNotMet {
                    task: task.id,
                    blocker: *dep,
                });
            }
        }
        Ok(())
    }

    /// The current wait-for graph between tasks.
    ///
    /// An edge is only drawn when the party that must act is *determined*. A
    /// role-addressed offer that nobody has accepted yet draws no edge: any
    /// holder of that role might still pick it up, so calling it a deadlock
    /// would release work that was about to proceed.
    pub fn wait_graph(&self) -> WaitGraph {
        let mut graph = WaitGraph::new();
        for task in self.tasks.values() {
            if task.state != TaskState::Blocked {
                continue;
            }
            for reason in &task.blocked_on {
                match reason {
                    BlockReason::AwaitingTask { task: dep } => graph.add_wait(task.id, *dep),
                    BlockReason::AwaitingHandoff { handoff } => {
                        let Some(handoff) = self.handoffs.get(handoff) else {
                            continue;
                        };
                        if !handoff.state.is_pending() {
                            continue;
                        }
                        let responsible = match (handoff.state, &handoff.to) {
                            (HandoffState::Accepted, _) => handoff.accepted_by,
                            (HandoffState::Offered, Recipient::Agent { agent }) => Some(*agent),
                            _ => None,
                        };
                        if let Some(agent) = responsible {
                            for blocker in self.active_tasks_of(agent) {
                                graph.add_wait(task.id, blocker);
                            }
                        }
                    }
                    // A human or an unmodelled dependency is outside the graph:
                    // the engine cannot reason about when it will clear.
                    BlockReason::AwaitingHuman { .. } | BlockReason::Other { .. } => {}
                }
            }
        }
        graph
    }

    // --------------------------------------------------------------- mutation

    /// Validate and apply a command, returning the events it produced.
    ///
    /// On error nothing is appended and no state changes.
    pub fn apply(&mut self, command: Command) -> Result<Vec<EventEnvelope>> {
        if command.requires_running_room() && !self.room.phase.is_live() {
            return Err(EngineError::RoomNotRunning {
                phase: self.room.phase,
            });
        }
        match command {
            Command::JoinAgent { spec } => self.join_agent(*spec),
            Command::LeaveAgent { agent, reason } => self.leave_agent(agent, reason),
            Command::StartRoom => self.set_phase(RoomPhase::Running, "started"),
            Command::PauseRoom { reason } => self.set_phase(RoomPhase::Paused, &reason),
            Command::ResumeRoom => self.set_phase(RoomPhase::Running, "resumed"),
            Command::CloseRoom { reason } => self.close_room(reason),

            Command::PostMessage { message } => self.post_message(*message),

            Command::CreateTask {
                title,
                brief,
                by,
                required_role,
                priority,
                acceptance_criteria,
                depends_on,
                parent,
            } => self.create_task(
                title,
                brief,
                by,
                required_role,
                priority,
                acceptance_criteria,
                depends_on,
                parent,
            ),
            Command::OpenTask { task } => self.transition(task, TaskTransition::Open, None),
            Command::ClaimTask { task, by } => self.claim_task(task, by),
            Command::StartTask { task, by } => self.start_task(task, by),
            Command::BlockTask { task, by, reason } => self.block_task(task, by, reason),
            Command::UnblockTask { task } => self.unblock_task(task),
            Command::ReleaseTask { task, by, reason } => {
                self.require_assignee(task, by)?;
                self.transition(task, TaskTransition::Release { reason }, Some(by))
            }
            Command::SubmitForReview {
                task,
                by,
                artifacts,
            } => self.submit_for_review(task, by, artifacts),
            Command::SubmitTask { task, by } => self.submit_task(task, by),
            Command::FailTask { task, reason } => {
                self.transition(task, TaskTransition::Fail { reason }, None)
            }
            Command::CancelTask { task, reason } => {
                self.transition(task, TaskTransition::Cancel { reason }, None)
            }

            Command::OfferHandoff {
                task,
                from,
                to,
                kind,
                brief,
                expects,
                context,
                blocks_sender,
                timeout_ms,
            } => self.offer_handoff(
                task,
                from,
                to,
                kind,
                brief,
                expects,
                context,
                blocks_sender,
                timeout_ms,
            ),
            Command::AcceptHandoff { handoff, by } => self.accept_handoff(handoff, by),
            Command::RejectHandoff {
                handoff,
                by,
                reason,
            } => self.drive_handoff(handoff, HandoffTransition::Reject { by, reason }),
            Command::CompleteHandoff {
                handoff,
                by,
                artifacts,
                summary,
            } => self.complete_handoff(handoff, by, artifacts, summary),
            Command::FailHandoff {
                handoff,
                by,
                reason,
            } => self.drive_handoff(handoff, HandoffTransition::Fail { by, reason }),
            Command::WithdrawHandoff {
                handoff,
                by,
                reason,
            } => self.drive_handoff(handoff, HandoffTransition::Withdraw { by, reason }),

            Command::AddArtifact {
                task,
                author,
                name,
                body,
            } => self.add_artifact(task, author, name, body),
            Command::RecordReview {
                task,
                inspector,
                findings,
                summary,
                criteria_met,
                criteria_failed,
            } => self.record_review(
                task,
                inspector,
                findings,
                summary,
                criteria_met,
                criteria_failed,
            ),

            Command::RecordTurn { agent } => self.record_turn(agent),
            Command::ReportError {
                agent,
                task,
                error,
            } => self.report_error(agent, task, error),
            Command::Tick { now } => self.tick(now),
        }
    }

    // ------------------------------------------------------- roster & lifecycle

    fn join_agent(&mut self, spec: AgentSpec) -> Result<Vec<EventEnvelope>> {
        if let Some(existing) = self.handles.get(&spec.handle) {
            if *existing != spec.id {
                return Err(EngineError::DuplicateHandle {
                    handle: spec.handle,
                });
            }
        }
        self.handles.insert(spec.handle.clone(), spec.id);
        self.statuses.insert(spec.id, AgentStatus::Idle);
        self.specs.insert(spec.id, spec.clone());
        Ok(vec![self.emit(Event::AgentJoined {
            spec: Box::new(spec),
        })])
    }

    fn leave_agent(&mut self, agent: AgentId, reason: String) -> Result<Vec<EventEnvelope>> {
        self.require_agent(agent)?;
        let mut events = Vec::new();

        // Releasing first means the bot's work returns to the pool instead of
        // being stranded on a bot that is no longer there.
        for task in self.active_tasks_of(agent) {
            if let Ok(released) = self.transition(
                task,
                TaskTransition::Release {
                    reason: format!("{} left the room", agent.short()),
                },
                Some(agent),
            ) {
                events.extend(released);
            }
        }
        // Withdraw anything it was still waiting on, so nobody waits on a ghost.
        let pending: Vec<HandoffId> = self
            .handoffs
            .values()
            .filter(|h| h.from == agent && h.state.is_pending())
            .map(|h| h.id)
            .collect();
        for handoff in pending {
            if let Ok(withdrawn) = self.drive_handoff(
                handoff,
                HandoffTransition::Withdraw {
                    by: agent,
                    reason: "sender left the room".into(),
                },
            ) {
                events.extend(withdrawn);
            }
        }

        if let Some(spec) = self.specs.remove(&agent) {
            self.handles.remove(&spec.handle);
        }
        self.statuses.insert(agent, AgentStatus::Offline);
        events.push(self.emit(Event::AgentLeft { agent, reason }));
        Ok(events)
    }

    fn set_phase(&mut self, phase: RoomPhase, note: &str) -> Result<Vec<EngineEvent>> {
        self.room.phase = phase;
        self.room.updated_at = Timestamp::now();
        let room = self.room.id;
        let message = ChatMessage::system(room, format!("room {}: {note}", phase_label(phase)));
        Ok(vec![self.emit(Event::MessagePosted {
            message: Box::new(message),
        })])
    }

    fn close_room(&mut self, reason: String) -> Result<Vec<EventEnvelope>> {
        self.room.phase = RoomPhase::Completed;
        self.room.updated_at = Timestamp::now();
        let room = self.room.id;
        Ok(vec![self.emit(Event::RoomClosed { room, reason })])
    }

    // ------------------------------------------------------------ conversation

    fn post_message(&mut self, message: ChatMessage) -> Result<Vec<EventEnvelope>> {
        if let Some(agent) = message.author.agent() {
            self.require_agent(agent)?;
        }
        self.transcript.push(message.clone());
        Ok(vec![self.emit(Event::MessagePosted {
            message: Box::new(message),
        })])
    }

    // ------------------------------------------------------------- task board

    #[allow(clippy::too_many_arguments)]
    fn create_task(
        &mut self,
        title: String,
        brief: String,
        by: Option<AgentId>,
        required_role: Option<String>,
        priority: Priority,
        acceptance_criteria: Vec<String>,
        depends_on: Vec<TaskId>,
        parent: Option<TaskId>,
    ) -> Result<Vec<EventEnvelope>> {
        if let Some(creator) = by {
            let spec = self.require_agent(creator)?;
            if !spec.autonomy.may_create_tasks {
                return Err(EngineError::NotPermitted {
                    agent: creator,
                    action: "create tasks".into(),
                });
            }
        }
        for dep in &depends_on {
            if !self.tasks.contains_key(dep) {
                return Err(EngineError::UnknownTask { task: *dep });
            }
        }
        let mut task = Task::draft(self.room.id, title, brief);
        task.required_role = required_role;
        task.priority = priority;
        task.acceptance_criteria = acceptance_criteria;
        task.depends_on = depends_on;
        task.parent = parent;
        self.tasks.insert(task.id, task.clone());
        Ok(vec![self.emit(Event::TaskCreated {
            task: Box::new(task),
        })])
    }

    fn claim_task(&mut self, task: TaskId, by: AgentId) -> Result<Vec<EventEnvelope>> {
        let spec = self.require_agent(by)?.clone();
        let record = self.require_task(task)?;
        if !spec.can_take(record.required_role.as_deref()) {
            return Err(EngineError::RoleMismatch {
                task,
                agent: by,
                required: record.required_role.clone().unwrap_or_default(),
            });
        }
        self.dependencies_met(record)?;
        self.transition(task, TaskTransition::Claim { by }, Some(by))
    }

    fn start_task(&mut self, task: TaskId, by: AgentId) -> Result<Vec<EventEnvelope>> {
        self.require_assignee(task, by)?;
        let record = self.require_task(task)?;
        self.dependencies_met(record)?;
        self.transition(task, TaskTransition::Start, Some(by))
    }

    fn block_task(
        &mut self,
        task: TaskId,
        by: AgentId,
        reason: BlockReason,
    ) -> Result<Vec<EventEnvelope>> {
        self.require_assignee(task, by)?;
        let mut events = self.transition(task, TaskTransition::Block { reason }, Some(by))?;
        // A new wait edge is the only thing that can create a deadlock, so this
        // is where it is cheapest to look for one.
        events.extend(self.sweep_deadlocks());
        Ok(events)
    }

    fn unblock_task(&mut self, task: TaskId) -> Result<Vec<EventEnvelope>> {
        self.transition(task, TaskTransition::Unblock, None)
    }

    fn submit_for_review(
        &mut self,
        task: TaskId,
        by: AgentId,
        artifacts: Vec<ArtifactId>,
    ) -> Result<Vec<EventEnvelope>> {
        self.require_assignee(task, by)?;
        for artifact in &artifacts {
            if !self.artifacts.contains_key(artifact) {
                return Err(EngineError::UnknownArtifact {
                    artifact: *artifact,
                });
            }
        }
        self.transition(task, TaskTransition::SubmitForReview { artifacts }, Some(by))
    }

    fn submit_task(&mut self, task: TaskId, by: AgentId) -> Result<Vec<EventEnvelope>> {
        let spec = self.require_agent(by)?;
        if !spec.autonomy.may_submit {
            return Err(EngineError::NotPermitted {
                agent: by,
                action: "submit tasks".into(),
            });
        }
        self.transition(task, TaskTransition::Submit { by }, Some(by))
    }

    // --------------------------------------------------------------- handoffs

    #[allow(clippy::too_many_arguments)]
    fn offer_handoff(
        &mut self,
        task: TaskId,
        from: AgentId,
        to: Recipient,
        kind: HandoffKind,
        brief: String,
        expects: Vec<String>,
        context: Vec<ArtifactId>,
        blocks_sender: Option<bool>,
        timeout_ms: Option<i64>,
    ) -> Result<Vec<EventEnvelope>> {
        let spec = self.require_agent(from)?.clone();
        self.require_task(task)?;

        if let Recipient::Agent { agent } = &to {
            if *agent == from {
                return Err(EngineError::SelfHandoff);
            }
            self.require_agent(*agent)?;
        }

        let open = self.open_handoffs_from(from);
        if open >= spec.autonomy.max_open_handoffs {
            return Err(EngineError::TooManyOpenHandoffs {
                agent: from,
                open,
                limit: spec.autonomy.max_open_handoffs,
            });
        }

        let mut handoff = Handoff::offer(task, from, to, kind, brief);
        handoff.expects = expects;
        handoff.context = context;
        if let Some(blocks) = blocks_sender {
            handoff.blocks_sender = blocks;
        }
        let timeout = timeout_ms.unwrap_or(self.room.spec.limits.handoff_timeout_ms);
        if timeout > 0 {
            handoff.deadline = Some(Timestamp::now().plus_millis(timeout));
        }
        let (id, blocks) = (handoff.id, handoff.blocks_sender);
        self.handoffs.insert(id, handoff.clone());

        let mut events = vec![self.emit(Event::HandoffOffered {
            handoff: Box::new(handoff),
        })];

        // A blocking handoff parks the sender's task on the spot; otherwise the
        // sender would have to remember to block itself and a forgetful model
        // would busy-wait.
        if blocks && self.tasks.get(&task).is_some_and(|t| t.state == TaskState::InProgress) {
            events.extend(self.transition(
                task,
                TaskTransition::Block {
                    reason: BlockReason::AwaitingHandoff { handoff: id },
                },
                Some(from),
            )?);
            events.extend(self.sweep_deadlocks());
        }
        Ok(events)
    }

    fn accept_handoff(&mut self, handoff: HandoffId, by: AgentId) -> Result<Vec<EventEnvelope>> {
        let roles = self.require_agent(by)?.roles.clone();
        let record = self
            .handoffs
            .get_mut(&handoff)
            .ok_or(EngineError::UnknownHandoff { handoff })?;
        let from = record.state;
        let kind = record.kind;
        let task = record.task;
        record.accept_by(by, &roles, Timestamp::now())?;
        let to = record.state;

        let mut events = vec![self.emit(Event::HandoffTransitioned {
            handoff,
            from,
            to,
            transition: Box::new(HandoffTransition::Accept { by }),
        })];

        // A delegation moves the task itself; an assist or review does not.
        if kind.transfers_ownership() {
            if let Some(record) = self.tasks.get_mut(&task) {
                record.assignee = Some(by);
                record.updated_at = Timestamp::now();
            }
        }
        events.extend(self.recompute_status(by));
        Ok(events)
    }

    fn complete_handoff(
        &mut self,
        handoff: HandoffId,
        by: AgentId,
        artifacts: Vec<ArtifactId>,
        summary: String,
    ) -> Result<Vec<EventEnvelope>> {
        let mut events = self.drive_handoff(
            handoff,
            HandoffTransition::Complete {
                by,
                artifacts,
                summary,
            },
        )?;
        events.extend(self.release_waiters_on(handoff)?);
        Ok(events)
    }

    /// Drive a handoff through a transition and, if it settled, wake whoever
    /// was waiting on it.
    fn drive_handoff(
        &mut self,
        handoff: HandoffId,
        transition: HandoffTransition,
    ) -> Result<Vec<EventEnvelope>> {
        let record = self
            .handoffs
            .get_mut(&handoff)
            .ok_or(EngineError::UnknownHandoff { handoff })?;
        let from = record.state;
        record.apply(&transition, Timestamp::now())?;
        let to = record.state;
        let actor = transition.actor();

        let mut events = vec![self.emit(Event::HandoffTransitioned {
            handoff,
            from,
            to,
            transition: Box::new(transition),
        })];

        if to.is_terminal() && !matches!(to, HandoffState::Completed) {
            // A rejected, failed, withdrawn or expired handoff must not leave
            // the sender blocked forever.
            events.extend(self.release_waiters_on(handoff)?);
        }
        if let Some(actor) = actor {
            events.extend(self.recompute_status(actor));
        }
        Ok(events)
    }

    /// Unblock every task that was waiting on `handoff`.
    fn release_waiters_on(&mut self, handoff: HandoffId) -> Result<Vec<EventEnvelope>> {
        let waiting: Vec<TaskId> = self
            .tasks
            .values()
            .filter(|t| t.state == TaskState::Blocked)
            .filter(|t| {
                t.blocked_on.iter().any(
                    |r| matches!(r, BlockReason::AwaitingHandoff { handoff: h } if *h == handoff),
                )
            })
            .map(|t| t.id)
            .collect();

        let mut events = Vec::new();
        for task in waiting {
            // Drop just this blocker; a task can be waiting on several things.
            if let Some(record) = self.tasks.get_mut(&task) {
                record.blocked_on.retain(|r| {
                    !matches!(r, BlockReason::AwaitingHandoff { handoff: h } if *h == handoff)
                });
                if !record.blocked_on.is_empty() {
                    continue;
                }
            }
            events.extend(self.transition(task, TaskTransition::Unblock, None)?);
        }
        Ok(events)
    }

    // ------------------------------------------------------ artifacts & review

    fn add_artifact(
        &mut self,
        task: TaskId,
        author: AgentId,
        name: String,
        body: ArtifactBody,
    ) -> Result<Vec<EventEnvelope>> {
        self.require_agent(author)?;
        let revision = self.require_task(task)?.revision;
        let mut artifact = Artifact::new(task, author, name, body);
        artifact.revision = revision;
        let id = artifact.id;
        self.artifacts.insert(id, artifact.clone());
        if let Some(record) = self.tasks.get_mut(&task) {
            record.artifacts.push(id);
            record.updated_at = Timestamp::now();
        }
        Ok(vec![self.emit(Event::ArtifactAdded {
            artifact: Box::new(artifact),
        })])
    }

    fn record_review(
        &mut self,
        task: TaskId,
        inspector: AgentId,
        findings: Vec<Finding>,
        summary: String,
        criteria_met: Vec<String>,
        criteria_failed: Vec<String>,
    ) -> Result<Vec<EventEnvelope>> {
        let spec = self.require_agent(inspector)?;
        if !spec.autonomy.may_inspect {
            return Err(EngineError::NotPermitted {
                agent: inspector,
                action: "inspect work".into(),
            });
        }
        let record = self.require_task(task)?;
        // The author of the work can never be its inspector.
        if record.assignee == Some(inspector) {
            return Err(EngineError::SelfInspection {
                task,
                agent: inspector,
            });
        }
        let (revision, artifacts) = (record.revision, record.artifacts.clone());

        let mut review = Review::from_findings(task, inspector, revision, findings, summary);
        review.criteria_met = criteria_met;
        review.criteria_failed = criteria_failed;
        review.inspected = artifacts;
        let verdict = review.verdict;
        let reason = review
            .blockers()
            .first()
            .map(|f| f.summary.clone())
            .unwrap_or_else(|| review.summary.clone());
        self.reviews.push(review.clone());

        let mut events = vec![self.emit(Event::ReviewRecorded {
            review: Box::new(review),
        })];

        events.extend(match verdict {
            Verdict::Approve => self.transition(task, TaskTransition::Approve { by: inspector }, Some(inspector))?,
            Verdict::RequestChanges => self.request_changes(task, inspector, reason)?,
            Verdict::Reject => self.transition(
                task,
                TaskTransition::Fail {
                    reason: format!("rejected on inspection: {reason}"),
                },
                Some(inspector),
            )?,
        });
        Ok(events)
    }

    /// Send work back for rework, failing the task if its budget is spent.
    ///
    /// Without this cap a worker and an inspector that disagree will bounce a
    /// task between them until the room's turn budget runs out.
    fn request_changes(
        &mut self,
        task: TaskId,
        inspector: AgentId,
        reason: String,
    ) -> Result<Vec<EventEnvelope>> {
        let record = self.require_task(task)?;
        let budget = record.max_revisions;
        if record.revision >= budget {
            let events = self.transition(
                task,
                TaskTransition::Fail {
                    reason: format!("exhausted {budget} revisions; last blocker: {reason}"),
                },
                Some(inspector),
            )?;
            return Ok(events);
        }
        self.transition(
            task,
            TaskTransition::RequestChanges {
                by: inspector,
                reason,
            },
            Some(inspector),
        )
    }

    // ---------------------------------------------------------- housekeeping

    fn record_turn(&mut self, agent: AgentId) -> Result<Vec<EventEnvelope>> {
        self.require_agent(agent)?;
        self.room.turns_used = self.room.turns_used.saturating_add(1);
        self.error_streak.insert(agent, 0);
        if self.room.is_out_of_turns() {
            return self.set_phase(RoomPhase::Paused, "turn budget exhausted");
        }
        Ok(Vec::new())
    }

    fn report_error(
        &mut self,
        agent: AgentId,
        task: Option<TaskId>,
        error: String,
    ) -> Result<Vec<EventEnvelope>> {
        self.require_agent(agent)?;
        let attempt = self.error_streak.entry(agent).or_insert(0);
        *attempt += 1;
        let attempt = *attempt;

        let mut events = vec![self.emit(Event::AgentErrored {
            agent,
            task,
            error,
            attempt,
        })];

        if attempt >= self.room.spec.limits.max_consecutive_errors {
            events.extend(self.set_status(agent, AgentStatus::Faulted));
            // Hand back whatever the faulted bot was holding so the room can
            // continue without it.
            for held in self.active_tasks_of(agent) {
                if let Ok(released) = self.transition(
                    held,
                    TaskTransition::Release {
                        reason: format!("{} faulted after {attempt} errors", agent.short()),
                    },
                    Some(agent),
                ) {
                    events.extend(released);
                }
            }
            self.statuses.insert(agent, AgentStatus::Faulted);
        }
        Ok(events)
    }

    /// Expire stale handoffs and look for deadlocks.
    fn tick(&mut self, now: Timestamp) -> Result<Vec<EventEnvelope>> {
        let expired: Vec<HandoffId> = self
            .handoffs
            .values()
            .filter(|h| h.is_expired_at(now))
            .map(|h| h.id)
            .collect();

        let mut events = Vec::new();
        for handoff in expired {
            events.extend(self.drive_handoff(handoff, HandoffTransition::Expire)?);
        }
        events.extend(self.sweep_deadlocks());
        Ok(events)
    }

    /// Break every cycle currently in the wait-for graph.
    ///
    /// Each pass resolves one cycle; the loop repeats until the graph is
    /// acyclic, bounded so a pathological graph cannot spin here.
    ///
    /// The preferred remedy is to **withdraw the handoff the victim is waiting
    /// on** rather than to release the victim's task. Withdrawing keeps the
    /// victim assigned and working, and — because the victim is by definition
    /// also the party someone else is waiting on — frees it to service the
    /// request pointed at it. That usually unwinds the whole jam. Releasing the
    /// task is the fallback for cycles built from task dependencies, where
    /// there is no handoff to withdraw.
    fn sweep_deadlocks(&mut self) -> Vec<EventEnvelope> {
        let mut events = Vec::new();
        for _ in 0..MAX_DEADLOCK_PASSES {
            let Some(cycle) = self.wait_graph().find_cycle() else {
                break;
            };
            let Some(victim) = deadlock::victim(&cycle) else {
                break;
            };
            let agents: Vec<AgentId> = cycle
                .iter()
                .filter_map(|t| self.tasks.get(t).and_then(|t| t.assignee))
                .collect();

            // The handoff keeping the victim parked, and who may withdraw it.
            let blocking = self.tasks.get(&victim).and_then(|task| {
                task.blocked_on.iter().find_map(|reason| match reason {
                    BlockReason::AwaitingHandoff { handoff } => Some(*handoff),
                    _ => None,
                })
            });
            let withdrawable =
                blocking.and_then(|id| self.handoffs.get(&id).map(|h| (id, h.from)));

            let resolution = match withdrawable {
                Some((handoff, _)) => DeadlockResolution::WithdrewHandoff { handoff },
                None => DeadlockResolution::ReleasedTask { task: victim },
            };
            events.push(self.emit(Event::DeadlockDetected {
                cycle: cycle.clone(),
                agents,
                resolution,
            }));

            let broken = match withdrawable {
                Some((handoff, sender)) => self
                    .drive_handoff(
                        handoff,
                        HandoffTransition::Withdraw {
                            by: sender,
                            reason: "withdrawn to break a deadlock".into(),
                        },
                    )
                    .map(|more| events.extend(more))
                    .is_ok(),
                None => {
                    let holder = self.tasks.get(&victim).and_then(|t| t.assignee);
                    if let Some(task) = self.tasks.get_mut(&victim) {
                        task.blocked_on.clear();
                    }
                    self.transition(
                        victim,
                        TaskTransition::Release {
                            reason: "released to break a deadlock".into(),
                        },
                        holder,
                    )
                    .map(|more| events.extend(more))
                    .is_ok()
                }
            };

            if !broken {
                // Nothing the engine can do on its own; surface it and stop
                // rather than looping on an unbreakable cycle.
                events.push(self.emit(Event::DeadlockDetected {
                    cycle,
                    agents: Vec::new(),
                    resolution: DeadlockResolution::EscalatedToHuman,
                }));
                break;
            }
        }
        events
    }

    // --------------------------------------------------------------- internals

    /// Apply a task transition, keeping derived fields and bot status in step.
    fn transition(
        &mut self,
        task: TaskId,
        transition: TaskTransition,
        actor: Option<AgentId>,
    ) -> Result<Vec<EventEnvelope>> {
        let record = self.require_task(task)?;
        let from = record.state;
        let to = from.apply(&transition)?;
        let previous_holder = record.assignee;

        let record = self
            .tasks
            .get_mut(&task)
            .expect("task presence checked immediately above");
        match &transition {
            TaskTransition::Claim { by } => record.assignee = Some(*by),
            TaskTransition::Release { .. } | TaskTransition::Reopen => {
                record.assignee = None;
                record.blocked_on.clear();
            }
            TaskTransition::Block { reason } => {
                if !record.blocked_on.contains(reason) {
                    record.blocked_on.push(reason.clone());
                }
            }
            TaskTransition::Unblock => record.blocked_on.clear(),
            TaskTransition::SubmitForReview { artifacts } => {
                for artifact in artifacts {
                    if !record.artifacts.contains(artifact) {
                        record.artifacts.push(*artifact);
                    }
                }
            }
            TaskTransition::RequestChanges { .. } => record.revision += 1,
            _ => {}
        }
        record.state = to;
        record.updated_at = Timestamp::now();

        let mut events = vec![self.emit(Event::TaskTransitioned {
            task,
            from,
            to,
            transition: Box::new(transition),
            actor,
        })];

        // Whoever held the task before and whoever holds it now may both have
        // changed availability.
        for agent in [previous_holder, actor].into_iter().flatten().collect::<BTreeSet<_>>() {
            events.extend(self.recompute_status(agent));
        }
        Ok(events)
    }

    /// Derive a bot's availability from the tasks it holds.
    fn recompute_status(&mut self, agent: AgentId) -> Vec<EventEnvelope> {
        if !self.specs.contains_key(&agent) {
            return Vec::new();
        }
        if self.status(agent) == AgentStatus::Faulted {
            return Vec::new();
        }
        let held: Vec<&Task> = self
            .tasks
            .values()
            .filter(|t| t.assignee == Some(agent) && t.state.is_assigned())
            .collect();

        let next = if held.is_empty() {
            AgentStatus::Idle
        } else if held.iter().all(|t| t.state.is_waiting()) {
            AgentStatus::Waiting
        } else {
            AgentStatus::Working
        };
        self.set_status(agent, next)
    }

    fn set_status(&mut self, agent: AgentId, to: AgentStatus) -> Vec<EventEnvelope> {
        let from = self.status(agent);
        if from == to {
            return Vec::new();
        }
        self.statuses.insert(agent, to);
        vec![self.emit(Event::AgentStatusChanged { agent, from, to })]
    }

    fn require_agent(&self, agent: AgentId) -> Result<&AgentSpec> {
        self.specs
            .get(&agent)
            .ok_or(EngineError::UnknownAgent { agent })
    }

    fn require_task(&self, task: TaskId) -> Result<&Task> {
        self.tasks.get(&task).ok_or(EngineError::UnknownTask { task })
    }

    fn require_assignee(&self, task: TaskId, agent: AgentId) -> Result<()> {
        let record = self.require_task(task)?;
        if record.assignee == Some(agent) {
            return Ok(());
        }
        Err(EngineError::NotAssignee {
            task,
            agent,
            holder: record
                .assignee
                .map(|a| a.to_string())
                .unwrap_or_else(|| "nobody".into()),
        })
    }

    fn emit(&mut self, event: Event) -> EventEnvelope {
        self.seq = self.seq.next();
        let envelope = EventEnvelope::new(self.seq, self.room.id, event);
        self.log.push(envelope.clone());
        envelope
    }
}

/// Alias kept so the `set_phase` signature reads clearly.
type EngineEvent = EventEnvelope;

/// Upper bound on deadlock-breaking passes in a single sweep.
const MAX_DEADLOCK_PASSES: usize = 32;

fn phase_label(phase: RoomPhase) -> &'static str {
    match phase {
        RoomPhase::Assembling => "assembling",
        RoomPhase::Running => "running",
        RoomPhase::Paused => "paused",
        RoomPhase::Completed => "completed",
        RoomPhase::Aborted => "aborted",
    }
}
