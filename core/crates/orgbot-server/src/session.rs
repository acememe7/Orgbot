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

use std::time::Duration;

use orgbot_engine::Command;
use orgbot_proto::chat::Audience;
use orgbot_proto::event::Seq;
use orgbot_proto::task::BlockReason;
use orgbot_proto::{
    Author, ChatMessage, MessageIntent, RoomPhase, TaskId, TaskState, Timestamp,
};
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
    /// A human speaks into the chat.
    HumanMessage {
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
}

/// A cheap, clonable handle to a running room.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    control: mpsc::Sender<Control>,
    events: broadcast::Sender<ServerMessage>,
}

impl SessionHandle {
    /// Subscribe to the room's live event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<ServerMessage> {
        self.events.subscribe()
    }

    /// Send a control message. Returns `false` if the room has stopped.
    pub async fn send(&self, control: Control) -> bool {
        self.control.send(control).await.is_ok()
    }

    /// Request a snapshot, or `None` if the room has stopped.
    pub async fn snapshot(&self) -> Option<ServerMessage> {
        let (tx, rx) = oneshot::channel();
        if self.control.send(Control::Snapshot(tx)).await.is_err() {
            return None;
        }
        rx.await.ok()
    }
}

/// A room and the machinery to run it.
#[derive(Debug)]
pub struct RoomSession {
    orchestrator: Orchestrator,
    control: mpsc::Receiver<Control>,
    events: broadcast::Sender<ServerMessage>,
    /// The last log position broadcast, so each step ships only new events.
    broadcast_head: Seq,
    /// Whether the room has finished its work. A settled room takes no more
    /// turns but stays connected, so its transcript can still be read and a
    /// person can still speak into it.
    settled: bool,
}

