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

use crate::bot::{Bot, BotMemory, TurnReport};
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
    /// Bots handed back by turns that were cut short. A running turn holds
    /// its bot outside `bots`, so other bots can be scheduled meanwhile; if
    /// the turn is dropped (a pause, a stop, a deadline) the bot is parked
    /// here rather than lost, and put back before anything reads `bots`.
    parked: Arc<std::sync::Mutex<Vec<(AgentId, Bot)>>>,
    /// Backends of the turns now running, so filling a free slot respects the
    /// per-backend cap the same way a fresh batch does.
    busy_backends: BTreeMap<String, usize>,
    /// Each bot now in a turn, as it was when the turn began. A save made
    /// mid-run writes this for it: what a turn reads is only marked seen when
    /// the turn ends, so a room reopened from that save runs the turn again.
    leased_memory: BTreeMap<AgentId, BotMemory>,
    /// Writes the room to disk while a run is under way.
    autosave: Option<Autosave>,
    /// Set when the owner pauses mid-run: start no new turns, and let the
    /// ones under way finish, so pausing loses no work.
    holding: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// Saves the room's checkpoint. A run lasts as long as its slowest turn, and
/// an agent CLI can work for an hour; the room must not go that long unsaved.
#[derive(Clone)]
pub struct Autosave(pub Arc<dyn Fn(serde_json::Value) + Send + Sync>);

impl std::fmt::Debug for Autosave {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Autosave")
    }
}

/// How long a bot whose last turn achieved nothing waits before another, when
/// it still has something to do. A run now lasts as long as there is work, so
/// "not again in this run" meant never: such bots sat idle until their claims
/// lapsed.
const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(2 * 60);

/// How often a run saves the room while turns are under way.
const AUTOSAVE_EVERY: std::time::Duration = std::time::Duration::from_secs(2 * 60);

/// How often a run looks for a free slot it could fill.
const REFILL_POLL: std::time::Duration = std::time::Duration::from_secs(1);

/// A bot on loan to its running turn; parks the bot if the turn is dropped.
struct TurnLease {
    id: AgentId,
    bot: Option<Bot>,
    parked: Arc<std::sync::Mutex<Vec<(AgentId, Bot)>>>,
}

impl Drop for TurnLease {
    fn drop(&mut self) {
        if let Some(bot) = self.bot.take() {
            if let Ok(mut parked) = self.parked.lock() {
                parked.push((self.id, bot));
            }
        }
    }
}

type TurnOutcome = (AgentId, Bot, Result<TurnReport, orgbot_providers::ProviderError>);

impl Orchestrator {
    /// Put back every bot whose turn was dropped.
    fn reclaim(&mut self) {
        let parked = std::mem::take(&mut *self.parked.lock().unwrap());
        for (id, bot) in parked {
            self.bots.insert(id, bot);
        }
        self.busy_backends.clear();
    }

    /// Expire temporary capabilities after a caller cancels a running batch,
    /// before saving the paused room or starting another turn.
    pub async fn finish_interrupted_turns(&mut self) {
        self.reclaim();
        for bot in self.bots.values_mut() {
            bot.finish_turn(&self.engine).await;
        }
    }

