//! A running room, as an actor.
//!
//! One [`RoomSession`] owns one [`Orchestrator`] and runs it on its own task.
//! Nothing outside touches the engine directly: clients hold a cheap
//! [`SessionHandle`], send it control messages, and subscribe to a broadcast of
//! everything that happens.
//!
//! This is where a live server differs from the batch `Orchestrator::run`: when
//! no bot is actionable the session does **not** end the room — it waits for a
//! human to say something. A group chat is meant to sit idle between requests.

use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use orgbot_engine::Command;
use orgbot_proto::graph::TaskOrigin;
use orgbot_proto::chat::Audience;
use orgbot_proto::event::{EventEnvelope, Seq};
use orgbot_proto::task::BlockReason;
use orgbot_proto::{Author, ChatMessage, MessageIntent, RoomPhase, TaskId, TaskState, Timestamp};
use orgbot_runtime::{Orchestrator, RunOutcome};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::config::ResolvedRoom;
use crate::protocol::{AgentView, ServerMessage};

/// How often the session wakes on its own to expire stale handoffs and sweep
/// for deadlocks while it is otherwise idle.
const IDLE_TICK: Duration = Duration::from_secs(5);

/// Capacity of the event broadcast. A client slower than this falls behind and
/// must `Resync`; the number is generous so that only a truly stuck client
/// lags.
const BROADCAST_CAPACITY: usize = 1024;

/// A control message a client can send into a room.
#[derive(Debug)]
pub enum Control {
    /// Request a fresh snapshot of the whole room.
    Snapshot(oneshot::Sender<ServerMessage>),
    /// Ask for the durable log after `since`.
    ///
    /// A client that attaches after the room has started would otherwise never
    /// see what already happened: the snapshot carries the roster and the
    /// board, but the transcript lives in the event log.
    Backlog {
        /// Everything after this position.
        since: Seq,
        /// Where to send it.
        reply: oneshot::Sender<Vec<EventEnvelope>>,
    },
    /// The operator answers a question the room asked.
    AnswerDecision {
        /// The question being answered.
        decision: orgbot_proto::ids::DecisionId,
        /// What they chose.
        chosen: String,
        /// Anything they added.
        note: String,
    },
    /// A human speaks into the chat.
    HumanMessage {
        files: Vec<String>,
        /// The person's name.
        author: String,
        /// The text.
        body: String,
        /// The task under discussion, if any.
        task: Option<TaskId>,
    },
    /// A human answers a bot blocked awaiting one.
    Answer {
        /// The task that was waiting.
        task: TaskId,
        /// The answer.
        answer: String,
    },
    /// Pause the room.
    Pause {
        /// Why.
        reason: String,
    },
    /// Resume it.
    Resume,
    /// Stop the session and cancel an in-flight model call.
    Stop,
}

/// A cheap, clonable handle to a running room.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    control: mpsc::Sender<Control>,
    events: broadcast::Sender<ServerMessage>,
    published: Arc<RwLock<Published>>,
    interrupt: Arc<Mutex<CancellationToken>>,
    input_interrupt: Arc<Mutex<CancellationToken>>,
}

impl SessionHandle {
    /// Subscribe to the room's live event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<ServerMessage> {
        self.events.subscribe()
    }

    /// Enqueue control without waiting for a model to finish.
    pub async fn send(&self, control: Control) -> bool {
        self.try_send(control)
    }

    pub fn try_send(&self, control: Control) -> bool {
        let interrupts = matches!(control, Control::Pause { .. } | Control::Stop);
        let input = matches!(
            control,
            Control::HumanMessage { .. } | Control::Answer { .. } | Control::AnswerDecision { .. }
        );
        // Queue and cancel under the same lock: the session must not replace
        // this token between accepting the input and receiving its signal.
        let input_interrupt = self.input_interrupt.lock().unwrap();
        if self.control.try_send(control).is_err() {
            return false;
        }
        if input {
            input_interrupt.cancel();
        }
        if interrupts {
            self.interrupt.lock().unwrap().cancel();
        }
        true
    }

    /// The last committed state is readable even during a model/tool call.
    pub async fn snapshot(&self) -> Option<ServerMessage> {
        Some(self.published.read().unwrap().snapshot.clone())
    }

    pub async fn backlog(&self, since: Seq) -> Vec<EventEnvelope> {
        self.published
            .read()
            .unwrap()
            .backlog
            .iter()
            .filter(|e| e.seq > since)
            .cloned()
            .collect()
    }
}

#[derive(Debug)]
struct Published {
    snapshot: ServerMessage,
    backlog: Vec<EventEnvelope>,
}

/// A room and the machinery to run it.
#[derive(Debug)]
pub struct RoomSession {
    state_path: Option<std::path::PathBuf>,
    saved_at: Option<(Seq, orgbot_proto::room::RoomPhase, u32)>,
    orchestrator: Orchestrator,
    control: mpsc::Receiver<Control>,
    events: broadcast::Sender<ServerMessage>,
    /// The last log position broadcast, so each step ships only new events.
    broadcast_head: Seq,
    /// Whether the room has finished its work. A settled room takes no more
    /// turns but stays connected, so its transcript can still be read and a
    /// person can still speak into it.
    settled: bool,
    published: Arc<RwLock<Published>>,
    interrupt: Arc<Mutex<CancellationToken>>,
    input_interrupt: Arc<Mutex<CancellationToken>>,
    /// MCP servers to reach out to, connected when the room starts running.
    ///
    /// Held as configuration rather than connected at build time: starting a
    /// server is I/O, building a session is not, and a room that has been
    /// assembled but never run should not have left processes behind.
    mcp: Vec<orgbot_runtime::mcp_client::McpServerSpec>,
}

