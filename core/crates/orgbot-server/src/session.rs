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

use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use orgbot_engine::Command;
use orgbot_proto::chat::Audience;
use orgbot_proto::event::{EventEnvelope, Seq};
use orgbot_proto::graph::TaskOrigin;
use orgbot_proto::task::BlockReason;
use orgbot_proto::{
    Author, ChatMessage, MessageId, MessageIntent, RoomPhase, TaskId, TaskState, Timestamp,
};
use orgbot_runtime::{Orchestrator, RunOutcome};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::config::ResolvedRoom;
use crate::protocol::{AgentView, MemoryAction, MemoryRequest, MemorySearchItem, ServerMessage};

/// How often the session wakes on its own to expire stale handoffs and sweep
/// for deadlocks while it is otherwise idle.
const IDLE_TICK: Duration = Duration::from_secs(5);

/// How often work done during a batch is sent to clients while it runs.
const LIVE_FLUSH: Duration = Duration::from_millis(500);

/// Desktop alerts for when no app window is open, where the platform has them.
#[cfg(target_os = "linux")]
type Alerts = Option<crate::desktop_notifications::DesktopNotifications>;
#[cfg(not(target_os = "linux"))]
type Alerts = ();

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
        /// The message being replied to, when this is part of a thread.
        reply_to: Option<MessageId>,
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
    /// Perform an owner-authorized project memory operation.
    Memory {
        request: MemoryRequest,
        reply: oneshot::Sender<ServerMessage>,
    },
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
        let stop = matches!(&control, Control::Stop);
        if self.try_send(control) {
            return true;
        }
        if !stop {
            return false;
        }
        // Shutdown must be able to interrupt an in-flight turn even when
        // ordinary queued controls filled the bounded channel.
        self.interrupt.lock().unwrap().cancel();
        self.input_interrupt.lock().unwrap().cancel();
        self.control.send(Control::Stop).await.is_ok()
    }

    pub fn try_send(&self, control: Control) -> bool {
        let stop = matches!(control, Control::Stop);
        let interrupts = matches!(control, Control::Pause { .. } | Control::Stop);
        // Human input never cuts a running turn short. Messages and decision
        // answers land in the room while turns run (see `step`), and each bot
        // reads them on its next turn; the bots already at work keep going.
        // Queue and cancel under the same lock: the session must not replace
        // this token between accepting a stop and receiving its signal.
        let input_interrupt = self.input_interrupt.lock().unwrap();
        if self.control.try_send(control).is_err() {
            return false;
        }
        if stop {
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
    desktop_notifications: Alerts,
    state_path: Option<std::path::PathBuf>,
    saved_at: Option<(Seq, orgbot_proto::room::RoomPhase, u32)>,
    orchestrator: Orchestrator,
    control: mpsc::Receiver<Control>,
    /// Controls that arrived while turns were running and wait for them to
    /// end, in arrival order, ahead of anything still in the channel.
    deferred: std::collections::VecDeque<Control>,
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
    memory_root: Option<std::path::PathBuf>,
    memory_project: Option<String>,
    memory_projects: Vec<String>,
    memory_setup_error: Option<String>,
    /// Attachment ids become importable only after their message is committed.
    committed_attachment_ids: Arc<RwLock<HashSet<String>>>,
}

impl RoomSession {
    /// Build a session from a resolved room, returning it and a handle.
    ///
    /// The room is seated and seeded but not started; call [`Self::run`] to
    /// begin. Errors here mean the room itself is invalid (a bad roster, a seed
    /// task that names a missing role); a caller should surface them rather than
    /// spawn a room that cannot work.
    pub fn build(resolved: ResolvedRoom) -> anyhow::Result<(Self, SessionHandle)> {
        Self::build_with_resume(resolved, false)
    }

    /// Build a room, optionally resuming only when its saved phase was running.
    /// The Linux supervisor uses this on process/login recovery; ordinary room
    /// opens remain paused as before.
    pub fn build_with_resume(
        mut resolved: ResolvedRoom,
        resume_was_running: bool,
    ) -> anyhow::Result<(Self, SessionHandle)> {
        let allowances = resolved.allowances.take();
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
        let saved_was_running = saved
            .as_ref()
            .and_then(saved_phase)
            .is_some_and(|phase| phase == "running");
        let roles = resolved.roles.clone();
        let skills = resolved.skills.clone();
        let mcp = resolved.mcp.clone();
        let memory_root = resolved.memory_root.clone();
        let memory_project = resolved.project_id.clone();
        let memory_projects = resolved.memory_projects.clone();
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
        if let Some(allowances) = allowances {
            orchestrator.attach_allowances(allowances);
        }
        if resume_was_running && saved_was_running {
            orchestrator
                .engine_mut()
                .apply(Command::ResumeRoom)
                .map_err(|error| {
                    anyhow::anyhow!("resuming the previously running room: {error}")
                })?;
        }

        // Applied to restored rooms too: the roles a room declares are current
        // configuration, not history, and a saved room should pick up a table
        // added since it was written.
        orchestrator.engine_mut().set_roles(roles);
        // Same reasoning: a skill edited on disk should take effect the next
        // time the room opens, not be frozen into state the day it was written.
        orchestrator.engine_mut().set_skills(skills);
        // Owner input that arrived mid-turn after the last save.
        if let Some(path) = &state_path {
            let attachments = path.parent().map(|p| p.join("attachments"));
            journal_replay(
                &mut orchestrator.engine_mut(),
                &journal_path(path),
                attachments.as_deref(),
            );
        }
        // The archive carries both earlier completed boards and the separate
        // project memory store. It remains present even when either source is
        // empty so owner controls have a single scoped API.
        let archive = if !resolved.prior_boards.is_empty() {
            let boards: Vec<std::path::PathBuf> = resolved
                .prior_boards
                .iter()
                .map(std::path::PathBuf::from)
                .collect();
            let this_room = orchestrator.engine().room().spec.name.clone();
            orgbot_engine::Archive::load(&boards, Some(&this_room))
        } else {
            orgbot_engine::Archive::default()
        };
        let memory_setup_error = match (&memory_root, &memory_project) {
            (Some(root), Some(project)) => archive
                .configure_memory(root, project)
                .err()
                .map(|error| error.to_string()),
            _ => None,
        };
        if !resolved.prior_boards.is_empty() {
            tracing::info!(rooms = archive.rooms().len(), "earlier rooms available");
        }
        orchestrator.engine_mut().set_archive(archive);

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
        let committed_attachment_ids = Arc::new(RwLock::new(
            orchestrator
                .engine()
                .transcript()
                .iter()
                .flat_map(|message| message.files.iter().map(|file| file.id.clone()))
                .collect(),
        ));
        let interrupt = Arc::new(Mutex::new(CancellationToken::new()));
        let input_token = CancellationToken::new();
        orchestrator.attach_input_interrupt(input_token.clone());
        // A run lasts as long as its slowest turn; the room is saved meanwhile.
        if let Some(path) = state_path.clone() {
            orchestrator.attach_autosave(orgbot_runtime::Autosave(Arc::new(move |board| {
                if let Err(error) = write_board(&path, board) {
                    tracing::warn!(%error, "room could not be saved during a run");
                }
            })));
        }
        let input_interrupt = Arc::new(Mutex::new(input_token));
        let mut session = Self {
            desktop_notifications: Alerts::default(),
            state_path,
            saved_at: None,
            orchestrator,
            control: control_rx,
            deferred: std::collections::VecDeque::new(),
            events: events_tx.clone(),
            broadcast_head: Seq::ZERO,
            settled: false,
            published: published.clone(),
            interrupt: interrupt.clone(),
            input_interrupt: input_interrupt.clone(),
            mcp,
            memory_root,
            memory_project,
            memory_projects,
            memory_setup_error,
            committed_attachment_ids,
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

    /// Shared, live set of attachment ids present on committed room messages.
    pub(crate) fn committed_attachment_ids(&self) -> Arc<RwLock<HashSet<String>>> {
        Arc::clone(&self.committed_attachment_ids)
    }

    pub(crate) fn state_path(&self) -> Option<&std::path::Path> {
        self.state_path.as_deref()
    }

    #[cfg(target_os = "linux")]
    pub fn enable_desktop_notifications(&mut self) {
        self.desktop_notifications = self
            .state_path
            .as_deref()
            .and_then(crate::desktop_notifications::DesktopNotifications::new);
    }

    /// Save completed state atomically before announcing it to clients.
    fn save_state(&mut self) -> anyhow::Result<()> {
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
        write_board(path, self.orchestrator.checkpoint())?;
        self.saved_at = Some(stamp);
        // Everything journaled mid-turn is in the board just written.
        let _ = std::fs::remove_file(journal_path(path));
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
            let servers = orgbot_runtime::mcp_client::McpClient::lazy(
                &specs,
                self.orchestrator.workspace(),
            );
            #[cfg(target_os = "linux")]
            let servers = match self.state_path.as_ref().and_then(|p| p.parent()) {
                Some(path) => servers.with_owner_approval(path.to_path_buf()),
                None => {
                    tracing::warn!("External MCP tools require a private state_path for durable owner approvals");
                    orgbot_runtime::mcp_client::McpClient::default()
                }
            };
            tracing::info!(
                servers = servers.server_names().len(),
                "mcp servers available on demand"
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
    /// Each running bot streams on its own channel, tagged with it, and a
    /// freed slot is filled while slower turns continue (see
    /// `Orchestrator::step_many_with`).
    async fn step(&mut self) {
        let batch = self.orchestrator.batch_size();
        // Every running bot gets its own live stream, tagged with it, so the
        // room shows each one working rather than only the first.
        let stream_events = self.events.clone();
        let sinks = move |agent| Some(stream_forwarder(stream_events.clone(), agent));

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
        // Turns can run for minutes (an agent CLI is never cut short), so the
        // room keeps listening meanwhile. Messages and decision answers are
        // recorded at once, without interrupting anyone: the engine is shared
        // and room tools already change it mid turn. Everything else waits, in
        // order, for the turns to end.
        let attachments_root = self.attachments_root();
        let default_target = self.orchestrator.default_human_target();
        let mut spoke = false;
        // How far this loop has sent the log on its own (see below).
        let mut sent_head = self.broadcast_head;
        let journal = self.state_path.as_deref().map(journal_path);
        let published = self.published.clone();
        let memory = self.memory_context();
        let engine = self.orchestrator.shared_engine();
        let events = self.events.clone();
        let control = &mut self.control;
        let deferred = &mut self.deferred;
        let alerts = &mut self.desktop_notifications;
        let committed = self.committed_attachment_ids.clone();
        let mut answered = false;
        // Scoped so the turns future (which borrows the orchestrator) is gone
        // before the room is touched again below.
        let interrupted = {
            let turns = self.orchestrator.step_rolling(batch, &sinks);
            tokio::pin!(turns);
            tokio::pin!(deadline);
            let mut live = tokio::time::interval(LIVE_FLUSH);
            live.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    _ = interrupt.cancelled() => break true,
                    _ = &mut deadline => break true,
                    _ = &mut turns => break false,
                    _ = live.tick() => send_unsent(&engine, &events, alerts, &committed, &published, &mut sent_head),
                    Some(received) = control.recv() => match received {
                        Control::AnswerDecision { decision, chosen, note } => {
                            let entry = JournaledInput::Decision {
                                decision,
                                chosen: chosen.clone(),
                                note: note.clone(),
                            };
                            if let Some(path) = &journal {
                                if journal_append(path, &entry).is_err() {
                                    // Not durable yet: apply it after the turns
                                    // and their save, as before.
                                    deferred.push_back(Control::AnswerDecision { decision, chosen, note });
                                    continue;
                                }
                            }
                            match engine.with(|engine| record_decision(engine, decision, chosen, note)) {
                                Err(message) => {
                                    let _ = events.send(ServerMessage::Error { message });
                                }
                                Ok(newly) => {
                                    answered |= newly;
                                    send_unsent(&engine, &events, alerts, &committed, &published, &mut sent_head);
                                    let digest = engine.with(|engine| engine.digest());
                                    let _ = events.send(ServerMessage::Digest {
                                        digest: Box::new(digest),
                                    });
                                }
                            }
                        }
                        Control::HumanMessage { author, body, task, files, reply_to } => {
                            let input = HumanInput {
                                id: MessageId::new(),
                                author,
                                body,
                                task,
                                files,
                                reply_to,
                            };
                            if let Some(path) = &journal {
                                let entry = JournaledInput::Message {
                                    input: input.clone(),
                                    default_target,
                                };
                                if journal_append(path, &entry).is_err() {
                                    deferred.push_back(Control::HumanMessage {
                                        author: input.author,
                                        body: input.body,
                                        task: input.task,
                                        files: input.files,
                                        reply_to: input.reply_to,
                                    });
                                    continue;
                                }
                            }
                            let posted = engine.with(|engine| {
                                record_human_message(
                                    engine,
                                    attachments_root.as_deref(),
                                    default_target,
                                    input,
                                )
                            });
                            match posted {
                                Err(message) => {
                                    let _ = events.send(ServerMessage::Error { message });
                                }
                                Ok(_) => {
                                    spoke = true;
                                    send_unsent(&engine, &events, alerts, &committed, &published, &mut sent_head);
                                }
                            }
                        }
                        // A bot blocked on a person gets its answer now, not
                        // when the slowest turn in the run ends.
                        Control::Answer { task, answer } => {
                            engine.with(|engine| answer_waiting_task(engine, task, answer));
                            spoke = true;
                            send_unsent(&engine, &events, alerts, &committed, &published, &mut sent_head);
                        }
                        // Answered now, under the engine lock the bots' room
                        // tools also take, so the memory panel does not time
                        // out behind a long turn.
                        Control::Memory { request, reply } => {
                            let _ = reply.send(engine.with(|engine| memory_reply(engine, &memory, request)));
                        }
                        other => deferred.push_back(other),
                    },
                }
            }
        };
        if answered || spoke {
            self.orchestrator.wake();
        }
        if answered {
            self.settled = false;
        }
        if interrupted {
            self.orchestrator.finish_interrupted_turns().await;
        }
        // What went out live is not sent again.
        self.broadcast_head = self.broadcast_head.max(sent_head);
        self.broadcast_new_events();
        let phase = self.orchestrator.engine().room().phase;
        let _ = self.events.send(ServerMessage::Phase { phase });
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
        *self.committed_attachment_ids.write().unwrap() =
            committed_attachments(&self.orchestrator.engine());
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
            announce(&self.events, &mut self.desktop_notifications, envelope);
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

    /// Where message attachments for this room are kept, if anywhere.
    fn attachments_root(&self) -> Option<std::path::PathBuf> {
        self.state_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|p| p.join("attachments"))
    }

    /// Apply all immediately-available control messages.
    fn drain_control(&mut self) -> ControlFlow {
        while let Some(control) = self.deferred.pop_front() {
            if let ControlFlow::Stop(outcome) = self.handle(control) {
                return ControlFlow::Stop(outcome);
            }
        }
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
                reply_to,
            } => {
                let root = self.attachments_root();
                let default_target = self.orchestrator.default_human_target();
                let completed = self.orchestrator.engine().room().phase == RoomPhase::Completed;
                let posted = {
                    let mut engine = self.orchestrator.engine_mut();
                    record_human_message(
                        &mut engine,
                        root.as_deref(),
                        default_target,
                        HumanInput {
                            id: MessageId::new(),
                            author,
                            body,
                            task,
                            files,
                            reply_to,
                        },
                    )
                };
                if let Err(message) = posted {
                    let _ = self.events.send(ServerMessage::Error { message });
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
                let outcome = {
                    let mut engine = self.orchestrator.engine_mut();
                    record_decision(&mut engine, decision, chosen, note)
                };
                match outcome {
                    Err(message) => {
                        let _ = self.events.send(ServerMessage::Error { message });
                        return ControlFlow::Continue;
                    }
                    // A lost acknowledgement may retry an already recorded
                    // answer; resend the digest so the client settles.
                    Ok(false) => {
                        let _ = self.events.send(ServerMessage::Digest {
                            digest: Box::new(self.orchestrator.engine().digest()),
                        });
                        return ControlFlow::Continue;
                    }
                    Ok(true) => {}
                }
                self.orchestrator.wake();
                self.settled = false;
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
            Control::Memory { request, reply } => {
                let _ = reply.send(self.handle_memory(request));
            }
        }
        ControlFlow::Continue
    }

    fn handle_memory(&self, request: MemoryRequest) -> ServerMessage {
        let engine = self.orchestrator.engine();
        memory_reply(&engine, &self.memory_context(), request)
    }

    /// The project-memory settings, detached from the session.
    fn memory_context(&self) -> MemoryContext {
        MemoryContext {
            setup_error: self.memory_setup_error.clone(),
            root: self.memory_root.clone(),
            project: self.memory_project.clone(),
            projects: self.memory_projects.clone(),
        }
    }
    /// Unblock a task that was waiting on a human, recording the answer as a
    /// message so the working bot sees it on its next turn.
    fn answer_waiting_task(&mut self, task: TaskId, answer: String) {
        answer_waiting_task(&mut self.orchestrator.engine_mut(), task, answer);
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
            RunOutcome::OutOfTime | RunOutcome::OutOfTurns => Command::PauseRoom {
                reason: describe(outcome),
            },
            _ => Command::CloseRoom {
                reason: describe(outcome),
            },
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
        if matches!(
            outcome,
            RunOutcome::Completed | RunOutcome::OutOfTurns | RunOutcome::OutOfTime
        ) {
            self.finish(&outcome);
        }
        let _ = self.events.send(ServerMessage::Closed {
            reason: describe(&outcome),
        });
        outcome
    }
}

fn saved_phase(value: &serde_json::Value) -> Option<&str> {
    value
        .get("engine")
        .and_then(|engine| engine.get("room"))
        .or_else(|| value.get("room"))
        .and_then(|room| room.get("phase"))
        .and_then(serde_json::Value::as_str)
}

fn snapshot_of(orchestrator: &Orchestrator) -> ServerMessage {
    snapshot_from(&orchestrator.engine())
}

/// What a connecting client loads first: the board as it stands now.
fn snapshot_from(engine: &orgbot_engine::Engine) -> ServerMessage {
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
            | Event::DecisionQuestionJoined { .. }
            | Event::DecisionAnswered { .. }
            | Event::ReviewRecorded { .. }
            | Event::ArtifactAdded { .. }
            | Event::UsageRecorded { .. }
    )
}

#[test]
fn usage_updates_the_live_room_digest_without_a_task_transition() {
    assert!(touches_the_board(
        &orgbot_proto::event::Event::UsageRecorded {
            agent: orgbot_proto::AgentId::new(),
            usage: orgbot_proto::spend::TokenUsage::missing(),
        }
    ));
}

/// Tags direct attention; the room transcript remains visible to everyone.
#[cfg(test)]
fn human_audience(
    engine: &orgbot_engine::Engine,
    body: &str,
    parent_author: Option<&Author>,
) -> Audience {
    let default_target = default_human_target(engine);
    human_audience_with_default(engine, body, parent_author, default_target)
}

/// Wire one bot's streaming deltas onto the broadcast, tagged with the bot.
fn stream_forwarder(
    events: broadcast::Sender<ServerMessage>,
    agent: orgbot_proto::AgentId,
) -> mpsc::Sender<orgbot_providers::StreamEvent> {
    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            // A closed broadcast (no subscribers) is fine; the durable
            // event still lands in the log.
            let _ = events.send(ServerMessage::Stream { agent, event });
        }
        // The turn is over, however it ended. A failed or interrupted turn
        // sends no completion of its own, and a bot that holds no task has
        // no status change to clear its indicator either.
        let _ = events.send(ServerMessage::Stream {
            agent,
            event: orgbot_providers::StreamEvent::Done {
                stop_reason: orgbot_providers::StopReason::EndTurn,
            },
        });
    });
    tx
}

