//! Who acts next, and when the room is finished.
//!
//! Bots take turns in batches. A turn is almost entirely time spent waiting on
//! a model, so running them one at a time meant a room of twenty bots had
//! nineteen idle at every instant, and seating more of them could not make the
//! work finish sooner. `max_parallel_turns` sets how many act at once.
//!
//! Two bots acting on the same board at the same time is only safe because of
//! where the checking happens. What a bot is told in its prompt is a snapshot
//! from when the prompt was built, and by the time it acts, minutes later, the
//! board may have moved. So nothing trusts that snapshot: every command
//! revalidates when it applies, against the board as it is then. Claiming goes
//! through the task state machine, which refuses a task already claimed;
//! anything touching work in progress goes through `require_assignee`, which
//! refuses a task held by somebody else. A bot that loses a race is told the
//! task was taken and picks another — it cannot overwrite the winner's work.
//! See [`crate::shared`] for the lock that keeps single commands atomic.
//!
//! Scheduling then tries to avoid staging those races at all: when picking a
//! batch, work that has already been earmarked for a bot earlier in the batch
//! is not offered again, so free bots are woken for distinct tasks rather than
//! sent to compete for one. Losing a race is safe but it still costs a turn.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use orgbot_engine::{Command, Engine};
use orgbot_proto::event::Event;
use orgbot_proto::room::{CompletionRule, RoomPhase, RoomSpec};
use orgbot_proto::{AgentId, AgentSpec, AgentStatus, TaskId, Timestamp};
use orgbot_providers::{Provider, StreamEvent};
use tokio::sync::mpsc;

use crate::bot::{Bot, TurnReport};
use crate::shared::SharedEngine;

/// Why a run stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// The room's completion rule was satisfied.
    Completed,
    /// Nobody had anything to do, but the completion rule was not met.
    ///
    /// Usually means every remaining task is blocked on a human.
    Idle,
    /// The room spent its turn budget.
    OutOfTurns,
    /// The room ran past its wall-clock ceiling.
    OutOfTime,
    /// Bots kept taking turns without changing anything.
    Stalled {
        /// Consecutive turns that achieved nothing.
        rounds: u32,
    },
    /// A human paused the room.
    Paused,
}

impl RunOutcome {
    /// Whether the room delivered what it set out to.
    pub const fn is_success(&self) -> bool {
        matches!(self, Self::Completed)
    }
}

/// Drives a room.
#[derive(Debug)]
pub struct Orchestrator {
    engine: SharedEngine,
    bots: BTreeMap<AgentId, Bot>,
    /// Turn order. Kept separate from `bots` so scheduling is round-robin
    /// rather than by id, which would starve whoever sorts last.
    rotation: Vec<AgentId>,
    cursor: usize,
    /// Consecutive turns that changed nothing.
    idle_rounds: u32,
    /// The last thing each bot said, so a bot repeating itself verbatim does
    /// not read as progress and stall detection still fires.
    last_said: BTreeMap<AgentId, String>,
    /// The computer the room is connected to, kept so MCP servers installed on
    /// it can be run there — a phone cannot start a process, and the machine
    /// it is already talking to is usually where the servers live.
    workspace: Option<crate::workspace::Workspace>,
    allowances: Option<crate::allowance::Allowances>,
}

impl Orchestrator {
    /// Complete room state and bot conversations, without provider credentials.
    pub fn checkpoint(&self) -> serde_json::Value {
        let engine = self.engine.borrow();
        serde_json::json!({
            "version": 1, "engine": &*engine,
            "bots": self.bots.iter().map(|(id, bot)| (*id, bot.memory())).collect::<BTreeMap<_, _>>(),
            "cursor": self.cursor,
        })
    }