impl RoomSession {
    /// Build a session from a resolved room, returning it and a handle.
    ///
    /// The room is seated and seeded but not started; call [`Self::run`] to
    /// begin. Errors here mean the room itself is invalid (a bad roster, a seed
    /// task that names a missing role); a caller should surface them rather than
    /// spawn a room that cannot work.
    pub fn build(resolved: ResolvedRoom) -> anyhow::Result<(Self, SessionHandle)> {
        let state_path = resolved.state_path.clone();
        let saved = match &state_path {
            Some(path) => {
                anyhow::ensure!(
                    path.is_absolute(),
                    "Room memory needs an absolute local path"
                );
                match std::fs::symlink_metadata(path) {
                    Ok(meta) => {
                        anyhow::ensure!(
                            meta.is_file() && meta.len() <= 64 * 1024 * 1024,
                            "Saved room memory is not a regular file or exceeds 64 MiB"
                        );
                        let mut board =
                            serde_json::from_slice::<serde_json::Value>(&std::fs::read(path)?)?;
                        let damaged =
                            crate::bodies::restore(&mut board, &crate::bodies::directory_for(path));
                        if !damaged.is_empty() {
                            // Named rather than swallowed: a deliverable that
                            // cannot be read back is something a person needs
                            // to know about, not something to discover in a
                            // review.
                            tracing::warn!(
                                count = damaged.len(),
                                artifacts = ?damaged,
                                "some deliverables could not be read back"
                            );
                        }
                        Some(board)
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => return Err(e.into()),
                }
            }
            None => None,
        };
        let restored = saved.is_some();
        let roles = resolved.roles.clone();
        let skills = resolved.skills.clone();
        let mcp = resolved.mcp.clone();
        let mut orchestrator = if let Some(saved) = saved {
            Orchestrator::restore(
                saved,
                resolved.spec,
                resolved
                    .bots
                    .into_iter()
                    .map(|b| (b.spec, b.provider))
                    .collect(),
            )
            .map_err(|e| {
                anyhow::anyhow!("Could not restore saved room; the file was preserved: {e}")
            })?
        } else {
            let mut orchestrator = Orchestrator::new(resolved.spec);
            for bot in resolved.bots {
                orchestrator
                    .seat(bot.spec, bot.provider)
                    .map_err(|e| anyhow::anyhow!("seating a bot: {e}"))?;
            }
            orchestrator
                .start()
                .map_err(|e| anyhow::anyhow!("starting the room: {e}"))?;
            orchestrator
        };

        // Applied to restored rooms too: the roles a room declares are current
        // configuration, not history, and a saved room should pick up a table
        // added since it was written.
        orchestrator.engine_mut().set_roles(roles);
        // Same reasoning: a skill edited on disk should take effect the next
        // time the room opens, not be frozen into state the day it was written.
        orchestrator.engine_mut().set_skills(skills);
        // What earlier rooms built. Excludes this room's own board, so a room
        // is never offered its own work as something somebody else did.
        if !resolved.prior_boards.is_empty() {
            let boards: Vec<std::path::PathBuf> = resolved
                .prior_boards
                .iter()
                .map(std::path::PathBuf::from)
                .collect();
            let this_room = orchestrator.engine().room().spec.name.clone();
            let archive = orgbot_engine::Archive::load(&boards, Some(&this_room));
            tracing::info!(rooms = archive.rooms().len(), "earlier rooms available");
            orchestrator.engine_mut().set_archive(archive);
        }


        if let Some(parent) = state_path.as_ref().and_then(|path| path.parent()) {
            orchestrator.attach_attachments(parent.join("attachments"));
        }
        // Seed the board after the room is running, since CreateTask is a
        // running-room command.
        for task in resolved.tasks.into_iter().filter(|_| !restored) {
            let events = orchestrator
                .engine_mut()
                .apply(Command::CreateTask {
                    title: task.title,
                    brief: task.brief,
                    by: None,
                    required_role: task.required_role,
                    priority: task.priority,
                    acceptance_criteria: task.acceptance_criteria,
                    depends_on: vec![],
                    origin: TaskOrigin::plain(),
                })
                .map_err(|e| anyhow::anyhow!("seeding a task: {e}"))?;
            if task.open {
                if let Some(id) = events.iter().find_map(|e| e.event.task()) {
                    orchestrator
                        .engine_mut()
                        .apply(Command::OpenTask { task: id })
                        .map_err(|e| anyhow::anyhow!("opening a seed task: {e}"))?;
                }
            }
        }

        let (control_tx, control_rx) = mpsc::channel(64);
        let (events_tx, _) = broadcast::channel(BROADCAST_CAPACITY);

        let published = Arc::new(RwLock::new(Published {
            snapshot: snapshot_of(&orchestrator),
            backlog: orchestrator.engine().log().to_vec(),
        }));
        let interrupt = Arc::new(Mutex::new(CancellationToken::new()));
        let input_token = CancellationToken::new();
        orchestrator.attach_input_interrupt(input_token.clone());
        let input_interrupt = Arc::new(Mutex::new(input_token));
        let mut session = Self {
            state_path,
            saved_at: None,
            orchestrator,
            control: control_rx,
            events: events_tx.clone(),
            broadcast_head: Seq::ZERO,
            settled: false,
            published: published.clone(),
            interrupt: interrupt.clone(),
            input_interrupt: input_interrupt.clone(),
            mcp,
        };
        let handle = SessionHandle {
            control: control_tx,
            events: events_tx,
            published,
            interrupt,
            input_interrupt,
        };
        session.save_state()?;
        Ok((session, handle))
    }

    /// Save completed state atomically before announcing it to clients.
    fn save_state(&mut self) -> anyhow::Result<()> {
        use std::io::Write;
        let Some(path) = &self.state_path else {
            return Ok(());
        };
        // Scoped, because checkpoint() below takes the engine again and this
        // lock is not reentrant.
        let stamp = {
            let engine = self.orchestrator.engine();
            let room = engine.room();
            (engine.head(), room.phase, room.turns_used)
        };
        if self.saved_at == Some(stamp) {
            return Ok(());
        }
        let mut board = self.orchestrator.checkpoint();
        // Deliverables leave the board before the board is written, so it can
        // never reference a file that is not there yet. A body that fails to
        // write stays put: a larger board is a problem, a lost deliverable is
        // not recoverable.
        let bodies = crate::bodies::directory_for(path);
        if let Err(error) = crate::bodies::spill(&mut board, &bodies) {
            tracing::warn!(%error, "artifact bodies stayed in the board");
        }
        let bytes = serde_json::to_vec(&board)?;
        anyhow::ensure!(
            bytes.len() <= 64 * 1024 * 1024,
            "Room memory exceeds 64 MiB"
        );
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Invalid memory path"))?;
        std::fs::create_dir_all(parent)?;
        let temp = parent.join(format!(".{}.tmp", orgbot_proto::EventId::new()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| -> std::io::Result<()> {
            let mut file = options.open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&temp, path)?;
            std::fs::File::open(parent)?.sync_all()
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result?;
        self.saved_at = Some(stamp);
        Ok(())
    }

    /// Run the room to a stopping point.
    ///
    /// Bots yield API waits for human input, finishing any workspace operation
    /// already in progress before applying it. When no
    /// bot has anything to do the session waits — it does not close — because a
    /// person may still speak. It stops only when the completion rule is met, a
    /// budget is spent, or every client and control sender has gone away.
    pub fn attach_workspace(&mut self, workspace: orgbot_runtime::workspace::Workspace) {
        self.orchestrator.attach_workspace(workspace);
    }

    pub async fn run(self) -> RunOutcome {
        self.run_mode(false).await
    }

    /// A batch run has no human input to wait for.
    pub async fn run_batch(self) -> RunOutcome {
        self.run_mode(true).await
    }

    async fn run_mode(mut self, batch: bool) -> RunOutcome {
        // What the room was told to use, plus whatever the connected computer
        // already has registered. Inheriting the machine's own list is the
        // point: it is the same list a CLI running there would read, and it is
        // exactly what an API-backed bot has been missing.
        let mut specs = self.mcp.clone();
        if let Some(workspace) = self.orchestrator.workspace() {
            for found in orgbot_runtime::mcp_client::discover_on_workspace(workspace).await {
                if !specs.iter().any(|s| s.name == found.name) {
                    specs.push(found);
                }
            }
        }
        if !specs.is_empty() {
            let servers = orgbot_runtime::mcp_client::McpClient::connect_with(
                &specs,
                self.orchestrator.workspace(),
            )
            .await;
            tracing::info!(
                servers = servers.len(),
                tools = servers.tools().len(),
                "mcp servers available to this room"
            );
            self.orchestrator.attach_mcp_servers(servers);
        }
        let mut ticker = tokio::time::interval(IDLE_TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            // Apply any control that arrived, without blocking.
            match self.drain_control() {
                ControlFlow::Continue => {}
                ControlFlow::Stop(outcome) => return self.close(outcome),
            }

            // Whether the room should stop taking turns for now.
            //
            // Idle and Stalled both mean "nothing useful to do right now",
            // which for a chat you can talk to is a resting state rather than
            // an ending — but the room must actually rest. Continuing to take
            // turns while stalled burned the whole turn budget in milliseconds
            // and paused the room before a client could even connect.
            let outcome = self.orchestrator.stop_condition();
            if batch {
                if let Some(outcome) = outcome {
                    return self.close(outcome);
                }
                if self.orchestrator.next_actor().is_none() {
                    return self.close(RunOutcome::Idle);
                }
            }
            let resting = match outcome {
                None => false,
                Some(RunOutcome::Idle | RunOutcome::Stalled { .. } | RunOutcome::Paused) => true,
                Some(settled) => {
                    if !self.settled {
                        self.settled = true;
                        self.finish(&settled);
                    }
                    true
                }
            };

            if !resting && self.orchestrator.next_actor().is_some() {
                self.step().await;
                continue;
            }

            // Nobody actionable: wait for a person or the housekeeping tick.
            tokio::select! {
                control = self.control.recv() => match control {
                    Some(control) => {
                        if let ControlFlow::Stop(outcome) = self.handle(control) {
                            return self.close(outcome);
                        }
                    }
                    // Every client and control sender has gone: nothing can
                    // reach this room again, so it is safe to stop.
                    None => return self.close(RunOutcome::Idle),
                },
                _ = ticker.tick() => {
                    let _ = self.orchestrator.engine_mut().apply(Command::Tick {
                        now: Timestamp::now(),
                    });
                    self.broadcast_new_events();
                }
            }
        }
    }

    /// Give a batch of bots a turn, streaming and broadcasting the result.
    ///
    /// Only the first bot of the batch is streamed. Deltas from several turns
    /// on one channel would interleave into text none of them wrote, and the
    /// others' work still reaches the room as events.
    async fn step(&mut self) {
        // Peek who will act so the live stream can be tagged with the bot. The
        // scheduler is deterministic on state, which does not change between
        // this peek and the step.
        let batch = self.orchestrator.batch_size();
        let actor = self.orchestrator.next_actors(batch).first().copied();
        let sink = actor.map(|agent| self.spawn_stream_forwarder(agent));

        let interrupt = self.interrupt.lock().unwrap().clone();
        // Scoped: the turns below take the engine for every command they make.
        let (limit, remaining) = {
            let engine = self.orchestrator.engine();
            let room = engine.room();
            let limit = room.spec.limits.max_duration_ms;
            (
                limit,
                (limit - room.created_at.elapsed_to(Timestamp::now())).max(0) as u64,
            )
        };
        let deadline = async {
            if limit > 0 {
                tokio::time::sleep(Duration::from_millis(remaining)).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            biased;
            _ = interrupt.cancelled() => {},
            _ = deadline => {},
            _ = self.orchestrator.step_many(batch, sink) => {},
        }
        self.broadcast_new_events();
        let phase = self.orchestrator.engine().room().phase;
        let _ = self.events.send(ServerMessage::Phase { phase });
    }

    /// Wire a bot's streaming deltas onto the broadcast, tagged with the bot.
    fn spawn_stream_forwarder(
        &self,
        agent: orgbot_proto::AgentId,
    ) -> mpsc::Sender<orgbot_providers::StreamEvent> {
        let (tx, mut rx) = mpsc::channel(64);
        let events = self.events.clone();
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                // A closed broadcast (no subscribers) is fine; the durable
                // event still lands in the log.
                let _ = events.send(ServerMessage::Stream { agent, event });
            }
        });
        tx
    }

    /// Broadcast every log event not yet sent.
    fn broadcast_new_events(&mut self) {
        if self.save_state().is_err() {
            if self.orchestrator.engine().room().phase.is_live() {
                let _ = self.orchestrator.engine_mut().apply(Command::PauseRoom {
                    reason: "Room memory could not be saved".into(),
                });
            }
            let _ = self.events.send(ServerMessage::Error {
                message: "Room memory could not be saved. Work is paused; free storage and resume."
                    .into(),
            });
        }
        let new: Vec<_> = self
            .orchestrator
            .engine()
            .events_since(self.broadcast_head)
            .cloned()
            .collect();
        {
            let mut published = self.published.write().unwrap();
            published.snapshot = snapshot_of(&self.orchestrator);
            let head = published.backlog.last().map(|e| e.seq).unwrap_or(Seq::ZERO);
            published
                .backlog
                .extend(new.iter().filter(|e| e.seq > head).cloned());
        }
        let _ = self.events.send(ServerMessage::Phase {
            phase: self.orchestrator.engine().room().phase,
        });
        // Whether anything a person's view depends on has moved. Checked
        // before the events are consumed below.
        let board_moved = new.iter().any(|e| touches_the_board(&e.event));

        for envelope in new {
            self.broadcast_head = self.broadcast_head.max(envelope.seq);
            let _ = self.events.send(ServerMessage::Event {
                notification: orgbot_engine::notification_for(&envelope.event),
                envelope: Box::new(envelope),
            });
        }

        // The digest is a fold over every task, so it cannot be patched from
        // one event. Recomputing and resending it whole is both correct and
        // cheap next to the task records already on the wire.
        if board_moved {
            let _ = self.events.send(ServerMessage::Digest {
                digest: Box::new(self.orchestrator.engine().digest()),
            });
        }
    }

    /// Apply all immediately-available control messages.
    fn drain_control(&mut self) -> ControlFlow {
        loop {
            match self.control.try_recv() {
                Ok(control) => {
                    if let ControlFlow::Stop(outcome) = self.handle(control) {
                        return ControlFlow::Stop(outcome);
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => return ControlFlow::Continue,
                // All handles dropped: keep running for the bots' sake, but
                // there will be no more human input.
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    return ControlFlow::Stop(RunOutcome::Idle)
                }
            }
        }
    }

    /// Act on one control message.
    fn handle(&mut self, control: Control) -> ControlFlow {
        if matches!(
            control,
            Control::HumanMessage { .. } | Control::Answer { .. } | Control::AnswerDecision { .. }
        ) {
            let mut token = self.input_interrupt.lock().unwrap();
            *token = CancellationToken::new();
            self.orchestrator.attach_input_interrupt(token.clone());
        }
        match control {
            Control::Snapshot(reply) => {
                let _ = reply.send(self.snapshot());
            }
            Control::Backlog { since, reply } => {
                let events = self
                    .orchestrator
                    .engine()
                    .events_since(since)
                    .cloned()
                    .collect();
                let _ = reply.send(events);
            }
            Control::HumanMessage {
                author,
                body,
                task,
                files,
            } => {
                let root = self
                    .state_path
                    .as_ref()
                    .and_then(|p| p.parent())
                    .map(|p| p.join("attachments"));
                let files = match orgbot_runtime::attachments::manifests(root.as_deref(), &files) {
                    Ok(files) => files,
                    Err(message) => {
                        let _ = self.events.send(ServerMessage::Error {
                            message: format!(
                                "Message was not sent: {message}. Your files are kept."
                            ),
                        });
                        return ControlFlow::Continue;
                    }
                };
                let room = self.orchestrator.engine().room_id();
                let audience = human_audience(&self.orchestrator.engine(), &body);
                let mut message = ChatMessage {
                    id: orgbot_proto::MessageId::new(),
                    room,
                    author: Author::Human { name: author },
                    audience,
                    intent: MessageIntent::Ask,
                    body,
                    task,
                    handoff: None,
                    reply_to: None,
                    attachments: vec![],

                    files,
                    sent_at: Timestamp::now(),
                };
                // A human message is addressed to the room and expects a reply,
                // so it wakes the bots.
                message.intent = MessageIntent::Ask;
                let completed = self.orchestrator.engine().room().phase == RoomPhase::Completed;
                if let Err(error) = self.orchestrator.engine_mut().apply(Command::PostMessage {
                    message: Box::new(message),
                }) {
                    let _ = self.events.send(ServerMessage::Error {
                        message: format!("Message was not sent: {error}"),
                    });
                    return ControlFlow::Continue;
                }
                if completed {
                    *self.interrupt.lock().unwrap() = CancellationToken::new();
                    self.settled = false;
                }
                // Someone spoke: give the room a fresh budget so it does not
                // immediately re-declare itself stalled on old history.
                self.orchestrator.wake();
                self.broadcast_new_events();
            }
            Control::Answer { task, answer } => {
                self.answer_waiting_task(task, answer);
                self.broadcast_new_events();
            }
            Control::AnswerDecision {
                decision,
                chosen,
                note,
            } => {
                let decided = if note.trim().is_empty() {
                    format!("Decision: {chosen}.")
                } else {
                    format!("Decision: {chosen}. {}", note.trim())
                };
                // A refused answer is not worth stopping the room for: the
                // question stays open and the person can try again.
                let _ = self
                    .orchestrator
                    .engine_mut()
                    .apply(Command::AnswerDecision {
                        decision,
                        answer: orgbot_proto::decision::Answer {
                            chosen,
                            note,
                            at: orgbot_proto::Timestamp::now(),
                        },
                    });
                // Put the answer in the transcript too. The board already
                // knows, but a bot reading back over the room should see the
                // decision where it was made, not have to infer it from a
                // task quietly becoming claimable.
                let room = self.orchestrator.engine().room_id();
                let _ = self
                    .orchestrator
                    .engine_mut()
                    .apply(Command::PostMessage {
                        message: Box::new(ChatMessage {
                            id: orgbot_proto::MessageId::new(),
                            room,
                            author: Author::Human {
                                name: "operator".into(),
                            },
                            audience: Audience::Everyone,
                            intent: MessageIntent::Answer,
                            body: decided,
                            task: None,
                            handoff: None,
                            reply_to: None,
                            attachments: vec![],
                            files: vec![],
                            sent_at: Timestamp::now(),
                        }),
                    });
                self.broadcast_new_events();
            }
            Control::Stop => return ControlFlow::Stop(RunOutcome::Idle),
            Control::Pause { reason } => {
                if self.orchestrator.engine().room().phase.is_finished() {
                    return ControlFlow::Continue;
                }
                let _ = self
                    .orchestrator
                    .engine_mut()
                    .apply(Command::PauseRoom { reason });
                self.broadcast_new_events();
            }
            Control::Resume => {
                if !matches!(
                    self.orchestrator.engine().room().phase,
                    RoomPhase::Paused | RoomPhase::Completed
                ) {
                    return ControlFlow::Continue;
                }
                *self.interrupt.lock().unwrap() = CancellationToken::new();
                let _ = self.orchestrator.engine_mut().apply(Command::ResumeRoom);
                self.orchestrator.wake();
                self.settled = false;
                self.broadcast_new_events();
            }
        }
        ControlFlow::Continue
    }

    /// Unblock a task that was waiting on a human, recording the answer as a
    /// message so the working bot sees it on its next turn.
    fn answer_waiting_task(&mut self, task: TaskId, answer: String) {
        // Read what is needed and release the engine: writing below takes it
        // again, and holding it across both would be waiting on ourselves.
        let (room, assignee) = {
            let engine = self.orchestrator.engine();
            let Some(record) = engine.task(task) else {
                return;
            };
            let waiting_on_human = record.state == TaskState::Blocked
                && record.blocked_on.iter().any(BlockReason::needs_human);
            if !waiting_on_human {
                return;
            }
            (engine.room_id(), record.assignee)
        };

        // Post the answer, directed at the bot that asked so it is woken.
        let mut message = ChatMessage {
            id: orgbot_proto::MessageId::new(),
            room,
            author: Author::Human {
                name: "operator".into(),
            },
            audience: assignee
                .map(|agent| Audience::Agents {
                    agents: vec![agent],
                })
                .unwrap_or(Audience::Everyone),
            intent: MessageIntent::Answer,
            body: answer,
            task: Some(task),
            handoff: None,
            reply_to: None,
            attachments: vec![],

            files: vec![],
            sent_at: Timestamp::now(),
        };
        message.intent = MessageIntent::Answer;
        let mut engine = self.orchestrator.engine_mut();
        let _ = engine.apply(Command::PostMessage {
            message: Box::new(message),
        });
        let _ = engine.apply(Command::UnblockTask { task });
    }

    /// A full snapshot of the room right now.
    pub fn snapshot(&self) -> ServerMessage {
        snapshot_of(&self.orchestrator)
    }

    /// Record that the room has finished, without disconnecting anyone.
    ///
    /// Deliberately does not send [`ServerMessage::Closed`]: that tells clients
    /// to tear down, and a finished room should still be readable.
    fn finish(&mut self, outcome: &RunOutcome) {
        let command = match outcome {
            RunOutcome::OutOfTime | RunOutcome::OutOfTurns => Command::PauseRoom { reason: describe(outcome) },
            _ => Command::CloseRoom { reason: describe(outcome) },
        };
        if self.orchestrator.engine().room().phase != RoomPhase::Completed {
            let _ = self.orchestrator.engine_mut().apply(command);
            self.broadcast_new_events();
        }
        let _ = self.events.send(ServerMessage::Phase {
            phase: self.orchestrator.engine().room().phase,
        });
    }

    /// Close the room and tell every subscriber why.
    fn close(mut self, outcome: RunOutcome) -> RunOutcome {
        // Only a genuinely finished room emits room_closed; a pause or an idle
        // wait that ended because the last client left leaves it resumable.
        if matches!(outcome, RunOutcome::Completed | RunOutcome::OutOfTurns | RunOutcome::OutOfTime) {
            self.finish(&outcome);
        }
        let _ = self.events.send(ServerMessage::Closed {
            reason: describe(&outcome),
        });
        outcome
    }
}

fn snapshot_of(orchestrator: &Orchestrator) -> ServerMessage {
    let engine = orchestrator.engine();
    let roster = engine
        .agents()
        .map(|spec| AgentView {
            id: spec.id,
            handle: spec.handle.clone(),
            display_name: spec.display_name.clone(),
            roles: spec.roles.clone(),
            status: engine.status(spec.id),
            error_streak: engine.error_streak(spec.id),
        })
        .collect();
    ServerMessage::Snapshot {
        wire_version: crate::protocol::WIRE_VERSION,
        room: Box::new(engine.room().clone()),
        roster,
        tasks: engine.tasks().cloned().collect(),
        head: engine.head(),
        digest: Box::new(engine.digest()),
    }
}

/// Whether an event changes what a person sees on the objectives view.
///
/// Chat and streaming deltas do not: a room that is only talking has not
/// changed its board, and resending the digest for every token would drown
/// the useful updates.
fn touches_the_board(event: &orgbot_proto::event::Event) -> bool {
    use orgbot_proto::event::Event;
    matches!(
        event,
        Event::TaskCreated { .. }
            | Event::TaskTransitioned { .. }
            | Event::TaskAuthorized { .. }
            | Event::DecisionAsked { .. }
            | Event::DecisionAnswered { .. }
            | Event::ReviewRecorded { .. }
            | Event::ArtifactAdded { .. }
    )
}

/// Tags direct attention; the room transcript remains visible to everyone.
fn human_audience(engine: &orgbot_engine::Engine, body: &str) -> Audience {
    let tags: Vec<_> = body
        .split(|c: char| !(c.is_alphanumeric() || matches!(c, '@' | '_' | '-')))
        .filter_map(|word| word.strip_prefix('@'))
        .filter(|tag| !tag.is_empty())
        .collect();
    if tags.is_empty() || tags.iter().any(|tag| tag.eq_ignore_ascii_case("all")) {
        return Audience::Everyone;
    }
    Audience::Agents {
        agents: engine
            .agents()
            .filter(|agent| {
                tags.iter()
                    .any(|tag| tag.eq_ignore_ascii_case(&agent.handle))
            })
            .map(|agent| agent.id)
            .collect(),
    }
}

/// Whether the run loop should keep going or stop.
enum ControlFlow {
    Continue,
    Stop(RunOutcome),
}

/// A human-readable reason for a stop.
fn describe(outcome: &RunOutcome) -> String {
    match outcome {
        RunOutcome::Completed => "the room's goal was met".into(),
        RunOutcome::Idle => "everyone idle and no clients remain".into(),
        RunOutcome::OutOfTurns => "the room spent its turn budget".into(),
        RunOutcome::OutOfTime => "the room ran out of time".into(),
        RunOutcome::Stalled { rounds } => {
            format!("the room stalled after {rounds} fruitless turns")
        }
        RunOutcome::Paused => "the room was paused".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RoomConfig;

    const SCRIPTED_ROOM: &str = r#"
        [room]
        name = "test"
        goal = "do a thing"
        completion = "all_tasks_terminal"

        [[bots]]
        handle = "lead"
        roles = ["coordinator"]
        autonomy = "coordinator"
        provider = { kind = "scripted", script = "nothing to add" }

        [[tasks]]
        title = "the only task"
        brief = "just exist"
    "#;

    fn session() -> (RoomSession, SessionHandle) {
        let resolved = RoomConfig::parse(SCRIPTED_ROOM, "t.toml")
            .unwrap()
            .resolve()
            .unwrap();
        RoomSession::build(resolved).unwrap()
    }

    #[tokio::test]
    async fn exhausted_budgets_pause_unfinished_work_and_resume_without_losing_it() {
        for outcome in [RunOutcome::OutOfTime, RunOutcome::OutOfTurns] {
            let (mut session, _) = session();
            let task = session.orchestrator.engine().tasks().next().unwrap().id;
            session.finish(&outcome);
            assert_eq!(session.orchestrator.engine().room().phase, RoomPhase::Paused);
            assert!(session.orchestrator.engine().task(task).is_some());
            assert!(!session.orchestrator.engine().log().iter().any(|e| matches!(e.event, orgbot_proto::Event::RoomClosed { .. })));
            session.handle(Control::Resume);
            assert_eq!(session.orchestrator.engine().room().phase, RoomPhase::Running);
            assert!(session.orchestrator.engine().task(task).is_some());
        }
    }

    #[tokio::test]
    async fn completed_rooms_answer_followups_and_resume_saved_unread_requests() {
        for restored_request in [false, true] {
            let (mut session, mut handle) = session();
            let task = session.orchestrator.engine().tasks().next().unwrap().id;
            session
                .orchestrator
                .engine_mut()
                .apply(Command::CancelTask {
                    task,
                    reason: "prior work settled".into(),
                })
                .unwrap();
            session.finish(&RunOutcome::Completed);
            session.settled = true;
            let id = session.orchestrator.engine().room_id();
            let dir = std::env::temp_dir().join(format!("orgbot-continue-{id}"));
            if restored_request {
                // Reproduce an older checkpoint: saved human input exists but
                // the phase is still completed and no bot consumed the request.
                session.handle(Control::HumanMessage {
                    author: "Alex".into(),
                    body: "Read the context I already sent".into(),
                    task: None,
                    files: vec![],
                });
                session
                    .orchestrator
                    .engine_mut()
                    .apply(Command::CloseRoom {
                        reason: "old completed state".into(),
                    })
                    .unwrap();
                std::fs::create_dir_all(&dir).unwrap();
                session.state_path = Some(dir.join("state.json"));
                session.save_state().unwrap();
                let mut config = RoomConfig::parse(SCRIPTED_ROOM, "test.toml").unwrap();
                config.room.state_path = session.state_path.clone();
                (session, handle) = RoomSession::build(config.resolve().unwrap()).unwrap();
                assert_eq!(session.orchestrator.engine().room_id(), id);
            }
            let mut events = handle.subscribe();
            let run = tokio::spawn(session.run());
            for pass in 0..2 {
                if restored_request && pass == 0 {
                    handle.send(Control::Resume).await;
                } else {
                    handle
                        .send(Control::HumanMessage {
                            author: "Alex".into(),
                            body: format!("Follow-up {pass}"),
                            task: None,
                            files: vec![],
                        })
                        .await;
                }
                let mut answered = false;
                tokio::time::timeout(Duration::from_secs(3), async {
                    loop {
                        match events.recv().await.unwrap() {
                            ServerMessage::Event { envelope, .. } => {
                                if let orgbot_proto::Event::MessagePosted { message } = envelope.event {
                                    if message.author.agent().is_some() {
                                        answered = true;
                                    }
                                }
                            }
                            ServerMessage::Phase {
                                phase: RoomPhase::Completed,
                            } if answered => break,
                            _ => {}
                        }
                    }
                })
                .await
                .expect(
                    "a completed board must not swallow a new request or prevent settling again",
                );
                let ServerMessage::Snapshot { room, .. } = handle.snapshot().await.unwrap() else {
                    panic!("snapshot")
                };
                assert_eq!(room.id, id);
                assert_eq!(room.phase, RoomPhase::Completed);
            }
            // An explicit pause continues to park messages until Resume.
            // Post directly because Pause ignores an already-completed session.
            run.abort();
            let _ = run.await;
            if dir.exists() {
                std::fs::remove_dir_all(dir).unwrap();
            }
        }
        let (mut session, _) = session();
        session.handle(Control::Pause {
            reason: "owner paused".into(),
        });
        session.handle(Control::HumanMessage {
            author: "Alex".into(),
            body: "Kept until resume".into(),
            task: None,
            files: vec![],
        });
        assert_eq!(
            session.orchestrator.engine().room().phase,
            RoomPhase::Paused
        );
    }

    #[test]
    fn human_tags_route_attention_and_ignore_email_addresses() {
        let (session, _handle) = session();
        let engine = session.orchestrator.engine();
        let lead = engine.agent_by_handle("lead").unwrap().id;
        assert_eq!(
            human_audience(&engine, "@LEAD, please check"),
            Audience::Agents { agents: vec![lead] }
        );
        assert_eq!(
            human_audience(&engine, "@lead @lead check"),
            Audience::Agents { agents: vec![lead] }
        );
        assert_eq!(
            human_audience(&engine, "@missing check"),
            Audience::Agents { agents: vec![] }
        );
        assert_eq!(human_audience(&engine, "@all hello"), Audience::Everyone);
        assert_eq!(
            human_audience(&engine, "mail foo@lead.example"),
            Audience::Everyone
        );
    }

    #[test]
    fn prompt_labels_the_actual_recipient_and_exposes_existing_artifacts() {
        let (mut session, _handle) = session();
        let mut engine = session.orchestrator.engine_mut();
        let agent = engine.agent_by_handle("lead").unwrap().clone();
        let task = engine.tasks().next().unwrap().id;
        engine
            .apply(Command::ClaimTask { task, by: agent.id })
            .unwrap();
        engine
            .apply(Command::StartTask { task, by: agent.id })
            .unwrap();
        let outcome = orgbot_tools::dispatch(
            &mut engine,
            agent.id,
            "add_artifact",
            &serde_json::json!({
                "task":task, "name":"Product context", "content":"A social app, not a work tracker."
            }),
        );
        assert!(!outcome.is_error, "{}", outcome.content);
        let room = engine.room_id();
        engine
            .apply(Command::PostMessage {
                message: Box::new(ChatMessage {
                    id: orgbot_proto::MessageId::new(),
                    room,
                    author: Author::Human {
                        name: "Alex".into(),
                    },
                    audience: Audience::Agents { agents: vec![] },
                    intent: MessageIntent::Ask,
                    body: "hello another bot".into(),
                    task: None,
                    handoff: None,
                    reply_to: None,
                    attachments: vec![],

                    files: vec![],
                    sent_at: Timestamp::now(),
                }),
            })
            .unwrap();
        let prompt = orgbot_runtime::prompt::situation(&engine, agent.id, Seq::ZERO);
        assert!(prompt.contains("Alex (to other bots): hello another bot"));
        assert!(!prompt.contains("Alex (to you)"));
        assert!(prompt.contains("[artifact available]"));
        assert!(prompt.contains("Product context"));
    }

    #[test]
    fn a_built_session_has_seeded_its_board_and_started() {
        let (session, _handle) = session();
        assert_eq!(
            session.orchestrator.engine().room().phase,
            RoomPhase::Running
        );
        assert_eq!(session.orchestrator.engine().tasks().count(), 1);
        let engine = session.orchestrator.engine();
        let task = engine.tasks().next().unwrap();
        assert_eq!(
            task.state,
            TaskState::Ready,
            "a seeded task with open=true is claimable"
        );
    }

    #[test]
    fn a_snapshot_carries_the_roster_and_the_board() {
        let (session, _handle) = session();
        match session.snapshot() {
            ServerMessage::Snapshot { roster, tasks, .. } => {
                assert_eq!(roster.len(), 1);
                assert_eq!(roster[0].handle, "lead");
                assert_eq!(tasks.len(), 1);
            }
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_subscriber_receives_events_as_the_room_runs() {
        let (session, handle) = session();
        let mut events = handle.subscribe();
        let run = tokio::spawn(session.run());

        // The scripted coordinator has nothing to do with a task it cannot
        // work, so the room settles into rest quickly. A room you can talk to
        // does not end there — it waits — so close it the way a client does,
        // by going away.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        drop(handle);
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), run)
            .await
            .expect("a resting room must stop once its last client is gone")
            .unwrap();
        assert!(matches!(
            outcome,
            RunOutcome::Stalled { .. } | RunOutcome::Idle | RunOutcome::Completed
        ));

        let mut saw_something = false;
        while let Ok(message) = events.try_recv() {
            if matches!(
                message,
                ServerMessage::Event { .. } | ServerMessage::Closed { .. }
            ) {
                saw_something = true;
            }
        }
        assert!(saw_something, "a subscriber should see the room's activity");
    }

    #[tokio::test]
    async fn a_snapshot_can_be_requested_over_the_handle() {
        let (session, handle) = session();
        let run = tokio::spawn(session.run());

        let snapshot = handle.snapshot().await;
        // The room may already be resting when the request lands; either a
        // snapshot or a cleanly stopped room is acceptable, never a hang.
        if let Some(ServerMessage::Snapshot { roster, .. }) = snapshot {
            assert_eq!(roster.len(), 1);
        }
        drop(handle);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), run).await;
    }

    #[tokio::test]
    async fn a_human_message_is_accepted_while_the_room_runs() {
        let (session, handle) = session();
        let mut events = handle.subscribe();
        let run = tokio::spawn(session.run());

        handle
            .send(Control::HumanMessage {
                files: vec![],
                author: "Alex".into(),
                body: "any progress?".into(),
                task: None,
            })
            .await;

        // Give the room a moment to fold the message into the transcript, then
        // close it: a resting room waits for a client rather than ending.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        drop(handle);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), run).await;

        let mut saw_message = false;
        while let Ok(message) = events.try_recv() {
            if let ServerMessage::Event { envelope, .. } = message {
                if let orgbot_proto::Event::MessagePosted { message } = &envelope.event {
                    if message.body.contains("any progress") {
                        saw_message = true;
                    }
                }
            }
        }
        assert!(saw_message, "the human message should reach the transcript");
    }
}

#[cfg(test)]
mod resting_tests {
    use super::*;
    use crate::config::RoomConfig;
    use orgbot_proto::event::Event;