/// Send, in order, every log event not yet sent, while turns are running.
///
/// Run on a short interval during a batch as well as after owner input: a bot
/// that finishes early must not have its messages and board changes held back
/// until the slowest turn in the batch ends, which read as a silent room.
///
/// The bots' own mid-turn events are included: a client that saw a later
/// sequence number first would drop the earlier ones as repeats. The
/// checkpoint catches up when the turns end, and the end-of-turn broadcast
/// sends only what this did not.
fn send_unsent(
    engine: &orgbot_runtime::SharedEngine,
    events: &broadcast::Sender<ServerMessage>,
    alerts: &mut Alerts,
    committed: &RwLock<HashSet<String>>,
    published: &RwLock<Published>,
    sent_head: &mut Seq,
) {
    // Read under one lock, so the snapshot is exactly as new as the events
    // that go out with it: a client loads the snapshot, then replays only
    // what comes after it.
    let (unsent, snapshot) = engine.with(|engine| {
        let unsent: Vec<_> = engine.events_since(*sent_head).cloned().collect();
        let snapshot = (!unsent.is_empty()).then(|| snapshot_from(engine));
        (unsent, snapshot)
    });
    // A client reconnecting mid-turn loads the snapshot and replays the
    // backlog, so both must hold these too. Refreshing the snapshot only when
    // the turns ended showed an answered question as still waiting, to any
    // app that reconnected, for as long as a round of turns ran.
    {
        let mut published = published.write().unwrap();
        let head = published.backlog.last().map(|e| e.seq).unwrap_or(Seq::ZERO);
        published
            .backlog
            .extend(unsent.iter().filter(|e| e.seq > head).cloned());
        if let Some(snapshot) = snapshot {
            published.snapshot = snapshot;
        }
    }
    // Files on a message just posted, the owner's or a bot's generated
    // image, can be imported by a bot at once rather than after the turns.
    for envelope in &unsent {
        if let orgbot_proto::Event::MessagePosted { message } = &envelope.event {
            if !message.files.is_empty() {
                committed
                    .write()
                    .unwrap()
                    .extend(message.files.iter().map(|file| file.id.clone()));
            }
        }
    }
    let board_moved = unsent.iter().any(|e| touches_the_board(&e.event));
    for envelope in unsent {
        *sent_head = (*sent_head).max(envelope.seq);
        announce(events, alerts, envelope);
    }
    // The board panels read the digest, so a moved board resends it too.
    if board_moved {
        let digest = engine.with(|engine| engine.digest());
        let _ = events.send(ServerMessage::Digest {
            digest: Box::new(digest),
        });
    }
}