    /// Reopen saved work paused; providers come from the current account settings.
    pub fn restore(
        value: serde_json::Value,
        spec: RoomSpec,
        providers: Vec<(AgentSpec, Arc<dyn Provider>)>,
    ) -> Result<Self, String> {
        let version = value["version"]
            .as_u64()
            .ok_or("Missing room memory version")?;
        let mut engine = match version {
            0 => Engine::from_archive(
                serde_json::from_value(value["room"].clone()).map_err(|e| e.to_string())?,
                serde_json::from_value(value["events"].clone()).map_err(|e| e.to_string())?,
            )?,
            1 => serde_json::from_value::<Engine>(value["engine"].clone())
                .map_err(|e| e.to_string())?,
            _ => return Err("Room memory was written by a newer app".into()),
        };
        engine.configure_saved_room(spec);
        let configured_handles: BTreeSet<_> = providers
            .iter()
            .map(|(agent, _)| agent.handle.as_str())
            .collect();
        let removed: Vec<_> = engine
            .agents()
            .filter(|agent| !configured_handles.contains(agent.handle.as_str()))
            .map(|agent| agent.id)
            .collect();
        for agent in removed {
            engine
                .apply(Command::LeaveAgent {
                    agent,
                    reason: "removed from the configured team".into(),
                })
                .map_err(|error| error.to_string())?;
        }
        let memory: BTreeMap<AgentId, crate::bot::BotMemory> = if version == 1 {
            serde_json::from_value(value["bots"].clone()).map_err(|e| e.to_string())?
        } else {
            BTreeMap::new()
        };
        let mut restored = Self {
            engine: SharedEngine::new(engine),
            bots: BTreeMap::new(),
            rotation: vec![],
            cursor: value["cursor"].as_u64().unwrap_or(0) as usize,
            idle_rounds: 0,
            last_said: BTreeMap::new(),
            workspace: None,
            allowances: None,
        };
        for (mut configured, provider) in providers {
            let saved = restored
                .engine
                .with(|engine| engine.agent_by_handle(&configured.handle).cloned());
            if let Some(saved) = saved {
                configured.id = saved.id;
                configured.created_at = saved.created_at;
                let same_provider = saved.provider == configured.provider;
                restored
                    .engine
                    .with(|engine| engine.update_agent(configured.clone()));
                let mut bot = Bot::new(configured.clone(), provider);
                if same_provider {
                    if let Some(memory) = memory.get(&configured.id) {
                        bot.restore_memory(memory.clone());
                    }
                }
                if let Some((rx, socket)) = spawn_mcp_host(&configured) {
                    bot.attach_mcp(rx, socket);
                }
                restored.rotation.push(configured.id);
                restored.bots.insert(configured.id, bot);
            } else {
                restored
                    .seat(configured, provider)
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(restored)
    }
    /// Open a room.
    pub fn new(spec: RoomSpec) -> Self {
        Self {
            engine: SharedEngine::new(Engine::new(spec)),
            bots: BTreeMap::new(),
            rotation: Vec::new(),
            cursor: 0,
            idle_rounds: 0,
            last_said: BTreeMap::new(),
            workspace: None,
            allowances: None,
        }
    }

    /// Seat a bot.
    pub fn seat(
        &mut self,
        spec: AgentSpec,
        provider: Arc<dyn Provider>,
    ) -> Result<AgentId, orgbot_engine::EngineError> {
        let id = spec.id;
        self.engine.with(|engine| {
            engine.apply(Command::JoinAgent {
                spec: Box::new(spec.clone()),
            })
        })?;
        let mut bot = Bot::new(spec.clone(), provider);
        if let Some(allowances) = &self.allowances {
            bot.attach_allowances(allowances.clone());
        }
        if let Some((rx, socket)) = spawn_mcp_host(&spec) {
            bot.attach_mcp(rx, socket);
        }
        self.bots.insert(id, bot);
        self.rotation.push(id);
        Ok(id)
    }

    /// Give every bot the room's MCP servers.
    ///
    /// One client shared by the whole roster: a stdio server is one process
    /// with one pipe, and twenty bots each spawning their own copy of it would
    /// be twenty processes racing over the same database.
    pub fn attach_mcp_servers(&mut self, servers: crate::mcp_client::McpClient) {
        let shared = Arc::new(servers);
        for bot in self.bots.values_mut() {
            bot.attach_mcp_servers(shared.clone());
        }
    }

    pub fn attach_workspace(&mut self, workspace: crate::workspace::Workspace) {
        for bot in self.bots.values_mut() {
            bot.attach_workspace(workspace.clone());
        }
        self.workspace = Some(workspace);
    }

    pub fn attach_allowances(&mut self, allowances: crate::allowance::Allowances) {
        for bot in self.bots.values_mut() {
            bot.attach_allowances(allowances.clone());
        }
        self.allowances = Some(allowances);
    }

    /// The computer this room is connected to, if any.
    pub fn workspace(&self) -> Option<&crate::workspace::Workspace> {
        self.workspace.as_ref()
    }

    pub fn attach_attachments(&mut self, root: std::path::PathBuf) {
        for bot in self.bots.values_mut() {
            bot.attachments_root = Some(root.clone());
        }
    }

    /// Yield API waits to human input without interrupting workspace operations.
    pub fn attach_input_interrupt(&mut self, token: tokio_util::sync::CancellationToken) {
        for bot in self.bots.values_mut() {
            bot.input_interrupt = token.clone();
        }
    }

    /// The room engine, for inspection.
    ///
    /// Borrows it until the returned guard drops. Every bot taking a turn
    /// waits on this, so read what you need and let it go; never hold it
    /// across an `.await`.
    pub fn engine(&self) -> impl std::ops::Deref<Target = Engine> + use<'_> {
        self.engine.borrow()
    }

    /// The room engine, for a human issuing commands mid-run.
    ///
    /// An operator's command is applied through the same lock as a bot's, so
    /// it lands between turns rather than in the middle of one.
    pub fn engine_mut(&mut self) -> impl std::ops::DerefMut<Target = Engine> + use<'_> {
        self.engine.borrow_mut()
    }