impl RoomSession {
    /// Build a session from a resolved room, returning it and a handle.
    ///
    /// The room is seated and seeded but not started; call [`Self::run`] to
    /// begin. Errors here mean the room itself is invalid (a bad roster, a seed
    /// task that names a missing role); a caller should surface them rather than
    /// spawn a room that cannot work.
    pub fn build(resolved: ResolvedRoom) -> anyhow::Result<(Self, SessionHandle)> {
        let mut orchestrator = Orchestrator::new(resolved.spec);
        for bot in resolved.bots {
            orchestrator
                .seat(bot.spec, bot.provider)
                .map_err(|e| anyhow::anyhow!("seating a bot: {e}"))?;
        }
        orchestrator
            .start()
            .map_err(|e| anyhow::anyhow!("starting the room: {e}"))?;

        // Seed the board after the room is running, since CreateTask is a
        // running-room command.
        for task in resolved.tasks {
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
                    parent: None,
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

        let session = Self {
            orchestrator,
            control: control_rx,
            events: events_tx.clone(),
            broadcast_head: Seq::ZERO,
            settled: false,
        };
        let handle = SessionHandle {
            control: control_tx,
            events: events_tx,
        };
        Ok((session, handle))
    }

    /// Run the room to a stopping point.
    ///
    /// Bots take turns; between turns, pending human input is applied. When no
    /// bot has anything to do the session waits — it does not close — because a
    /// person may still speak. It stops only when the completion rule is met, a
    /// budget is spent, or every client and control sender has gone away.
    pub async fn run(mut self) -> RunOutcome {
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
            let resting = match self.orchestrator.stop_condition() {
                None => false,
                Some(RunOutcome::Idle | RunOutcome::Stalled { .. }) => true,
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

    /// Give one bot one turn, streaming its output and broadcasting the result.
    async fn step(&mut self) {
        // Peek who will act so the live stream can be tagged with the bot. The
        // scheduler is deterministic on state, which does not change between
        // this peek and the step.
        let actor = self.orchestrator.next_actor();
        let sink = actor.map(|agent| self.spawn_stream_forwarder(agent));

        self.orchestrator.step(sink).await;
        self.broadcast_new_events();
        let _ = self.events.send(ServerMessage::Phase {
            phase: self.orchestrator.engine().room().phase,
        });
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
        let new: Vec<_> = self
            .orchestrator
            .engine()
            .events_since(self.broadcast_head)
            .cloned()
            .collect();
        for envelope in new {
            self.broadcast_head = self.broadcast_head.max(envelope.seq);
            let _ = self.events.send(ServerMessage::Event {
                envelope: Box::new(envelope),
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
                Err(mpsc::error::TryRecvError::Disconnected) => return ControlFlow::Continue,
            }
        }
    }

    /// Act on one control message.
    fn handle(&mut self, control: Control) -> ControlFlow {
        match control {
            Control::Snapshot(reply) => {
                let _ = reply.send(self.snapshot());
            }
            Control::HumanMessage { author, body, task } => {
                let room = self.orchestrator.engine().room_id();
                let mut message = ChatMessage {
                    id: orgbot_proto::MessageId::new(),
                    room,
                    author: Author::Human { name: author },
                    audience: Audience::Everyone,
                    intent: MessageIntent::Ask,
                    body,
                    task,
                    handoff: None,
                    reply_to: None,
                    attachments: vec![],
                    sent_at: Timestamp::now(),
                };
                // A human message is addressed to the room and expects a reply,
                // so it wakes the bots.
                message.intent = MessageIntent::Ask;
                let _ = self.orchestrator.engine_mut().apply(Command::PostMessage {
                    message: Box::new(message),
                });
                // Someone spoke: give the room a fresh budget so it does not
                // immediately re-declare itself stalled on old history.
                self.orchestrator.wake();
                self.broadcast_new_events();
            }
            Control::Answer { task, answer } => {
                self.answer_waiting_task(task, answer);
                self.broadcast_new_events();
            }
            Control::Pause { reason } => {
                let _ = self
                    .orchestrator
                    .engine_mut()
                    .apply(Command::PauseRoom { reason });
                self.broadcast_new_events();
            }
            Control::Resume => {
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
        let engine = self.orchestrator.engine();
        let Some(record) = engine.task(task) else {
            return;
        };
        let waiting_on_human = record.state == TaskState::Blocked
            && record.blocked_on.iter().any(BlockReason::needs_human);
        if !waiting_on_human {
            return;
        }
        let room = engine.room_id();
        let assignee = record.assignee;

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
            sent_at: Timestamp::now(),
        };
        message.intent = MessageIntent::Answer;
        let engine = self.orchestrator.engine_mut();
        let _ = engine.apply(Command::PostMessage {
            message: Box::new(message),
        });
        let _ = engine.apply(Command::UnblockTask { task });
    }

    /// A full snapshot of the room right now.
    pub fn snapshot(&self) -> ServerMessage {
        let engine = self.orchestrator.engine();
        let roster = engine
            .agents()
            .map(|spec| AgentView {
                id: spec.id,
                handle: spec.handle.clone(),
                display_name: spec.display_name.clone(),
                roles: spec.roles.clone(),
                status: engine.status(spec.id),
            })
            .collect();
        ServerMessage::Snapshot {
            wire_version: crate::protocol::WIRE_VERSION,
            room: Box::new(engine.room().clone()),
            roster,
            tasks: engine.tasks().cloned().collect(),
            head: engine.head(),
        }
    }

    /// Record that the room has finished, without disconnecting anyone.
    ///
    /// Deliberately does not send [`ServerMessage::Closed`]: that tells clients
    /// to tear down, and a finished room should still be readable.
    fn finish(&mut self, outcome: &RunOutcome) {
        if self.orchestrator.engine().room().phase != RoomPhase::Completed {
            let _ = self.orchestrator.engine_mut().apply(Command::CloseRoom {
                reason: describe(outcome),
            });
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
        ) && self.orchestrator.engine().room().phase != RoomPhase::Completed
        {
            let _ = self.orchestrator.engine_mut().apply(Command::CloseRoom {
                reason: describe(&outcome),
            });
            self.broadcast_new_events();
        }
        let _ = self.events.send(ServerMessage::Closed {
            reason: describe(&outcome),
        });
        outcome
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
    use orgbot_proto::event::Event;

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

    #[test]
    fn a_built_session_has_seeded_its_board_and_started() {
        let (session, _handle) = session();
        assert_eq!(session.orchestrator.engine().room().phase, RoomPhase::Running);
        assert_eq!(session.orchestrator.engine().tasks().count(), 1);
        let task = session.orchestrator.engine().tasks().next().unwrap();
        assert_eq!(task.state, TaskState::Ready, "a seeded task with open=true is claimable");
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

        // The scripted coordinator has nothing to do with a task it cannot work,
        // so the room stalls quickly — but a subscriber still sees real events
        // (at minimum the closing message) before it ends.
        let outcome = run.await.unwrap();
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
        // The room may finish before the request lands; either a snapshot or a
        // cleanly stopped room is acceptable, never a hang.
        if let Some(ServerMessage::Snapshot { roster, .. }) = snapshot {
            assert_eq!(roster.len(), 1);
        }
        let _ = run.await;
    }

    #[tokio::test]
    async fn a_human_message_is_accepted_while_the_room_runs() {
        let (session, handle) = session();
        let mut events = handle.subscribe();
        let run = tokio::spawn(session.run());

        handle
            .send(Control::HumanMessage {
                author: "Alex".into(),
                body: "any progress?".into(),
                task: None,
            })
            .await;

        let _ = run.await;
        let mut saw_message = false;
        while let Ok(message) = events.try_recv() {
            if let ServerMessage::Event { envelope } = message {
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
            if let ServerMessage::Event { envelope } = message {
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