/// Send one log event to every client, and to the desktop when no app window
/// is open to show it, as it happens rather than when the turns end.
fn announce(events: &broadcast::Sender<ServerMessage>, alerts: &mut Alerts, envelope: EventEnvelope) {
    let notification = orgbot_engine::notification_for(&envelope.event);
    #[cfg(target_os = "linux")]
    if events.receiver_count() == 0 {
        if let (Some(delivery), Some(notice)) = (alerts.as_mut(), &notification) {
            delivery.show(envelope.seq.0, envelope.id.to_string(), notice.clone());
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = alerts;
    let _ = events.send(ServerMessage::Event {
        notification,
        envelope: Box::new(envelope),
    });
}

/// Attachment ids on messages already in the room, which bots may import.
fn committed_attachments(engine: &orgbot_engine::Engine) -> HashSet<String> {
    engine
        .transcript()
        .iter()
        .flat_map(|message| message.files.iter().map(|file| file.id.clone()))
        .collect()
}

/// Answer a bot that is blocked waiting on a person: post the answer to it
/// and unblock its task.
fn answer_waiting_task(engine: &mut orgbot_engine::Engine, task: TaskId, answer: String) {
    let Some(record) = engine.task(task) else {
        return;
    };
    let waiting_on_human = record.state == TaskState::Blocked
        && record.blocked_on.iter().any(BlockReason::needs_human);
    if !waiting_on_human {
        return;
    }
    let assignee = record.assignee;
    let message = ChatMessage {
        id: orgbot_proto::MessageId::new(),
        room: engine.room_id(),
        author: Author::Human {
            name: engine.owner_name(),
        },
        // Directed at the bot that asked, so it is woken.
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
    let _ = engine.apply(Command::PostMessage {
        message: Box::new(message),
    });
    let _ = engine.apply(Command::UnblockTask { task });
}

/// Write a room checkpoint to `path`, atomically.
fn write_board(path: &std::path::Path, mut board: serde_json::Value) -> anyhow::Result<()> {
    use std::io::Write;
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
    Ok(())
}

/// What a project-memory request needs besides the engine.
#[derive(Debug, Clone)]
struct MemoryContext {
    setup_error: Option<String>,
    root: Option<std::path::PathBuf>,
    project: Option<String>,
    projects: Vec<String>,
}

/// Answer a project-memory request from the room's engine and settings.
///
/// Free of the session so it can be served while turns are running.
fn memory_reply(
engine: &orgbot_engine::Engine,
memory: &MemoryContext,
request: MemoryRequest,
) -> ServerMessage {
    let action = request.action;
    let request_id = request.request_id.clone();
    let fail = |message: String| ServerMessage::MemoryResult {
        request_id: request_id.clone(),
        action,
        ok: false,
        entries: vec![],
        hits: vec![],
        error: Some(message),
    };
    if request.request_id.is_empty() || request.request_id.len() > 80 {
        return fail("Invalid memory request identifier.".into());
    }
    if let Some(message) = &memory.setup_error {
        return fail(format!("Project memory is unavailable: {message}"));
    }
    let archive = engine.archive();
    let root = memory.root.as_deref();
    let result = match action {
        MemoryAction::List => archive.memory_entries().map(|entries| (entries, vec![])),
        MemoryAction::Search => {
            let Some(query) = request.query.as_deref() else {
                return fail("A search query is required.".into());
            };
            if query.trim().is_empty() || query.len() > 1000 {
                return fail("Search query must be 1–1000 characters.".into());
            }
            archive.search_memory(query, 100).map(|hits| {
                (
                    vec![],
                    hits.into_iter()
                        .map(|hit| MemorySearchItem {
                            item: hit.entry,
                            score: hit.score,
                        })
                        .collect(),
                )
            })
        }
        MemoryAction::SetShared => {
            let (Some(id), Some(shared)) = (request.id.as_deref(), request.shared) else {
                return fail("An entry and visibility value are required.".into());
            };
            if id.is_empty() || id.len() > 256 {
                return fail("Invalid memory entry identifier.".into());
            }
            archive
                .set_memory_shared(id, shared)
                .and_then(|_| archive.memory_entries())
                .map(|entries| (entries, vec![]))
        }
        MemoryAction::Delete => {
            let Some(id) = request.id.as_deref() else {
                return fail("A memory entry is required.".into());
            };
            if id.is_empty() || id.len() > 256 {
                return fail("Invalid memory entry identifier.".into());
            }
            archive
                .delete_memory(id)
                .and_then(|_| archive.memory_entries())
                .map(|entries| (entries, vec![]))
        }
        MemoryAction::ShareToProject => {
            let (Some(id), Some(target)) =
                (request.id.as_deref(), request.target_project.as_deref())
            else {
                return fail("An entry and target project are required.".into());
            };
            if id.is_empty() || id.len() > 256 {
                return fail("Invalid memory entry identifier.".into());
            }
            if !memory.projects.iter().any(|known| known == target)
                || memory.project.as_deref() == Some(target)
            {
                return fail("That project is not available for sharing.".into());
            }
            let Some(root) = root else {
                return fail("Cross-project memory storage is unavailable.".into());
            };
            archive
                .share_memory_to_project(id, root, target)
                .and_then(|_| archive.memory_entries())
                .map(|entries| (entries, vec![]))
        }
    };
    match result {
        Ok((entries, hits)) => ServerMessage::MemoryResult {
            request_id,
            action,
            ok: true,
            entries,
            hits,
            error: None,
        },
        Err(error) => fail(error.to_string()),
    }
}

/// Owner input applied while turns were running, kept until the next save.
///
/// The room is saved between turns, and a turn can run for a long time. So
/// input that lands mid-turn is first appended here, and only then applied
/// and shown: if the room stops before its next save, the input is replayed
/// on restore rather than lost after the person saw it arrive.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JournaledInput {
    Message {
        input: HumanInput,
        default_target: Option<orgbot_proto::AgentId>,
    },
    Decision {
        decision: orgbot_proto::DecisionId,
        chosen: String,
        note: String,
    },
}

fn journal_path(state_path: &std::path::Path) -> std::path::PathBuf {
    state_path.with_file_name("owner-input.jsonl")
}

/// Append one entry and flush it to disk before returning.
fn journal_append(path: &std::path::Path, entry: &JournaledInput) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let mut line = serde_json::to_vec(entry).map_err(std::io::Error::other)?;
    line.push(b'\n');
    file.write_all(&line)?;
    file.sync_all()
}

/// Re-apply journaled input that the last save does not contain yet.
fn journal_replay(
    engine: &mut orgbot_engine::Engine,
    path: &std::path::Path,
    attachments_root: Option<&std::path::Path>,
) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        // A torn last line from a crash mid-write is skipped, not fatal.
        let Ok(entry) = serde_json::from_str::<JournaledInput>(line) else {
            continue;
        };
        let result = match entry {
            JournaledInput::Message {
                input,
                default_target,
            } => record_human_message(engine, attachments_root, default_target, input).map(|_| ()),
            JournaledInput::Decision {
                decision,
                chosen,
                note,
            } => record_decision(engine, decision, chosen, note).map(|_| ()),
        };
        if let Err(error) = result {
            tracing::warn!(%error, "journaled owner input could not be replayed");
        }
    }
}