    /// A handle to the shared engine, for code that runs alongside turns.
    pub fn shared_engine(&self) -> SharedEngine {
        self.engine.clone()
    }

    /// Move the room into the running phase.
    pub fn start(&mut self) -> Result<(), orgbot_engine::EngineError> {
        self.engine
            .with(|engine| engine.apply(Command::StartRoom))?;
        Ok(())
    }

    /// Why a bot should be woken, if it should be.
    ///
    /// Ordered by how much the room gains: answering a handoff unblocks
    /// another bot, so it outranks starting fresh work.
    ///
    /// This says *why* rather than just yes, because batching needs to know.
    /// Some reasons are the bot's own business — its held work, a question
    /// addressed to it — and several bots can act on them at once without ever
    /// meeting. Others point at a shared pool, and waking two bots for the
    /// same item in that pool means one of them wastes a turn discovering the
    /// other got there first.
    fn wake_reason(&self, engine: &Engine, agent: AgentId) -> Option<Wake> {
        let status = engine.status(agent);
        if matches!(status, AgentStatus::Offline | AgentStatus::Faulted) {
            return None;
        }
        // Someone is blocked on this bot's answer.
        if !engine.offers_for(agent).is_empty() {
            return Some(Wake::Handoff);
        }
        // A question addressed to this bot that it has not answered yet. This
        // is what lets a human (or another bot) talk to the room and get a
        // reply, rather than the message landing in the log unheard.
        if self.has_unanswered_message(engine, agent) {
            return Some(Wake::Question);
        }
        // Work in hand that is not parked.
        let held = engine.active_tasks_of(agent);
        let has_live_work = held.iter().any(|id| {
            engine
                .task(*id)
                .is_some_and(|task| !task.state.is_waiting())
        });
        if has_live_work {
            return Some(Wake::HeldWork);
        }
        // Something to inspect.
        let queue = engine.review_queue_for(agent);
        if !queue.is_empty() {
            return Some(Wake::Review { queue });
        }
        // Approved work waiting to be delivered. A coordinator holds no tasks
        // of its own, so this is the only thing that wakes it at the end.
        if engine
            .agent(agent)
            .is_some_and(|spec| spec.autonomy.may_submit)
            && !engine.awaiting_delivery().is_empty()
        {
            return Some(Wake::Delivery);
        }
        // An empty board is a coordinator's cue to plan. Without this nobody is
        // actionable at the start of a room and it idles before it begins.
        if engine
            .agent(agent)
            .is_some_and(|spec| spec.autonomy.may_create_tasks)
            && engine.tasks().next().is_none()
        {
            return Some(Wake::Planning);
        }
        // Free, with work available.
        //
        // Reaching here means nothing this bot holds is live. Holding nothing
        // is the ordinary case. The other one is a bot holding only *blocked*
        // work, and it used to be a trap: never woken for the blocked task,
        // because it is parked, and never offered another, because it was
        // holding something. A bot that landed there went silent for the rest
        // of the room's life. In a real room five of twenty-one bots had.
        //
        // Work in review is deliberately not treated the same way. That wait
        // ends by itself — an inspector acts and the task comes back — so the
        // bot will be woken without needing anything else to hold. A blocked
        // task carries no such promise.
        let parked_indefinitely = !held.is_empty()
            && held.iter().all(|id| {
                engine
                    .task(*id)
                    .is_some_and(|task| task.state == orgbot_proto::TaskState::Blocked)
            });
        if held.is_empty() || parked_indefinitely {
            let open = engine.claimable_by(agent);
            if !open.is_empty() {
                return Some(Wake::FreshWork { open });
            }
        }
        None
    }

