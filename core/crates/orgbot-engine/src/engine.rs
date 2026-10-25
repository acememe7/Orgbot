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

use orgbot_proto::agent::AgentStatus;
use orgbot_proto::artifact::{Artifact, ArtifactBody};
use orgbot_proto::convergence::{self, Convergence};
use orgbot_proto::decision::{Answer, Decision, DecisionLog, Governance, Request, Settled};
use orgbot_proto::event::{DeadlockResolution, Event, EventEnvelope, Seq};
use orgbot_proto::graph::{AuthorizationState, DependencyState, Readiness, TaskKind, TaskOrigin};
use orgbot_proto::handoff::{Handoff, HandoffKind, HandoffState, HandoffTransition, Recipient};
use orgbot_proto::ids::DecisionId;
use orgbot_proto::review::{Finding, Review, Verdict};
use orgbot_proto::roles::{Eligibility as RoleFit, RoleTable};
use orgbot_proto::room::{Room, RoomPhase, RoomSpec};
use orgbot_proto::skill::Skill;
use orgbot_proto::spend::{Spend, TokenUsage};
use orgbot_proto::task::{
    BlockReason, Priority, Task, TaskState, TaskTransition, DEFAULT_LEASE_MILLIS,
};
use orgbot_proto::text;
use orgbot_proto::{
    AgentId, AgentSpec, ArtifactId, ChatMessage, HandoffId, RoomId, TaskId, Timestamp,
};

use crate::command::Command;
use crate::deadlock::{self, WaitGraph};
use crate::eligibility::{Eligibility, LeaseState, Reason};
use crate::error::{EngineError, Result};

/// How many open remediations one task may carry before the room is told to
/// consolidate rather than add another.
///
/// Three leaves room for genuinely separate follow-ups from one review while
/// stopping the chains that turned twelve objectives into ninety-six records.
pub const MAX_OPEN_REMEDIATIONS: usize = 3;

/// Why the work graph refuses this task, in words aimed at a bot.
///
/// Deliberately not [`orgbot_proto::graph::Readiness::summary`], which is
/// written for the person watching the room and reassures them that nothing is
/// needed from them — exactly the wrong thing to tell an agent it cannot claim.
/// A dependency that has not landed yet.
#[derive(Debug, Clone)]
struct UnmetDependency {
    /// The upstream task, after following any supersession.
    task: TaskId,
    /// Its title, or a note that it is not here.
    title: String,
    /// Who is expected to clear it.
    owner: Option<AgentId>,
    /// Whether the upstream is actually in this room.
    present: bool,
}

/// A live group chat and everything in it.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Engine {
    room: Room,
    specs: BTreeMap<AgentId, AgentSpec>,
    statuses: BTreeMap<AgentId, AgentStatus>,
    handles: BTreeMap<String, AgentId>,
    tasks: BTreeMap<TaskId, Task>,
    /// The condition last reported, so a standing problem is raised once.
    #[serde(default)]
    last_health: String,
    /// What each bot has drawn, so a room can say what it has cost.
    #[serde(default)]
    spend: BTreeMap<AgentId, Spend>,
    /// Playbooks the room's bots can reach for.
    #[serde(default)]
    skills: Vec<Skill>,
    /// What earlier rooms already built.
    #[serde(default, skip)]
    archive: crate::archive::Archive,
    handoffs: BTreeMap<HandoffId, Handoff>,
    artifacts: BTreeMap<ArtifactId, Artifact>,
    reviews: Vec<Review>,
    transcript: Vec<ChatMessage>,
    log: Vec<EventEnvelope>,
    seq: Seq,
    /// Consecutive failed turns per bot, reset by any success.
    error_streak: BTreeMap<AgentId, u32>,
    /// Every question put to the operator and every answer given, so the room
    /// can tell what it has already been told.
    ///
    /// Defaulted, because this struct is what a saved room is written as: a
    /// room saved before decisions existed has none, and must still load.
    #[serde(default)]
    decisions: DecisionLog,
    /// What each role can do, and who may stand in for it.
    ///
    /// Held here rather than on the room spec because this is where
    /// eligibility is decided, and because a room that declares nothing must
    /// keep matching roles by name exactly as it always did.
    #[serde(default)]
    roles: RoleTable,
    /// The room's standing rules, and which version of each is in force.
    #[serde(default)]
    governance: Governance,
}

impl Engine {
    /// Restore older app journals without executing a command or external tool.
    pub fn from_archive(
        room: Room,
        events: Vec<EventEnvelope>,
    ) -> std::result::Result<Self, String> {
        let mut engine = Self::new(room.spec.clone());
        engine.room = room;
        engine.log.clear();
        engine.seq = Seq::ZERO;
        for envelope in events {
            if envelope.room != engine.room.id || envelope.seq != engine.seq.next() {
                return Err("Saved room history has a gap or belongs to another room".into());
            }
            match &envelope.event {
                // A report about the room, not a change to it: replaying one
                // must not re-raise it.
                Event::RoomOpened { .. } | Event::RoomHealthChanged { .. } => {}
                Event::RoomClosed { .. } => engine.room.phase = RoomPhase::Completed,
                Event::AgentJoined { spec } => {
                    engine.handles.insert(spec.handle.clone(), spec.id);
                    engine.statuses.insert(spec.id, AgentStatus::Idle);
                    engine.specs.insert(spec.id, *spec.clone());
                }
                Event::AgentLeft { agent, .. } => {
                    engine.statuses.insert(*agent, AgentStatus::Offline);
                }
                Event::AgentStatusChanged { agent, from, to } => {
                    engine.statuses.insert(*agent, *to);
                    if *from == AgentStatus::Faulted && *to == AgentStatus::Idle {
                        engine.error_streak.insert(*agent, 0);
                    }
                }
                Event::MessagePosted { message } => {
                    // Phase notices live in the journal/UI, outside model conversation.
                    if !matches!(message.author, orgbot_proto::chat::Author::System) {
                        engine.transcript.push(*message.clone());
                    }
                }
                Event::TaskCreated { task } => {
                    engine.tasks.insert(task.id, *task.clone());
                }
                Event::DecisionAsked { decision } => {
                    engine.decisions.decisions.push(*decision.clone());
                }
                Event::DecisionQuestionJoined { decision, by, task } => {
                    if let Some(question) = engine
                        .decisions
                        .decisions
                        .iter_mut()
                        .find(|question| question.id == *decision)
                    {
                        if !question.asked_by.contains(by) {
                            question.asked_by.push(*by);
                        }
                        if question.task.is_none() {
                            question.task = *task;
                        }
                    }
                }
                Event::DecisionAnswered { decision, answer } => {
                    engine.decisions.answer(*decision, answer.clone());
                }
                Event::TaskAuthorized { task, .. } => {
                    if let Some(record) = engine.tasks.get_mut(task) {
                        record.authorization = AuthorizationState::Authorized;
                    }
                }
                Event::TaskTransitioned {
                    task,
                    transition,
                    actor,
                    ..
                } => {
                    // Reuse the task projection, discarding its newly emitted envelopes.
                    engine
                        .transition(*task, *transition.clone(), *actor)
                        .map_err(|e| e.to_string())?;
                }
                Event::HandoffOffered { handoff } => {
                    engine.handoffs.insert(handoff.id, *handoff.clone());
                }
                Event::HandoffTransitioned {
                    handoff,
                    transition,
                    ..
                } => {
                    let record = engine
                        .handoffs
                        .get_mut(handoff)
                        .ok_or("Missing saved handoff")?;
                    if let HandoffTransition::Accept { by } = **transition {
                        let roles = &engine
                            .specs
                            .get(&by)
                            .ok_or("Missing saved recipient")?
                            .roles;
                        record
                            .accept_by(by, roles, envelope.at)
                            .map_err(|e| e.to_string())?;
                        if record.kind.transfers_ownership() {
                            engine
                                .tasks
                                .get_mut(&record.task)
                                .ok_or("Missing saved task")?
                                .assignee = Some(by);
                        }
                    } else {
                        record
                            .apply(transition, envelope.at)
                            .map_err(|e| e.to_string())?;
                    }
                }
                Event::ArtifactAdded { artifact } => {
                    let task = engine
                        .tasks
                        .get_mut(&artifact.task)
                        .ok_or("Missing artifact task")?;
                    if !task.artifacts.contains(&artifact.id) {
                        task.artifacts.push(artifact.id);
                    }
                    engine.artifacts.insert(artifact.id, *artifact.clone());
                }
                Event::ReviewRecorded { review } => engine.reviews.push(*review.clone()),
                Event::AgentErrored { agent, attempt, .. } => {
                    engine.error_streak.insert(*agent, *attempt);
                }
                Event::DeadlockDetected { .. } => {}
            }
            // The journal remains the original record, including original ids/times.
            engine.log.truncate((envelope.seq.0 - 1) as usize);
            engine.seq = envelope.seq;
            engine.log.push(envelope);
        }
        if engine.log.is_empty() {
            return Err("Saved room history is empty".into());
        }
        Ok(engine)
    }

    /// Rebind current configuration to the saved identity and board.
    pub fn configure_saved_room(&mut self, spec: RoomSpec) {
        self.room.spec = spec;
        self.room.spec.roster = self.specs.keys().copied().collect();
        // Opening saved work never replays an interrupted external operation.
        if !self.room.phase.is_finished() {
            self.room.phase = RoomPhase::Paused;
        }
    }

    /// Keep ids stable while applying edited model, persona and permissions.
    pub fn update_agent(&mut self, spec: AgentSpec) {
        if self
            .specs
            .get(&spec.id)
            .is_some_and(|old| old.provider != spec.provider)
        {
            self.statuses.insert(spec.id, AgentStatus::Idle);
            self.error_streak.remove(&spec.id);
        }
        self.handles.insert(spec.handle.clone(), spec.id);
        let review_only = spec.autonomy.review_only;
        let agent = spec.id;
        self.specs.insert(spec.id, spec);
        if review_only {
            let out_of_scope: Vec<_> = self
                .tasks
                .values()
                .filter(|task| {
                    task.assignee == Some(agent)
                        && !review_only_task(task.kind, task.required_role.as_deref())
                })
                .map(|task| task.id)
                .collect();
            for task in out_of_scope {
                self.try_release(task, "the assignee is now restricted to review work".into());
            }
        }
    }