/// One message from the person, as it arrived.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct HumanInput {
    /// Fixed before the message is journaled, so a replay cannot post it twice.
    id: MessageId,
    author: String,
    body: String,
    task: Option<TaskId>,
    files: Vec<String>,
    reply_to: Option<MessageId>,
}

/// Post a message from the person into the room.
///
/// Works on the engine directly so it can run while turns are in flight:
/// the message is in the room at once, and each bot reads it on its next turn.
/// Returns the events it produced, or the reason it was refused.
fn record_human_message(
    engine: &mut orgbot_engine::Engine,
    attachments_root: Option<&std::path::Path>,
    default_target: Option<orgbot_proto::AgentId>,
    input: HumanInput,
) -> Result<Vec<EventEnvelope>, String> {
    let HumanInput {
        id,
        author,
        body,
        task,
        files,
        reply_to,
    } = input;
    if engine.transcript().iter().any(|message| message.id == id) {
        return Ok(Vec::new());
    }
    let files = orgbot_runtime::attachments::manifests(attachments_root, &files)
        .map_err(|message| format!("Message was not sent: {message}. Your files are kept."))?;
    let room = engine.room_id();
    let parent_author = match reply_to {
        Some(reply_to) => match engine.transcript().iter().find(|m| m.id == reply_to) {
            Some(parent) => Some(parent.author.clone()),
            None => {
                return Err("Message was not sent: the reply target is not in this room.".into())
            }
        },
        None => None,
    };
    let audience =
        human_audience_with_default(engine, &body, parent_author.as_ref(), default_target);
    let intent = if audience == Audience::Human {
        MessageIntent::Answer
    } else {
        MessageIntent::Ask
    };
    let message = ChatMessage {
        id,
        room,
        author: Author::Human { name: author },
        audience,
        intent,
        body,
        task,
        handoff: None,
        reply_to,
        attachments: vec![],
        files,
        sent_at: Timestamp::now(),
    };
    engine
        .apply(Command::PostMessage {
            message: Box::new(message),
        })
        .map_err(|error| format!("Message was not sent: {error}"))
}