    /// Whether `agent` has anything worth waking up for.
    pub fn is_actionable(&self, agent: AgentId) -> bool {
        let engine = self.engine.borrow();
        self.wake_reason(&engine, agent).is_some()
    }

    /// Whether a message the bot has not seen yet is addressed to it and
    /// expects a reply.
    fn has_unanswered_message(&self, engine: &Engine, agent: AgentId) -> bool {
        let Some(bot) = self.bots.get(&agent) else {
            return false;
        };
        let roles = engine
            .agent(agent)
            .map(|spec| spec.roles.clone())
            .unwrap_or_default();
        engine.events_since(bot.last_seen()).any(|envelope| {
            matches!(&envelope.event, Event::MessagePosted { message } if message.wakes(agent, &roles))
        })
    }

    /// The next bot that should act, if any.
    pub fn next_actor(&self) -> Option<AgentId> {
        self.next_actors(1).first().copied()
    }

    /// Up to `limit` bots that should act now, in turn order.
    ///
    /// Round-robin from wherever the last batch left off, so a chatty bot
    /// cannot monopolise the room.
    ///
    /// Bots woken for shared work are held to what is actually there. A free
    /// bot is only included if some claimable task has not already been
    /// earmarked for a bot ahead of it in this batch, so nine free bots and
    /// eight open tasks wake eight bots rather than nine racing for the same
    /// pile. Planning an empty board is capped at one bot outright: two
    /// coordinators drawing up plans at the same moment produces two sets of
    /// tasks for the same goal, which is worse than a wasted turn.
    pub fn next_actors(&self, limit: usize) -> Vec<AgentId> {
        if self.rotation.is_empty() || limit == 0 {
            return Vec::new();
        }
        let engine = self.engine.borrow();
        let mut chosen = Vec::new();
        // Work an earlier bot in this batch is expected to take.
        let mut earmarked: BTreeSet<TaskId> = BTreeSet::new();
        let mut planner_taken = false;
        let mut deliveries = engine.awaiting_delivery().len();
        // Bots sharing a backend share its rate limit, so starting several at
        // once turns concurrency into 429s rather than throughput. The turns
        // are spent either way; the work only gets done if the request lands.
        let per_backend = engine.room().spec.limits.parallel_per_backend();
        let mut in_flight: BTreeMap<String, usize> = BTreeMap::new();

        let routing = crate::allowance::routing(self.allowances.as_ref());
        let mut candidates = (0..self.rotation.len())
            .map(|offset| self.rotation[(self.cursor + offset) % self.rotation.len()])
            .collect::<Vec<_>>();
        candidates.sort_by_key(|agent| {
            let backend = engine
                .agent(*agent)
                .map(|spec| spec.provider.backend())
                .unwrap_or_default();
            routing.get(&backend).copied().flatten().unwrap_or(0)
        });
        for agent in candidates {
            if chosen.len() >= limit {
                break;
            }
            if self.bots.get(&agent).is_some_and(|bot| !bot.available()) {
                continue;
            }
            let Some(reason) = self.wake_reason(&engine, agent) else {
                continue;
            };
            let backend = engine
                .agent(agent)
                .map(|spec| spec.provider.backend())
                .unwrap_or_default();
            if routing.get(&backend) == Some(&None) {
                continue;
            }
            if !backend.is_empty() {
                let running = in_flight.entry(backend.clone()).or_default();
                if *running >= per_backend {
                    continue;
                }
            }
            match reason {
                Wake::FreshWork { open } => {
                    // Reserve one task for this bot. Which one is not binding —
                    // the bot chooses for itself — but it keeps the count
                    // honest, and keeps two bots off a single open task.
                    let Some(task) = open.into_iter().find(|id| !earmarked.contains(id)) else {
                        continue;
                    };
                    earmarked.insert(task);
                }
                Wake::Planning => {
                    if planner_taken {
                        continue;
                    }
                    planner_taken = true;
                }
                Wake::Delivery => {
                    if deliveries == 0 {
                        continue;
                    }
                    deliveries -= 1;
                }
                Wake::Review { queue } => {
                    // The review queue is shared, not per-bot: every inspector
                    // sees the same work in review. Waking two of them for one
                    // task means the second arrives to find it already
                    // reviewed and sent back, and its own review refused —
                    // which reads like the board contradicting itself rather
                    // than like a race it lost. This is the same reservation
                    // the free-work case makes, and it was missing here
                    // because the queue looked per-bot and is not.
                    let Some(task) = queue.into_iter().find(|id| !earmarked.contains(id)) else {
                        continue;
                    };
                    earmarked.insert(task);
                }
                // Nobody else can act on these, so there is nothing to divide.
                Wake::Handoff | Wake::Question | Wake::HeldWork => {}
            }
            if !backend.is_empty() {
                *in_flight.entry(backend).or_default() += 1;
            }
            chosen.push(agent);
        }
        chosen
    }