    /// Open a new room.
    pub fn new(spec: RoomSpec) -> Self {
        let room = Room::new(spec);
        let mut engine = Self {
            room,
            specs: BTreeMap::new(),
            statuses: BTreeMap::new(),
            handles: BTreeMap::new(),
            tasks: BTreeMap::new(),
            last_health: String::new(),
            spend: BTreeMap::new(),
            skills: Vec::new(),
            archive: crate::archive::Archive::default(),
            handoffs: BTreeMap::new(),
            artifacts: BTreeMap::new(),
            reviews: Vec::new(),
            transcript: Vec::new(),
            log: Vec::new(),
            seq: Seq::ZERO,
            error_streak: BTreeMap::new(),
            decisions: DecisionLog::default(),
            roles: RoleTable::default(),
            governance: Governance::default(),
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
        self.statuses
            .get(&id)
            .copied()
            .unwrap_or(AgentStatus::Offline)
    }

    /// Current consecutive failures, excluding errors from an older configuration.
    pub fn error_streak(&self, id: AgentId) -> u32 {
        self.error_streak.get(&id).copied().unwrap_or(0)
    }

    /// A task on the board.
    pub fn task(&self, id: TaskId) -> Option<&Task> {
        self.tasks.get(&id)
    }

    /// Every task on the board.
    /// Adopt a standing rule, returning the version now in force.
    ///
    /// A rule answers a class of questions in advance so they never have to be
    /// asked. Adopting one with an existing name replaces it rather than
    /// sitting beside it, so an obsolete rule cannot resurface as live
    /// authority.
    pub fn adopt_policy(&mut self, policy: orgbot_proto::decision::Policy) -> u32 {
        self.governance.adopt(policy)
    }

    /// Withdraw a rule, keeping it on the record.
    pub fn withdraw_policy(&mut self, name: &str) -> bool {
        self.governance.withdraw(name)
    }

    /// The room's standing rules.
    pub const fn governance(&self) -> &Governance {
        &self.governance
    }

    /// Whether a standing rule already permits `request`.
    ///
    /// Consulted before a decision is raised: if a rule covers it, nobody
    /// needs to be asked.
    pub fn policy_permits(&self, request: &Request) -> Option<&orgbot_proto::decision::Policy> {
        self.governance.permits(request, Timestamp::now())
    }

    /// Declare what each role can do and who may stand in for it.
    pub fn set_roles(&mut self, roles: RoleTable) {
        self.roles = roles;
    }

    /// The room's role table.
    pub const fn roles(&self) -> &RoleTable {
        &self.roles
    }

    /// How well `agent` matches work requiring `required`.
    pub fn eligibility(&self, agent: AgentId, required: Option<&str>) -> RoleFit {
        self.specs.get(&agent).map_or(RoleFit::No, |spec| {
            self.roles.eligibility(&spec.roles, required)
        })
    }

    /// Every question this room has put to the operator, and every answer.
    pub const fn decisions(&self) -> &DecisionLog {
        &self.decisions
    }

    /// What a person should see instead of the raw board.
    ///
    /// Objectives, progress and the decisions only they can make. The full
    /// graph stays available through [`Engine::tasks`] for the bots, which
    /// need all of it.
    pub fn digest(&self) -> crate::digest::RoomDigest {
        let mut view = crate::digest::digest(self.tasks(), &self.decisions);
        view.utilization = self.utilization();
        view.spend = self
            .spend_by_backend()
            .into_iter()
            .map(|(backend, spend)| crate::digest::AccountSpend {
                backend,
                turns: spend.turns,
                tokens: spend.usage.total(),
                per_turn: spend.per_turn(),
            })
            .collect();

        // Review history lives here, not on the tasks, so the digest cannot
        // work this out for itself.
        let reviewed: Vec<usize> = self
            .tasks
            .keys()
            .map(|task| self.review_cycles(*task).len())
            .filter(|cycles| *cycles > 0)
            .collect();
        view.health.average_review_cycles = if reviewed.is_empty() {
            0.0
        } else {
            reviewed.iter().sum::<usize>() as f32 / reviewed.len() as f32
        };
        view
    }

    /// Where the room's effort is going, bot by bot.
    pub fn utilization(&self) -> crate::digest::Utilization {
        let mut bots: Vec<crate::digest::BotState> = self
            .specs
            .values()
            .map(|spec| {
                let holding = self.workload(spec.id);
                let status = self.status(spec.id);
                // The bot's own state first, then what its work says. A bot
                // holding only blocked tasks is waiting, not working, however
                // recently it took a turn.
                let doing = match status {
                    AgentStatus::Offline => "offline",
                    AgentStatus::Faulted => "faulted",
                    _ if holding == 0 => "idle",
                    _ if self.holds_only_blocked(spec.id) => "waiting",
                    _ if self.holds_a_review(spec.id) => "reviewing",
                    _ => "working",
                };
                crate::digest::BotState {
                    agent: spec.id,
                    handle: spec.handle.clone(),
                    doing: doing.to_string(),
                    holding,
                    free: status.accepts_work() && holding == 0,
                }
            })
            .collect();
        bots.sort_by(|a, b| b.holding.cmp(&a.holding).then(a.handle.cmp(&b.handle)));

        crate::digest::Utilization {
            starved_roles: self.starved_roles(),
            idle: bots.iter().filter(|bot| bot.free).count() as u32,
            unclaimed: {
                let now = Timestamp::now();
                self.tasks
                    .values()
                    .filter(|task| self.evaluate_task(task.id, now).eligible)
                    .count() as u32
            },
            bots,
        }
    }

    /// Whether every task this bot holds is blocked.
    fn holds_only_blocked(&self, agent: AgentId) -> bool {
        let mut held = self
            .tasks
            .values()
            .filter(|task| task.assignee == Some(agent) && !task.state.is_terminal())
            .peekable();
        held.peek().is_some()
            && held
                .all(|task| !task.readiness().is_progressing() || task.state == TaskState::Blocked)
    }

    /// Whether this bot is holding something it is inspecting.
    fn holds_a_review(&self, agent: AgentId) -> bool {
        self.tasks
            .values()
            .any(|task| task.assignee == Some(agent) && task.kind == TaskKind::Review)
    }

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

    /// Whether this task could be claimed by anyone right now, and if not why.
    ///
    /// The task half of the decision: everything that does not depend on who
    /// is asking. [`Self::evaluate_claim`] adds the bot half. Splitting it
    /// this way is what lets a room-wide count of open work and a single bot's
    /// offer of work come from the same reasoning instead of two.
    pub fn evaluate_task(&self, task: TaskId, now: Timestamp) -> Eligibility {
        let Some(record) = self.tasks.get(&task) else {
            return Eligibility {
                eligible: false,
                reason: Reason::UnknownTask,
                detail: format!("there is no task {task} in this room"),
                effective: Readiness::Closed { state: "unknown" },
                blocking_tasks: vec![],
                holder: None,
                lease: LeaseState::Unheld,
                lease_expires_at: None,
                ownership_epoch: 0,
                repair_required: false,
            };
        };

        let lease = self.lease_state_of(record, now);
        let base = |reason: Reason, detail: String| Eligibility {
            eligible: false,
            reason,
            detail,
            effective: record.readiness(),
            blocking_tasks: vec![],
            holder: record.assignee,
            lease,
            lease_expires_at: record.lease_expires_at,
            ownership_epoch: record.ownership_epoch,
            repair_required: reason.needs_repair(),
        };

        // 1. Replaced work is never actionable again, whatever else is true.
        if let Some(replacement) = record.lineage.superseded_by {
            return base(
                Reason::Superseded,
                format!("it was superseded by {replacement}; work that instead"),
            );
        }

        // 2. Finished is finished.
        if record.state.is_terminal() {
            let reason = match record.state {
                TaskState::Submitted => Reason::Delivered,
                TaskState::Retired => Reason::Retired,
                TaskState::Superseded => Reason::Superseded,
                _ => Reason::Closed,
            };
            return base(
                reason,
                format!(
                    "it is {}, and will not be worked again",
                    record.state.as_str()
                ),
            );
        }

        // 3. Permission to run, before anything about whether it could.
        match &record.authorization {
            AuthorizationState::HeldByPolicy { policy, reason } => {
                return base(
                    Reason::PolicyHeld,
                    format!("the `{policy}` policy holds it: {reason}"),
                );
            }
            AuthorizationState::SecurityHold { finding, .. } => {
                return base(
                    Reason::SecurityHold,
                    format!("security is holding it: {finding}"),
                );
            }
            AuthorizationState::AwaitingOperator { because, .. } => {
                return base(
                    Reason::BlockedOperator,
                    format!("it needs an operator decision: {because}"),
                );
            }
            AuthorizationState::Authorized => {}
        }

        // 4. Upstream work. Computed from the edges, not from the projection
        //    of them, so the answer here and the answer at claim time are the
        //    same answer.
        if let Some(unmet) = self.first_unmet_dependency(record) {
            let mut result = base(
                if unmet.present {
                    Reason::DependencyBlocked
                } else {
                    Reason::DependencyMissing
                },
                if unmet.present {
                    format!("it waits on {} ({})", unmet.task, unmet.title)
                } else {
                    format!(
                        "it waits on {}, which is not in this room and so can never land",
                        unmet.task
                    )
                },
            );
            result.blocking_tasks = vec![unmet.task];
            return result;
        }

        // 5. Where the work itself has got to.
        match record.state {
            TaskState::Ready => {}
            TaskState::Draft => {
                return base(
                    Reason::NotOpenYet,
                    "it has not been opened for claiming yet".into(),
                );
            }
            TaskState::Blocked => {
                // Reuse the readiness rendering rather than writing a second
                // description of the same blocker.
                let detail = match record.readiness() {
                    Readiness::BlockedInternal { reason } => format!("it is blocked: {reason}"),
                    other => other.summary(),
                };
                return base(Reason::BlockedInternal, detail);
            }
            _ => {}
        }

        // 6. Who holds it. A state that says somebody is working on this is
        //    only true if somebody actually is; the two ways that can be false
        //    are both recoverable, and both used to be permanent.
        match (record.state.is_assigned(), record.assignee, lease) {
            (true, None, _) => {
                return base(
                    Reason::ZombieInProgress,
                    format!(
                        "it is {} with nobody holding it; the room will return it to the pool",
                        record.state.as_str()
                    ),
                );
            }
            (true, Some(holder), LeaseState::Expired | LeaseState::Missing) => {
                return base(
                    Reason::LeaseExpired,
                    format!(
                        "{} holds it but has stopped reporting progress; the room will return it to the pool",
                        holder.short()
                    ),
                );
            }
            (true, Some(holder), _) => {
                return base(
                    Reason::AlreadyClaimed,
                    format!("{} is holding it", holder.short()),
                );
            }
            // Ready with a holder recorded is not a state the transitions can
            // produce, but a saved room could carry one. Treat the holder as
            // real rather than handing the task to somebody else.
            (false, Some(holder), LeaseState::Active) => {
                return base(
                    Reason::AlreadyClaimed,
                    format!("{} is holding it", holder.short()),
                );
            }
            (false, _, _) => {}
        }

        Eligibility {
            eligible: true,
            reason: Reason::ReadyNow,
            detail: "it is open and everything it needs has landed".into(),
            effective: record.readiness(),
            blocking_tasks: vec![],
            holder: None,
            lease: LeaseState::Unheld,
            lease_expires_at: record.lease_expires_at,
            ownership_epoch: record.ownership_epoch,
            repair_required: false,
        }
    }

    /// Whether `agent` could claim `task` right now, and if not why.
    ///
    /// **This is the only place that decides.** The per-bot prompt, the
    /// claimable listing, the scheduler, the board counts, the Work UI and
    /// `claim_task` itself are all callers. Anything that advertises work
    /// without going through here can advertise work the claim would refuse,
    /// which is exactly the bug this exists to make impossible.
    ///
    /// It is still a reading, not a promise: the board moves, and a claim
    /// re-evaluates at the moment it applies.
    pub fn evaluate_claim(&self, task: TaskId, agent: AgentId, now: Timestamp) -> Eligibility {
        let Some(spec) = self.specs.get(&agent) else {
            return Eligibility {
                eligible: false,
                reason: Reason::UnknownBot,
                detail: format!("there is no bot {agent} in this room"),
                effective: Readiness::Closed { state: "unknown" },
                blocking_tasks: vec![],
                holder: None,
                lease: LeaseState::Unheld,
                lease_expires_at: None,
                ownership_epoch: 0,
                repair_required: false,
            };
        };

        let mut result = self.evaluate_task(task, now);
        if !result.eligible {
            return result;
        }

        // A bot that is not taking work must not be offered any. Without this
        // a faulted bot is still told about open tasks every turn, and every
        // claim it makes is refused somewhere further down.
        let status = self.status(agent);
        if !status.is_available() {
            result.eligible = false;
            result.reason = if status == AgentStatus::Faulted {
                Reason::BotFaulted
            } else {
                Reason::BotUnavailable
            };
            result.detail = format!("{} is {status:?} and is not taking work", spec.handle);
            return result;
        }

        let Some(record) = self.tasks.get(&task) else {
            return result;
        };
        if spec.autonomy.review_only
            && !review_only_task(record.kind, record.required_role.as_deref())
        {
            result.eligible = false;
            result.reason = Reason::ReviewOnly;
            result.detail = format!(
                "{} may only claim review tasks or tasks explicitly requiring review, reviewer, audit, or security",
                spec.handle
            );
            return result;
        }
        let fit = self
            .roles
            .eligibility(&spec.roles, record.required_role.as_deref());
        if !fit.is_eligible() {
            result.eligible = false;
            result.reason = Reason::RoleMismatch;
            result.detail = format!(
                "it needs the `{}` role, which {} does not hold",
                record.required_role.clone().unwrap_or_default(),
                spec.handle
            );
            return result;
        }
        result
    }

    /// Turn a refusal into the error the caller already knows.
    ///
    /// The decision is the evaluator's; this only chooses how to say it. Every
    /// refusal that had a specific error before still gets that error, so
    /// nothing downstream had to learn a new shape to keep working — a claim
    /// on replaced work is still `NotReady`, a claim on a task somebody else
    /// holds is still the state machine's own complaint. `NotClaimable` is for
    /// what genuinely had no answer before: ownership that has come apart, and
    /// bots that cannot be given work.
    fn refusal(&self, task: TaskId, agent: AgentId, verdict: Eligibility) -> EngineError {
        match verdict.reason {
            Reason::ReviewOnly => EngineError::NotPermitted {
                agent,
                action: "claim work outside review-only scope".into(),
            },
            Reason::RoleMismatch => EngineError::RoleMismatch {
                task,
                agent,
                required: self
                    .tasks
                    .get(&task)
                    .and_then(|t| t.required_role.clone())
                    .unwrap_or_default(),
            },
            Reason::DependencyBlocked | Reason::DependencyMissing => {
                match verdict.blocking_tasks.first() {
                    Some(blocker) => EngineError::DependencyNotMet {
                        task,
                        blocker: *blocker,
                    },
                    None => EngineError::NotReady {
                        task,
                        reason: verdict.detail,
                    },
                }
            }
            Reason::Superseded
            | Reason::PolicyHeld
            | Reason::SecurityHold
            | Reason::BlockedOperator => EngineError::NotReady {
                task,
                reason: verdict.detail,
            },
            // New ground: nothing used to refuse these, so there is no older
            // error to be faithful to, and the reason code is the useful part.
            Reason::ZombieInProgress
            | Reason::LeaseExpired
            | Reason::BotFaulted
            | Reason::BotUnavailable
            | Reason::UnknownBot => EngineError::NotClaimable {
                task,
                agent,
                code: verdict.reason.code(),
                reason: verdict.detail,
            },
            // Everything else is the workflow saying no. Ask the state machine
            // so the wording is the one it has always given.
            _ => match self
                .tasks
                .get(&task)
                .map(|t| t.state.apply(&TaskTransition::Claim { by: agent }))
            {
                Some(Err(error)) => EngineError::Transition(error),
                _ => EngineError::NotClaimable {
                    task,
                    agent,
                    code: verdict.reason.code(),
                    reason: verdict.detail,
                },
            },
        }
    }

    /// The state of the claim on `task` as of `now`.
    fn lease_state_of(&self, task: &Task, now: Timestamp) -> LeaseState {
        if task.assignee.is_none() {
            return LeaseState::Unheld;
        }
        match task.lease_expires_at {
            // Held with no expiry recorded. Rooms saved before claims carried
            // one come back like this, and the sweep that returns abandoned
            // work to the pool compares against an expiry — so without
            // treating this as reclaimable the task is held forever.
            None if !task.state.is_terminal() => LeaseState::Missing,
            None => LeaseState::Unheld,
            Some(expiry) if now >= expiry => LeaseState::Expired,
            Some(_) => LeaseState::Active,
        }
    }

    /// Open tasks `agent` is eligible to claim, best first.
    ///
    /// Ordering is by priority then age, so a room drains urgent work before
    /// old work and old work before new.
    pub fn claimable_by(&self, agent: AgentId) -> Vec<TaskId> {
        let Some(spec) = self.specs.get(&agent) else {
            return Vec::new();
        };
        // Every gate is resolved before ranking, not after. Ranking first and
        // validating later is what let superseded and dependency-blocked work
        // be offered as ready.
        //
        // The gate is `evaluate_claim` and nothing else, so what is listed here
        // is by construction what a claim would accept. This list is the source
        // for the per-bot prompt and for `list_tasks(claimable)` alike.
        let now = Timestamp::now();
        let mut open: Vec<(&Task, RoleFit)> = self
            .tasks
            .values()
            .filter(|t| self.evaluate_claim(t.id, agent, now).eligible)
            .map(|t| {
                let fit = self
                    .roles
                    .eligibility(&spec.roles, t.required_role.as_deref());
                (t, fit)
            })
            .collect();
        // A bot's own work first, then work it can stand in for. Standing in
        // is a relief valve for a role-locked board, not the normal path.
        open.sort_by(|(a, a_fit), (b, b_fit)| {
            b_fit
                .cmp(a_fit)
                .then(b.priority.cmp(&a.priority))
                .then(a.created_at.cmp(&b.created_at))
                .then(a.id.cmp(&b.id))
        });
        open.into_iter().map(|(t, _)| t.id).collect()
    }

    /// Who should be asked to restore a lost or truncated artifact.
    ///
    /// The bot that wrote it, because it is the one that has the content.
    /// Asking a person for something the room produced is the wrong question,
    /// and asking a different bot invites a plausible reconstruction of
    /// something that has to be exact.
    pub fn recovery_owner(&self, artifact: ArtifactId) -> Option<AgentId> {
        self.artifacts.get(&artifact).map(|record| record.author)
    }

    /// Whether `agent` may publish a replacement for `artifact`.
    ///
    /// Anyone may correct ordinary work. Content carrying authority that
    /// cannot be re-derived — keys, signatures, schemas, protocol constants —
    /// may only be re-emitted by whoever produced it, because a second author
    /// can only produce something that looks right.
    pub fn may_replace(&self, artifact: ArtifactId, agent: AgentId) -> bool {
        self.artifacts
            .get(&artifact)
            .is_none_or(|record| !record.authority_bearing || record.author == agent)
    }

    /// Live work that looks like the same thing as `title`.
    ///
    /// Asked before a task is created. A board grows fastest when several bots
    /// independently notice the same gap and each raise it, and the records
    /// are near-impossible to reconcile afterwards because nothing links them.
    pub fn work_like(&self, title: &str) -> Vec<TaskId> {
        self.tasks
            .values()
            .filter(|task| !task.state.is_terminal() && !task.lineage.is_superseded())
            .filter(|task| text::looks_like_the_same_work(&task.title, title))
            .map(|task| task.id)
            .collect()
    }

    /// How many live remediations already hang off `task`.
    ///
    /// One review finding should not be able to spawn an open-ended chain of
    /// follow-up work. Past a handful, the answer is to consolidate rather
    /// than add another.
    pub fn remediation_count(&self, task: TaskId) -> usize {
        self.tasks
            .values()
            .filter(|record| !record.state.is_terminal() && !record.lineage.is_superseded())
            .filter(|record| record.lineage.remediation_of == Some(task))
            .count()
    }

    /// Mutable access to one task.
    ///
    /// Used by the consistency check's tests to construct states the command
    /// surface refuses, and by callers restoring a room from storage. Ordinary
    /// changes go through commands so the transition rules apply.
    pub fn task_mut(&mut self, id: TaskId) -> Option<&mut Task> {
        self.tasks.get_mut(&id)
    }

    /// Mutable access to one artifact, for callers restoring a room from
    /// storage or recording a correction.
    pub fn artifact_mut(&mut self, id: ArtifactId) -> Option<&mut Artifact> {
        self.artifacts.get_mut(&id)
    }

    /// The playbooks available in this room.
    pub fn skills(&self) -> &[Skill] {
        &self.skills
    }

    /// One playbook by name, for a bot that has decided it applies.
    pub fn skill(&self, name: &str) -> Option<&Skill> {
        self.skills
            .iter()
            .find(|skill| skill.name.eq_ignore_ascii_case(name))
    }

    /// Stock the room's playbooks. Replaces whatever was there.
    ///
    /// Set from the room file on every open, so editing a skill on disk takes
    /// effect the next time the room runs rather than being frozen into saved
    /// state at the moment it was first read.
    pub fn set_skills(&mut self, skills: Vec<Skill>) {
        self.skills = skills;
    }

    /// What earlier rooms left behind.
    pub fn archive(&self) -> &crate::archive::Archive {
        &self.archive
    }

    /// Give this room sight of what earlier ones built.
    pub fn set_archive(&mut self, archive: crate::archive::Archive) {
        self.archive = archive;
    }

    /// What one bot has drawn.
    pub fn spend_of(&self, agent: AgentId) -> Spend {
        self.spend.get(&agent).copied().unwrap_or_default()
    }

    /// What the room has drawn in total.
    pub fn spend_total(&self) -> Spend {
        let mut total = Spend::default();
        for spend in self.spend.values() {
            total.merge(*spend);
        }
        total
    }

    /// What each account has drawn, heaviest first.
    ///
    /// Grouped by backend rather than by bot because that is the unit an
    /// allowance is granted in: fifteen bots on one subscription draw on one
    /// limit, and a per-bot list hides that completely. It is the view that
    /// would have shown a weekly allowance being eaten four times faster than
    /// the week before.
    pub fn spend_by_backend(&self) -> Vec<(String, Spend)> {
        let mut totals: BTreeMap<String, Spend> = BTreeMap::new();
        for (agent, spend) in &self.spend {
            let Some(spec) = self.specs.get(agent) else {
                continue;
            };
            let backend = spec.provider.backend();
            if backend.is_empty() {
                continue;
            }
            totals.entry(backend).or_default().merge(*spend);
        }
        let mut ranked: Vec<(String, Spend)> = totals.into_iter().collect();
        ranked.sort_by(|(a_name, a), (b_name, b)| {
            b.usage
                .total()
                .cmp(&a.usage.total())
                .then(a_name.cmp(b_name))
        });
        ranked
    }

    /// Every invariant this room breaks, if any.
    ///
    /// An empty result means it is coherent. Anything returned is a bug
    /// upstream rather than a state to work around, so this reports and does
    /// not repair.
    pub fn inconsistencies(&self) -> Vec<crate::consistency::Violation> {
        crate::consistency::check(self)
    }

    /// Set how many times a task may come back from inspection.
    ///
    /// A real knob rather than a fixed number: hard work legitimately needs
    /// more rounds than the default allows, and the alternative — failing the
    /// task — throws away everything done so far.
    pub fn set_max_revisions(&mut self, task: TaskId, budget: u32) {
        if let Some(record) = self.tasks.get_mut(&task) {
            record.max_revisions = budget;
        }
    }

    /// Every review of `task`, oldest first, as blocking-finding sets.
    pub fn review_cycles(&self, task: TaskId) -> Vec<Vec<Finding>> {
        self.reviews
            .iter()
            .filter(|review| review.task == task)
            .map(|review| review.findings.clone())
            .collect()
    }

    /// Whether review of `task` is getting anywhere.
    ///
    /// Answered from the review history rather than from a model's impression
    /// of how it is going: the same invariant failing repeatedly, or a set
    /// that never shrinks, are both things the room should act on itself
    /// rather than send round again.
    pub fn convergence(&self, task: TaskId) -> Convergence {
        convergence::assess(&self.review_cycles(task))
    }

    /// A bot that could review `task`, chosen so nobody has to be asked.
    ///
    /// Independence first: a bot cannot inspect its own work, and neither can
    /// one that stood in on it. Then the least busy eligible bot, so review
    /// does not queue behind one overloaded inspector while others sit idle.
    /// Returns `None` only when no independent reviewer exists at all — which
    /// is the one case genuinely worth putting to a person.
    pub fn reviewer_for(&self, task: TaskId) -> Option<AgentId> {
        let record = self.tasks.get(&task)?;
        let mut candidates: Vec<(AgentId, u32)> = self
            .specs
            .values()
            .filter(|spec| spec.autonomy.may_inspect)
            // Independence, encoded rather than left to convention.
            .filter(|spec| Some(spec.id) != record.assignee)
            .filter(|spec| {
                self.roles
                    .eligibility(&spec.roles, record.required_role.as_deref())
                    .is_eligible()
                    || self
                        .roles
                        .eligibility(&spec.roles, Some("review"))
                        .is_eligible()
                    || spec.roles.iter().any(|role| role == "reviewer")
            })
            .filter(|spec| self.status(spec.id) != AgentStatus::Offline)
            .map(|spec| (spec.id, self.workload(spec.id)))
            .collect();

        // Least loaded first, then by id so the choice is stable across runs.
        candidates
            .sort_by(|(a_id, a_load), (b_id, b_load)| a_load.cmp(b_load).then(a_id.cmp(b_id)));
        candidates.first().map(|(id, _)| *id)
    }

    /// How much live work a bot is holding.
    ///
    /// The basis for spreading work: a room with idle bots and queued work is
    /// a scheduling failure, not a capacity problem.
    pub fn workload(&self, agent: AgentId) -> u32 {
        self.tasks
            .values()
            .filter(|task| task.assignee == Some(agent) && !task.state.is_terminal())
            .count() as u32
    }

    /// Roles that open work needs and no free bot can cover.
    ///
    /// The difference between "the scheduler is not matching bots to work" and
    /// "nobody who could do this is available" — which look identical from a
    /// count of idle bots beside a count of open tasks, and have completely
    /// different answers.
    /// Claimable work no bot in this room could ever take.
    ///
    /// Different question from [`Self::starved_roles`], which asks who is free
    /// *now* — a fair question for a board that wants to explain why something
    /// is not moving this minute, and a terrible one to notify on, because
    /// every eligible bot being mid-turn is ordinary and momentary. This asks
    /// whether anyone could cover it at all. A yes means a queue; a no means
    /// the roster does not match the work, and only a person can fix it.
    pub fn unstaffed_roles(&self) -> Vec<(String, u32)> {
        let now = Timestamp::now();
        let usable: Vec<&AgentSpec> = self
            .specs
            .values()
            .filter(|spec| self.status(spec.id).is_available())
            .collect();

        let mut missing: Vec<(String, u32)> = Vec::new();
        for task in self.tasks.values() {
            if !self.evaluate_task(task.id, now).eligible {
                continue;
            }
            let Some(role) = task.required_role.as_deref().filter(|r| !r.is_empty()) else {
                continue;
            };
            if usable.iter().any(|spec| {
                self.roles
                    .eligibility(&spec.roles, Some(role))
                    .is_eligible()
            }) {
                continue;
            }
            match missing.iter_mut().find(|(name, _)| name == role) {
                Some((_, count)) => *count += 1,
                None => missing.push((role.to_string(), 1)),
            }
        }
        missing.sort_by(|(a_role, a), (b_role, b)| b.cmp(a).then(a_role.cmp(b_role)));
        missing
    }

    pub fn starved_roles(&self) -> Vec<(String, u32)> {
        let free: Vec<&AgentSpec> = self
            .specs
            .values()
            .filter(|spec| self.status(spec.id).accepts_work() && self.workload(spec.id) == 0)
            .collect();

        let now = Timestamp::now();
        let mut starved: Vec<(String, u32)> = Vec::new();
        for task in self.tasks.values() {
            if !self.evaluate_task(task.id, now).eligible {
                continue;
            }
            let Some(role) = task.required_role.as_deref().filter(|r| !r.is_empty()) else {
                continue;
            };
            let covered = free.iter().any(|spec| {
                self.roles
                    .eligibility(&spec.roles, Some(role))
                    .is_eligible()
            });
            if covered {
                continue;
            }
            match starved.iter_mut().find(|(name, _)| name == role) {
                Some((_, count)) => *count += 1,
                None => starved.push((role.to_string(), 1)),
            }
        }
        starved.sort_by(|(a_role, a), (b_role, b)| b.cmp(a).then(a_role.cmp(b_role)));
        starved
    }

    /// Bots with nothing in hand, least loaded first.
    pub fn idle_agents(&self) -> Vec<AgentId> {
        let mut idle: Vec<AgentId> = self
            .specs
            .values()
            .filter(|spec| self.status(spec.id) != AgentStatus::Offline)
            .filter(|spec| self.workload(spec.id) == 0)
            .map(|spec| spec.id)
            .collect();
        idle.sort();
        idle
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
    /// Which objective a new task ultimately serves.
    ///
    /// Walks up whatever link the creator gave — a parent, or the task a review
    /// or remediation is about — until it reaches something declared an
    /// objective, and inherits that. A record created with no links serves
    /// itself, which is what makes an objective an objective.
    fn root_objective_for(&self, task: &Task) -> Option<TaskId> {
        if task.kind.is_headline() {
            return None;
        }
        let mut current = task.lineage.parent.or_else(|| task.lineage.subject())?;
        for _ in 0..32 {
            let record = self.tasks.get(&current)?;
            if record.kind.is_headline() {
                return Some(current);
            }
            // Inherit an already-resolved answer rather than walking further.
            if let Some(root) = record.lineage.root_objective {
                return Some(root);
            }
            match record.lineage.parent.or_else(|| record.lineage.subject()) {
                Some(next) if next != current => current = next,
                _ => return None,
            }
        }
        None
    }

    /// Follow a supersession chain to the task that now carries the work.
    ///
    /// Five downstream tasks depending on one replaced upstream should all
    /// end up pointing at its replacement, so the board shows one blocker
    /// rather than five stale ones. Bounded, because a corrupt chain must not
    /// hang the scheduler.
    fn follow_supersession(&self, start: TaskId) -> TaskId {
        let mut task = start;
        for _ in 0..32 {
            match self.tasks.get(&task).and_then(|t| t.lineage.superseded_by) {
                Some(next) if next != task => task = next,
                _ => break,
            }
        }
        task
    }

    /// The first dependency of `task` that has not landed, if any.
    ///
    /// One implementation, because there used to be two. This one walks the
    /// `depends_on` edges; the other read the `dependency` field the first one
    /// wrote. They agreed except on a dependency naming a task that is not in
    /// the room, which the projection skipped as satisfied and the claim gate
    /// refused — so such a task advertised itself as ready in every prompt and
    /// refused every claim.
    ///
    /// Returns the upstream and whether it exists. A dependency on a task that
    /// is not here can never be satisfied, so it is a blocker, not a nothing.
    fn first_unmet_dependency(&self, task: &Task) -> Option<UnmetDependency> {
        for dep in &task.depends_on {
            let resolved = self.follow_supersession(*dep);
            match self.tasks.get(&resolved) {
                Some(upstream) if upstream.state == TaskState::Submitted => continue,
                Some(upstream) => {
                    return Some(UnmetDependency {
                        task: resolved,
                        title: upstream.title.clone(),
                        owner: upstream.assignee,
                        present: true,
                    });
                }
                None => {
                    return Some(UnmetDependency {
                        task: resolved,
                        title: format!("{resolved}, which is not in this room"),
                        owner: None,
                        present: false,
                    });
                }
            }
        }
        None
    }

    /// Describe `task`'s first unmet dependency, or that it has none.
    fn dependency_state_of(&self, task: &Task) -> DependencyState {
        match self.first_unmet_dependency(task) {
            Some(unmet) => DependencyState::Waiting {
                on: unmet.task,
                // The upstream's own title, so the board can say what is being
                // waited on instead of printing `awaiting_task`.
                reason: unmet.title,
                // Who is expected to clear it, so nobody has to ask.
                owner: unmet.owner,
            },
            None => DependencyState::Satisfied,
        }
    }

    /// Recompute every task's dependency state from the current graph.
    ///
    /// Run after anything that changes a task, so downstream work becomes
    /// eligible the moment its upstream lands rather than waiting for a
    /// coordinator to notice and say so.
    fn recompute_dependencies(&mut self) {
        let updates: Vec<(TaskId, DependencyState)> = self
            .tasks
            .values()
            .map(|task| (task.id, self.dependency_state_of(task)))
            .collect();
        for (id, state) in updates {
            if let Some(task) = self.tasks.get_mut(&id) {
                if task.dependency != state {
                    task.dependency = state;
                }
            }
        }
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
                    BlockReason::AwaitingHuman { .. }
                    | BlockReason::AwaitingDecision { .. }
                    | BlockReason::Other { .. } => {}
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
        // Which task, if any, this command is a sign of life on.
        let touched = command.task();
        let outcome = match command {
            Command::JoinAgent { spec } => self.join_agent(*spec),
            Command::LeaveAgent { agent, reason } => self.leave_agent(agent, reason),
            Command::StartRoom => self.set_phase(RoomPhase::Running, "started"),
            Command::PauseRoom { reason } => self.set_phase(RoomPhase::Paused, &reason),
            Command::ResumeRoom => {
                // An explicit resume grants another execution budget. The journal
                // retains the original creation time and previous work.
                if self.room.is_out_of_time(Timestamp::now()) {
                    self.room.created_at = Timestamp::now();
                }
                if self.room.is_out_of_turns() {
                    self.room.turns_used = 0;
                }
                let mut events = self.set_phase(RoomPhase::Running, "resumed")?;
                // Resuming is the owner's retry action. Released work stays on
                // the board; no tool operation or old bot response is replayed.
                let faulted: Vec<_> = self
                    .statuses
                    .iter()
                    .filter_map(|(id, status)| (*status == AgentStatus::Faulted).then_some(*id))
                    .collect();
                for agent in faulted {
                    self.error_streak.insert(agent, 0);
                    events.extend(self.set_status(agent, AgentStatus::Idle));
                }
                Ok(events)
            }
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
                origin,
            } => self.create_task(
                title,
                brief,
                by,
                required_role,
                priority,
                acceptance_criteria,
                depends_on,
                origin,
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
                authority_bearing,
            } => self.add_artifact(task, author, name, body, authority_bearing),
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

            Command::SupersedeTask {
                task,
                replacement,
                reason,
            } => self.supersede_task(task, replacement, reason),
            Command::RetireTask { task, reason } => self.retire_task(task, reason),
            Command::SetAuthorization {
                task,
                authorization,
            } => self.set_authorization(task, authorization),

            Command::AskOwner {
                by,
                task,
                question,
                options,
            } => self.ask_owner(by, task, question, options),

            Command::AnswerDecision { decision, answer } => self.answer_decision(decision, answer),

            Command::RecordTurn { agent, usage } => self.record_turn(agent, usage),
            Command::ReportError {
                agent,
                task,
                error,
                permanent,
            } => self.report_error(agent, task, error, permanent),
            Command::Tick { now } => self.tick(now),
        };
        // Any command may have moved a task, and a task moving can free
        // work downstream. Recomputing here means a dependent becomes
        // eligible the moment its upstream lands, with no coordinator
        // needed to notice and announce it.
        if outcome.is_ok() {
            // Acting on a task is the heartbeat that holds its lease. A bot
            // working steadily never has its claim reclaimed; one that has
            // stopped does.
            if let Some(task) = touched {
                self.renew_lease(task);
            }
            self.recompute_dependencies();
        }
        outcome
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
        // A new human request continues a completed conversation. An explicit
        // pause or abort still requires the owner to resume it.
        let mut events = if self.room.phase == RoomPhase::Completed
            && matches!(message.author, orgbot_proto::chat::Author::Human { .. })
            && message.intent.expects_reply()
        {
            self.apply(Command::ResumeRoom)?
        } else {
            vec![]
        };
        self.transcript.push(message.clone());
        events.push(self.emit(Event::MessagePosted {
            message: Box::new(message),
        }));
        Ok(events)
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
        origin: TaskOrigin,
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
        // One review finding must not spawn an open-ended chain. Past a
        // handful of open remediations on one task, adding another is not
        // progress — consolidating them is.
        if let Some(subject) = origin.lineage.remediation_of {
            let open = self.remediation_count(subject);
            if open >= MAX_OPEN_REMEDIATIONS {
                return Err(EngineError::NotReady {
                    task: subject,
                    reason: format!(
                        "it already has {open} open remediations. Fold this into \
                         one of them, or raise a single remediation that covers \
                         the invariant they share."
                    ),
                });
            }
        }
        let mut task = Task::draft(self.room.id, title, brief);
        task.required_role = required_role;
        task.priority = priority;
        task.acceptance_criteria = acceptance_criteria;
        task.depends_on = depends_on;
        task.kind = origin.kind;
        task.lineage = origin.lineage;
        // The board counts progress against objectives, so every record has to
        // know which one it serves. Working it out here means a bot only has to
        // say what a record is about, not trace the chain itself.
        task.lineage.root_objective = self.root_objective_for(&task);
        // `parent` stays in step with the lineage so nothing that already reads
        // it has to learn about the new field.
        task.parent = task.lineage.parent;
        self.tasks.insert(task.id, task.clone());
        Ok(vec![self.emit(Event::TaskCreated {
            task: Box::new(task),
        })])
    }

    /// Replace one task with another, permanently.
    ///
    /// The link is written before the transition so that the moment the state
    /// says `superseded`, the record already names what replaced it — nothing
    /// downstream ever sees a replaced task with no replacement to follow.
    fn supersede_task(
        &mut self,
        task: TaskId,
        replacement: TaskId,
        reason: String,
    ) -> Result<Vec<EventEnvelope>> {
        if task == replacement {
            return Err(EngineError::NotReady {
                task,
                reason: "a task cannot supersede itself".into(),
            });
        }
        if !self.tasks.contains_key(&replacement) {
            return Err(EngineError::UnknownTask { task: replacement });
        }
        // Refuse a cycle: following the replacement must not lead back here.
        if self.follow_supersession(replacement) == task {
            return Err(EngineError::NotReady {
                task,
                reason: format!("{replacement} is already superseded by it"),
            });
        }
        let record = self
            .tasks
            .get_mut(&task)
            .ok_or(EngineError::UnknownTask { task })?;
        record.lineage.superseded_by = Some(replacement);
        if let Some(other) = self.tasks.get_mut(&replacement) {
            other.lineage.supersedes = Some(task);
        }
        self.transition(
            task,
            TaskTransition::Supersede {
                by: replacement,
                reason,
            },
            None,
        )
    }

    fn retire_task(&mut self, task: TaskId, reason: String) -> Result<Vec<EventEnvelope>> {
        self.require_task(task)?;
        self.transition(task, TaskTransition::Retire { reason }, None)
    }

    /// Put an authorization hold on a task, or lift one.
    fn set_authorization(
        &mut self,
        task: TaskId,
        authorization: AuthorizationState,
    ) -> Result<Vec<EventEnvelope>> {
        // An operator gate must name a decision only a person can make.
        // Refusing here is what stops the room routing its own gaps upward.
        if let AuthorizationState::AwaitingOperator { because, options } = &authorization {
            orgbot_proto::graph::validate_escalation(because).map_err(|e| {
                EngineError::NotReady {
                    task,
                    reason: e.to_string(),
                }
            })?;

            // Has this already been asked? A room that puts the same question
            // twice is the single most tiring thing about running one.
            let mut decision = Decision::ask(self.room.id, because.clone());
            decision.options = options.clone();
            decision.task = Some(task);
            if let Some(assignee) = self.require_task(task)?.assignee {
                decision.asked_by.push(assignee);
            }
            // Record which rules were in force. Old work should be read
            // against the rules that governed it, not the ones adopted since.
            decision.governed_by = self.governance.stamp();
            let raised = decision.clone();
            match self.decisions.ask(decision) {
                Settled::Asked(_) => {
                    let record = self
                        .tasks
                        .get_mut(&task)
                        .ok_or(EngineError::UnknownTask { task })?;
                    record.authorization = authorization;
                    record.updated_at = Timestamp::now();
                    return Ok(vec![self.emit(Event::DecisionAsked {
                        decision: Box::new(raised),
                    })]);
                }
                // Already answered: apply it rather than asking again.
                Settled::Already(prior) => {
                    let record = self
                        .tasks
                        .get_mut(&task)
                        .ok_or(EngineError::UnknownTask { task })?;
                    record.authorization = AuthorizationState::Authorized;
                    record.updated_at = Timestamp::now();
                    return Ok(vec![self.emit(Event::TaskAuthorized {
                        task,
                        decision: prior.id,
                    })]);
                }
                // Already waiting on the same question; leave the one prompt
                // standing rather than raising a second.
                Settled::AlreadyAsked(_) => {}
            }
        }
        // A hold for want of proof is a request for work, not a wall. Raise
        // the evidence task here rather than leaving the room to notice that
        // it could go and find out.
        let evidence = match &authorization {
            AuthorizationState::SecurityHold { finding, material } if !material => {
                Some(finding.clone())
            }
            _ => None,
        };

        let record = self
            .tasks
            .get_mut(&task)
            .ok_or(EngineError::UnknownTask { task })?;
        record.authorization = authorization;
        record.updated_at = Timestamp::now();

        match evidence {
            Some(finding) => self.raise_evidence_for(task, &finding),
            None => Ok(Vec::new()),
        }
    }

    fn ask_owner(
        &mut self,
        by: AgentId,
        task: Option<TaskId>,
        question: String,
        options: Vec<String>,
    ) -> Result<Vec<EventEnvelope>> {
        self.require_agent(by)?;
        let question = question.trim();
        if question.is_empty() || question.chars().count() > 2_000 {
            return Err(EngineError::InvalidOwnerQuestion {
                reason: "question must contain 1–2000 characters".into(),
            });
        }
        let options: Vec<String> = options
            .into_iter()
            .map(|option| option.trim().to_string())
            .collect();
        if !options.is_empty() {
            if !(2..=6).contains(&options.len()) {
                return Err(EngineError::InvalidOwnerQuestion {
                    reason: "provide between 2 and 6 options, or none for a free-form answer".into(),
                });
            }
            let mut seen = BTreeSet::new();
            for option in &options {
                let normalized = option.trim().to_lowercase();
                if normalized.is_empty() || normalized.chars().count() > 120 {
                    return Err(EngineError::InvalidOwnerQuestion {
                        reason: "each option must contain 1–120 characters".into(),
                    });
                }
                if !seen.insert(normalized) {
                    return Err(EngineError::InvalidOwnerQuestion {
                        reason: "options must be distinct".into(),
                    });
                }
            }
        }
        if let Some(task) = task {
            self.require_assignee(task, by)?;
            let record = self.require_task(task)?;
            let transition = TaskTransition::Block {
                reason: BlockReason::AwaitingDecision {
                    decision: DecisionId::new(),
                },
            };
            if !record.state.can_apply(&transition) {
                return Err(EngineError::NotReady {
                    task,
                    reason: format!("cannot ask while task is {}", record.state.as_str()),
                });
            }
        }

        let mut proposed = Decision::ask(self.room.id, question);
        proposed.owner_question = true;
        proposed.options = options;
        proposed.task = task;
        proposed.asked_by.push(by);
        proposed.governed_by = self.governance.stamp();
        match self.decisions.ask(proposed) {
            Settled::Asked(id) => {
                let decision = self
                    .decisions
                    .decisions
                    .iter()
                    .find(|decision| decision.id == id)
                    .expect("new decision is in the log")
                    .clone();
                let mut events = vec![self.emit(Event::DecisionAsked {
                    decision: Box::new(decision),
                })];
                if let Some(task) = task {
                    events.extend(self.transition(
                        task,
                        TaskTransition::Block {
                            reason: BlockReason::AwaitingDecision { decision: id },
                        },
                        Some(by),
                    )?);
                }
                Ok(events)
            }
            Settled::AlreadyAsked(prior) => {
                let prior_id = prior.id;
                let mut joined = false;
                if let Some(stored) = self
                    .decisions
                    .decisions
                    .iter_mut()
                    .find(|decision| decision.id == prior_id)
                {
                    if !stored.asked_by.contains(&by) {
                        stored.asked_by.push(by);
                        joined = true;
                    }
                    if stored.task.is_none() && task.is_some() {
                        stored.task = task;
                        joined = true;
                    }
                }
                let mut events = if joined {
                    vec![self.emit(Event::DecisionQuestionJoined {
                        decision: prior_id,
                        by,
                        task,
                    })]
                } else {
                    Vec::new()
                };
                if let Some(task) = task {
                    events.extend(self.transition(
                        task,
                        TaskTransition::Block {
                            reason: BlockReason::AwaitingDecision {
                                decision: prior_id,
                            },
                        },
                        Some(by),
                    )?);
                }
                Ok(events)
            }
            Settled::Already(prior) => {
                let answer = prior.answer.expect("settled decision has an answer");
                let mut body = format!("The owner previously answered: {}", answer.chosen);
                if !answer.note.is_empty() {
                    body.push_str(" — ");
                    body.push_str(&answer.note);
                }
                let message = ChatMessage::human(self.room.id, "Owner", body)
                    .to_agents(vec![by])
                    .with_intent(orgbot_proto::chat::MessageIntent::Answer);
                self.post_message(message)
            }
        }
    }

    /// Raise the work that would settle a security hold, once.
    ///
    /// Attached to the task it unblocks rather than created as a peer, so
    /// gathering proof does not add another top-level card. Raised once: a
    /// hold restated in different words should not produce a second identical
    /// errand.
    fn raise_evidence_for(&mut self, task: TaskId, finding: &str) -> Result<Vec<EventEnvelope>> {
        let title = format!("Evidence: {finding}");
        let already = self.tasks.values().any(|record| {
            record.lineage.parent == Some(task)
                && record.kind == TaskKind::Evidence
                && !record.state.is_terminal()
                && text::looks_like_the_same_work(&record.title, &title)
        });
        if already {
            return Ok(Vec::new());
        }

        let required_role = self
            .tasks
            .get(&task)
            .and_then(|record| record.required_role.clone());
        let events = self.create_task(
            title,
            format!(
                "Security is holding `{}` for want of proof: {finding}. Gather \
                 what is needed and attach it, then the review can be run \
                 again. This is the room's own work; nobody outside it needs \
                 to be asked.",
                self.tasks
                    .get(&task)
                    .map_or_else(String::new, |record| record.title.clone())
            ),
            None,
            required_role,
            Priority::High,
            vec!["the evidence is attached as an artifact".into()],
            Vec::new(),
            TaskOrigin::under(TaskKind::Evidence, task),
        )?;
        let raised = events.iter().find_map(|envelope| match &envelope.event {
            Event::TaskCreated { task } => Some(task.id),
            _ => None,
        });
        // Open it, or it sits as a draft nobody can claim.
        let mut events = events;
        if let Some(id) = raised {
            events.extend(self.transition(id, TaskTransition::Open, None)?);
        }
        Ok(events)
    }

    /// Record the operator's answer, and release whatever it unblocks.
    ///
    /// One answer clears every task that was waiting on that question, which is
    /// the point of holding decisions separately from tasks: the person answers
    /// once, not once per card.
    fn answer_decision(
        &mut self,
        decision: DecisionId,
        answer: Answer,
    ) -> Result<Vec<EventEnvelope>> {
        if answer.chosen.trim().is_empty() || answer.chosen.chars().count() > 120 {
            return Err(EngineError::InvalidOwnerAnswer {
                reason: "answer must contain 1–120 characters".into(),
            });
        }
        if answer.note.chars().count() > 2_000 {
            return Err(EngineError::InvalidOwnerAnswer {
                reason: "custom answer text must be at most 2000 characters".into(),
            });
        }
        if self
            .decisions
            .decisions
            .iter()
            .any(|item| item.id == decision && item.answer.is_some())
        {
            // A retry after a lost acknowledgement must not release or wake
            // the same participants twice.
            return Ok(Vec::new());
        }
        if !self.decisions.answer(decision, answer.clone()) {
            return Err(EngineError::NotReady {
                task: TaskId::new(),
                reason: "that decision is not waiting for an answer".into(),
            });
        }
        let Some(settled) = self
            .decisions
            .decisions
            .iter()
            .find(|d| d.id == decision)
            .cloned()
        else {
            return Ok(Vec::new());
        };

        let mut events = vec![self.emit(Event::DecisionAnswered {
            decision,
            answer: answer.clone(),
        })];

        if !settled.asked_by.is_empty() {
            let mut body = format!("The owner answered: {}", answer.chosen);
            if !answer.note.trim().is_empty() {
                body.push_str(" — ");
                body.push_str(answer.note.trim());
            }
            let reply = ChatMessage::human(self.room.id, "Owner", body)
                .to_agents(settled.asked_by.clone())
                .with_intent(orgbot_proto::chat::MessageIntent::Answer);
            events.extend(self.post_message(reply)?);
        }

        // Release every task held by this exact question.
        let held_authorizations: Vec<TaskId> = self
            .tasks
            .values()
            .filter(|task| match &task.authorization {
                AuthorizationState::AwaitingOperator { because, .. } => {
                    let mut probe = Decision::ask(self.room.id, because.clone());
                    probe.envelope = settled.envelope.clone();
                    probe.options = settled.options.clone();
                    probe.fingerprint() == settled.fingerprint()
                }
                _ => false,
            })
            .map(|task| task.id)
            .collect();

        for task in held_authorizations {
            if let Some(record) = self.tasks.get_mut(&task) {
                record.authorization = AuthorizationState::Authorized;
                record.updated_at = Timestamp::now();
            }
            events.push(self.emit(Event::TaskAuthorized { task, decision }));
        }

        let held_questions: Vec<TaskId> = self
            .tasks
            .values()
            .filter(|task| {
                task.state == TaskState::Blocked
                    && task.blocked_on.iter().any(|reason| {
                        matches!(reason, BlockReason::AwaitingDecision { decision: held } if *held == decision)
                    })
            })
            .map(|task| task.id)
            .collect();
        for task in held_questions {
            events.extend(self.transition(task, TaskTransition::Unblock, None)?);
        }
        Ok(events)
    }

    fn claim_task(&mut self, task: TaskId, by: AgentId) -> Result<Vec<EventEnvelope>> {
        let spec = self.require_agent(by)?.clone();
        self.require_task(task)?;

        // The same evaluation that decided to advertise this, run again now.
        // A bot acts on a prompt built minutes earlier, and by now the board
        // may have moved — so what was advertised is a reading, and this is
        // the decision. Refusing here with the reason code says which of the
        // two happened, which is the difference between "somebody beat me to
        // it" and "the room told me something false".
        let verdict = self.evaluate_claim(task, by, Timestamp::now());
        if !verdict.eligible {
            return Err(self.refusal(task, by, verdict));
        }
        let fit = self.roles.eligibility(
            &spec.roles,
            self.require_task(task)?.required_role.as_deref(),
        );
        let events = self.transition(task, TaskTransition::Claim { by }, Some(by))?;
        if let Some(record) = self.tasks.get_mut(&task) {
            // A claim is a lease, not a deed. Without one, a bot that crashes
            // holds the work for the rest of the run and the board says only
            // that it is assigned.
            record.lease_expires_at = Some(Timestamp::now().plus_millis(DEFAULT_LEASE_MILLIS));
            record.stood_in = fit.is_stand_in();
        }
        Ok(events)
    }

    fn start_task(&mut self, task: TaskId, by: AgentId) -> Result<Vec<EventEnvelope>> {
        self.require_assignee(task, by)?;
        let record = self.require_task(task)?;
        if let Some(unmet) = self.first_unmet_dependency(record) {
            return Err(EngineError::DependencyNotMet {
                task,
                blocker: unmet.task,
            });
        }
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
        self.require_review_only_assignment(task, by)?;
        for artifact in &artifacts {
            if !self.artifacts.contains_key(artifact) {
                return Err(EngineError::UnknownArtifact {
                    artifact: *artifact,
                });
            }
        }
        // Freeze what is being inspected. An artifact that changes under the
        // inspector means the verdict describes something that no longer
        // exists, and neither party can tell.
        for artifact in &artifacts {
            if let Some(record) = self.artifacts.get_mut(artifact) {
                record.sealed = true;
            }
        }
        self.transition(
            task,
            TaskTransition::SubmitForReview { artifacts },
            Some(by),
        )
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
        self.require_assignee(task, from)?;
        let state = self.require_task(task)?.state;
        if !matches!(
            state,
            TaskState::Claimed | TaskState::InProgress | TaskState::Blocked
        ) {
            return Err(EngineError::NotPermitted {
                agent: from,
                action: "hand off work that is awaiting review or already finished".into(),
            });
        }

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
        if blocks
            && self
                .tasks
                .get(&task)
                .is_some_and(|t| t.state == TaskState::InProgress)
        {
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
        let recipient = self.require_agent(by)?;
        let roles = recipient.roles.clone();
        let offer = self
            .handoffs
            .get(&handoff)
            .ok_or(EngineError::UnknownHandoff { handoff })?;
        if offer.kind.transfers_ownership() {
            self.require_assignee(offer.task, offer.from)?;
            let task = self.require_task(offer.task)?;
            if !matches!(
                task.state,
                TaskState::Claimed | TaskState::InProgress | TaskState::Blocked
            ) {
                return Err(EngineError::NotPermitted {
                    agent: by,
                    action: "accept a delegation after the work entered review or finished".into(),
                });
            }
            if recipient.autonomy.review_only
                && !review_only_task(task.kind, task.required_role.as_deref())
            {
                return Err(EngineError::NotPermitted {
                    agent: by,
                    action: "accept a delegation outside review-only scope".into(),
                });
            }
            if !recipient.can_take(task.required_role.as_deref()) {
                return Err(EngineError::RoleMismatch {
                    task: task.id,
                    agent: by,
                    required: task.required_role.clone().unwrap_or_default(),
                });
            }
        }
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
                record.blocked_on.retain(
                    |r| !matches!(r, BlockReason::AwaitingHandoff { handoff: h } if *h == handoff),
                );
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
        authority_bearing: bool,
    ) -> Result<Vec<EventEnvelope>> {
        self.require_agent(author)?;
        self.require_review_only_assignment(task, author)?;
        let revision = self.require_task(task)?.revision;
        let mut artifact = Artifact::new(task, author, name, body);
        artifact.revision = revision;
        artifact.room = Some(self.room.id);
        artifact.authority_bearing = authority_bearing;
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

    /// Review-only bots can report and submit work only on a task they were
    /// allowed to claim as review work. The claim gate is shared, but these
    /// mutation commands also need their own check because callers can issue
    /// engine commands without going through the tool list.
    fn require_review_only_assignment(&self, task: TaskId, agent: AgentId) -> Result<()> {
        let spec = self.require_agent(agent)?;
        if !spec.autonomy.review_only {
            return Ok(());
        }
        let record = self.require_task(task)?;
        if record.assignee != Some(agent)
            || !review_only_task(record.kind, record.required_role.as_deref())
        {
            return Err(EngineError::NotPermitted {
                agent,
                action: "report work outside an assigned review task".into(),
            });
        }
        Ok(())
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
        // Validate before recording anything, including a critical rejection.
        //
        // Say what happened rather than only what is disallowed. Inspectors
        // share one queue, so two of them can be woken for the same task; the
        // second arrives after the first has already ruled and been sent back,
        // and "cannot record_review a task that is in_progress" reads like the
        // board contradicting itself. It is a race that was lost, and the
        // difference matters to whoever has to decide what to do next.
        if record.state != TaskState::InReview {
            let previous = self.reviews.iter().rfind(|review| review.task == task);
            return Err(EngineError::NotUnderReview {
                task,
                state: record.state.as_str(),
                detail: match previous {
                    Some(review) => format!(
                        "{} already inspected it at round {} and it is {} now",
                        review.inspector.short(),
                        review.revision,
                        record.state.as_str()
                    ),
                    None => format!(
                        "it is {} and has not been submitted for inspection",
                        record.state.as_str()
                    ),
                },
            });
        }
        record
            .state
            .apply(&TaskTransition::Approve { by: inspector })?;
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
        let partial = convergence::looks_partial(&review.findings, self.review_cycles(task).len());
        self.reviews.push(review.clone());

        let mut events = vec![self.emit(Event::ReviewRecorded {
            review: Box::new(review),
        })];

        events.extend(match verdict {
            Verdict::Approve => self.transition(
                task,
                TaskTransition::Approve { by: inspector },
                Some(inspector),
            )?,
            Verdict::RequestChanges => {
                let mut sent_back = self.request_changes(task, inspector, reason)?;
                // Rework loops on its own from here: the owner remediates, the
                // reviewer rechecks, and it continues without anyone being
                // asked whether to carry on. What the room *does* need is to
                // notice when going round again is the wrong move.
                sent_back.extend(self.report_convergence(task, partial));
                sent_back
            }
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

    /// Say something when review has stopped being worth repeating.
    ///
    /// Posted into the room rather than raised to a person: a review going
    /// round in circles is the room's problem to solve, and the message says
    /// what to do about it. A partial-looking review is called out in the same
    /// place, because one small finding per cycle turns a single pass into
    /// five, each costing a turn from two bots.
    fn report_convergence(&mut self, task: TaskId, partial: bool) -> Vec<EventEnvelope> {
        let verdict = self.convergence(task);
        let mut notes: Vec<String> = Vec::new();
        if let Some(advice) = verdict.recommendation() {
            notes.push(advice);
        }
        if partial {
            notes.push(
                "This review reported a single blocking finding on work that had                  already been sent back. Return the full set of blocking findings                  you can see in one pass; each extra cycle costs two bots a turn."
                    .into(),
            );
        }
        if notes.is_empty() {
            return Vec::new();
        }

        let title = self
            .tasks
            .get(&task)
            .map_or_else(String::new, |t| t.title.clone());
        let room = self.room.id;
        let message = ChatMessage {
            id: orgbot_proto::MessageId::new(),
            room,
            author: orgbot_proto::chat::Author::System,
            audience: orgbot_proto::chat::Audience::Everyone,
            intent: orgbot_proto::chat::MessageIntent::Status,
            body: format!("Review of \"{title}\": {}", notes.join(" ")),
            task: Some(task),
            handoff: None,
            reply_to: None,
            attachments: vec![],
            files: vec![],
            sent_at: Timestamp::now(),
        };
        self.apply(Command::PostMessage {
            message: Box::new(message),
        })
        .unwrap_or_default()
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

    fn record_turn(&mut self, agent: AgentId, usage: TokenUsage) -> Result<Vec<EventEnvelope>> {
        self.require_agent(agent)?;
        self.room.turns_used = self.room.turns_used.saturating_add(1);
        self.spend.entry(agent).or_default().record(usage);
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
        permanent: bool,
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

        // A misconfiguration does not get three goes. The next two attempts
        // would fail identically, and until they do the bot sits in the roster
        // looking free while being unable to take a turn.
        if permanent || attempt >= self.room.spec.limits.max_consecutive_errors {
            events.extend(self.set_status(agent, AgentStatus::Faulted));
            // Hand back whatever the faulted bot was holding so the room can
            // continue without it.
            for held in self.active_tasks_of(agent) {
                if let Ok(released) = self.transition(
                    held,
                    TaskTransition::Release {
                        reason: if permanent {
                            format!("{} cannot run as configured", agent.short())
                        } else {
                            format!("{} faulted after {attempt} errors", agent.short())
                        },
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
            // Expiring is not enough on its own: the sender is still waiting,
            // and a handoff nobody accepted leaves the work inert. Returning
            // the task to the pool is what makes the timeout a reroute rather
            // than a dead end.
            events.extend(self.drive_handoff(handoff, HandoffTransition::Expire)?);
        }
        events.extend(self.clear_landed_dependencies()?);
        events.extend(self.reconcile_ownership(now)?);
        events.extend(self.report_health());
        self.retain_recent();
        events.extend(self.sweep_deadlocks());
        Ok(events)
    }

    /// Put ownership back in agreement with itself.
    ///
    /// A task's state says whether somebody is working on it; the holder and
    /// the claim say who and until when. Those three can disagree, and every
    /// way they can disagree used to be permanent — the sweep that returned
    /// abandoned work to the pool only looked at claims that had an expiry to
    /// compare against, so a task held with no expiry, or in a working state
    /// with no holder at all, was stuck for the life of the room. It was not
    /// even visible as stuck: a task in progress with nobody on it reads as
    /// "closed" to the board.
    ///
    /// Each case here is repaired only where the repair is determined. A task
    /// the state machine will not release is left alone for the consistency
    /// report rather than forced, and one bad task does not stop the sweep.
    fn reconcile_ownership(&mut self, now: Timestamp) -> Result<Vec<EventEnvelope>> {
        let mut events = Vec::new();

        // 1. Held, with no claim recorded. Rooms saved before claims carried an
        //    expiry come back this way. Granting one now rather than reclaiming
        //    immediately gives a holder that is still alive its full window;
        //    one that is gone loses the task a window later, through the
        //    ordinary path below.
        let unrecorded: Vec<TaskId> = self
            .tasks
            .values()
            .filter(|task| {
                !task.state.is_terminal()
                    && task.assignee.is_some()
                    && task.lease_expires_at.is_none()
            })
            .map(|task| task.id)
            .collect();
        for task in unrecorded {
            if let Some(record) = self.tasks.get_mut(&task) {
                record.lease_expires_at = Some(now.plus_millis(DEFAULT_LEASE_MILLIS));
            }
        }

        // 2. A claim on work nobody holds. Releasing clears both together now,
        //    but a room that ran before it did still carries the leftovers —
        //    and an inconsistency the room reports and cannot clear is worse
        //    than one it never noticed, because it teaches the operator that
        //    the report is noise.
        let unheld: Vec<TaskId> = self
            .tasks
            .values()
            .filter(|task| task.assignee.is_none() && task.lease_expires_at.is_some())
            .map(|task| task.id)
            .collect();
        for task in unheld {
            if let Some(record) = self.tasks.get_mut(&task) {
                record.lease_expires_at = None;
            }
        }

        // 3. A working state with nobody working it. Nothing is being
        //    interrupted, so this returns to the pool immediately.
        let ownerless: Vec<TaskId> = self
            .tasks
            .values()
            .filter(|task| task.state.is_assigned() && task.assignee.is_none())
            .map(|task| task.id)
            .collect();
        for task in ownerless {
            events.extend(self.try_release(
                task,
                "it was left in a working state with nobody holding it".into(),
            ));
        }

        // 4. Held by a bot that is no longer in the room.
        let orphaned: Vec<TaskId> = self
            .tasks
            .values()
            .filter(|task| {
                !task.state.is_terminal()
                    && task
                        .assignee
                        .is_some_and(|holder| !self.specs.contains_key(&holder))
            })
            .map(|task| task.id)
            .collect();
        for task in orphaned {
            events.extend(
                self.try_release(task, "the bot holding it is no longer in this room".into()),
            );
        }

        // 5. Held by a bot that has stopped reporting progress. A faulted bot
        //    cannot act, so it cannot renew, so it arrives here on its own.
        let stale: Vec<TaskId> = self
            .tasks
            .values()
            .filter(|task| task.lease_expired_at(now))
            .map(|task| task.id)
            .collect();
        for task in stale {
            let held_by = self
                .tasks
                .get(&task)
                .and_then(|t| t.assignee)
                .map(|a| a.short())
                .unwrap_or_default();
            events.extend(self.try_release(
                task,
                format!("the claim lapsed; {held_by} stopped reporting progress"),
            ));
        }
        Ok(events)
    }

    /// Keep what the room reads; let the record live where records live.
    ///
    /// A saved board grows in four places and only two of them are unbounded.
    /// The tasks, artifacts and reviews are the work itself and are kept whole
    /// — trimming a deliverable would be throwing away the thing the room was
    /// built to produce. The event log and the transcript are a *record*, and
    /// the full one is already on disk in the run logs, segmented and readable.
    /// What is worth keeping here is the recent part the room actually consults:
    /// a bot catching up on what it missed, and a client asking for a backlog.
    ///
    /// A four-day room reached thirty-three megabytes, of which eleven were the
    /// log and three the transcript, against a ceiling of sixty-four at which
    /// the file stops being readable at all. Bounding this is what stops that
    /// arriving.
    fn retain_recent(&mut self) {
        if self.log.len() > RETAINED_EVENTS {
            self.log.drain(..self.log.len() - RETAINED_EVENTS);
        }
        if self.transcript.len() > RETAINED_MESSAGES {
            self.transcript
                .drain(..self.transcript.len() - RETAINED_MESSAGES);
        }
    }

    /// Say so when the room's condition changes.
    ///
    /// Checked after reconciliation, so what is raised is what the room has
    /// already tried and failed to fix by itself — never something it was
    /// about to resolve on the next line.
    fn report_health(&mut self) -> Vec<EventEnvelope> {
        let health = self.health();
        let fingerprint = health.fingerprint();
        if fingerprint == self.last_health {
            return Vec::new();
        }
        self.last_health = fingerprint;
        vec![self.emit(Event::RoomHealthChanged {
            ok: health.is_ok(),
            summary: health.summary(),
            detail: health.report(),
        })]
    }

    /// Unblock work whose upstream task has landed.
    ///
    /// A task can be blocked on a handoff or on another task. When a handoff
    /// resolves, the tasks waiting on it are released. When a *task* lands,
    /// nothing happened at all — the wait was recorded and then nobody was
    /// watching for it to end. So a task blocked on work that finished stayed
    /// blocked, and the bot holding it stayed parked, for the rest of the
    /// room's life.
    ///
    /// Only a dependency that actually succeeded clears a wait. One that
    /// failed or was cancelled leaves the wait in place: that work is not
    /// going to arrive, and deciding what should happen instead is not
    /// something to guess at silently.
    fn clear_landed_dependencies(&mut self) -> Result<Vec<EventEnvelope>> {
        let landed: BTreeSet<TaskId> = self
            .tasks
            .values()
            .filter(|task| task.state == TaskState::Submitted)
            .map(|task| task.id)
            .collect();

        let waiting: Vec<TaskId> = self
            .tasks
            .values()
            .filter(|task| task.state == TaskState::Blocked)
            .filter(|task| {
                task.blocked_on.iter().any(
                    |reason| matches!(reason, BlockReason::AwaitingTask { task } if landed.contains(task)),
                )
            })
            .map(|task| task.id)
            .collect();

        let mut events = Vec::new();
        for task in waiting {
            // Drop just the waits that are over; a task can be waiting on
            // several things, and the others still hold.
            let Some(record) = self.tasks.get_mut(&task) else {
                continue;
            };
            record.blocked_on.retain(
                |reason| !matches!(reason, BlockReason::AwaitingTask { task } if landed.contains(task)),
            );
            if !record.blocked_on.is_empty() {
                continue;
            }
            match self.transition(task, TaskTransition::Unblock, None) {
                Ok(unblocked) => events.extend(unblocked),
                Err(error) => {
                    tracing::debug!(%task, %error, "could not unblock a task whose wait ended");
                }
            }
        }
        Ok(events)
    }

    /// Return one task to the pool, or leave it be if the board will not.
    ///
    /// Repair is best-effort by design: a state the transition table refuses
    /// to release is a state nobody has decided how to unwind, and forcing it
    /// would be inventing a decision. It stays for the consistency report,
    /// and the rest of the sweep carries on.
    fn try_release(&mut self, task: TaskId, reason: String) -> Vec<EventEnvelope> {
        match self.transition(task, TaskTransition::Release { reason }, None) {
            Ok(events) => {
                if let Some(record) = self.tasks.get_mut(&task) {
                    record.lease_expires_at = None;
                }
                events
            }
            Err(error) => {
                tracing::debug!(%task, %error, "ownership repair left this task alone");
                Vec::new()
            }
        }
    }

    /// Extend the holder's lease. Called whenever a bot acts on its task.
    fn renew_lease(&mut self, task: TaskId) {
        if let Some(record) = self.tasks.get_mut(&task) {
            if record.assignee.is_some() && !record.state.is_terminal() {
                record.lease_expires_at = Some(Timestamp::now().plus_millis(DEFAULT_LEASE_MILLIS));
            }
        }
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
            let withdrawable = blocking.and_then(|id| self.handoffs.get(&id).map(|h| (id, h.from)));

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
                // The claim goes with the holder. Clearing it only on the
                // paths that reclaim work left every bot-initiated release
                // holding a lease on a task nobody owned — harmless in itself,
                // since a claim with no holder expires against nothing, but a
                // standing invariant violation that the room now reports.
                record.lease_expires_at = None;
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
        // The holder changed, so this is a different holding of the task.
        // Nothing is fenced on this — the holder's identity is what every
        // mutation is checked against — but it is what lets a reader tell one
        // holding from the next when the same bot picks the task back up.
        if record.assignee != previous_holder {
            record.ownership_epoch = record.ownership_epoch.saturating_add(1);
        }
        record.state = to;
        record.updated_at = Timestamp::now();
        // Carried on the event so a reader needs nothing else to understand
        // it, and so a delivered objective can be told from a delivered
        // subtask without a second lookup.
        let (title, kind) = (record.title.clone(), record.kind);

        let mut events = vec![self.emit(Event::TaskTransitioned {
            title,
            kind,
            task,
            from,
            to,
            transition: Box::new(transition),
            actor,
        })];

        // Whoever held the task before and whoever holds it now may both have
        // changed availability.
        for agent in [previous_holder, actor]
            .into_iter()
            .flatten()
            .collect::<BTreeSet<_>>()
        {
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
        self.tasks
            .get(&task)
            .ok_or(EngineError::UnknownTask { task })
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

fn review_only_task(kind: TaskKind, required_role: Option<&str>) -> bool {
    kind == TaskKind::Review
        || matches!(
            required_role,
            Some("review" | "reviewer" | "audit" | "security")
        )
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

/// Events kept in the saved board.
///
/// Enough that a bot which has not acted for a long stretch still gets the
/// catch-up it is owed, and a client connecting mid-run gets a backlog worth
/// having. Everything older is in the run logs, which is where a person reads
/// history anyway.
const RETAINED_EVENTS: usize = 3_000;

/// Chat messages kept in the saved board.
const RETAINED_MESSAGES: usize = 1_000;