/// Record the owner's answer to a decision, and put it in the transcript.
///
/// Returns whether the answer was newly recorded; a retried answer that is
/// already on the board is `Ok(false)`. Works on the engine directly so it can
/// run while turns are in flight as well as between them.
fn record_decision(
    engine: &mut orgbot_engine::Engine,
    decision: orgbot_proto::DecisionId,
    chosen: String,
    note: String,
) -> Result<bool, String> {
    let decided = if note.trim().is_empty() {
        format!("Decision: {chosen}.")
    } else {
        format!("Decision: {chosen}. {}", note.trim())
    };
    // A refused answer is not worth stopping the room for: the question stays
    // open and the person can try again.
    let events = engine
        .apply(Command::AnswerDecision {
            decision,
            answer: orgbot_proto::decision::Answer {
                chosen,
                note,
                at: orgbot_proto::Timestamp::now(),
            },
        })
        .map_err(|error| format!("Answer was not accepted: {error}"))?;
    if !events
        .iter()
        .any(|event| matches!(event.event, orgbot_proto::Event::DecisionAnswered { .. }))
    {
        return Ok(false);
    }
    // Put the answer in the transcript too. The board already knows, but a bot
    // reading back over the room should see the decision where it was made,
    // not have to infer it from a task quietly becoming claimable.
    if !events
        .iter()
        .any(|event| matches!(event.event, orgbot_proto::Event::MessagePosted { .. }))
    {
        let room = engine.room_id();
        let name = engine.owner_name();
        let _ = engine.apply(Command::PostMessage {
            message: Box::new(ChatMessage {
                id: orgbot_proto::MessageId::new(),
                room,
                author: Author::Human { name },
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
    }
    Ok(true)
}

fn human_audience_with_default(
    engine: &orgbot_engine::Engine,
    body: &str,
    parent_author: Option<&Author>,
    default_target: Option<orgbot_proto::AgentId>,
) -> Audience {
    let tags: Vec<_> = body
        .split(|c: char| !(c.is_alphanumeric() || matches!(c, '@' | '_' | '-')))
        .filter_map(|word| word.strip_prefix('@'))
        .filter(|tag| !tag.is_empty())
        .collect();
    if tags.iter().any(|tag| orgbot_proto::is_broadcast_tag(tag)) {
        return Audience::Everyone;
    }
    if tags.is_empty() {
        return match parent_author {
            Some(Author::Agent { agent }) => Audience::Agents {
                agents: vec![*agent],
            },
            Some(Author::Human { .. } | Author::System) => Audience::Human,
            None => default_target
                .map(|agent| Audience::Agents {
                    agents: vec![agent],
                })
                .unwrap_or(Audience::Agents { agents: vec![] }),
        };
    }
    let mut targets = Vec::new();
    for tag in tags {
        let matches: Vec<_> = engine
            .agents()
            .filter(|agent| {
                tag.eq_ignore_ascii_case(&agent.handle)
                    || tag.eq_ignore_ascii_case(&agent.display_name)
            })
            .map(|agent| agent.id)
            .collect();
        if matches.len() == 1 && !targets.contains(&matches[0]) {
            targets.push(matches[0]);
        }
    }
    Audience::Agents { agents: targets }
}

/// Choose one recipient for an unthreaded human request. An available
/// coordinator should plan ordinary room work; custom rosters fall back to an
/// available planner, then any available agent. If the whole roster is
/// unavailable, keep the request queued for one stable recipient instead of
/// implicitly broadcasting it.
#[cfg(test)]
fn default_human_target(engine: &orgbot_engine::Engine) -> Option<orgbot_proto::AgentId> {
    let agents: Vec<_> = engine.agents().collect();
    let available = |agent: &&orgbot_proto::AgentSpec| engine.status(agent.id).is_available();
    let coordinator = |agent: &&orgbot_proto::AgentSpec| {
        agent.has_role("coordinator") && agent.autonomy.may_create_tasks
    };
    let planner = |agent: &&orgbot_proto::AgentSpec| agent.autonomy.may_create_tasks;

    agents
        .iter()
        .find(|agent| available(agent) && coordinator(agent))
        .or_else(|| {
            agents
                .iter()
                .find(|agent| available(agent) && planner(agent))
        })
        .or_else(|| agents.iter().find(|agent| available(agent)))
        .or_else(|| agents.iter().find(|agent| coordinator(agent)))
        .or_else(|| agents.iter().find(|agent| planner(agent)))
        .or_else(|| agents.first())
        .map(|agent| agent.id)
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
    use async_trait::async_trait;
    use orgbot_providers::{
        CompletionRequest, CompletionResponse, ContentBlock, Provider, StopReason, Usage,
    };

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
    async fn a_decision_answer_is_written_under_the_owners_own_name() {
        let (mut session, _handle) = session();
        let asker = session.orchestrator.engine().agents().next().unwrap().id;
        let room = session.orchestrator.engine().room_id();
        session
            .orchestrator
            .engine_mut()
            .apply(Command::PostMessage {
                message: Box::new(ChatMessage::human(room, "Aece", "first audit the repo")),
            })
            .unwrap();
        let decision = session
            .orchestrator
            .engine_mut()
            .apply(Command::AskOwner {
                by: asker,
                task: None,
                question: "Which finish line first?".into(),
                options: vec!["A".into(), "B".into()],
            })
            .unwrap()
            .iter()
            .find_map(|event| match &event.event {
                orgbot_proto::Event::DecisionAsked { decision } => Some(decision.id),
                _ => None,
            })
            .unwrap();
        session.handle(Control::AnswerDecision {
            decision,
            chosen: "Custom answer".into(),
            note: "read progress.md".into(),
        });
        let engine = session.orchestrator.engine();
        let reply = engine.transcript().last().unwrap();
        assert_eq!(reply.author, Author::Human { name: "Aece".into() });
        assert!(reply.body.starts_with("Aece answered: Custom answer"));
        assert!(engine
            .transcript()
            .iter()
            .all(|m| m.author != Author::Human { name: orgbot_proto::OWNER_STAND_IN.into() }));
    }

    #[test]
    fn owner_input_journaled_mid_turn_survives_a_crash_before_the_next_save() {
        let dir = std::env::temp_dir().join(format!(
            "orgbot-owner-journal-{}",
            orgbot_proto::RoomId::new()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        let config = || {
            let mut config = RoomConfig::parse(SCRIPTED_ROOM, "journal.toml").unwrap();
            config.room.state_path = Some(path.clone());
            config.resolve().unwrap()
        };
        let (mut session, _handle) = RoomSession::build(config()).unwrap();
        let asker = session.orchestrator.engine().agents().next().unwrap().id;
        let decision = session
            .orchestrator
            .engine_mut()
            .apply(Command::AskOwner {
                by: asker,
                task: None,
                question: "Which finish line first?".into(),
                options: vec!["A".into(), "B".into()],
            })
            .unwrap()
            .iter()
            .find_map(|event| match &event.event {
                orgbot_proto::Event::DecisionAsked { decision } => Some(decision.id),
                _ => None,
            })
            .unwrap();
        session.save_state().unwrap();

        // Mid-turn input is journaled, and then the room stops before saving.
        let journal = journal_path(&path);
        let id = MessageId::new();
        journal_append(
            &journal,
            &JournaledInput::Message {
                input: HumanInput {
                    id,
                    author: "Aece".into(),
                    body: "idea: read progress.md".into(),
                    task: None,
                    files: vec![],
                    reply_to: None,
                },
                default_target: None,
            },
        )
        .unwrap();
        journal_append(
            &journal,
            &JournaledInput::Decision {
                decision,
                chosen: "A".into(),
                note: String::new(),
            },
        )
        .unwrap();
        drop(session);

        // Recovery of a room that was running resumes it, and replays both.
        let (mut restored, _handle) = RoomSession::build_with_resume(config(), true).unwrap();
        {
            let engine = restored.orchestrator.engine();
            let ideas = engine
                .transcript()
                .iter()
                .filter(|m| m.id == id)
                .count();
            assert_eq!(ideas, 1, "the journaled message is restored once");
            assert!(engine.digest().needs_you.is_empty(), "the answer is restored");
        }
        // The next save holds it all, so the journal is no longer needed.
        restored.save_state().unwrap();
        assert!(!journal.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn owner_answer_wakes_room_and_reaches_only_its_asking_agent() {
        let (mut session, handle) = session();
        let asker = session.orchestrator.engine().agents().next().unwrap().id;
        let raised = session
            .orchestrator
            .engine_mut()
            .apply(Command::AskOwner {
                by: asker,
                task: None,
                question: "Which prototype should we develop?".into(),
                options: vec!["Desktop".into(), "Mobile".into()],
            })
            .unwrap();
        let decision = raised
            .iter()
            .find_map(|event| match &event.event {
                orgbot_proto::Event::DecisionAsked { decision } => Some(decision.id),
                _ => None,
            })
            .unwrap();
        session.settled = true;
        session.handle(Control::AnswerDecision {
            decision,
            chosen: "Desktop".into(),
            note: "Keyboard first".into(),
        });
        assert!(!session.settled);
        let engine = session.orchestrator.engine();
        assert_eq!(engine.transcript().len(), 1);
        let reply = &engine.transcript()[0];
        assert_eq!(
            reply.audience,
            Audience::Agents {
                agents: vec![asker]
            }
        );
        assert!(reply.body.contains("Keyboard first"));
        assert!(engine.digest().needs_you.is_empty());
        drop(engine);
        let mut updates = handle.subscribe();
        session.settled = true;
        session.handle(Control::AnswerDecision {
            decision,
            chosen: "Desktop".into(),
            note: "Keyboard first".into(),
        });
        assert!(session.settled, "a repeated answer must not wake the room");
        assert_eq!(session.orchestrator.engine().transcript().len(), 1);
        assert!(
            matches!(updates.try_recv(), Ok(ServerMessage::Digest { digest }) if digest.needs_you.is_empty())
        );
        session.handle(Control::AnswerDecision {
            decision: orgbot_proto::ids::DecisionId::new(),
            chosen: "Ignore this invalid answer".into(),
            note: String::new(),
        });
        assert!(matches!(
            updates.try_recv(),
            Ok(ServerMessage::Error { .. })
        ));
        assert_eq!(session.orchestrator.engine().transcript().len(), 1);
    }

    #[derive(Debug)]
    struct InFlightProvider(Arc<tokio::sync::Notify>);

    #[derive(Debug)]
    struct LoadSkillThenWait {
        calls: std::sync::atomic::AtomicUsize,
        waiting: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl Provider for LoadSkillThenWait {
        fn name(&self) -> &str {
            "capability cancellation fixture"
        }

        async fn complete(
            &self,
            request: CompletionRequest,
            _sink: Option<mpsc::Sender<orgbot_providers::StreamEvent>>,
        ) -> orgbot_providers::Result<CompletionResponse> {
            if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                return Ok(orgbot_providers::scripted::tool_call(
                    "skill-load",
                    "use_skill",
                    serde_json::json!({"skill":"pause-fixture"}),
                ));
            }
            assert!(serde_json::to_string(&request.messages)
                .unwrap()
                .contains("PAUSE_SKILL_BODY"));
            self.waiting.notify_one();
            std::future::pending().await
        }
    }

    #[async_trait]
    impl Provider for InFlightProvider {
        fn name(&self) -> &str {
            "in-flight fixture"
        }

        async fn complete(
            &self,
            _request: CompletionRequest,
            _sink: Option<mpsc::Sender<orgbot_providers::StreamEvent>>,
        ) -> orgbot_providers::Result<CompletionResponse> {
            self.0.notify_one();
            std::future::pending().await
        }
    }

    #[derive(Debug)]
    struct CountingProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        started: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl Provider for CountingProvider {
        fn name(&self) -> &str {
            "local allowance fixture"
        }

        async fn complete(
            &self,
            _request: CompletionRequest,
            _sink: Option<mpsc::Sender<orgbot_providers::StreamEvent>>,
        ) -> orgbot_providers::Result<CompletionResponse> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.started.notify_one();
            Ok(CompletionResponse {
                model: "fixture".into(),
                content: vec![ContentBlock::text("A local fixture response.")],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            })
        }
    }

    /// Answers only once it can see the owner's question in its prompt.
    #[derive(Debug)]
    struct AnswersQuestion;

    #[async_trait]
    impl Provider for AnswersQuestion {
        fn name(&self) -> &str {
            "answers-question fixture"
        }

        async fn complete(
            &self,
            request: CompletionRequest,
            _sink: Option<mpsc::Sender<orgbot_providers::StreamEvent>>,
        ) -> orgbot_providers::Result<CompletionResponse> {
            let seen = request
                .messages
                .iter()
                .any(|turn| turn.text().contains("are you there?"));
            Ok(CompletionResponse {
                model: "fixture".into(),
                content: if seen {
                    vec![ContentBlock::text("yes, here")]
                } else {
                    vec![]
                },
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            })
        }
    }

    #[tokio::test]
    async fn a_message_sent_while_paused_is_answered_after_resuming() {
        let mut resolved = RoomConfig::parse(
            r#"
            [room]
            name = "chat"
            goal = "talk"
            completion = "manual"

            [[bots]]
            handle = "lead"
            roles = ["coordinator"]
            autonomy = "coordinator"
            provider = { kind = "scripted", script = "" }
            "#,
            "paused-chat.toml",
        )
        .unwrap()
        .resolve()
        .unwrap();
        resolved.bots[0].provider = Arc::new(AnswersQuestion);
        let (session, handle) = RoomSession::build(resolved).unwrap();
        let mut updates = handle.subscribe();
        let running = tokio::spawn(session.run());

        async fn until(
            updates: &mut broadcast::Receiver<ServerMessage>,
            what: &str,
            mut done: impl FnMut(&ServerMessage) -> bool,
        ) {
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if let Ok(message) = updates.recv().await {
                        if done(&message) {
                            return;
                        }
                    }
                }
            })
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
        }
        let posted = |body: &'static str| {
            move |message: &ServerMessage| {
                matches!(message, ServerMessage::Event { envelope, .. }
                    if matches!(&envelope.event, orgbot_proto::Event::MessagePosted { message } if message.body == body))
            }
        };

        assert!(handle.send(Control::Pause { reason: "owner".into() }).await);
        until(&mut updates, "the pause", |m| {
            matches!(m, ServerMessage::Phase { phase: RoomPhase::Paused })
        })
        .await;
        assert!(
            handle
                .send(Control::HumanMessage {
                    author: "Aece".into(),
                    body: "are you there?".into(),
                    task: None,
                    files: vec![],
                    reply_to: None,
                })
                .await
        );
        until(&mut updates, "the message to land while paused", posted("are you there?")).await;
        assert!(handle.send(Control::Resume).await);
        until(&mut updates, "a reply after resuming", posted("yes, here")).await;
        running.abort();
    }

    #[tokio::test]
    async fn stop_interrupts_an_in_flight_turn_and_checkpoints_promptly() {
        let dir = std::env::temp_dir().join(format!(
            "orgbot-stop-in-flight-{}",
            orgbot_proto::RoomId::new()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        let mut config = RoomConfig::parse(SCRIPTED_ROOM, "stop.toml").unwrap();
        config.room.state_path = Some(path.clone());
        let mut resolved = config.resolve().unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        resolved.bots[0].provider = Arc::new(InFlightProvider(started.clone()));
        let (session, handle) = RoomSession::build(resolved).unwrap();
        let running = tokio::spawn(session.run());

        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("fixture provider should be in flight");
        let began = tokio::time::Instant::now();
        assert!(handle.send(Control::Stop).await);
        tokio::time::timeout(Duration::from_secs(2), running)
            .await
            .expect("stop should cancel the in-flight provider promptly")
            .expect("session task should finish");
        assert!(began.elapsed() < Duration::from_secs(2));
        assert!(
            path.is_file(),
            "shutdown should persist the room checkpoint"
        );
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["version"], 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A turn that never ends, counting how many times it was started.
    #[derive(Debug)]
    struct LongTurnProvider {
        started: Arc<tokio::sync::Notify>,
        turns: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl Provider for LongTurnProvider {
        fn name(&self) -> &str {
            "long-turn fixture"
        }

        async fn complete(
            &self,
            _request: CompletionRequest,
            _sink: Option<mpsc::Sender<orgbot_providers::StreamEvent>>,
        ) -> orgbot_providers::Result<CompletionResponse> {
            self.turns.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.started.notify_one();
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn owner_input_lands_while_a_turn_runs_without_interrupting_it() {
        let mut resolved = RoomConfig::parse(SCRIPTED_ROOM, "answer-mid-turn.toml")
            .unwrap()
            .resolve()
            .unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        resolved.bots[0].provider = Arc::new(LongTurnProvider {
            started: started.clone(),
            turns: turns.clone(),
        });
        let (mut session, handle) = RoomSession::build(resolved).unwrap();
        let asker = session.orchestrator.engine().agents().next().unwrap().id;
        let decision = session
            .orchestrator
            .engine_mut()
            .apply(Command::AskOwner {
                by: asker,
                task: None,
                question: "Which finish line first?".into(),
                options: vec!["A".into(), "B".into()],
            })
            .unwrap()
            .iter()
            .find_map(|event| match &event.event {
                orgbot_proto::Event::DecisionAsked { decision } => Some(decision.id),
                _ => None,
            })
            .unwrap();
        let shared = session.orchestrator.shared_engine();
        let room = session.orchestrator.engine().room_id();
        // Published while the question is open, as when a room starts or a
        // round of turns ends with the owner still to answer.
        session.broadcast_new_events();
        let needs_you = |snapshot: Option<ServerMessage>| match snapshot {
            Some(ServerMessage::Snapshot { digest, .. }) => digest.needs_you.len(),
            _ => panic!("a snapshot is always published"),
        };
        assert_eq!(needs_you(handle.snapshot().await), 1);
        let mut updates = handle.subscribe();
        let running = tokio::spawn(session.run());
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("a turn should be in flight");

        assert!(
            handle
                .send(Control::AnswerDecision {
                    decision,
                    chosen: "Custom answer".into(),
                    note: "the todo you read is outdated; read progress.md".into(),
                })
                .await
        );
        let landed = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(ServerMessage::Digest { digest }) = updates.recv().await {
                    if digest.needs_you.is_empty() {
                        return;
                    }
                }
            }
        })
        .await;
        assert!(
            landed.is_ok(),
            "the answer must land without waiting for the turn to end"
        );
        // An app reconnecting now loads the snapshot first. The question it
        // just answered must not come back.
        assert_eq!(
            needs_you(handle.snapshot().await),
            0,
            "a reconnecting app must not see an answered question again"
        );

        // A bot acts on the board mid-turn (as room tools do), then the owner
        // sends an idea. Both reach clients now, and in log order.
        let mut early = handle.subscribe();
        shared.with(|engine| {
            engine
                .apply(Command::PostMessage {
                    message: Box::new(ChatMessage::say(room, asker, "working on it")),
                })
                .unwrap();
        });
        // Shown while the turn is still running, without waiting for the
        // batch to end or for the owner to say anything.
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(ServerMessage::Event { envelope, .. }) = early.recv().await {
                    if matches!(&envelope.event,
                        orgbot_proto::Event::MessagePosted { message } if message.body == "working on it")
                    {
                        return;
                    }
                }
            }
        })
        .await
        .expect("a bot's message must reach clients while a slower turn runs");
        assert!(
            handle
                .send(Control::HumanMessage {
                    author: "Aece".into(),
                    body: "idea: check progress.md first".into(),
                    task: None,
                    files: vec![],
                    reply_to: None,
                })
                .await
        );
        let order = tokio::time::timeout(Duration::from_secs(2), async {
            let mut seen = Vec::new();
            loop {
                if let Ok(ServerMessage::Event { envelope, .. }) = updates.recv().await {
                    if let orgbot_proto::Event::MessagePosted { message } = &envelope.event {
                        seen.push(message.body.clone());
                        if message.body.starts_with("idea:") {
                            return seen;
                        }
                    }
                }
            }
        })
        .await
        .expect("the idea must reach clients without waiting for the turn to end");
        let bot = order.iter().position(|b| b == "working on it");
        let idea = order.iter().position(|b| b.starts_with("idea:"));
        assert!(bot.is_some() && bot < idea, "events must arrive in log order: {order:?}");
        // The memory panel's requests are answered during the turn too.
        let (reply, answer) = tokio::sync::oneshot::channel();
        assert!(
            handle
                .send(Control::Memory {
                    request: serde_json::from_value(serde_json::json!({
                        "request_id": "mem-1",
                        "action": "list"
                    }))
                    .unwrap(),
                    reply,
                })
                .await
        );
        let memory = tokio::time::timeout(Duration::from_secs(2), answer)
            .await
            .expect("project memory must answer while a turn runs")
            .unwrap();
        assert!(matches!(memory, ServerMessage::MemoryResult { .. }));

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            turns.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the running turn must not be cut short and restarted by an answer or a message"
        );
        assert!(!running.is_finished());
        assert!(handle.send(Control::Stop).await);
        let _ = tokio::time::timeout(Duration::from_secs(2), running).await;
    }

    #[tokio::test]
    async fn an_answer_to_a_waiting_bot_lands_while_a_long_turn_runs() {
        let mut resolved = RoomConfig::parse(SCRIPTED_ROOM, "answer-bot.toml")
            .unwrap()
            .resolve()
            .unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        resolved.bots[0].provider = Arc::new(LongTurnProvider {
            started: started.clone(),
            turns: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        let (mut session, handle) = RoomSession::build(resolved).unwrap();
        let bot = session.orchestrator.engine().agents().next().unwrap().id;
        let task = session.orchestrator.engine().tasks().next().unwrap().id;
        {
            let mut engine = session.orchestrator.engine_mut();
            for command in [
                Command::ClaimTask { task, by: bot },
                Command::StartTask { task, by: bot },
                Command::BlockTask {
                    task,
                    by: bot,
                    reason: BlockReason::AwaitingHuman {
                        prompt: "which colour?".into(),
                    },
                },
            ] {
                engine.apply(command).unwrap();
            }
            // Something else to do meanwhile, so the bot is mid-turn.
            let mut ask = ChatMessage::say(engine.room_id(), bot, "while you wait, tidy the notes")
                .to_agents(vec![bot])
                .with_intent(MessageIntent::Ask);
            ask.author = Author::Human { name: "Aece".into() };
            engine.apply(Command::PostMessage { message: Box::new(ask) }).unwrap();
        }
        let shared = session.orchestrator.shared_engine();
        let running = tokio::spawn(session.run());
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("a turn should be in flight");
        assert!(
            handle
                .send(Control::Answer {
                    task,
                    answer: "teal".into(),
                })
                .await
        );
        let unblocked = tokio::time::timeout(Duration::from_secs(2), async {
            while shared.with(|engine| engine.task(task).unwrap().state) == TaskState::Blocked {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(unblocked.is_ok(), "the answer must not wait for the turn to end");
        assert!(handle.send(Control::Stop).await);
        let _ = tokio::time::timeout(Duration::from_secs(2), running).await;
    }

    /// A turn that ends only when the test says so.
    #[derive(Debug)]
    struct HeldTurnProvider {
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl Provider for HeldTurnProvider {
        fn name(&self) -> &str {
            "held-turn fixture"
        }

        async fn complete(
            &self,
            _request: CompletionRequest,
            _sink: Option<mpsc::Sender<orgbot_providers::StreamEvent>>,
        ) -> orgbot_providers::Result<CompletionResponse> {
            self.started.notify_one();
            self.release.notified().await;
            Ok(CompletionResponse {
                model: "fixture".into(),
                content: vec![ContentBlock::text("done")],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            })
        }
    }

    #[tokio::test]
    async fn work_shown_live_is_not_sent_again_when_the_turn_ends() {
        let mut resolved = RoomConfig::parse(SCRIPTED_ROOM, "sent-once.toml")
            .unwrap()
            .resolve()
            .unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        resolved.bots[0].provider = Arc::new(HeldTurnProvider {
            started: started.clone(),
            release: release.clone(),
        });
        let (session, handle) = RoomSession::build(resolved).unwrap();
        let bot = session.orchestrator.engine().agents().next().unwrap().id;
        let room = session.orchestrator.engine().room_id();
        let shared = session.orchestrator.shared_engine();
        let mut updates = handle.subscribe();
        let running = tokio::spawn(session.run());
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("a turn should be in flight");
        let posted = shared.with(|engine| {
            engine
                .apply(Command::PostMessage {
                    message: Box::new(ChatMessage::say(room, bot, "halfway there")),
                })
                .unwrap()[0]
                .seq
        });
        // Shown live, then the turn ends and the room catches up.
        tokio::time::sleep(LIVE_FLUSH * 3).await;
        release.notify_one();
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let mut times = 0;
        while let Ok(message) = updates.try_recv() {
            if let ServerMessage::Event { envelope, .. } = message {
                times += usize::from(envelope.seq == posted);
            }
        }
        assert_eq!(times, 1, "each event reaches clients once");
        assert!(handle.send(Control::Stop).await);
        let _ = tokio::time::timeout(Duration::from_secs(2), running).await;
    }

    #[tokio::test]
    async fn interrupted_turn_releases_temporary_skill_before_checkpoint_and_restore() {
        let dir = std::env::temp_dir().join(format!(
            "orgbot-capability-pause-{}",
            orgbot_proto::RoomId::new()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        let mut config = RoomConfig::parse(SCRIPTED_ROOM, "pause-capability.toml").unwrap();
        config.room.state_path = Some(path.clone());
        let mut resolved = config.resolve().unwrap();
        let waiting = Arc::new(tokio::sync::Notify::new());
        resolved.bots[0].provider = Arc::new(LoadSkillThenWait {
            calls: std::sync::atomic::AtomicUsize::new(0),
            waiting: waiting.clone(),
        });
        let (mut session, handle) = RoomSession::build(resolved).unwrap();
        session
            .orchestrator
            .engine_mut()
            .set_skills(vec![orgbot_proto::skill::Skill {
                name: "pause-fixture".into(),
                description: "fixture".into(),
                body: "PAUSE_SKILL_BODY".into(),
            }]);
        let running = tokio::spawn(session.run());
        tokio::time::timeout(Duration::from_secs(2), waiting.notified())
            .await
            .unwrap();
        assert!(handle.send(Control::Stop).await);
        tokio::time::timeout(Duration::from_secs(2), running)
            .await
            .unwrap()
            .unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(!saved["bots"].to_string().contains("PAUSE_SKILL_BODY"));
        for bot in saved["bots"].as_object().unwrap().values() {
            assert!(bot["capabilities"]["leases"].as_array().unwrap().is_empty());
        }
        let (restored, _) = RoomSession::build(config.resolve().unwrap()).unwrap();
        assert!(!restored.orchestrator.checkpoint()["bots"]
            .to_string()
            .contains("PAUSE_SKILL_BODY"));
    }

    #[tokio::test]
    async fn supervised_allowance_is_attached_without_process_environment_and_survives_restart() {
        let dir = std::env::temp_dir().join(format!(
            "orgbot-allowance-supervised-{}",
            orgbot_proto::RoomId::new()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let state_path = dir.join("state.json");
        let allowance_config = dir.join("allowances.json");
        let allowance_ledger = dir.join("ledger.json");
        let policy = orgbot_runtime::allowance::Policy {
            max_requests: 1,
            ..Default::default()
        };
        std::fs::write(
            &allowance_config,
            serde_json::to_vec(&orgbot_runtime::allowance::Config {
                providers: std::collections::BTreeMap::from([("deepseek".into(), policy)]),
            })
            .unwrap(),
        )
        .unwrap();
        let environment = std::collections::HashMap::from([
            (
                "ORGBOT_ALLOWANCE_CONFIG".into(),
                allowance_config.display().to_string(),
            ),
            (
                "ORGBOT_ALLOWANCE_LEDGER".into(),
                allowance_ledger.display().to_string(),
            ),
        ]);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());

        let resolved = || {
            let mut config = RoomConfig::parse(SCRIPTED_ROOM, "allowance.toml").unwrap();
            config.room.state_path = Some(state_path.clone());
            let mut resolved = config.resolve_with_environment(&environment).unwrap();
            resolved.bots[0].spec.provider = orgbot_proto::agent::ProviderRef::DeepSeek {
                model: "fixture".into(),
            };
            resolved.bots[0].provider = Arc::new(CountingProvider {
                calls: calls.clone(),
                started: started.clone(),
            });
            resolved
        };

        let (session, handle) = RoomSession::build(resolved()).unwrap();
        let mut events = handle.subscribe();
        let running = tokio::spawn(session.run());
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("first local request should pass the allowance");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let ServerMessage::Event { envelope, .. } = events.recv().await.unwrap() {
                    if matches!(envelope.event, orgbot_proto::Event::MessagePosted { .. }) {
                        break;
                    }
                }
            }
        })
        .await
        .expect("provider result should be committed before restart");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(handle.send(Control::Stop).await);
        tokio::time::timeout(Duration::from_secs(2), running)
            .await
            .expect("first supervised session should stop")
            .unwrap();

        let (restored, restored_handle) = RoomSession::build_with_resume(resolved(), true).unwrap();
        let restored_run = tokio::spawn(restored.run());
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the persisted one-request allowance must block another provider call"
        );
        assert!(restored_handle.send(Control::Stop).await);
        tokio::time::timeout(Duration::from_secs(2), restored_run)
            .await
            .expect("restored session should stop")
            .unwrap();
        let ledger: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&allowance_ledger).unwrap()).unwrap();
        assert_eq!(ledger["providers"]["deepseek"]["requests"], 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn owner_memory_controls_return_mutated_entries_and_reject_unknown_projects() {
        let root = std::env::temp_dir().join(format!(
            "orgbot-memory-control-{}",
            orgbot_proto::RoomId::new()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut config = RoomConfig::parse(SCRIPTED_ROOM, "memory.toml").unwrap();
        config.room.project_id = Some("project:source".into());
        let environment = std::collections::HashMap::from([
            ("ORGBOT_MEMORY_ROOT".into(), root.display().to_string()),
            (
                "ORGBOT_MEMORY_PROJECTS".into(),
                r#"["project:source","project:target"]"#.into(),
            ),
        ]);
        let (session, _) =
            RoomSession::build(config.resolve_with_environment(&environment).unwrap()).unwrap();
        let entry = session
            .orchestrator
            .engine()
            .archive()
            .remember("Release choice", "Keep the stable API surface", None, None)
            .unwrap();

        let hidden = session.handle_memory(MemoryRequest {
            request_id: "visibility".into(),
            action: MemoryAction::SetShared,
            id: Some(entry.id.clone()),
            shared: Some(false),
            query: None,
            target_project: None,
        });
        let ServerMessage::MemoryResult { entries, ok, .. } = hidden else {
            panic!("memory result")
        };
        assert!(ok);
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].shared);

        let rejected = session.handle_memory(MemoryRequest {
            request_id: "unknown-target".into(),
            action: MemoryAction::ShareToProject,
            id: Some(entry.id.clone()),
            shared: None,
            query: None,
            target_project: Some("project:unconfigured".into()),
        });
        assert!(matches!(
            rejected,
            ServerMessage::MemoryResult { ok: false, .. }
        ));

        let shared = session.handle_memory(MemoryRequest {
            request_id: "known-target".into(),
            action: MemoryAction::ShareToProject,
            id: Some(entry.id),
            shared: None,
            query: None,
            target_project: Some("project:target".into()),
        });
        assert!(matches!(
            shared,
            ServerMessage::MemoryResult { ok: true, .. }
        ));
        let target = orgbot_engine::Archive::default();
        target.configure_memory(&root, "project:target").unwrap();
        let target_entries = target.memory_entries().unwrap();
        assert_eq!(target_entries.len(), 1);
        assert!(target_entries[0].shared_from_scope.is_some());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn supervised_restore_resumes_only_checkpoints_that_were_running() {
        let dir = std::env::temp_dir().join(format!(
            "orgbot-supervised-restore-{}",
            orgbot_proto::RoomId::new()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        let mut config = RoomConfig::parse(SCRIPTED_ROOM, "resume.toml").unwrap();
        config.room.state_path = Some(path.clone());
        let (running, _) = RoomSession::build(config.resolve().unwrap()).unwrap();
        drop(running);
        let (resumed, _) = RoomSession::build_with_resume(config.resolve().unwrap(), true).unwrap();
        assert_eq!(
            resumed.orchestrator.engine().room().phase,
            RoomPhase::Running
        );
        drop(resumed);

        let (mut paused, _) = RoomSession::build(config.resolve().unwrap()).unwrap();
        paused.handle(Control::Pause {
            reason: "owner paused".into(),
        });
        drop(paused);
        let (still_paused, _) =
            RoomSession::build_with_resume(config.resolve().unwrap(), true).unwrap();
        assert_eq!(
            still_paused.orchestrator.engine().room().phase,
            RoomPhase::Paused
        );
        drop(still_paused);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn exhausted_budgets_pause_unfinished_work_and_resume_without_losing_it() {
        for outcome in [RunOutcome::OutOfTime, RunOutcome::OutOfTurns] {
            let (mut session, _) = session();
            let task = session.orchestrator.engine().tasks().next().unwrap().id;
            session.finish(&outcome);
            assert_eq!(
                session.orchestrator.engine().room().phase,
                RoomPhase::Paused
            );
            assert!(session.orchestrator.engine().task(task).is_some());
            assert!(!session
                .orchestrator
                .engine()
                .log()
                .iter()
                .any(|e| matches!(e.event, orgbot_proto::Event::RoomClosed { .. })));
            session.handle(Control::Resume);
            assert_eq!(
                session.orchestrator.engine().room().phase,
                RoomPhase::Running
            );
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
                    reply_to: None,
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
                            reply_to: None,
                        })
                        .await;
                }
                let mut answered = false;
                tokio::time::timeout(Duration::from_secs(3), async {
                    loop {
                        match events.recv().await.unwrap() {
                            ServerMessage::Event { envelope, .. } => {
                                if let orgbot_proto::Event::MessagePosted { message } =
                                    envelope.event
                                {
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
            reply_to: None,
        });
        assert_eq!(
            session.orchestrator.engine().room().phase,
            RoomPhase::Paused
        );
        let message = session
            .orchestrator
            .engine()
            .transcript()
            .last()
            .cloned()
            .expect("human message is retained while paused");
        assert_eq!(
            message.audience,
            Audience::Agents {
                agents: vec![
                    session
                        .orchestrator
                        .engine()
                        .agent_by_handle("lead")
                        .unwrap()
                        .id
                ]
            }
        );
    }

    #[test]
    fn human_tags_route_attention_and_ignore_email_addresses() {
        let (mut session, _handle) = session();
        let worker = orgbot_proto::AgentSpec::new(
            "worker",
            "Worker",
            orgbot_proto::agent::ProviderRef::Scripted {
                script: "test".into(),
            },
        )
        .with_roles(["writer"]);
        let worker_id = worker.id;
        session
            .orchestrator
            .engine_mut()
            .apply(Command::JoinAgent {
                spec: Box::new(worker),
            })
            .unwrap();
        let mut named_agent = session
            .orchestrator
            .engine()
            .agent_by_handle("lead")
            .unwrap()
            .clone();
        named_agent.display_name = "Julian".into();
        session.orchestrator.engine_mut().update_agent(named_agent);
        let engine = session.orchestrator.engine();
        let lead = engine.agent_by_handle("lead").unwrap().id;
        assert_eq!(
            human_audience(&engine, "Please help with this", None),
            Audience::Agents { agents: vec![lead] }
        );
        let untagged = ChatMessage {
            author: Author::Human {
                name: "Alex".into(),
            },
            audience: human_audience(&engine, "Please help with this", None),
            ..ChatMessage::say(engine.room_id(), lead, "Please help with this")
        };
        assert!(untagged.wakes(lead, &["coordinator".into()]));
        assert!(!untagged.wakes(worker_id, &["writer".into()]));
        assert_eq!(
            human_audience(&engine, "@LEAD, please check", None),
            Audience::Agents { agents: vec![lead] }
        );
        assert_eq!(
            human_audience(&engine, "@lead @lead check", None),
            Audience::Agents { agents: vec![lead] }
        );
        assert_eq!(
            human_audience(&engine, "@Julian check", None),
            Audience::Agents { agents: vec![lead] }
        );
        drop(engine);
        let duplicate = orgbot_proto::AgentSpec::new(
            "backup",
            "Julian",
            orgbot_proto::agent::ProviderRef::Scripted {
                script: "test".into(),
            },
        );
        session
            .orchestrator
            .engine_mut()
            .apply(Command::JoinAgent {
                spec: Box::new(duplicate),
            })
            .unwrap();
        let engine = session.orchestrator.engine();
        assert_eq!(
            human_audience(&engine, "@Julian check", None),
            Audience::Agents { agents: vec![] }
        );
        assert_eq!(
            human_audience(&engine, "@missing check", None),
            Audience::Agents { agents: vec![] }
        );
        assert_eq!(
            human_audience(&engine, "@all hello", None),
            Audience::Everyone
        );
        assert_eq!(
            human_audience(&engine, "mail foo@lead.example", None),
            Audience::Agents { agents: vec![lead] }
        );
        assert_eq!(
            human_audience(&engine, "follow-up", Some(&Author::Agent { agent: lead })),
            Audience::Agents { agents: vec![lead] }
        );
        assert_eq!(
            human_audience(
                &engine,
                "thanks",
                Some(&Author::Human {
                    name: "Alex".into()
                }),
            ),
            Audience::Human
        );
        assert_eq!(
            human_audience(
                &engine,
                "@all instead",
                Some(&Author::Agent { agent: lead }),
            ),
            Audience::Everyone
        );
    }

    #[test]
    fn untagged_messages_fall_back_to_one_available_planner_without_a_coordinator() {
        let (mut session, _) = session();
        let mut lead = session
            .orchestrator
            .engine()
            .agent_by_handle("lead")
            .unwrap()
            .clone();
        lead.roles = vec!["writer".into()];
        lead.autonomy = orgbot_proto::agent::Autonomy::default();
        session.orchestrator.engine_mut().update_agent(lead);
        let planner = orgbot_proto::AgentSpec::new(
            "planner",
            "Planner",
            orgbot_proto::agent::ProviderRef::Scripted {
                script: "test".into(),
            },
        )
        .with_roles(["builder"])
        .with_autonomy(orgbot_proto::agent::Autonomy::coordinator());
        let planner_id = planner.id;
        session
            .orchestrator
            .engine_mut()
            .apply(Command::JoinAgent {
                spec: Box::new(planner),
            })
            .unwrap();

        let engine = session.orchestrator.engine();
        assert_eq!(
            human_audience(&engine, "Please plan the work", None),
            Audience::Agents {
                agents: vec![planner_id]
            }
        );
        drop(engine);

        let mut lead = session
            .orchestrator
            .engine()
            .agent_by_handle("lead")
            .unwrap()
            .clone();
        lead.autonomy = orgbot_proto::agent::Autonomy::default();
        session.orchestrator.engine_mut().update_agent(lead);
        let mut planner = session
            .orchestrator
            .engine()
            .agent_by_handle("planner")
            .unwrap()
            .clone();
        planner.autonomy = orgbot_proto::agent::Autonomy::default();
        session.orchestrator.engine_mut().update_agent(planner);

        let engine = session.orchestrator.engine();
        let first_available = engine.agents().next().unwrap().id;
        assert_eq!(
            human_audience(&engine, "Please handle this", None),
            Audience::Agents {
                agents: vec![first_available]
            }
        );
    }

    #[test]
    fn untagged_request_skips_allowance_disabled_coordinator() {
        let directory = std::env::temp_dir().join(format!(
            "orgbot-routing-allowance-{}",
            orgbot_proto::RoomId::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("allowances.json");
        let ledger_path = directory.join("ledger.json");
        let disabled = orgbot_runtime::allowance::Policy {
            enabled: false,
            ..Default::default()
        };
        std::fs::write(
            &config_path,
            serde_json::to_vec(&orgbot_runtime::allowance::Config {
                providers: std::collections::BTreeMap::from([("deepseek".into(), disabled)]),
            })
            .unwrap(),
        )
        .unwrap();
        let environment = std::collections::HashMap::from([
            (
                "ORGBOT_ALLOWANCE_CONFIG".into(),
                config_path.display().to_string(),
            ),
            (
                "ORGBOT_ALLOWANCE_LEDGER".into(),
                ledger_path.display().to_string(),
            ),
        ]);
        let source = r#"
            [room]
            id = "routing-test"
            name = "Routing test"
            goal = "Test recipient selection"

            [[bots]]
            handle = "lead"
            roles = ["coordinator"]
            autonomy = "coordinator"
            provider = { kind = "scripted", script = "fixture" }

            [[bots]]
            handle = "backup"
            roles = ["coordinator"]
            autonomy = "coordinator"
            provider = { kind = "scripted", script = "fixture" }
        "#;
        let mut resolved = RoomConfig::parse(source, "routing.toml")
            .unwrap()
            .resolve_with_environment(&environment)
            .unwrap();
        resolved.bots[0].spec.provider = orgbot_proto::agent::ProviderRef::DeepSeek {
            model: "fixture".into(),
        };
        resolved.bots[1].spec.provider = orgbot_proto::agent::ProviderRef::Anthropic {
            model: "fixture".into(),
        };
        let (session, _) = RoomSession::build(resolved).unwrap();
        let expected = session
            .orchestrator
            .engine()
            .agent_by_handle("backup")
            .unwrap()
            .id;
        let target = session.orchestrator.default_human_target();
        assert_eq!(target, Some(expected));
        let engine = session.orchestrator.engine();
        assert_eq!(
            human_audience_with_default(&engine, "Please handle this", None, target),
            Audience::Agents {
                agents: vec![expected]
            }
        );
        drop(engine);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn attachment_import_allowlist_tracks_only_committed_message_files() {
        let (mut session, _handle) = session();
        let id = "att_0123456789abcdef0123456789abcdef".to_string();
        assert!(!session
            .committed_attachment_ids
            .read()
            .unwrap()
            .contains(&id));
        let (room, author) = {
            let engine = session.orchestrator.engine();
            (engine.room_id(), engine.agent_by_handle("lead").unwrap().id)
        };
        let mut message = ChatMessage::say(room, author, "attached file");
        message.files.push(orgbot_proto::chat::ChatFile {
            id: id.clone(),
            name: "source.bin".into(),
            mime_type: "application/octet-stream".into(),
            bytes: 1,
            has_text: false,
            images: 0,
            note: String::new(),
        });
        session
            .orchestrator
            .engine_mut()
            .apply(Command::PostMessage {
                message: Box::new(message),
            })
            .unwrap();
        session.broadcast_new_events();
        assert!(session
            .committed_attachment_ids
            .read()
            .unwrap()
            .contains(&id));
    }

    #[test]
    fn human_replies_are_threaded_and_invalid_parents_are_rejected() {
        let (mut session, handle) = session();
        let target = session
            .orchestrator
            .engine()
            .agent_by_handle("lead")
            .unwrap()
            .id;
        let parent = ChatMessage::say(
            session.orchestrator.engine().room_id(),
            target,
            "Can you clarify?",
        );
        let parent_id = parent.id;
        session
            .orchestrator
            .engine_mut()
            .apply(Command::PostMessage {
                message: Box::new(parent),
            })
            .unwrap();

        session.handle(Control::HumanMessage {
            author: "Alex".into(),
            body: "Here is the detail.".into(),
            task: None,
            files: vec![],
            reply_to: Some(parent_id),
        });
        let reply = session
            .orchestrator
            .engine()
            .transcript()
            .last()
            .unwrap()
            .clone();
        assert_eq!(reply.reply_to, Some(parent_id));
        assert_eq!(
            reply.audience,
            Audience::Agents {
                agents: vec![target]
            }
        );
        assert_eq!(reply.intent, MessageIntent::Ask);
        assert!(reply.wakes(target, &["coordinator".into()]));

        let human_parent = ChatMessage {
            author: Author::Human {
                name: "Alex".into(),
            },
            ..ChatMessage::say(
                session.orchestrator.engine().room_id(),
                target,
                "I will follow up with myself.",
            )
        };
        let human_parent_id = human_parent.id;
        session
            .orchestrator
            .engine_mut()
            .apply(Command::PostMessage {
                message: Box::new(human_parent),
            })
            .unwrap();
        session.handle(Control::HumanMessage {
            author: "Alex".into(),
            body: "Acknowledged.".into(),
            task: None,
            files: vec![],
            reply_to: Some(human_parent_id),
        });
        let human_reply = session
            .orchestrator
            .engine()
            .transcript()
            .last()
            .unwrap()
            .clone();
        assert_eq!(human_reply.audience, Audience::Human);
        assert_eq!(human_reply.intent, MessageIntent::Answer);

        let before = session.orchestrator.engine().transcript().len();
        let mut events = handle.subscribe();
        session.handle(Control::HumanMessage {
            author: "Alex".into(),
            body: "This must not broadcast.".into(),
            task: None,
            files: vec![],
            reply_to: Some(MessageId::new()),
        });
        assert_eq!(session.orchestrator.engine().transcript().len(), before);
        assert!(matches!(
            events.try_recv(),
            Ok(ServerMessage::Error { message }) if message.contains("reply target is not in this room")
        ));
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
                reply_to: None,
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
                    reply_to: None,
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