    /// Complete room state and bot conversations, without provider credentials.
    pub fn checkpoint(&self) -> serde_json::Value {
        let engine = self.engine.borrow();
        serde_json::json!({
            "version": 1, "engine": &*engine,
            "bots": self
                .bots
                .iter()
                .map(|(id, bot)| (*id, bot.memory()))
                // A bot parked by a dropped turn is still part of the room.
                .chain(self.parked.lock().unwrap().iter().map(|(id, bot)| (*id, bot.memory())))
                // So is one in a turn right now.
                .chain(self.leased_memory.iter().map(|(id, memory)| (*id, memory.clone())))
                .collect::<BTreeMap<_, _>>(),
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
            parked: Default::default(),
            busy_backends: BTreeMap::new(),
            leased_memory: BTreeMap::new(),
            autosave: None,
            holding: Default::default(),
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
            parked: Default::default(),
            busy_backends: BTreeMap::new(),
            leased_memory: BTreeMap::new(),
            autosave: None,
            holding: Default::default(),
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

    /// Save the room every few minutes while a run is under way.
    pub fn attach_autosave(&mut self, save: Autosave) {
        self.autosave = Some(save);
    }

    /// A switch that, while set, stops this room starting new turns without
    /// touching the ones under way. The session sets it when the owner pauses
    /// mid-run and pauses the room once those turns have finished.
    pub fn hold_switch(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.holding.clone()
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
        // Work it took on for someone who is waiting for the result, unless it
        // is waiting on the owner itself: rerunning it then cannot deliver,
        // and in the live room only spent turns until the owner answered.
        if !engine.handoffs_accepted_by(agent).is_empty() && !engine.awaiting_owner(agent) {
            return Some(Wake::HeldWork);
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
            && engine.decisions().pending().next().is_none()
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

    /// Whether an unseen message from the owner asks this bot something.
    fn has_unanswered_owner_ask(&self, engine: &Engine, agent: AgentId) -> bool {
        let Some(bot) = self.bots.get(&agent) else {
            return false;
        };
        let roles = engine
            .agent(agent)
            .map(|spec| spec.roles.clone())
            .unwrap_or_default();
        engine.events_since(bot.last_seen()).any(|envelope| {
            matches!(&envelope.event, Event::MessagePosted { message }
                if matches!(message.author, orgbot_proto::Author::Human { .. })
                    && message.wakes(agent, &roles))
        })
    }

    /// The next bot that should act, if any.
    pub fn next_actor(&self) -> Option<AgentId> {
        self.next_actors(1).first().copied()
    }

    /// Select one recipient for a new unthreaded human request using the same
    /// eligibility rules as the scheduler. Read the allowance ledger once for
    /// the whole roster, before callers borrow the room engine to post it.
    /// When every bot is temporarily unavailable, retain a stable recipient
    /// so the message remains queued for a later scheduler pass.
    pub fn default_human_target(&self) -> Option<AgentId> {
        if self.rotation.is_empty() {
            return None;
        }
        let engine = self.engine.borrow();
        let routing = crate::allowance::routing(self.allowances.as_ref());
        let eligible = |agent: &AgentId| {
            self.bots.get(agent).is_some_and(|bot| bot.available())
                && engine.status(*agent).is_available()
                && engine
                    .agent(*agent)
                    .is_some_and(|spec| routing.get(&spec.provider.backend()) != Some(&None))
        };
        let agents = self
            .rotation
            .iter()
            .filter(|agent| eligible(agent))
            .filter_map(|agent| engine.agent(*agent).map(|spec| (*agent, spec)))
            .collect::<Vec<_>>();
        let roster = self
            .rotation
            .iter()
            .filter_map(|agent| engine.agent(*agent).map(|spec| (*agent, spec)))
            .collect::<Vec<_>>();
        let coordinator = agents
            .iter()
            .find(|(_, spec)| spec.has_role("coordinator") && spec.autonomy.may_create_tasks);
        let planner = agents
            .iter()
            .find(|(_, spec)| spec.autonomy.may_create_tasks);
        let fallback_coordinator = roster
            .iter()
            .find(|(_, spec)| spec.has_role("coordinator") && spec.autonomy.may_create_tasks);
        let fallback_planner = roster
            .iter()
            .find(|(_, spec)| spec.autonomy.may_create_tasks);
        coordinator
            .or(planner)
            .or_else(|| agents.first())
            .map(|(id, _)| *id)
            .or_else(|| fallback_coordinator.map(|(id, _)| *id))
            .or_else(|| fallback_planner.map(|(id, _)| *id))
            .or_else(|| roster.first().map(|(id, _)| *id))
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
        self.pick_actors(limit, &mut BTreeMap::new(), |_| true)
    }

    /// [`Self::next_actors`] for a run already under way.
    ///
    /// `reserved` holds what bots still mid-turn were woken for, and gains
    /// what is reserved for the bots picked now. Without it, a slot freed
    /// halfway through a run is filled as if nobody were working: a second
    /// inspector is woken for the review the first is still doing, and
    /// arrives to find it done. Only bots `eligible` allows are considered.
    fn pick_actors(
        &self,
        limit: usize,
        reserved: &mut BTreeMap<AgentId, Reserved>,
        eligible: impl Fn(AgentId) -> bool,
    ) -> Vec<AgentId> {
        if self.rotation.is_empty() || limit == 0 {
            return Vec::new();
        }
        let engine = self.engine.borrow();
        let mut chosen = Vec::new();
        // Work a bot already picked is expected to take.
        let mut earmarked: BTreeSet<TaskId> = reserved
            .values()
            .filter_map(|held| match held {
                Reserved::Task(task) => Some(*task),
                _ => None,
            })
            .collect();
        let mut deliveries = engine.awaiting_delivery().len().saturating_sub(
            reserved
                .values()
                .filter(|held| matches!(held, Reserved::Delivery))
                .count(),
        );
        // Bots sharing a backend share its rate limit, so starting several at
        // once turns concurrency into 429s rather than throughput. The turns
        // are spent either way; the work only gets done if the request lands.
        let per_backend = engine.room().spec.limits.parallel_per_backend();
        // Turns already running count against their backend's cap.
        let mut in_flight: BTreeMap<String, usize> = self.busy_backends.clone();

        let routing = crate::allowance::routing(self.allowances.as_ref());
        let mut candidates = (0..self.rotation.len())
            .map(|offset| self.rotation[(self.cursor + offset) % self.rotation.len()])
            .collect::<Vec<_>>();
        candidates.retain(|agent| {
            eligible(*agent)
                && self.bots.get(agent).is_some_and(|bot| bot.available())
                && engine.status(*agent).is_available()
                && engine
                    .agent(*agent)
                    .is_some_and(|spec| routing.get(&spec.provider.backend()) != Some(&None))
        });
        let coordinator_available = candidates.iter().any(|agent| {
            engine.agent(*agent).is_some_and(|spec| {
                spec.autonomy.may_create_tasks
                    && spec.roles.iter().any(|role| role == "coordinator")
            })
        });
        // Answering the request already provides the planning turn.
        let mut planner_taken = reserved.values().any(|held| matches!(held, Reserved::Planning))
            || candidates
                .iter()
                .any(|agent| self.has_unanswered_message(&engine, *agent));
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
            let Some(reason) = self.wake_reason(&engine, agent) else {
                continue;
            };
            let backend = engine
                .agent(agent)
                .map(|spec| spec.provider.backend())
                .unwrap_or_default();
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
                    reserved.insert(agent, Reserved::Task(task));
                }
                Wake::Planning => {
                    if planner_taken
                        || (coordinator_available
                            && !engine.agent(agent).is_some_and(|spec| {
                                spec.roles.iter().any(|role| role == "coordinator")
                            }))
                    {
                        continue;
                    }
                    planner_taken = true;
                    reserved.insert(agent, Reserved::Planning);
                }
                Wake::Delivery => {
                    if deliveries == 0 {
                        continue;
                    }
                    deliveries -= 1;
                    reserved.insert(agent, Reserved::Delivery);
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
                    reserved.insert(agent, Reserved::Task(task));
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

    /// Bots with something to do that are waiting only for a free slot.
    ///
    /// From the live room: Sol's task was unblocked while the other two bots
    /// on its account held both of that account's slots. Sol waited seventeen
    /// minutes for a turn and lost the task for "not reporting progress" it
    /// never had the chance to make. A faulted or offline bot is not waiting,
    /// so its claims still lapse.
    fn waiting_for_a_slot(&self) -> Vec<AgentId> {
        let engine = self.engine.borrow();
        self.rotation
            .iter()
            .copied()
            .filter(|agent| {
                self.bots.get(agent).is_some_and(|bot| bot.available())
                    && engine.status(*agent).is_available()
                    && self.wake_reason(&engine, *agent).is_some()
            })
            .collect()
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
        // The single stream goes to the first bot chosen, which is the bot
        // callers tag it with (they peek `next_actors`).
        self.reclaim();
        let lead = self.next_actors(limit).first().copied();
        let sink = std::sync::Mutex::new(sink);
        let sinks = |agent| {
            if Some(agent) == lead {
                sink.lock().unwrap().take()
            } else {
                None
            }
        };
        self.run_turns(limit, &sinks, false).await
    }

    /// Like [`Self::step_many`], with a live stream for every running bot.
    pub async fn step_many_with<F>(&mut self, limit: usize, sinks: &F) -> Vec<TurnReport>
    where
        F: Fn(AgentId) -> Option<mpsc::Sender<StreamEvent>> + Sync,
    {
        self.run_turns(limit, sinks, false).await
    }

    /// Give up to `limit` bots a turn at once, keeping slots full as turns end.
    ///
    /// A batch used to wait for its slowest turn before anyone else could
    /// start, so a bot the owner had just asked something sat idle behind an
    /// unrelated agent CLI working for many minutes. Now, whenever a slot is
    /// free (a turn finished, or someone became actionable), the next bot
    /// with something to do starts, for as long as the run lasts: a bot goes
    /// again after a productive turn, at once when the owner asks it
    /// something, and otherwise after [`RETRY_AFTER`]. The room is saved every [`AUTOSAVE_EVERY`] meanwhile, when
    /// an [`Autosave`] is attached.
    ///
    /// `sinks` gives each bot its own live stream, tagged with it, so every
    /// running bot shows what it is doing.
    pub async fn step_rolling<F>(&mut self, limit: usize, sinks: &F) -> Vec<TurnReport>
    where
        F: Fn(AgentId) -> Option<mpsc::Sender<StreamEvent>> + Sync,
    {
        self.run_turns(limit, sinks, true).await
    }

    /// Run a batch of turns; with `refill`, keep free slots filled.
    async fn run_turns<F>(&mut self, limit: usize, sinks: &F, refill: bool) -> Vec<TurnReport>
    where
        F: Fn(AgentId) -> Option<mpsc::Sender<StreamEvent>> + Sync,
    {
        use futures::stream::{FuturesUnordered, StreamExt};

        self.reclaim();
        // A pause is waiting for the turns under way: start none.
        if self.holding.load(std::sync::atomic::Ordering::SeqCst) {
            return Vec::new();
        }
        // What each bot mid-turn was woken for, so refills leave it to them.
        let mut reserved: BTreeMap<AgentId, Reserved> = BTreeMap::new();
        let chosen = self.pick_actors(limit, &mut reserved, |_| true);
        if chosen.is_empty() {
            return Vec::new();
        }

        let engine = self.engine.clone();
        let completed_before: BTreeSet<_> = engine.with(|engine| {
            engine
                .tasks()
                .filter(|task| task.state == orgbot_proto::TaskState::Submitted)
                .map(|task| task.id)
                .collect()
        });

        let mut running: FuturesUnordered<
            std::pin::Pin<Box<dyn std::future::Future<Output = TurnOutcome> + Send + '_>>,
        > = FuturesUnordered::new();
        let mut running_backend: BTreeMap<AgentId, String> = BTreeMap::new();
        // Who has had a turn in this run, and which of them got something done
        // in their last one. A bot goes again in the same run only after a
        // productive turn (or when the owner asks it something), so one that
        // achieved nothing cannot be restarted the moment it stops and burn
        // the budget. A run lasts as long as its slowest turn, so the others
        // must be able to keep working meanwhile.
        let mut ran: BTreeSet<AgentId> = BTreeSet::new();
        let mut productive: BTreeSet<AgentId> = BTreeSet::new();
        let mut finished_at: BTreeMap<AgentId, tokio::time::Instant> = BTreeMap::new();
        let mut last_save = tokio::time::Instant::now();
        let mut finished = Vec::new();

        // Lend `agent` to a turn of its own.
        let start = |this: &mut Self,
                         running: &mut FuturesUnordered<_>,
                         running_backend: &mut BTreeMap<AgentId, String>,
                         agent: AgentId| {
            let Some(bot) = this.bots.remove(&agent) else {
                return;
            };
            this.leased_memory.insert(agent, bot.memory());
            // Resume after the last bot picked, so the next pick starts with
            // whoever has been waiting longest.
            if let Some(index) = this.rotation.iter().position(|id| *id == agent) {
                this.cursor = index + 1;
            }
            let backend = engine
                .with(|engine| engine.agent(agent).map(|spec| spec.provider.backend()))
                .unwrap_or_default();
            if !backend.is_empty() {
                *this.busy_backends.entry(backend.clone()).or_default() += 1;
            }
            running_backend.insert(agent, backend);
            let lease = TurnLease {
                id: agent,
                bot: Some(bot),
                parked: this.parked.clone(),
            };
            let engine = engine.clone();
            let sink = sinks(agent);
            let turn: std::pin::Pin<Box<dyn std::future::Future<Output = TurnOutcome> + Send>> =
                Box::pin(async move {
                    let mut lease = lease;
                    let result = lease
                        .bot
                        .as_mut()
                        .expect("a leased bot")
                        .take_turn(&engine, sink)
                        .await;
                    let bot = lease.bot.take().expect("a leased bot");
                    (lease.id, bot, result)
                });
            running.push(turn);
        };

        for agent in chosen {
            ran.insert(agent);
            start(self, &mut running, &mut running_backend, agent);
        }

        let mut poll = tokio::time::interval(REFILL_POLL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        poll.tick().await;
        loop {
            tokio::select! {
                outcome = running.next() => {
                    let Some((agent, bot, result)) = outcome else { break };
                    self.bots.insert(agent, bot);
                    self.leased_memory.remove(&agent);
                    reserved.remove(&agent);
                    if matches!(&result, Ok(report) if report.tool_calls > report.tool_errors) {
                        productive.insert(agent);
                    }
                    finished_at.insert(agent, tokio::time::Instant::now());
                    if let Some(backend) = running_backend.remove(&agent) {
                        if let Some(count) = self.busy_backends.get_mut(&backend) {
                            *count = count.saturating_sub(1);
                        }
                    }
                    // Expire stale handoffs and break any circular wait the
                    // turn created, before choosing who goes next. Bots still
                    // mid-turn are alive: their claims are renewed first so
                    // the tick cannot hand their work to someone else, and
                    // handoffs waiting on them since their turn began are
                    // held open, as are those of bots waiting for a slot.
                    let waiting = self.waiting_for_a_slot();
                    self.engine.with(|engine| {
                        for agent in running_backend.keys().chain(&waiting) {
                            let _ = engine.apply(Command::KeepAlive { agent: *agent });
                        }
                        let _ = engine.apply(Command::Tick { now: Timestamp::now() });
                    });
                    finished.push((agent, result));
                }
                _ = poll.tick() => {
                    let waiting = self.waiting_for_a_slot();
                    self.engine.with(|engine| {
                        for agent in running_backend.keys().chain(&waiting) {
                            let _ = engine.apply(Command::KeepAlive { agent: *agent });
                        }
                        // A faulted bot's retry is due on its own schedule, not
                        // whenever another turn happens to finish.
                        let now = Timestamp::now();
                        if engine.retry_due(now) {
                            let _ = engine.apply(Command::Tick { now });
                        }
                    });
                    if last_save.elapsed() >= AUTOSAVE_EVERY {
                        last_save = tokio::time::Instant::now();
                        if let Some(Autosave(save)) = self.autosave.clone() {
                            save(self.checkpoint());
                        }
                    }
                }
            }
            // Fill free slots, unless the room should stop taking turns.
            let holding = self.holding.load(std::sync::atomic::Ordering::SeqCst);
            if refill && !holding && self.stop_condition().is_none() {
                let slots = self
                    .parallelism()
                    .min(self.turns_left().saturating_sub(running.len()))
                    .saturating_sub(running.len());
                if slots > 0 {
                    let engine = self.engine.borrow();
                    let asked: BTreeSet<AgentId> = self
                        .rotation
                        .iter()
                        .copied()
                        .filter(|agent| self.has_unanswered_owner_ask(&engine, *agent))
                        .collect();
                    drop(engine);
                    let next = self.pick_actors(slots, &mut reserved, |agent| {
                        asked.contains(&agent)
                            || !ran.contains(&agent)
                            || productive.contains(&agent)
                            || finished_at
                                .get(&agent)
                                .is_some_and(|at| at.elapsed() >= RETRY_AFTER)
                    });
                    for agent in next {
                        ran.insert(agent);
                        productive.remove(&agent);
                        start(self, &mut running, &mut running_backend, agent);
                    }
                }
            }
            if running.is_empty() {
                break;
            }
        }
        drop(running);
        self.reclaim();
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
        if engine.decisions().pending().next().is_some() {
            return false;
        }
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

/// Shared work a bot was woken for, held for it until its turn ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reserved {
    /// An open task, or a task waiting for inspection.
    Task(TaskId),
    /// Drawing up a plan for an empty board.
    Planning,
    /// Delivering approved work.
    Delivery,
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