    /// A roster with nothing productive to do: one worker that only talks, and
    /// a task nobody can finish. This is the shape that used to kill a room
    /// before a client could connect.
    const UNPRODUCTIVE: &str = r#"
        [room]
        name = "test"
        goal = "respond"
        completion = "all_tasks_terminal"

        [[bots]]
        handle = "dennis"
        roles = ["engineer"]
        provider = { kind = "scripted", script = "Working on it." }

        [[tasks]]
        title = "respond"
        brief = "respond"
    "#;

    fn session() -> (RoomSession, SessionHandle) {
        let resolved = RoomConfig::parse(UNPRODUCTIVE, "t.toml")
            .unwrap()
            .resolve()
            .unwrap();
        RoomSession::build(resolved).unwrap()
    }

    /// The bug this guards: a stalled room kept taking turns, burned its whole
    /// budget in milliseconds, paused itself, and every later client was told
    /// the room had already finished.
    #[tokio::test]
    async fn a_room_with_nothing_to_do_rests_instead_of_burning_its_budget() {
        let (session, handle) = session();
        let run = tokio::spawn(session.run());

        // Give it long enough to have spent 500 turns if it were spinning.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;

        let snapshot = handle.snapshot().await.expect("the room must still be up");
        match snapshot {
            ServerMessage::Snapshot { room, .. } => {
                assert_eq!(
                    room.phase,
                    RoomPhase::Running,
                    "a room with nothing to do is resting, not finished"
                );
                assert!(
                    room.turns_used < room.spec.limits.max_total_turns,
                    "the room spent {} of {} turns while idle",
                    room.turns_used,
                    room.spec.limits.max_total_turns
                );
            }
            other => panic!("expected a snapshot, got {other:?}"),
        }

        drop(handle);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), run).await;
    }

    /// A client that attaches after the room has started must still see what
    /// was already said. The snapshot carries the roster and the board but not
    /// the transcript, so without a replay a message posted in the gap between
    /// the engine starting and the UI connecting simply vanished.
    #[tokio::test]
    async fn a_late_client_can_still_read_what_it_missed() {
        let (session, handle) = session();
        let run = tokio::spawn(session.run());

        // Let the room get going before anyone is listening.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;

        let backlog = handle.backlog(Seq::ZERO).await;
        assert!(
            !backlog.is_empty(),
            "the room's log must be replayable from the beginning"
        );
        assert!(
            backlog.windows(2).all(|pair| pair[0].seq < pair[1].seq),
            "the replay must be in order so a client can track what it applied"
        );

        // Asking for everything after the last one yields nothing new.
        let head = backlog.last().expect("a last event").seq;
        assert!(
            handle.backlog(head).await.is_empty(),
            "a caught-up client must not be sent the same events again"
        );

        drop(handle);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), run).await;
    }

    /// The other half of the same complaint: what you type must appear.
    #[tokio::test]
    async fn a_human_message_reaches_the_transcript_and_wakes_the_room() {
        let (session, handle) = session();
        let mut events = handle.subscribe();
        let run = tokio::spawn(session.run());

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(
            handle
                .send(Control::HumanMessage {
                    files: vec![],
                    author: "Tester".into(),
                    body: "hello?".into(),
                    task: None,
                })
                .await,
            "the room must still be accepting messages"
        );
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;

        let mut said_it = false;
        let mut bot_replied = false;
        while let Ok(message) = events.try_recv() {
            if let ServerMessage::Event { envelope, .. } = message {
                if let Event::MessagePosted { message } = &envelope.event {
                    if message.body.contains("hello?") {
                        said_it = true;
                    }
                    if message.author.agent().is_some() {
                        bot_replied = true;
                    }
                }
            }
        }
        assert!(said_it, "what the person typed must appear in the room");
        assert!(bot_replied, "and a bot must be woken to answer it");

        drop(handle);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), run).await;
    }
}
