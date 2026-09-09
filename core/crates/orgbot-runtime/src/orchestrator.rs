//! Who acts next, and when the room is finished.
//!
//! Bots do not run concurrently against the engine. One bot takes a turn, the
//! room state changes, and the next choice is made against the new state. That
//! is slower than fanning out, but it is what makes the room's behaviour
//! reproducible from its event log — and with a shared task board, two bots
//! acting on stale state is exactly how you get two claims on one task.
//!
//! Concurrency lives one level down instead: a single bot's turn can issue
//! parallel tool calls, and its model call streams.

use std::collections::BTreeMap;
use std::sync::Arc;

use orgbot_engine::{Command, Engine};
use orgbot_proto::event::Event;
use orgbot_proto::room::{CompletionRule, RoomPhase, RoomSpec};
use orgbot_proto::{AgentId, AgentSpec, AgentStatus, Timestamp};
use orgbot_providers::{Provider, StreamEvent};
use tokio::sync::mpsc;

use crate::bot::{Bot, TurnReport};

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
    engine: Engine,
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
}

impl Orchestrator {
    /// Complete room state and bot conversations, without provider credentials.
    pub fn checkpoint(&self) -> serde_json::Value {
        serde_json::json!({
            "version": 1, "engine": self.engine,
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
        let memory: BTreeMap<AgentId, crate::bot::BotMemory> = if version == 1 {
            serde_json::from_value(value["bots"].clone()).map_err(|e| e.to_string())?
        } else {
            BTreeMap::new()
        };
        let mut restored = Self {
            engine,
            bots: BTreeMap::new(),
            rotation: vec![],
            cursor: value["cursor"].as_u64().unwrap_or(0) as usize,
            idle_rounds: 0,
            last_said: BTreeMap::new(),
        };
        for (mut configured, provider) in providers {
            if let Some(saved) = restored.engine.agent_by_handle(&configured.handle).cloned() {
                configured.id = saved.id;
                configured.created_at = saved.created_at;
                let same_provider = saved.provider == configured.provider;
                restored.engine.update_agent(configured.clone());
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
            engine: Engine::new(spec),
            bots: BTreeMap::new(),
            rotation: Vec::new(),
            cursor: 0,
            idle_rounds: 0,
            last_said: BTreeMap::new(),
        }
    }

    /// Seat a bot.
    pub fn seat(
        &mut self,
        spec: AgentSpec,
        provider: Arc<dyn Provider>,
    ) -> Result<AgentId, orgbot_engine::EngineError> {
        let id = spec.id;
        self.engine.apply(Command::JoinAgent {
            spec: Box::new(spec.clone()),
        })?;
        let mut bot = Bot::new(spec.clone(), provider);
        if let Some((rx, socket)) = spawn_mcp_host(&spec) {
            bot.attach_mcp(rx, socket);
        }
        self.bots.insert(id, bot);
        self.rotation.push(id);
        Ok(id)
    }

    pub fn attach_workspace(&mut self, workspace: crate::workspace::Workspace) {
        for bot in self.bots.values_mut() {
            bot.attach_workspace(workspace.clone());
        }
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
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// The room engine, for a human issuing commands mid-run.
    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    /// Move the room into the running phase.
    pub fn start(&mut self) -> Result<(), orgbot_engine::EngineError> {
        self.engine.apply(Command::StartRoom)?;
        Ok(())
    }

    /// Whether `agent` has anything worth waking up for.
    ///
    /// Ordered by how much the room gains: answering a handoff unblocks another
    /// bot, so it outranks starting fresh work.
    pub fn is_actionable(&self, agent: AgentId) -> bool {
        let status = self.engine.status(agent);
        if matches!(status, AgentStatus::Offline | AgentStatus::Faulted) {
            return false;
        }
        // Someone is blocked on this bot's answer.
        if !self.engine.offers_for(agent).is_empty() {
            return true;
        }
        // A question addressed to this bot that it has not answered yet. This
        // is what lets a human (or another bot) talk to the room and get a
        // reply, rather than the message landing in the log unheard.
        if self.has_unanswered_message(agent) {
            return true;
        }
        // Work in hand that is not parked.
        let held = self.engine.active_tasks_of(agent);
        let has_live_work = held.iter().any(|id| {
            self.engine
                .task(*id)
                .is_some_and(|task| !task.state.is_waiting())
        });
        if has_live_work {
            return true;
        }
        // Something to inspect.
        if !self.engine.review_queue_for(agent).is_empty() {
            return true;
        }
        // Approved work waiting to be delivered. A coordinator holds no tasks
        // of its own, so this is the only thing that wakes it at the end.
        if self
            .engine
            .agent(agent)
            .is_some_and(|spec| spec.autonomy.may_submit)
            && !self.engine.awaiting_delivery().is_empty()
        {
            return true;
        }
        // An empty board is a coordinator's cue to plan. Without this nobody is
        // actionable at the start of a room and it idles before it begins.
        if self
            .engine
            .agent(agent)
            .is_some_and(|spec| spec.autonomy.may_create_tasks)
            && self.engine.tasks().next().is_none()
        {
            return true;
        }
        // Free, with work available. A bot whose only tasks are parked is still
        // free to pick up something else.
        held.is_empty() && !self.engine.claimable_by(agent).is_empty()
    }

    /// Whether a message the bot has not seen yet is addressed to it and
    /// expects a reply.
    fn has_unanswered_message(&self, agent: AgentId) -> bool {
        let Some(bot) = self.bots.get(&agent) else {
            return false;
        };
        let roles = self
            .engine
            .agent(agent)
            .map(|spec| spec.roles.clone())
            .unwrap_or_default();
        self.engine.events_since(bot.last_seen()).any(|envelope| {
            matches!(&envelope.event, Event::MessagePosted { message } if message.wakes(agent, &roles))
        })
    }

    /// The next bot that should act, if any.
    ///
    /// Round-robin from wherever the last turn left off, so a chatty bot cannot
    /// monopolise the room.
    pub fn next_actor(&self) -> Option<AgentId> {
        if self.rotation.is_empty() {
            return None;
        }
        (0..self.rotation.len())
            .map(|offset| self.rotation[(self.cursor + offset) % self.rotation.len()])
            .find(|agent| self.is_actionable(*agent))
    }

    /// Give one bot one turn.
    ///
    /// Returns `None` when nobody is actionable. A provider failure is recorded
    /// against the bot and reported as a turn that achieved nothing, rather
    /// than ending the run — one flaky backend should not take the room down.
    pub async fn step(&mut self, sink: Option<mpsc::Sender<StreamEvent>>) -> Option<TurnReport> {
        let agent = self.next_actor()?;
        self.cursor = self
            .rotation
            .iter()
            .position(|id| *id == agent)
            .map(|index| index + 1)
            .unwrap_or(0);

        let bot = self.bots.get_mut(&agent)?;
        let report = match bot.take_turn(&mut self.engine, sink).await {
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
        };

        // Expire stale handoffs and break any circular wait the turn created.
        let _ = self.engine.apply(Command::Tick {
            now: Timestamp::now(),
        });

        // A successful action is always progress. Saying something is progress
        // only if it is not a verbatim repeat of what this bot last said — a
        // bot stuck repeating itself must not keep the room awake.
        let did_work = report.tool_calls > report.tool_errors;
        let said_something_new = report
            .said
            .as_deref()
            .is_some_and(|said| self.last_said.get(&agent).map(String::as_str) != Some(said));
        if let Some(said) = &report.said {
            self.last_said.insert(agent, said.clone());
        }
        if did_work || said_something_new {
            self.idle_rounds = 0;
        } else {
            self.idle_rounds += 1;
        }
        Some(report)
    }

    /// Run until the room finishes, stalls, or runs out of budget.
    pub async fn run(&mut self, sink: Option<mpsc::Sender<StreamEvent>>) -> RunOutcome {
        loop {
            if let Some(outcome) = self.stop_condition() {
                if outcome == RunOutcome::Completed {
                    let _ = self.engine.apply(Command::CloseRoom {
                        reason: "the room's goal was met".into(),
                    });
                }
                return outcome;
            }
            if self.step(sink.clone()).await.is_none() {
                // Nobody actionable. The completion rule decides whether that
                // is success or a room that has run out of road.
                return if self.is_complete() {
                    let _ = self.engine.apply(Command::CloseRoom {
                        reason: "the room's goal was met".into(),
                    });
                    RunOutcome::Completed
                } else {
                    RunOutcome::Idle
                };
            }
        }
    }

    /// Whether a batch run should stop before taking another turn.
    ///
    /// A server hosting a long-lived, human-in-the-loop room reads this too, but
    /// treats [`RunOutcome::Idle`] as "wait for a person" rather than a reason to
    /// close — see the note on [`Self::next_actor`].
    pub fn stop_condition(&self) -> Option<RunOutcome> {
        let room = self.engine.room();
        match room.phase {
            RoomPhase::Paused => return Some(RunOutcome::Paused),
            phase if phase.is_finished() => return Some(RunOutcome::Completed),
            _ => {}
        }
        if self.is_complete() {
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
        // A settled board does not answer a new question. Give each available
        // addressee a turn before applying the board's completion rule again.
        if self.rotation.iter().any(|agent| {
            !matches!(
                self.engine.status(*agent),
                AgentStatus::Offline | AgentStatus::Faulted
            ) && self.has_unanswered_message(*agent)
        }) {
            return false;
        }
        let mut tasks = self.engine.tasks().peekable();
        if tasks.peek().is_none() {
            // A room with no board has not completed; it has not started.
            return false;
        }
        match self.engine.room().spec.completion {
            CompletionRule::AllTasksTerminal => {
                self.engine.tasks().all(|task| task.state.is_terminal())
            }
            CompletionRule::RootTaskSubmitted => self
                .engine
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
        self.engine.room().turns_used
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

/// Consecutive fruitless turns before the run is called stalled.
///
/// One or two are normal — a bot checks the board and finds nothing. A run of
/// them means the bots are talking past each other.
const MAX_IDLE_ROUNDS: u32 = 6;
