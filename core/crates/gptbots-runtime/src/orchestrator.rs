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

use gptbots_engine::{Command, Engine};
use gptbots_proto::room::{CompletionRule, RoomPhase, RoomSpec};
use gptbots_proto::{AgentId, AgentSpec, AgentStatus, Timestamp};
use gptbots_providers::{Provider, StreamEvent};
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
}

impl Orchestrator {
    /// Open a room.
    pub fn new(spec: RoomSpec) -> Self {
        Self {
            engine: Engine::new(spec),
            bots: BTreeMap::new(),
            rotation: Vec::new(),
            cursor: 0,
            idle_rounds: 0,
        }
    }

    /// Seat a bot.
    pub fn seat(
        &mut self,
        spec: AgentSpec,
        provider: Arc<dyn Provider>,
    ) -> Result<AgentId, gptbots_engine::EngineError> {
        let id = spec.id;
        self.engine.apply(Command::JoinAgent {
            spec: Box::new(spec.clone()),
        })?;
        self.bots.insert(id, Bot::new(spec, provider));
        self.rotation.push(id);
        Ok(id)
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
    pub fn start(&mut self) -> Result<(), gptbots_engine::EngineError> {
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
                    stop_reason: gptbots_providers::StopReason::Other("provider_error".into()),
                }
            }
        };

        // Expire stale handoffs and break any circular wait the turn created.
        let _ = self.engine.apply(Command::Tick {
            now: Timestamp::now(),
        });

        if report.made_progress() {
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

    /// Whether the run should stop before taking another turn.
    fn stop_condition(&self) -> Option<RunOutcome> {
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

    /// Turns taken so far.
    pub fn turns_used(&self) -> u32 {
        self.engine.room().turns_used
    }
}

/// Consecutive fruitless turns before the run is called stalled.
///
/// One or two are normal — a bot checks the board and finds nothing. A run of
/// them means the bots are talking past each other.
const MAX_IDLE_ROUNDS: u32 = 6;