    /// Give one bot one turn.
    ///
    /// Returns `None` when nobody is actionable. A provider failure is recorded
    /// against the bot and reported as a turn that achieved nothing, rather
    /// than ending the run — one flaky backend should not take the room down.
    pub async fn step(&mut self, sink: Option<mpsc::Sender<StreamEvent>>) -> Option<TurnReport> {
        self.step_many(1, sink).await.pop()
    }

    /// Give up to `limit` bots a turn at the same time.
    ///
    /// Returns what each of them did, or an empty batch when nobody is
    /// actionable. Turns run concurrently but every board change still goes
    /// through the engine one at a time, so what lands in the log is a
    /// sequence — see this module's opening note for why that is enough.
    ///
    /// Whether a batch counted as progress is judged over the batch as a
    /// whole: if any bot in it did something, the room is not stalling. That
    /// makes a round longer than it was, and stall detection correspondingly
    /// less twitchy, which is the right trade — a room only looks stuck now if
    /// several bots working at once all achieve nothing.
    pub async fn step_many(
        &mut self,
        limit: usize,
        sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> Vec<TurnReport> {
        let chosen = self.next_actors(limit);
        if chosen.is_empty() {
            return Vec::new();
        }
        // Resume after the last bot picked, so the next batch starts with
        // whoever has been waiting longest.
        if let Some(last) = chosen.last() {
            self.cursor = self
                .rotation
                .iter()
                .position(|id| id == last)
                .map(|index| index + 1)
                .unwrap_or(0);
        }

        let engine = self.engine.clone();
        let completed_before: BTreeSet<_> = engine.with(|engine| {
            engine
                .tasks()
                .filter(|task| task.state == orgbot_proto::TaskState::Submitted)
                .map(|task| task.id)
                .collect()
        });
        // Only one bot streams. The deltas from several turns arriving on one
        // channel would interleave into a paragraph none of them wrote; the
        // rest of the batch still posts its work to the room as usual.
        let mut streaming = sink;
        let turns = self
            .bots
            .iter_mut()
            .filter(|(id, _)| chosen.contains(id))
            .map(|(id, bot)| {
                let id = *id;
                let engine = engine.clone();
                let sink = streaming.take();
                async move { (id, bot.take_turn(&engine, sink).await) }
            })
            .collect::<Vec<_>>();

        let finished = futures::future::join_all(turns).await;
        let memory_capture = self.engine.with(|engine| {
            let repository = engine.archive().memory_repository();
            let room_id = engine.room_id().to_string();
            let captures = engine
                .tasks()
                .filter(|task| {
                    task.state == orgbot_proto::TaskState::Submitted
                        && !completed_before.contains(&task.id)
                })
                .map(|task| {
                    let artifacts = task
                        .artifacts
                        .iter()
                        .filter_map(|id| engine.artifact(*id))
                        .collect::<Vec<_>>();
                    let names = artifacts
                        .iter()
                        .map(|artifact| format!("{} ({})", artifact.name, artifact.id))
                        .collect::<Vec<_>>();
                    let decisions = artifacts
                        .iter()
                        .filter_map(|artifact| match &artifact.body {
                            orgbot_proto::artifact::ArtifactBody::Text { text } => {
                                Some(text.chars().take(1200).collect::<String>())
                            }
                            _ => None,
                        })
                        .take(3)
                        .collect::<Vec<_>>();
                    (
                        task.id.to_string(),
                        task.title.chars().take(160).collect::<String>(),
                        task.brief.chars().take(4000).collect::<String>(),
                        decisions,
                        names,
                    )
                })
                .collect::<Vec<_>>();
            (repository, room_id, captures)
        });
        match memory_capture.0 {
            Ok(Some(repository)) if !memory_capture.2.is_empty() => {
                let room_id = memory_capture.1;
                let captures = memory_capture.2;
                match tokio::task::spawn_blocking(move || {
                    captures
                        .into_iter()
                        .map(|(task_id, title, brief, decisions, names)| {
                            let content = orgbot_engine::memory::completed_work_content(
                                &brief, &decisions, &names,
                            );
                            (
                                task_id.clone(),
                                repository.add(&title, &content, Some(&room_id), Some(&task_id)),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .await
                {
                    Ok(results) => {
                        for (task, result) in results {
                            if let Err(error) = result {
                                tracing::warn!(%error, %task, "completed work could not be saved to project memory");
                            }
                        }
                    }
                    Err(error) => tracing::warn!(%error, "project memory capture worker failed"),
                }
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "project memory repository unavailable"),
        }
        let mut reports = Vec::with_capacity(finished.len());
        for (agent, result) in finished {
            reports.push(match result {
                Ok(report) => report,
                Err(error) => {
                    tracing::warn!(%agent, %error, "bot turn failed");
                    TurnReport {
                        agent,
                        tool_calls: 0,
                        tool_errors: 0,
                        said: None,
                        steps: 0,
                        usage: Default::default(),
                        stop_reason: orgbot_providers::StopReason::Other("provider_error".into()),
                    }
                }
            });
        }

        // Expire stale handoffs and break any circular wait the batch created.
        self.engine.with(|engine| {
            let _ = engine.apply(Command::Tick {
                now: Timestamp::now(),
            });
        });

        // A successful action is always progress. Saying something is progress
        // only if it is not a verbatim repeat of what this bot last said — a
        // bot stuck repeating itself must not keep the room awake.
        let mut anyone_progressed = false;
        for report in &reports {
            let did_work = report.tool_calls > report.tool_errors;
            let said_something_new = report.said.as_deref().is_some_and(|said| {
                self.last_said.get(&report.agent).map(String::as_str) != Some(said)
            });
            if let Some(said) = &report.said {
                self.last_said.insert(report.agent, said.clone());
            }
            anyone_progressed |= did_work || said_something_new;
        }
        if anyone_progressed {
            self.idle_rounds = 0;
        } else if !reports.iter().any(|report| {
            report.stop_reason == orgbot_providers::StopReason::Other("provider_wait".into())
        }) {
            self.idle_rounds += 1;
        }
        reports
    }

    /// How many bots this room may run at once.
    pub fn parallelism(&self) -> usize {
        self.engine.borrow().room().spec.limits.parallel_turns()
    }

    /// Run until the room finishes, stalls, or runs out of budget.
    pub async fn run(&mut self, sink: Option<mpsc::Sender<StreamEvent>>) -> RunOutcome {
        loop {
            if let Some(outcome) = self.stop_condition() {
                if outcome == RunOutcome::Completed {
                    self.close_as_met();
                }
                return outcome;
            }
            if self
                .step_many(self.batch_size(), sink.clone())
                .await
                .is_empty()
            {
                // Nobody actionable. The completion rule decides whether that
                // is success or a room that has run out of road.
                return if self.is_complete() {
                    self.close_as_met();
                    RunOutcome::Completed
                } else {
                    RunOutcome::Idle
                };
            }
        }
    }

    /// How many bots to run in the next batch.
    ///
    /// The room's parallelism, narrowed so that running several bots at once
    /// cannot overshoot a turn budget the operator set. Never zero: a room
    /// with budget left always gets to try.
    pub fn batch_size(&self) -> usize {
        self.parallelism().min(self.turns_left()).max(1)
    }

    /// Turns left in the budget, or unbounded when there is no ceiling.
    ///
    /// A batch is bounded by this so that running several bots at once cannot
    /// overshoot a limit the operator set; the last batch of a room narrows to
    /// whatever remains.
    fn turns_left(&self) -> usize {
        let room = self.engine.borrow();
        let room = room.room();
        let limit = room.spec.limits.max_total_turns;
        if limit == 0 {
            return usize::MAX;
        }
        (limit.saturating_sub(room.turns_used)) as usize
    }

    fn close_as_met(&mut self) {
        self.engine.with(|engine| {
            let _ = engine.apply(Command::CloseRoom {
                reason: "the room's goal was met".into(),
            });
        });
    }

    /// Whether a batch run should stop before taking another turn.
    ///
    /// A server hosting a long-lived, human-in-the-loop room reads this too, but
    /// treats [`RunOutcome::Idle`] as "wait for a person" rather than a reason to
    /// close — see the note on [`Self::next_actor`].
    pub fn stop_condition(&self) -> Option<RunOutcome> {
        let engine = self.engine.borrow();
        self.stop_condition_of(&engine)
    }

    /// As [`Self::stop_condition`], against an engine already in hand.
    ///
    /// Every predicate here has this shape: the inner form takes the engine as
    /// an argument and the public one acquires it. The lock is not reentrant,
    /// so a method that holds it and then calls another that acquires it hangs
    /// the room — which is exactly what happened when `stop_condition` called
    /// `is_complete` while holding the engine.
    fn stop_condition_of(&self, engine: &Engine) -> Option<RunOutcome> {
        let room = engine.room();
        match room.phase {
            RoomPhase::Paused => return Some(RunOutcome::Paused),
            phase if phase.is_finished() => return Some(RunOutcome::Completed),
            _ => {}
        }
        if self.is_complete_of(engine) {
            return Some(RunOutcome::Completed);
        }
        if room.is_out_of_turns() {
            return Some(RunOutcome::OutOfTurns);
        }
        if room.is_out_of_time(Timestamp::now()) {
            return Some(RunOutcome::OutOfTime);
        }
        if self.idle_rounds >= MAX_IDLE_ROUNDS {
            return Some(RunOutcome::Stalled {
                rounds: self.idle_rounds,
            });
        }
        None
    }

    /// Whether the room's completion rule is satisfied.
    pub fn is_complete(&self) -> bool {
        let engine = self.engine.borrow();
        self.is_complete_of(&engine)
    }

    /// As [`Self::is_complete`], against an engine already in hand.
    fn is_complete_of(&self, engine: &Engine) -> bool {
        // A settled board does not answer a new question. Give each available
        // addressee a turn before applying the board's completion rule again.
        if self.rotation.iter().any(|agent| {
            !matches!(
                engine.status(*agent),
                AgentStatus::Offline | AgentStatus::Faulted
            ) && self.has_unanswered_message(engine, *agent)
        }) {
            return false;
        }
        let mut tasks = engine.tasks().peekable();
        if tasks.peek().is_none() {
            // A room with no board has not completed; it has not started.
            return false;
        }
        match engine.room().spec.completion {
            CompletionRule::AllTasksTerminal => engine.tasks().all(|task| task.state.is_terminal()),
            CompletionRule::RootTaskSubmitted => engine
                .tasks()
                .filter(|task| task.parent.is_none())
                .all(|task| task.state.is_terminal()),
            CompletionRule::Manual => false,
        }
    }

    /// Clear the stall counter, because something new has happened.
    ///
    /// A room that ran out of things to do is resting, not finished — when a
    /// person speaks it should get a fresh budget rather than immediately
    /// declaring itself stalled again on the strength of what it did before.
    pub fn wake(&mut self) {
        self.idle_rounds = 0;
    }

    /// Turns taken so far.
    pub fn turns_used(&self) -> u32 {
        self.engine.borrow().room().turns_used
    }
}

/// Give an agent-CLI bot a room-MCP bridge: a unix socket whose connections
/// are forwarded into the bot's turn loop, where inbound tools/call requests
/// are dispatched against the engine mid-episode. Returns the channel the bot
/// listens on plus the socket path handed to the CLI.
fn spawn_mcp_host(
    spec: &orgbot_proto::agent::AgentSpec,
) -> Option<(
    tokio::sync::mpsc::Receiver<crate::mcp::McpCall>,
    std::path::PathBuf,
)> {
    use orgbot_proto::agent::ProviderRef;

    if !matches!(&spec.provider, ProviderRef::CliAgent { mcp: true, .. }) {
        return None;
    }
    let socket = std::env::temp_dir().join(format!("orgbot-mcp-{}.sock", spec.id));
    let _ = std::fs::remove_file(&socket);
    let std_listener = match std::os::unix::net::UnixListener::bind(&socket) {
        Ok(listener) => listener,
        Err(error) => {
            tracing::warn!(%error, "room MCP bind failed for {}", socket.display());
            return None;
        }
    };
    if std_listener.set_nonblocking(true).is_err() {
        return None;
    }
    let listener = match tokio::net::UnixListener::from_std(std_listener) {
        Ok(listener) => listener,
        Err(error) => {
            tracing::warn!(%error, "room MCP listener setup failed");
            return None;
        }
    };
    use std::os::unix::fs::PermissionsExt;
    if std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).is_err() {
        return None;
    }
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let cleanup = socket.clone();
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                _ = tx.closed() => break,
                accepted = listener.accept() => accepted,
            };
            match accepted {
                Ok((stream, _)) => {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        crate::mcp::serve_stream(stream, tx).await;
                    });
                }
                Err(error) => {
                    tracing::debug!(%error, "room MCP accept loop ended");
                    break;
                }
            }
        }
        let _ = std::fs::remove_file(cleanup);
    });
    Some((rx, socket))
}

/// Why a bot is being woken.
///
/// The distinction that matters for batching is whether the reason points at
/// something only this bot can act on, or at a pool several bots share.
#[derive(Debug, Clone)]
enum Wake {
    /// Another bot is blocked waiting for this one to answer.
    Handoff,
    /// A question addressed to this bot is unanswered.
    Question,
    /// It is holding work that is not parked.
    HeldWork,
    /// Work is queued for it to inspect — a shared pool.
    Review {
        /// Tasks this bot could inspect right now.
        queue: Vec<TaskId>,
    },
    /// Approved work is waiting to be delivered — a shared pool.
    Delivery,
    /// The board is empty and this bot can plan — contended by definition.
    Planning,
    /// It is free, and these tasks are open to it — a shared pool.
    FreshWork {
        /// Tasks this bot could claim right now.
        open: Vec<TaskId>,
    },
}

/// Consecutive fruitless rounds before the run is called stalled.
///
/// One or two are normal — a bot checks the board and finds nothing. A run of
/// them means the bots are talking past each other.
const MAX_IDLE_ROUNDS: u32 = 6;
