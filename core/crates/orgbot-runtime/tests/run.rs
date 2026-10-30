//! Whole-room runs.
//!
//! The bots here are driven by a provider that *reads what it was told* and
//! responds, rather than replaying a fixed script. That matters: it means these
//! tests exercise the real prompt text and the real tool-result strings. If the
//! situation digest stops naming a task id, or a tool result stops echoing the
//! handle a bot needs next, these fail — which is exactly the class of bug that
//! is invisible to unit tests and fatal in production.

use std::sync::Arc;

use async_trait::async_trait;
use orgbot_proto::graph::TaskOrigin;
use orgbot_proto::agent::{Autonomy, ProviderRef};
use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
use orgbot_proto::{AgentSpec, TaskState};
use orgbot_providers::types::{
    CompletionRequest, CompletionResponse, ContentBlock, Role, StopReason, StreamEvent, Usage,
};
use orgbot_providers::{Provider, ScriptedProvider};
use orgbot_runtime::{Orchestrator, RunOutcome};
use serde_json::{json, Value};
use tokio::sync::mpsc;

// ------------------------------------------------------------------- harness

/// A provider that decides from the text it was given, like a model does.
struct Reactive {
    name: String,
    #[allow(clippy::type_complexity)]
    respond: Box<dyn Fn(&str, &str) -> CompletionResponse + Send + Sync>,
}

impl std::fmt::Debug for Reactive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reactive")
            .field("name", &self.name)
            .finish()
    }
}

#[async_trait]
impl Provider for Reactive {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        _sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> orgbot_providers::Result<CompletionResponse> {
        // What the bot was told most recently, and everything it can remember.
        let last = request
            .messages
            .iter()
            .rev()
            .find(|turn| turn.role == Role::User)
            .map(text_of)
            .unwrap_or_default();
        let all = request
            .messages
            .iter()
            .map(text_of)
            .collect::<Vec<_>>()
            .join("\n");
        Ok((self.respond)(&last, &all))
    }
}

/// All readable text in a turn, tool results included.
fn text_of(turn: &orgbot_providers::types::Turn) -> String {
    turn.content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            ContentBlock::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tokens(text: &str, prefix: &str) -> Vec<String> {
    let needle = format!("{prefix}_");
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| word.starts_with(&needle) && word.len() > needle.len() + 8)
        .map(str::to_string)
        .collect()
}

fn first_id(text: &str, prefix: &str) -> Option<String> {
    tokens(text, prefix).into_iter().next()
}

fn last_id(text: &str, prefix: &str) -> Option<String> {
    tokens(text, prefix).into_iter().next_back()
}

/// The first id of `prefix` appearing after `marker`.
fn id_after(text: &str, marker: &str, prefix: &str) -> Option<String> {
    let start = text.find(marker)? + marker.len();
    first_id(&text[start..], prefix)
}

fn call(name: &str, input: Value) -> CompletionResponse {
    CompletionResponse {
        model: "reactive".into(),
        content: vec![ContentBlock::ToolUse {
            id: format!("toolu_{name}"),
            name: name.into(),
            input,
        }],
        stop_reason: StopReason::ToolUse,
        usage: Usage::default(),
    }
}

fn say(text: &str) -> CompletionResponse {
    CompletionResponse {
        model: "reactive".into(),
        content: vec![ContentBlock::text(text)],
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    }
}

/// End the turn having nothing to add. Registers as no progress.
fn nothing() -> CompletionResponse {
    CompletionResponse {
        model: "reactive".into(),
        content: Vec::new(),
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    }
}

fn reactive(
    name: &str,
    respond: impl Fn(&str, &str) -> CompletionResponse + Send + Sync + 'static,
) -> Arc<dyn Provider> {
    Arc::new(Reactive {
        name: name.into(),
        respond: Box::new(respond),
    })
}

fn spec(handle: &str, roles: &[&str], autonomy: Autonomy) -> AgentSpec {
    AgentSpec::new(
        handle,
        handle,
        ProviderRef::Scripted {
            script: "reactive".into(),
        },
    )
    .with_roles(roles.to_vec())
    .with_autonomy(autonomy)
}

fn room(goal: &str) -> Orchestrator {
    Orchestrator::new(RoomSpec {
        name: "release".into(),
        goal: goal.into(),
        roster: vec![],
        completion: CompletionRule::AllTasksTerminal,
        limits: RoomLimits::default(),
        access: Default::default(),
    })
}

#[tokio::test]
async fn an_owner_question_waits_without_spending_planning_turns_or_completing() {
    use orgbot_engine::Command;
    let mut room = room("wait for the owner");
    let lead = spec("lead", &["coordinator"], Autonomy::coordinator());
    let id = lead.id;
    room.seat(lead, Arc::new(ScriptedProvider::new(vec![]))).unwrap();
    room.start().unwrap();
    room.engine_mut().apply(Command::AskOwner {
        by: id,
        task: None,
        question: "Which direction should we take?".into(),
        options: vec!["Desktop".into(), "Mobile".into()],
    }).unwrap();
    assert!(!room.is_actionable(id), "waiting must not trigger more planning calls");
    assert!(room.step(None).await.is_none());
    assert_eq!(room.turns_used(), 0);
    let created = room.engine_mut().apply(Command::CreateTask {
        title: "Previous work".into(), brief: String::new(), by: None,
        required_role: None, priority: orgbot_proto::task::Priority::Normal,
        acceptance_criteria: vec![], depends_on: vec![], origin: TaskOrigin::plain(),
    }).unwrap();
    let task = created.iter().find_map(|event| match &event.event {
        orgbot_proto::Event::TaskCreated { task } => Some(task.id), _ => None,
    }).unwrap();
    room.engine_mut().apply(Command::RetireTask { task, reason: "Fixture".into() }).unwrap();
    assert!(!room.is_complete(), "pending owner questions keep a finished board open");
}

// ----------------------------------------------------------- the bot behaviours

/// A coordinator: plans once, then delivers approved work.
fn coordinator() -> Arc<dyn Provider> {
    reactive("lead", |last, _all| {
        if let Some(task) = id_after(last, "Approved and waiting for you to deliver:", "task") {
            return call("submit_task", json!({"task": task}));
        }
        if last.contains("The board is empty") {
            return call(
                "create_task",
                json!({
                    "title": "write the release notes",
                    "brief": "summarise everything since v1.2",
                    "acceptance_criteria": ["covers every merged PR"],
                    "required_role": "writer",
                    "priority": "high"
                }),
            );
        }
        if last.contains("is now submitted") {
            return say("delivered");
        }
        if last.contains("Created and opened") {
            return say("task is on the board");
        }
        nothing()
    })
}

/// A worker: takes open work, does it, sends it for inspection. Also answers
/// handoffs addressed to it.
fn worker(handle: &'static str) -> Arc<dyn Provider> {
    reactive(handle, move |last, all| {
        // Answering someone who is blocked comes first.
        if let Some(handoff) = id_after(last, "Handoffs waiting on your answer", "ho") {
            return call("accept_handoff", json!({"handoff": handoff}));
        }
        if last.contains("Accepted ho_") {
            if let Some(handoff) = first_id(last, "ho") {
                return call(
                    "complete_handoff",
                    json!({"handoff": handoff, "summary": "12 entries since v1.2"}),
                );
            }
        }
        if last.contains("Delivered.") {
            return say("sent it over");
        }

        // Then this bot's own task, step by step off each tool result.
        if last.contains("Recorded `notes.md`") {
            let (task, artifact) = (last_id(all, "task"), first_id(last, "art"));
            if let (Some(task), Some(artifact)) = (task, artifact) {
                return call(
                    "submit_for_review",
                    json!({"task": task, "artifacts": [artifact]}),
                );
            }
        }
        if last.contains("is now in_progress") {
            if let Some(task) = first_id(last, "task") {
                return call(
                    "add_artifact",
                    json!({
                        "task": task,
                        "name": "notes.md",
                        "content": "## v1.3\n- fixed the thing"
                    }),
                );
            }
        }
        if last.contains("is now claimed") {
            if let Some(task) = first_id(last, "task") {
                return call("start_task", json!({"task": task}));
            }
        }
        // Holding a started task that has not been sent for inspection — either
        // fresh work or a revision that came back. Either way: produce and
        // resubmit.
        if last.contains("[in_progress]") {
            if let Some(task) = id_after(last, "Your tasks:", "task") {
                return call(
                    "add_artifact",
                    json!({
                        "task": task,
                        "name": "notes.md",
                        "content": "## v1.3\n- fixed the thing, again"
                    }),
                );
            }
        }
        if let Some(task) = id_after(last, "Open tasks you could claim:", "task") {
            return call("claim_task", json!({"task": task}));
        }
        if last.contains("Sent for inspection") {
            return say("submitted for review");
        }
        nothing()
    })
}

/// An inspector: reviews whatever is queued, approving clean work.
fn inspector() -> Arc<dyn Provider> {
    reactive("qa", |last, _all| {
        if let Some(task) = id_after(last, "Awaiting your inspection:", "task") {
            return call(
                "record_review",
                json!({
                    "task": task,
                    "summary": "covers the criteria",
                    "findings": [],
                    "criteria_met": ["covers every merged PR"]
                }),
            );
        }
        if last.contains("Review recorded") {
            return say("approved");
        }
        nothing()
    })
}

/// An inspector that always finds a blocking problem.
fn harsh_inspector() -> Arc<dyn Provider> {
    reactive("qa", |last, _all| {
        if let Some(task) = id_after(last, "Awaiting your inspection:", "task") {
            return call(
                "record_review",
                json!({
                    "task": task,
                    "summary": "still not right",
                    "findings": [{"severity": "major", "summary": "the version is wrong"}]
                }),
            );
        }
        if last.contains("Review recorded") {
            return say("sent back");
        }
        nothing()
    })
}

// ------------------------------------------------------------------- the runs

#[tokio::test]
async fn a_room_plans_works_inspects_and_delivers() {
    let mut room = room("ship the release notes");
    room.seat(
        spec("lead", &["coordinator"], Autonomy::coordinator()),
        coordinator(),
    )
    .unwrap();
    room.seat(spec("w1", &["writer"], Autonomy::default()), worker("w1"))
        .unwrap();
    room.seat(
        spec("qa", &["reviewer"], Autonomy::inspector()),
        inspector(),
    )
    .unwrap();
    room.start().unwrap();

    let outcome = room.run(None).await;
    assert_eq!(
        outcome,
        RunOutcome::Completed,
        "the room should finish its goal"
    );

    // Exactly one task, delivered.
    let engine = room.engine();
    let tasks: Vec<_> = engine.tasks().collect();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].state, TaskState::Submitted);

    // The work itself exists, and was inspected by someone other than its author.
    assert_eq!(tasks[0].artifacts.len(), 1);
    let reviews = engine.reviews();
    assert_eq!(reviews.len(), 1);
    assert_ne!(
        Some(reviews[0].inspector),
        tasks[0].assignee,
        "the inspector must not be the author"
    );

    // And the run is reconstructible from its log.
    let log = engine.log();
    assert!(log.iter().any(|e| e.event.name() == "task_created"));
    assert!(log.iter().any(|e| e.event.name() == "review_recorded"));
    assert!(log.iter().any(|e| e.event.name() == "room_closed"));
    for (index, envelope) in log.iter().enumerate() {
        assert_eq!(envelope.seq.0 as usize, index + 1, "gapless log");
    }
}

#[tokio::test]
async fn work_that_keeps_failing_inspection_fails_rather_than_looping() {
    let mut room = room("ship the release notes");
    room.seat(
        spec("lead", &["coordinator"], Autonomy::coordinator()),
        coordinator(),
    )
    .unwrap();
    room.seat(spec("w1", &["writer"], Autonomy::default()), worker("w1"))
        .unwrap();
    room.seat(
        spec("qa", &["reviewer"], Autonomy::inspector()),
        harsh_inspector(),
    )
    .unwrap();
    room.start().unwrap();

    let outcome = room.run(None).await;
    // Everything read from one borrow: asking for the engine again while this
    // one is alive is a nested lock, and the engine refuses it.
    let (state, revision, max_revisions, used, budget) = {
        let engine = room.engine();
        let task = engine.tasks().next().expect("the board has a task");
        (
            task.state,
            task.revision,
            task.max_revisions,
            engine.room().turns_used,
            engine.room().spec.limits.max_total_turns,
        )
    };
    assert_eq!(
        state,
        TaskState::Failed,
        "a worker and an inspector that never agree must terminate"
    );
    assert!(
        revision <= max_revisions,
        "the rework budget must be respected, saw revision {revision}"
    );
    assert!(
        used < budget,
        "it should fail on the revision budget, not by exhausting the room"
    );
    assert!(matches!(
        outcome,
        RunOutcome::Completed | RunOutcome::Idle | RunOutcome::Stalled { .. }
    ));
}

#[tokio::test]
async fn a_room_of_silent_bots_stops_instead_of_spinning() {
    let mut room = room("do something");
    // Coordinators, so they are actionable, but with nothing to say.
    for handle in ["a", "b"] {
        room.seat(
            spec(handle, &["coordinator"], Autonomy::coordinator()),
            Arc::new(ScriptedProvider::new(vec![])),
        )
        .unwrap();
    }
    room.start().unwrap();

    let outcome = room.run(None).await;
    assert!(
        matches!(outcome, RunOutcome::Stalled { .. } | RunOutcome::Idle),
        "got {outcome:?}"
    );
    assert!(
        room.turns_used() <= 12,
        "a stalled room must be noticed quickly, not after the whole budget: {} turns",
        room.turns_used()
    );
}

#[tokio::test]
async fn the_turn_budget_stops_a_room_that_will_not_settle() {
    let mut room = Orchestrator::new(RoomSpec {
        name: "chatty".into(),
        goal: "talk forever".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits {
            max_total_turns: 5,
            ..RoomLimits::default()
        },
        access: Default::default(),
    });
    // A bot that always has something to say never looks stalled, so only the
    // budget stops it.
    room.seat(
        spec("a", &["coordinator"], Autonomy::coordinator()),
        reactive("a", |_last, _all| say("still going")),
    )
    .unwrap();
    room.seat(spec("w", &["writer"], Autonomy::default()), worker("w"))
        .unwrap();
    room.start().unwrap();

    let outcome = room.run(None).await;
    assert!(
        matches!(outcome, RunOutcome::OutOfTurns | RunOutcome::Paused),
        "got {outcome:?}"
    );
    assert!(room.turns_used() >= 5);
}

#[tokio::test]
async fn a_paused_room_takes_no_turns() {
    let mut room = room("ship the release notes");
    room.seat(
        spec("lead", &["coordinator"], Autonomy::coordinator()),
        coordinator(),
    )
    .unwrap();
    room.start().unwrap();
    room.engine_mut()
        .apply(orgbot_engine::Command::PauseRoom {
            reason: "operator stepped in".into(),
        })
        .unwrap();

    assert_eq!(room.run(None).await, RunOutcome::Paused);
    assert_eq!(room.turns_used(), 0);
}

#[tokio::test]
async fn a_bot_whose_backend_fails_does_not_take_the_room_down() {
    /// A provider that always fails, as an unreachable endpoint would.
    #[derive(Debug)]
    struct Broken;

    #[async_trait]
    impl Provider for Broken {
        fn name(&self) -> &str {
            "broken"
        }
        async fn complete(
            &self,
            _request: CompletionRequest,
            _sink: Option<mpsc::Sender<StreamEvent>>,
        ) -> orgbot_providers::Result<CompletionResponse> {
            Err(orgbot_providers::ProviderError::Unauthorized {
                provider: "broken".into(),
                status: 401,
            })
        }
    }

    let mut room = room("ship the release notes");
    room.seat(
        spec("lead", &["coordinator"], Autonomy::coordinator()),
        coordinator(),
    )
    .unwrap();
    room.seat(
        spec("w1", &["writer"], Autonomy::default()),
        Arc::new(Broken),
    )
    .unwrap();
    room.seat(
        spec("qa", &["reviewer"], Autonomy::inspector()),
        inspector(),
    )
    .unwrap();
    room.start().unwrap();

    let outcome = room.run(None).await;

    // The failing bot is quarantined and the room reaches a clean stop rather
    // than hanging or panicking.
    let broken = room
        .engine()
        .agents()
        .find(|spec| spec.handle == "w1")
        .map(|spec| spec.id)
        .unwrap();
    assert_eq!(
        room.engine().status(broken),
        orgbot_proto::AgentStatus::Faulted
    );
    assert!(
        matches!(outcome, RunOutcome::Idle | RunOutcome::Stalled { .. }),
        "got {outcome:?}"
    );
    // And its task went back to the pool rather than being stranded on it.
    assert!(room
        .engine()
        .tasks()
        .all(|task| task.assignee != Some(broken)));
}

#[tokio::test]
async fn a_bot_is_scheduled_to_answer_a_question_addressed_to_it() {
    // A room that is otherwise idle: one worker, no tasks, manual completion.
    let mut room = Orchestrator::new(RoomSpec {
        name: "chat".into(),
        goal: "answer questions".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
        access: Default::default(),
    });
    let worker = room
        .seat(
            spec("helper", &["writer"], Autonomy::default()),
            reactive("helper", |_last, _all| say("Hi, I am here.")),
        )
        .unwrap();
    room.start().unwrap();

    // With nothing to do, nobody is actionable.
    assert!(room.next_actor().is_none());

    // A human asks the room a question.
    let room_id = room.engine().room_id();
    room.engine_mut()
        .apply(orgbot_engine::Command::PostMessage {
            message: Box::new(orgbot_proto::ChatMessage {
                id: orgbot_proto::MessageId::new(),
                room: room_id,
                author: orgbot_proto::Author::Human {
                    name: "Alex".into(),
                },
                audience: orgbot_proto::Audience::Everyone,
                intent: orgbot_proto::MessageIntent::Ask,
                body: "anyone there?".into(),
                task: None,
                handoff: None,
                reply_to: None,
                attachments: vec![],

                files: vec![],
                sent_at: orgbot_proto::Timestamp::now(),
            }),
        })
        .unwrap();

    // Now the worker is actionable, and a step produces its reply.
    assert_eq!(room.next_actor(), Some(worker));
    let report = room.step(None).await.expect("the worker should act");
    assert_eq!(report.agent, worker);
    assert!(
        room.engine()
            .transcript()
            .iter()
            .any(|m| m.body.contains("here")),
        "the worker should have answered into the transcript"
    );
}

#[tokio::test]
async fn an_ask_to_everyone_is_answered_by_every_bot() {
    // The roll call from the desktop: "@all say hi" must reach every seat, and
    // each bot must be told the broadcast wants its own reply, addressed to
    // the owner by the name they wrote under.
    let mut room = Orchestrator::new(RoomSpec {
        name: "chat".into(),
        goal: "answer questions".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
        access: Default::default(),
    });
    let handles = ["ada", "bo", "cy"];
    let mut seats = Vec::new();
    for handle in handles {
        let reply = format!("hi from {handle}");
        seats.push(
            room.seat(
                spec(handle, &["writer"], Autonomy::default()),
                reactive(handle, move |_last, all| {
                    if all.contains("to everyone, including you: reply")
                        && all.contains("Answer it yourself in the chat")
                        && all.contains("Address them as Aece")
                    {
                        say(&reply)
                    } else {
                        nothing()
                    }
                }),
            )
            .unwrap(),
        );
    }
    room.start().unwrap();
    assert!(room.next_actor().is_none());

    let room_id = room.engine().room_id();
    room.engine_mut()
        .apply(orgbot_engine::Command::PostMessage {
            message: Box::new(orgbot_proto::ChatMessage {
                id: orgbot_proto::MessageId::new(),
                room: room_id,
                author: orgbot_proto::Author::Human {
                    name: "Aece".into(),
                },
                audience: orgbot_proto::Audience::Everyone,
                intent: orgbot_proto::MessageIntent::Ask,
                body: "@all say hi".into(),
                task: None,
                handoff: None,
                reply_to: None,
                attachments: vec![],
                files: vec![],
                sent_at: orgbot_proto::Timestamp::now(),
            }),
        })
        .unwrap();

    // A decision answer the room records for the owner carries a stand-in
    // author; it must not become the name bots call the owner.
    room.engine_mut()
        .apply(orgbot_engine::Command::PostMessage {
            message: Box::new(
                orgbot_proto::ChatMessage::human(
                    room_id,
                    orgbot_proto::OWNER_STAND_IN,
                    "The owner answered: A",
                )
                .with_intent(orgbot_proto::MessageIntent::Answer),
            ),
        })
        .unwrap();

    let mut acted = Vec::new();
    while room.next_actor().is_some() && acted.len() < 10 {
        acted.push(room.step(None).await.expect("a bot should act").agent);
    }
    for seat in &seats {
        assert!(acted.contains(seat), "every bot should take a turn");
    }
    for handle in handles {
        assert!(
            room.engine()
                .transcript()
                .iter()
                .any(|m| m.body == format!("hi from {handle}")),
            "@{handle} should have answered the roll call"
        );
    }
}

/// Waits for a release before answering, like an agent CLI on a long job.
#[derive(Debug)]
struct SlowWorker(Arc<tokio::sync::Notify>);

#[async_trait]
impl Provider for SlowWorker {
    fn name(&self) -> &str {
        "slow"
    }

    async fn complete(
        &self,
        _request: CompletionRequest,
        _sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> orgbot_providers::Result<CompletionResponse> {
        self.0.notified().await;
        Ok(say("slow job done"))
    }
}

fn owner_asks(room: &orgbot_runtime::SharedEngine, to: Option<orgbot_proto::AgentId>, body: &str) {
    room.with(|engine| {
        let room_id = engine.room_id();
        engine
            .apply(orgbot_engine::Command::PostMessage {
                message: Box::new(orgbot_proto::ChatMessage {
                    id: orgbot_proto::MessageId::new(),
                    room: room_id,
                    author: orgbot_proto::Author::Human { name: "Aece".into() },
                    audience: match to {
                        Some(agent) => orgbot_proto::Audience::Agents { agents: vec![agent] },
                        None => orgbot_proto::Audience::Everyone,
                    },
                    intent: orgbot_proto::MessageIntent::Ask,
                    body: body.into(),
                    task: None,
                    handoff: None,
                    reply_to: None,
                    attachments: vec![],
                    files: vec![],
                    sent_at: orgbot_proto::Timestamp::now(),
                }),
            })
            .unwrap();
    });
}

#[tokio::test]
async fn a_bot_asked_by_the_owner_answers_while_a_slower_turn_runs() {
    // The room from the screenshot: one bot deep in a long turn, another done
    // already, and the owner asking the free one a question. It must answer
    // now, not after the slow turn ends.
    let mut room = Orchestrator::new(RoomSpec {
        name: "rolling".into(),
        goal: "answer".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits {
            max_parallel_turns: 2,
            ..RoomLimits::default()
        },
        access: Default::default(),
    });
    let release = Arc::new(tokio::sync::Notify::new());
    room.seat(
        spec("slow", &["writer"], Autonomy::default()),
        Arc::new(SlowWorker(release.clone())),
    )
    .unwrap();
    let quick = room
        .seat(
            spec("quick", &["writer"], Autonomy::default()),
            reactive("quick", |_last, all| {
                if all.contains("what is slow waiting on?") {
                    say("slow is waiting on its long job")
                } else {
                    say("hello")
                }
            }),
        )
        .unwrap();
    room.start().unwrap();
    let shared = room.shared_engine();
    owner_asks(&shared, None, "@all start");

    let said = |text: &'static str| {
        let shared = shared.clone();
        move || shared.with(|engine| engine.transcript().iter().any(|m| m.body == text))
    };
    let until = |done: Box<dyn Fn() -> bool>| async move {
        while !done() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };
    let sinks = |_| None;
    let run = room.step_rolling(2, &sinks);
    tokio::pin!(run);
    let scenario = async {
        until(Box::new(said("hello"))).await;
        owner_asks(&shared, Some(quick), "@quick what is slow waiting on?");
        until(Box::new(said("slow is waiting on its long job"))).await;
    };
    tokio::select! {
        _ = &mut run => panic!("the run ended while the slow turn was still going"),
        done = tokio::time::timeout(std::time::Duration::from_secs(5), scenario) => {
            done.expect("the asked bot must answer while the slow turn runs");
        }
    }
    assert!(!said("slow job done")(), "the slow turn is still running");

    release.notify_one();
    let reports = run.await;
    assert!(said("slow job done")());
    assert_eq!(reports.len(), 3, "slow once, quick twice: {reports:?}");
}

/// Reports a picture the way an agent CLI does: an image file it saved.
#[derive(Debug)]
struct Draws(std::path::PathBuf);

#[async_trait]
impl Provider for Draws {
    fn name(&self) -> &str {
        "draws"
    }

    async fn complete(
        &self,
        _request: CompletionRequest,
        _sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> orgbot_providers::Result<CompletionResponse> {
        Ok(CompletionResponse {
            model: "draws".into(),
            content: vec![
                ContentBlock::text("the poster is ready"),
                ContentBlock::Image {
                    media_type: "image/png".into(),
                    source: orgbot_providers::types::ImageSource::File {
                        path: self.0.clone(),
                    },
                },
            ],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        })
    }
}

#[tokio::test]
async fn an_image_a_bot_generates_is_shown_in_the_room() {
    // Codex and Grok draw with their own image tools and keep the picture in
    // their home, which for a bot is private: the owner never saw it.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("orgbot-drawn-room-{nonce}"));
    let drawn = dir.join("home/.codex/generated_images/s/exec-1.png");
    std::fs::create_dir_all(drawn.parent().unwrap()).unwrap();
    std::fs::write(&drawn, b"\x89PNG\r\n\x1a\nrest of the picture").unwrap();
    let attachments = dir.join("attachments");

    let mut room = Orchestrator::new(RoomSpec {
        name: "poster".into(),
        goal: "draw".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
        access: Default::default(),
    });
    let artist = room
        .seat(spec("artist", &["designer"], Autonomy::default()), Arc::new(Draws(drawn.clone())))
        .unwrap();
    room.attach_attachments(attachments.clone());
    room.start().unwrap();
    owner_asks(&room.shared_engine(), Some(artist), "@artist draw the poster");
    room.step(None).await;

    let posted = room.engine().transcript().last().unwrap().clone();
    assert_eq!(posted.body, "the poster is ready");
    assert_eq!(posted.files.len(), 1, "the picture comes with the bot's message");
    assert_eq!(posted.files[0].mime_type, "image/png");
    assert!(attachments.join(&posted.files[0].id).join("manifest.json").is_file());
    // It lives in the room, not in the conversation the bot carries forward.
    assert!(!room.checkpoint().to_string().contains(drawn.to_str().unwrap()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn a_bot_working_past_its_claim_window_keeps_its_task() {
    // From the live room: Devon and Quinn were each in one turn for over the
    // fifteen-minute claim window, running commands that do not touch the
    // board. Another bot finishing ran the housekeeping tick, which took their
    // tasks away mid-turn, and Quinn's submit then failed.
    let mut room = Orchestrator::new(RoomSpec {
        name: "keep alive".into(),
        goal: "finish".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits {
            max_parallel_turns: 2,
            ..RoomLimits::default()
        },
        access: Default::default(),
    });
    let release = Arc::new(tokio::sync::Notify::new());
    let slow = room
        .seat(
            spec("slow", &["writer"], Autonomy::default()),
            Arc::new(SlowWorker(release.clone())),
        )
        .unwrap();
    let quick = room
        .seat(
            spec("quick", &["writer"], Autonomy::default()),
            reactive("quick", |_last, _all| say("done quickly")),
        )
        .unwrap();
    room.start().unwrap();
    let task = {
        let mut engine = room.engine_mut();
        let id = engine
            .apply(orgbot_engine::Command::CreateTask {
                title: "long job".into(),
                brief: "work for a long time".into(),
                by: None,
                required_role: Some("writer".into()),
                priority: orgbot_proto::task::Priority::Normal,
                acceptance_criteria: vec![],
                depends_on: vec![],
                origin: TaskOrigin::plain(),
            })
            .unwrap()
            .iter()
            .find_map(|envelope| match &envelope.event {
                orgbot_proto::Event::TaskCreated { task } => Some(task.id),
                _ => None,
            })
            .unwrap();
        engine
            .apply(orgbot_engine::Command::OpenTask { task: id })
            .unwrap();
        engine
            .apply(orgbot_engine::Command::ClaimTask { task: id, by: slow })
            .unwrap();
        engine
            .apply(orgbot_engine::Command::StartTask { task: id, by: slow })
            .unwrap();
        id
    };
    let shared = room.shared_engine();
    owner_asks(&shared, Some(slow), "@slow keep going");
    owner_asks(&shared, Some(quick), "@quick say something");

    let sinks = |_| None;
    let run = room.step_rolling(2, &sinks);
    tokio::pin!(run);
    let scenario = async {
        // The slow turn outlasts its claim window...
        while !shared.with(|e| e.transcript().iter().any(|m| m.body == "done quickly")) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };
    // ...expire the claim while the slow turn is still running, before the
    // quick bot's finish triggers the tick.
    shared.with(|e| {
        e.task_mut(task).unwrap().lease_expires_at =
            Some(orgbot_proto::Timestamp::now().plus_millis(-1));
    });
    tokio::select! {
        _ = &mut run => panic!("the run ended while the slow turn was still going"),
        done = tokio::time::timeout(std::time::Duration::from_secs(5), scenario) => done.unwrap(),
    }
    // Let the poll and the tick after the quick turn run.
    let _ = tokio::time::timeout(std::time::Duration::from_millis(1500), &mut run).await;
    let state = shared.with(|e| e.task(task).unwrap().state);
    assert_eq!(state, TaskState::InProgress, "the working bot keeps its task");
    release.notify_one();
    run.await;
}

#[tokio::test]
async fn a_bot_waiting_for_a_slot_keeps_its_task() {
    // From the live room: Sol's task was unblocked while the two other bots
    // on its account held both of that account's slots. Sol waited for a turn
    // the whole claim window and lost the task for not reporting progress.
    let mut room = Orchestrator::new(RoomSpec {
        name: "queued".into(),
        goal: "wait in line".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits {
            max_parallel_turns: 2,
            max_parallel_per_backend: 1,
            ..RoomLimits::default()
        },
        access: Default::default(),
    });
    let same_account = |handle: &str| {
        AgentSpec::new(
            handle,
            handle,
            ProviderRef::Anthropic {
                model: "claude-opus-5".into(),
            },
        )
        .with_roles(vec!["writer"])
    };
    let release = Arc::new(tokio::sync::Notify::new());
    let slow = room
        .seat(same_account("slow"), Arc::new(SlowWorker(release.clone())))
        .unwrap();
    let queued = room
        .seat(same_account("queued"), reactive("queued", |_, _| say("my turn")))
        .unwrap();
    room.start().unwrap();
    let task = {
        let mut engine = room.engine_mut();
        let id = engine
            .apply(orgbot_engine::Command::CreateTask {
                title: "held work".into(),
                brief: "work".into(),
                by: None,
                required_role: Some("writer".into()),
                priority: orgbot_proto::task::Priority::Normal,
                acceptance_criteria: vec![],
                depends_on: vec![],
                origin: TaskOrigin::plain(),
            })
            .unwrap()
            .iter()
            .find_map(|envelope| match &envelope.event {
                orgbot_proto::Event::TaskCreated { task } => Some(task.id),
                _ => None,
            })
            .unwrap();
        for command in [
            orgbot_engine::Command::OpenTask { task: id },
            orgbot_engine::Command::ClaimTask { task: id, by: queued },
            orgbot_engine::Command::StartTask { task: id, by: queued },
        ] {
            engine.apply(command).unwrap();
        }
        id
    };
    let shared = room.shared_engine();
    owner_asks(&shared, Some(slow), "@slow start the long job");

    let sinks = |_| None;
    let run = room.step_rolling(2, &sinks);
    tokio::pin!(run);
    // The slow bot holds the account's only slot; the other waits with its
    // task, and the window runs out while it does.
    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), &mut run).await;
    shared.with(|e| {
        e.task_mut(task).unwrap().lease_expires_at =
            Some(orgbot_proto::Timestamp::now().plus_millis(-1));
    });
    let _ = tokio::time::timeout(std::time::Duration::from_millis(1500), &mut run).await;
    shared.with(|e| {
        let _ = e.apply(orgbot_engine::Command::Tick {
            now: orgbot_proto::Timestamp::now(),
        });
    });
    let (state, holder) = shared.with(|e| {
        let record = e.task(task).unwrap();
        (record.state, record.assignee)
    });
    assert_eq!(state, TaskState::InProgress, "waiting for a slot is not walking away");
    assert_eq!(holder, Some(queued));
    release.notify_one();
    run.await;
}

#[tokio::test]
async fn a_rolling_run_cut_short_loses_no_bot() {
    // A pause drops the run mid-turn. The bot that was on loan to its turn
    // must come back: it is saved with the room and can take the next turn.
    let mut room = Orchestrator::new(RoomSpec {
        name: "parked".into(),
        goal: "keep everyone".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
        access: Default::default(),
    });
    let release = Arc::new(tokio::sync::Notify::new());
    let slow = room
        .seat(
            spec("slow", &["writer"], Autonomy::default()),
            Arc::new(SlowWorker(release.clone())),
        )
        .unwrap();
    room.start().unwrap();
    let shared = room.shared_engine();
    owner_asks(&shared, Some(slow), "@slow begin");
    let sinks = |_| None;
    {
        let run = room.step_rolling(1, &sinks);
        // Cut the run off while the slow turn is in flight.
        let _ = tokio::time::timeout(std::time::Duration::from_millis(100), run).await;
    }
    let saved = room.checkpoint();
    assert!(
        saved["bots"].as_object().unwrap().contains_key(&slow.to_string()),
        "a bot parked by a dropped turn is still saved: {saved}"
    );
    room.finish_interrupted_turns().await;
    owner_asks(&shared, Some(slow), "@slow again");
    release.notify_one();
    let reports = room.step_rolling(1, &sinks).await;
    assert_eq!(reports.len(), 1, "the bot is back and takes its turn");
    assert_eq!(reports[0].agent, slow);
}

/// Streams its own handle, so a test can see whose turn a stream carries.
#[derive(Debug)]
struct NamedStreamer(&'static str);

#[async_trait]
impl Provider for NamedStreamer {
    fn name(&self) -> &str {
        self.0
    }

    async fn complete(
        &self,
        _request: CompletionRequest,
        sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> orgbot_providers::Result<CompletionResponse> {
        if let Some(sink) = sink {
            let _ = sink
                .send(StreamEvent::TextDelta {
                    text: self.0.to_string(),
                })
                .await;
        }
        Ok(say(&format!("hi from {}", self.0)))
    }
}

#[tokio::test]
async fn a_batch_streams_the_bot_its_stream_is_labelled_with() {
    // The server labels the live stream with the first bot `next_actors`
    // picks. Seats are kept in id order, which is unrelated, so the stream
    // must follow the pick and not the seat.
    let mut room = Orchestrator::new(RoomSpec {
        name: "stream".into(),
        goal: "answer".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
        access: Default::default(),
    });
    let mut seats = Vec::new();
    for handle in ["ada", "bo", "cy", "di", "ed", "fi"] {
        seats.push((
            room.seat(
                spec(handle, &["writer"], Autonomy::default()),
                Arc::new(NamedStreamer(handle)),
            )
            .unwrap(),
            handle,
        ));
    }
    room.start().unwrap();
    let room_id = room.engine().room_id();
    room.engine_mut()
        .apply(orgbot_engine::Command::PostMessage {
            message: Box::new(orgbot_proto::ChatMessage {
                id: orgbot_proto::MessageId::new(),
                room: room_id,
                author: orgbot_proto::Author::Human { name: "Aece".into() },
                audience: orgbot_proto::Audience::Everyone,
                intent: orgbot_proto::MessageIntent::Ask,
                body: "@all hi".into(),
                task: None,
                handoff: None,
                reply_to: None,
                attachments: vec![],
                files: vec![],
                sent_at: orgbot_proto::Timestamp::now(),
            }),
        })
        .unwrap();

    while room.next_actor().is_some() {
        let batch = room.batch_size().max(2);
        let lead = room.next_actors(batch)[0];
        let handle = seats.iter().find(|(id, _)| *id == lead).unwrap().1;
        let (tx, mut rx) = mpsc::channel(64);
        room.step_many(batch, Some(tx)).await;
        let mut streamed = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let StreamEvent::TextDelta { text } = event {
                streamed.push(text);
            }
        }
        assert_eq!(streamed, vec![handle.to_string()]);
    }
}

#[tokio::test]
async fn an_ordinary_broadcast_statement_does_not_wake_the_room() {
    let mut room = Orchestrator::new(RoomSpec {
        name: "chat".into(),
        goal: "stay quiet".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
        access: Default::default(),
    });
    room.seat(
        spec("helper", &["writer"], Autonomy::default()),
        reactive("helper", |_last, _all| say("unsolicited")),
    )
    .unwrap();
    room.start().unwrap();

    // A plain status broadcast (not a question) must not schedule anyone.
    let room_id = room.engine().room_id();
    let speaker = room.engine().agents().next().unwrap().id;
    room.engine_mut()
        .apply(orgbot_engine::Command::PostMessage {
            message: Box::new(orgbot_proto::ChatMessage::say(
                room_id,
                speaker,
                "just working away",
            )),
        })
        .unwrap();
    assert!(
        room.next_actor().is_none(),
        "ordinary chatter must not wake the room, or it never settles"
    );
}

// ------------------------------------------------------------ room MCP bridge

/// A provider that plays the agent CLI's side of the room-MCP bridge: it
/// connects to its own unix socket and claims the seeded task through the
/// board tools, mid-episode, exactly as an agent's tool loop would.
#[derive(Debug)]
struct McpClientProvider {
    task_id: Arc<std::sync::Mutex<Option<String>>>,
}

/// Ask the room's MCP bridge one question and read its answer.
async fn mcp_ask(
    wr: &mut tokio::net::unix::OwnedWriteHalf,
    reader: &mut tokio::io::Lines<tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>>,
    request: &str,
) -> orgbot_providers::Result<Value> {
    use tokio::io::AsyncWriteExt;
    wr.write_all(format!("{request}\n").as_bytes())
        .await
        .map_err(|error| {
            orgbot_providers::ProviderError::Unsupported(format!("mcp write: {error}"))
        })?;
    let reply = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut line = String::new();
        loop {
            line.clear();
            let read = reader
                .next_line()
                .await
                .map_err(|error| {
                    orgbot_providers::ProviderError::Unsupported(format!("mcp read: {error}"))
                })?
                .ok_or_else(|| {
                    orgbot_providers::ProviderError::Unsupported("mcp socket closed early".into())
                })?;
            if read.is_empty() {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<Value>(&read) {
                return Ok(value);
            }
        }
    })
    .await
    .map_err(|_| {
        orgbot_providers::ProviderError::Unsupported("mcp round trip timed out".into())
    })??;
    Ok(reply)
}

#[async_trait]
impl Provider for McpClientProvider {
    fn name(&self) -> &str {
        "mcp-client"
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        _sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> orgbot_providers::Result<CompletionResponse> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::UnixStream;

        let socket = request.mcp_socket.ok_or_else(|| {
            orgbot_providers::ProviderError::Unsupported("no mcp socket on request".into())
        })?;
        let stream = UnixStream::connect(&socket).await.map_err(|error| {
            orgbot_providers::ProviderError::Unsupported(format!(
                "connect room mcp {socket}: {error}"
            ))
        })?;
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half).lines();

        // Standard handshake.
        let _init = mcp_ask(
            &mut write_half,
            &mut reader,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"room-test","version":"1"}}}"#,
        )
        .await?;
        write_half
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
            .await
            .map_err(|error| {
                orgbot_providers::ProviderError::Unsupported(format!("mcp notify: {error}"))
            })?;

        // Claim the seeded task through the board tools.
        let task = self.task_id.lock().unwrap().clone().ok_or_else(|| {
            orgbot_providers::ProviderError::Unsupported("no task id supplied".into())
        })?;
        let call = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{{\"name\":\"claim_task\",\"arguments\":{{\"task\":\"{task}\"}}}}}}"
        );
        let reply = mcp_ask(&mut write_half, &mut reader, &call).await?;
        if reply
            .pointer("/result/isError")
            .is_some_and(|flag| flag.as_bool() == Some(true))
        {
            let text = reply
                .pointer("/result/content/0/text")
                .cloned()
                .unwrap_or_default();
            return Err(orgbot_providers::ProviderError::Unsupported(format!(
                "claim rejected by the room: {text}"
            )));
        }

        Ok(say("claimed the task through the room MCP bridge"))
    }
}

/// An agent CLI that says hello through the room tool, then ends its run with
/// its own summary of what it did, the way Claude Code and Codex do.
#[derive(Debug)]
struct McpPosterProvider;

#[async_trait]
impl Provider for McpPosterProvider {
    fn name(&self) -> &str {
        "mcp-poster"
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        _sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> orgbot_providers::Result<CompletionResponse> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::UnixStream;

        let unsupported = |detail: String| orgbot_providers::ProviderError::Unsupported(detail);
        let socket = request
            .mcp_socket
            .ok_or_else(|| unsupported("no mcp socket on request".into()))?;
        let stream = UnixStream::connect(&socket)
            .await
            .map_err(|error| unsupported(format!("connect: {error}")))?;
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half).lines();
        mcp_ask(
            &mut write_half,
            &mut reader,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"room-test","version":"1"}}}"#,
        )
        .await?;
        write_half
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
            .await
            .map_err(|error| unsupported(format!("notify: {error}")))?;
        let reply = mcp_ask(
            &mut write_half,
            &mut reader,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"post_message","arguments":{"body":"Hi Aece, Ellis here."}}}"#,
        )
        .await?;
        assert_ne!(
            reply.pointer("/result/isError").and_then(Value::as_bool),
            Some(true),
            "post_message should be accepted: {reply}"
        );
        Ok(say("I posted a reply in the room."))
    }
}

#[tokio::test]
async fn a_cli_bot_that_posted_through_the_room_tool_is_not_echoed() {
    let bot_spec = AgentSpec::new(
        "ellis",
        "Ellis",
        ProviderRef::CliAgent {
            cli: orgbot_proto::agent::CliKind::Claude,
            model: String::new(),
            workdir: String::new(),
            policy: orgbot_proto::agent::AgentPolicy::ReadOnly,
            mcp: true,
            effort: String::new(),
        },
    )
    .with_roles(["writer"]);
    let mut room = Orchestrator::new(RoomSpec {
        name: "echo-room".into(),
        goal: "say hi once".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
        access: Default::default(),
    });
    room.seat(bot_spec, Arc::new(McpPosterProvider)).unwrap();
    room.start().unwrap();
    let room_id = room.engine().room_id();
    room.engine_mut()
        .apply(orgbot_engine::Command::PostMessage {
            message: Box::new(orgbot_proto::ChatMessage {
                id: orgbot_proto::MessageId::new(),
                room: room_id,
                author: orgbot_proto::Author::Human { name: "Aece".into() },
                audience: orgbot_proto::Audience::Everyone,
                intent: orgbot_proto::MessageIntent::Ask,
                body: "@all reply hi".into(),
                task: None,
                handoff: None,
                reply_to: None,
                attachments: vec![],
                files: vec![],
                sent_at: orgbot_proto::Timestamp::now(),
            }),
        })
        .unwrap();

    room.step(None).await.expect("the bot answers");
    let bodies: Vec<String> = room
        .engine()
        .transcript()
        .iter()
        .filter(|m| m.author.agent().is_some())
        .map(|m| m.body.clone())
        .collect();
    assert_eq!(bodies, vec!["Hi Aece, Ellis here.".to_string()]);
}

#[tokio::test]
async fn an_mcp_agent_bot_claims_a_seeded_task_in_band() {
    use std::sync::Mutex;

    let task_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let provider = Arc::new(McpClientProvider {
        task_id: task_id.clone(),
    });
    let bot_spec = AgentSpec::new(
        "worker-a",
        "Worker",
        ProviderRef::CliAgent {
            cli: orgbot_proto::agent::CliKind::Claude,
            model: String::new(),
            workdir: String::new(),
            policy: orgbot_proto::agent::AgentPolicy::ReadOnly,
            mcp: true,
            effort: String::new(),
        },
    )
    .with_roles(["writer"]);

    let mut room = Orchestrator::new(RoomSpec {
        name: "bridge-room".into(),
        goal: "prove in-band claiming".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
        access: Default::default(),
    });
    room.seat(bot_spec.clone(), provider).unwrap();
    room.start().unwrap();

    let events = room
        .engine_mut()
        .apply(orgbot_engine::Command::CreateTask {
            title: "mcp-claimed task".into(),
            brief: "claim me through the bridge".into(),
            by: None,
            required_role: Some("writer".into()),
            priority: orgbot_proto::task::Priority::Normal,
            acceptance_criteria: vec![],
            depends_on: vec![],
            origin: TaskOrigin::plain(),
        })
        .unwrap();
    let task_id_value = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            orgbot_proto::Event::TaskCreated { task } => Some(task.id.to_string()),
            _ => None,
        })
        .expect("a task was created");
    room.engine_mut()
        .apply(orgbot_engine::Command::OpenTask {
            task: task_id_value.parse().unwrap(),
        })
        .unwrap();
    *task_id.lock().unwrap() = Some(task_id_value.clone());

    // One working episode: the bot claims its task through the bridge and
    // reports back. (A real agent would then start the task and do the work;
    // the scheduler wakes a holder of live work until it does.)
    let report = room.step(None).await.expect("the bot is actionable");
    assert!(
        report.tool_calls >= 1,
        "the in-band MCP claim must count as work"
    );

    let engine = room.engine();
    let task = engine
        .tasks()
        .find(|task| task.title == "mcp-claimed task")
        .expect("the task still exists");
    assert_eq!(
        task.state,
        TaskState::Claimed,
        "the bot claimed the task in-band through the room MCP bridge"
    );
    assert_eq!(task.assignee, Some(bot_spec.id));
}

#[tokio::test]
async fn a_real_codex_bot_claims_a_task_through_the_room_bridge() {
    // Live-gated: needs ORGBOT_TEST_LIVE_CLIS=1, codex installed and signed
    // in, and ORGBOT_MCP_RELAY_BIN pointing at a built `orgbot` binary
    // (which carries the room-mcp subcommand the agent CLI spawns).
    use orgbot_providers::agent_cli::AgentCliProvider;

    if std::env::var_os("ORGBOT_TEST_LIVE_CLIS").is_none()
        || !AgentCliProvider::available(orgbot_proto::agent::CliKind::Codex)
        || std::env::var_os("ORGBOT_MCP_RELAY_BIN").is_none()
    {
        eprintln!("skipped (live codex bridge test needs ORGBOT_TEST_LIVE_CLIS=1, codex, ORGBOT_MCP_RELAY_BIN)");
        return;
    }

    let provider = Arc::new(
        AgentCliProvider::new(
            orgbot_proto::agent::CliKind::Codex,
            "",
            "/tmp/orgbot-e2e-ws",
            orgbot_proto::agent::AgentPolicy::ReadOnly,
            true,
        )
        .unwrap(),
    );
    let bot_spec = AgentSpec::new(
        "coder",
        "Coder",
        ProviderRef::CliAgent {
            cli: orgbot_proto::agent::CliKind::Codex,
            model: String::new(),
            workdir: "/tmp/orgbot-e2e-ws".into(),
            policy: orgbot_proto::agent::AgentPolicy::ReadOnly,
            mcp: true,
            effort: String::new(),
        },
    )
    .with_roles(["writer"])
    .with_persona(
        "You are verifying a bridge. Claim the open task with claim_task, then post_message exactly: claimed-ok",
    );

    let mut room = Orchestrator::new(RoomSpec {
        name: "codex-bridge-live".into(),
        goal: "Claim the seeded task through the room MCP bridge and report it.".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
        access: Default::default(),
    });
    room.seat(bot_spec.clone(), provider).unwrap();
    room.start().unwrap();

    let events = room
        .engine_mut()
        .apply(orgbot_engine::Command::CreateTask {
            title: "bridge target".into(),
            brief: "Claim this task to prove the bridge.".into(),
            by: None,
            required_role: Some("writer".into()),
            priority: orgbot_proto::task::Priority::High,
            acceptance_criteria: vec![],
            depends_on: vec![],
            origin: TaskOrigin::plain(),
        })
        .unwrap();
    let task_id = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            orgbot_proto::Event::TaskCreated { task } => Some(task.id),
            _ => None,
        })
        .expect("a task was created");
    room.engine_mut()
        .apply(orgbot_engine::Command::OpenTask { task: task_id })
        .unwrap();

    // One working episode; the real codex agent claims through the bridge.
    let report = tokio::time::timeout(std::time::Duration::from_secs(240), room.step(None))
        .await
        .expect("the codex episode finished in time")
        .expect("the bot was actionable");

    let engine = room.engine();
    let task = engine
        .tasks()
        .find(|task| task.id == task_id)
        .expect("the task still exists");
    assert_eq!(
        task.state,
        TaskState::Claimed,
        "codex claimed the task in-band through the MCP bridge; report={report:?}"
    );
}

// ---------------------------------------------------- full lifecycle via bridge

/// A bridge client that replays a planned sequence of board actions, one
/// episode per step — the deterministic stand-in for a real agent episode.
#[derive(Debug)]
struct ScriptedMcpClient {
    task_id: Arc<std::sync::Mutex<Option<String>>>,
    steps: Vec<Vec<(&'static str, Value)>>,
    cursor: std::sync::atomic::AtomicUsize,
}

impl ScriptedMcpClient {
    async fn run_step(
        &self,
        request: &CompletionRequest,
    ) -> orgbot_providers::Result<CompletionResponse> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::UnixStream;

        let step = self
            .cursor
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let Some(actions) = self.steps.get(step) else {
            return Ok(nothing()); // plan exhausted; say nothing further
        };
        let socket = request.mcp_socket.as_deref().ok_or_else(|| {
            orgbot_providers::ProviderError::Unsupported("no mcp socket on request".into())
        })?;
        let stream = UnixStream::connect(socket).await.map_err(|error| {
            orgbot_providers::ProviderError::Unsupported(format!(
                "connect room mcp {socket}: {error}"
            ))
        })?;
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half).lines();

        let _init = mcp_ask(
            &mut write_half,
            &mut reader,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"scripted-bridge","version":"1"}}}"#,
        )
        .await?;
        write_half
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
            .await
            .map_err(|error| {
                orgbot_providers::ProviderError::Unsupported(format!("mcp notify: {error}"))
            })?;

        let task = self.task_id.lock().unwrap().clone().unwrap_or_default();
        for (idx, (name, args)) in actions.iter().enumerate() {
            let mut args = args.clone();
            args["task"] = json!(task);
            let call = json!({
                "jsonrpc": "2.0",
                "id": 10 + idx,
                "method": "tools/call",
                "params": { "name": name, "arguments": args },
            });
            let reply = mcp_ask(&mut write_half, &mut reader, &call.to_string()).await?;
            if reply
                .pointer("/result/isError")
                .is_some_and(|flag| flag.as_bool() == Some(true))
            {
                let text = reply
                    .pointer("/result/content/0/text")
                    .cloned()
                    .unwrap_or_default();
                return Err(orgbot_providers::ProviderError::Unsupported(format!(
                    "{name} rejected by the room: {text}"
                )));
            }
        }
        Ok(say(&format!(
            "acted: {}",
            actions
                .iter()
                .map(|(n, _)| *n)
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }
}

#[async_trait]
impl Provider for ScriptedMcpClient {
    fn name(&self) -> &str {
        "scripted-mcp"
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        _sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> orgbot_providers::Result<CompletionResponse> {
        self.run_step(&request).await
    }
}

#[tokio::test]
async fn a_room_cycle_claim_work_review_submit_runs_through_the_bridge() {
    use std::sync::Mutex;

    let task_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let cli_ref = |handle: &str, roles: &[&str], autonomy: Autonomy| {
        AgentSpec::new(
            handle,
            handle,
            ProviderRef::CliAgent {
                cli: orgbot_proto::agent::CliKind::Claude,
                model: String::new(),
                workdir: String::new(),
                policy: orgbot_proto::agent::AgentPolicy::ReadOnly,
                mcp: true,
                effort: String::new(),
            },
        )
        .with_roles(roles.iter().copied())
        .with_autonomy(autonomy)
    };

    // Worker: claim, do the work, ask for review; then submit once approved.
    let worker_spec = cli_ref(
        "worker",
        &["writer"],
        Autonomy {
            may_submit: true,
            ..Default::default()
        },
    );
    let reviewer_spec = cli_ref("reviewer", &["reviewer"], Autonomy::inspector());

    let worker = Arc::new(ScriptedMcpClient {
        task_id: task_id.clone(),
        steps: vec![
            vec![
                ("claim_task", json!({})),
                ("start_task", json!({})),
                ("submit_for_review", json!({ "artifacts": [] })),
            ],
            vec![("submit_task", json!({}))],
        ],
        cursor: std::sync::atomic::AtomicUsize::new(0),
    });
    let reviewer = Arc::new(ScriptedMcpClient {
        task_id: task_id.clone(),
        steps: vec![vec![(
            "record_review",
            json!({ "summary": "looks good", "findings": [] }),
        )]],
        cursor: std::sync::atomic::AtomicUsize::new(0),
    });

    let mut room = Orchestrator::new(RoomSpec {
        name: "cycle-room".into(),
        goal: "run a full task cycle through the room bridge".into(),
        roster: vec![],
        completion: CompletionRule::AllTasksTerminal,
        limits: RoomLimits::default(),
        access: Default::default(),
    });
    room.seat(worker_spec.clone(), worker).unwrap();
    room.seat(reviewer_spec.clone(), reviewer).unwrap();
    room.start().unwrap();

    let events = room
        .engine_mut()
        .apply(orgbot_engine::Command::CreateTask {
            title: "cycle target".into(),
            brief: "claim, work, review, submit".into(),
            by: None,
            required_role: Some("writer".into()),
            priority: orgbot_proto::task::Priority::High,
            acceptance_criteria: vec![],
            depends_on: vec![],
            origin: TaskOrigin::plain(),
        })
        .unwrap();
    let task = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            orgbot_proto::Event::TaskCreated { task } => Some(*task.clone()),
            _ => None,
        })
        .expect("a task was created");
    room.engine_mut()
        .apply(orgbot_engine::Command::OpenTask { task: task.id })
        .unwrap();
    *task_id.lock().unwrap() = Some(task.id.to_string());

    let outcome = room.run(None).await;
    let engine = room.engine();
    let final_task = engine.tasks().find(|t| t.id == task.id).unwrap();
    assert_eq!(
        outcome,
        RunOutcome::Completed,
        "final state: {final_task:?}"
    );
    assert!(
        final_task.state.is_terminal(),
        "the task reached a terminal state through the bridge: {:?}",
        final_task.state
    );
}

/// Records whether each turn it is asked to take carries the server key.
#[derive(Debug)]
struct RecordsServerAccess(Arc<std::sync::Mutex<Vec<bool>>>);

#[async_trait]
impl Provider for RecordsServerAccess {
    fn name(&self) -> &str {
        "records-server-access"
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        _sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> orgbot_providers::Result<CompletionResponse> {
        self.0.lock().unwrap().push(request.server_access);
        Ok(say("noted"))
    }
}

#[tokio::test]
async fn a_bot_turn_carries_the_server_key_only_once_the_owner_allows_it() {
    // From the live room: a bot installed a server release over SSH while the
    // room had it on hold, because every sandbox carried the bot server key.
    use orgbot_engine::Command;
    use orgbot_proto::decision::{Answer, SERVER_ACCESS_ALLOW};
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut room = Orchestrator::new(RoomSpec {
        name: "release".into(),
        goal: "deploy".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
        access: Default::default(),
    });
    let ops = room
        .seat(
            spec("ops", &["engineer"], Autonomy::default()),
            Arc::new(RecordsServerAccess(seen.clone())),
        )
        .unwrap();
    room.start().unwrap();
    let created = room
        .engine_mut()
        .apply(Command::CreateTask {
            title: "Deploy the reviewed build".into(),
            brief: String::new(),
            by: None,
            required_role: None,
            priority: orgbot_proto::task::Priority::Normal,
            acceptance_criteria: vec![],
            depends_on: vec![],
            origin: TaskOrigin::plain(),
        })
        .unwrap();
    let task = created
        .iter()
        .find_map(|event| match &event.event {
            orgbot_proto::Event::TaskCreated { task } => Some(task.id),
            _ => None,
        })
        .unwrap();
    for command in [
        Command::OpenTask { task },
        Command::ClaimTask { task, by: ops },
        Command::StartTask { task, by: ops },
    ] {
        room.engine_mut().apply(command).unwrap();
    }

    owner_asks(&room.shared_engine(), Some(ops), "@ops how is the deploy going?");
    room.step(None).await;
    assert_eq!(*seen.lock().unwrap(), vec![false], "no key until the owner allows it");

    let asked = room
        .engine_mut()
        .apply(Command::RequestServerAccess {
            by: ops,
            task,
            reason: "deploy the reviewed build".into(),
        })
        .unwrap();
    let decision = asked
        .iter()
        .find_map(|event| match &event.event {
            orgbot_proto::Event::DecisionAsked { decision } => Some(decision.id),
            _ => None,
        })
        .unwrap();
    room.engine_mut()
        .apply(Command::AnswerDecision {
            decision,
            answer: Answer {
                chosen: SERVER_ACCESS_ALLOW.into(),
                note: String::new(),
                at: orgbot_proto::Timestamp::now(),
            },
        })
        .unwrap();
    owner_asks(&room.shared_engine(), Some(ops), "@ops go ahead");
    room.step(None).await;
    assert_eq!(seen.lock().unwrap().last(), Some(&true), "allowed, the next turn has the key");
}

#[tokio::test]
async fn a_bot_is_woken_for_an_assist_it_accepted_and_has_not_delivered() {
    // From the live room: a bot accepted an assist and ended its turn without
    // delivering. An assist never makes its recipient the assignee, so it was
    // never shown the handoff or woken for it again, and the sender waited for
    // it to expire.
    use orgbot_engine::Command;
    use orgbot_proto::handoff::{HandoffKind, Recipient};
    let mut room = Orchestrator::new(RoomSpec {
        name: "release".into(),
        goal: "ship".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
        access: Default::default(),
    });
    let lead = room
        .seat(spec("lead", &["engineer"], Autonomy::default()), Arc::new(ScriptedProvider::new(vec![])))
        .unwrap();
    let helper = room
        .seat(spec("helper", &["engineer"], Autonomy::default()), Arc::new(ScriptedProvider::new(vec![])))
        .unwrap();
    room.start().unwrap();
    let created = room
        .engine_mut()
        .apply(Command::CreateTask {
            title: "Fix the lint failure".into(),
            brief: String::new(),
            by: None,
            required_role: None,
            priority: orgbot_proto::task::Priority::Normal,
            acceptance_criteria: vec![],
            depends_on: vec![],
            origin: TaskOrigin::plain(),
        })
        .unwrap();
    let task = created
        .iter()
        .find_map(|event| match &event.event {
            orgbot_proto::Event::TaskCreated { task } => Some(task.id),
            _ => None,
        })
        .unwrap();
    for command in [
        Command::OpenTask { task },
        Command::ClaimTask { task, by: lead },
        Command::StartTask { task, by: lead },
    ] {
        room.engine_mut().apply(command).unwrap();
    }
    let offered = room
        .engine_mut()
        .apply(Command::OfferHandoff {
            task,
            from: lead,
            to: Recipient::Agent { agent: helper },
            kind: HandoffKind::Assist,
            brief: "fix the empty catch in the QA script".into(),
            expects: vec![],
            context: vec![],
            blocks_sender: None,
            timeout_ms: None,
        })
        .unwrap();
    let handoff = offered
        .iter()
        .find_map(|event| match &event.event {
            orgbot_proto::Event::HandoffOffered { handoff } => Some(handoff.id),
            _ => None,
        })
        .unwrap();
    room.engine_mut()
        .apply(Command::AcceptHandoff { handoff, by: helper })
        .unwrap();

    assert!(room.is_actionable(helper), "the helper is woken for the work it took on");
    let situation = orgbot_runtime::prompt::situation(&room.engine(), helper, orgbot_proto::event::Seq::ZERO);
    assert!(situation.contains(&handoff.to_string()), "and shown it: {situation}");
    assert!(situation.contains("fail_handoff"));
}

#[tokio::test]
async fn an_accepted_assist_waits_without_rerunning_while_its_bot_waits_on_the_owner() {
    // From the live room: @opus accepted an assist, asked the owner what it
    // needed to finish it, and was rerun every few minutes for the accepted
    // assist while the owner had the question. The assist then ran out and
    // its sender offered it again.
    use orgbot_engine::Command;
    use orgbot_proto::handoff::{HandoffKind, HandoffState, Recipient};
    let mut room = Orchestrator::new(RoomSpec {
        name: "release".into(),
        goal: "ship".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
        access: Default::default(),
    });
    let lead = room
        .seat(spec("lead", &["engineer"], Autonomy::default()), Arc::new(ScriptedProvider::new(vec![])))
        .unwrap();
    let helper = room
        .seat(spec("helper", &["engineer"], Autonomy::default()), Arc::new(ScriptedProvider::new(vec![])))
        .unwrap();
    room.start().unwrap();
    let created = room
        .engine_mut()
        .apply(Command::CreateTask {
            title: "Release the party build".into(),
            brief: String::new(),
            by: None,
            required_role: None,
            priority: orgbot_proto::task::Priority::Normal,
            acceptance_criteria: vec![],
            depends_on: vec![],
            origin: TaskOrigin::plain(),
        })
        .unwrap();
    let task = created
        .iter()
        .find_map(|event| match &event.event {
            orgbot_proto::Event::TaskCreated { task } => Some(task.id),
            _ => None,
        })
        .unwrap();
    for command in [
        Command::OpenTask { task },
        Command::ClaimTask { task, by: lead },
        Command::StartTask { task, by: lead },
    ] {
        room.engine_mut().apply(command).unwrap();
    }
    let offered = room
        .engine_mut()
        .apply(Command::OfferHandoff {
            task,
            from: lead,
            to: Recipient::Agent { agent: helper },
            kind: HandoffKind::Assist,
            brief: "recheck the hosted version".into(),
            expects: vec![],
            context: vec![],
            blocks_sender: None,
            timeout_ms: Some(60_000),
        })
        .unwrap();
    let handoff = offered
        .iter()
        .find_map(|event| match &event.event {
            orgbot_proto::Event::HandoffOffered { handoff } => Some(handoff.id),
            _ => None,
        })
        .unwrap();
    room.engine_mut()
        .apply(Command::AcceptHandoff { handoff, by: helper })
        .unwrap();
    let asked = room
        .engine_mut()
        .apply(Command::AskOwner {
            by: helper,
            task: None,
            question: "What does the version page show after a reload?".into(),
            options: vec![],
        })
        .unwrap();
    let decision = asked
        .iter()
        .find_map(|event| match &event.event {
            orgbot_proto::Event::DecisionAsked { decision } => Some(decision.id),
            _ => None,
        })
        .unwrap();

    assert!(!room.is_actionable(helper), "not rerun while the owner has its question");
    room.engine_mut()
        .apply(Command::Tick {
            now: orgbot_proto::Timestamp::now().plus_millis(10 * 60_000),
        })
        .unwrap();
    assert_eq!(
        room.engine().handoff(handoff).unwrap().state,
        HandoffState::Accepted,
        "and what it took on does not run out meanwhile"
    );

    room.engine_mut()
        .apply(Command::AnswerDecision {
            decision,
            answer: orgbot_proto::decision::Answer {
                chosen: "Custom answer".into(),
                note: "It shows the new tag".into(),
                at: orgbot_proto::Timestamp::now(),
            },
        })
        .unwrap();
    assert!(room.is_actionable(helper), "the answer wakes it to finish the assist");
}

#[tokio::test]
async fn a_bot_back_after_a_long_absence_gets_a_bounded_catch_up() {
    // From the live room: @grok sat out eight hours while the room posted 258
    // messages. Its next prompt (133 KB) was longer than Linux allows one
    // command-line argument to be, so the CLI never started and the bot
    // faulted.
    let mut room = Orchestrator::new(RoomSpec {
        name: "release".into(),
        goal: "ship".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
        access: Default::default(),
    });
    let lead = room
        .seat(spec("lead", &["engineer"], Autonomy::default()), Arc::new(ScriptedProvider::new(vec![])))
        .unwrap();
    let away = room
        .seat(spec("away", &["engineer"], Autonomy::default()), Arc::new(ScriptedProvider::new(vec![])))
        .unwrap();
    room.start().unwrap();
    let ask = format!(
        "@away please take the lint failure when you are back. {}",
        "The failing rule is the empty catch in the QA script. ".repeat(30)
    );
    owner_asks(&room.shared_engine(), Some(away), &ask);
    for n in 0..258 {
        let room_id = room.engine().room_id();
        room.engine_mut()
            .apply(orgbot_engine::Command::PostMessage {
                message: Box::new(orgbot_proto::ChatMessage {
                    id: orgbot_proto::MessageId::new(),
                    room: room_id,
                    author: orgbot_proto::Author::Agent { agent: lead },
                    audience: orgbot_proto::Audience::Everyone,
                    intent: orgbot_proto::MessageIntent::Status,
                    body: format!("status {n}: {}", "progress on the freeze capture ".repeat(17)),
                    task: None,
                    handoff: None,
                    reply_to: None,
                    attachments: vec![],
                    files: vec![],
                    sent_at: orgbot_proto::Timestamp::now(),
                }),
            })
            .unwrap();
    }

    let situation = orgbot_runtime::prompt::situation(&room.engine(), away, orgbot_proto::event::Seq::ZERO);
    assert!(situation.len() < 64 * 1024, "bounded: {} bytes", situation.len());
    assert!(situation.contains("please take the lint failure"), "what was asked of it is kept");
    assert!(situation.contains("status 257:"), "the latest is kept");
    assert!(!situation.contains("status 0:"), "the oldest chatter is left out");
    assert!(situation.contains("earlier updates are left out"), "and says so");
}

/// A room in `mode` lending Wrangler (risky) and a read-only docs sign-in
/// (regular), with `lead` holding a task under way.
fn access_room(
    mode: orgbot_proto::room::AccessMode,
) -> (Orchestrator, orgbot_proto::AgentId, orgbot_proto::TaskId) {
    use orgbot_engine::Command;
    use orgbot_proto::room::{LendItem, Risk, RoomAccess};
    let mut room = Orchestrator::new(RoomSpec {
        name: "release".into(),
        goal: "ship".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
        access: RoomAccess {
            mode,
            lend: vec![
                LendItem {
                    name: "wrangler".into(),
                    label: "Cloudflare (Wrangler)".into(),
                    paths: vec![".config/.wrangler".into()],
                    risk: Risk::Risky,
                },
                LendItem {
                    name: "docs".into(),
                    label: "Docs reader".into(),
                    paths: vec![".config/docs-reader".into()],
                    risk: Risk::Regular,
                },
            ],
        },
    });
    let lead = room
        .seat(spec("lead", &["engineer"], Autonomy::default()), Arc::new(ScriptedProvider::new(vec![])))
        .unwrap();
    room.start().unwrap();
    let created = room
        .engine_mut()
        .apply(Command::CreateTask {
            title: "Publish the website".into(),
            brief: String::new(),
            by: None,
            required_role: None,
            priority: orgbot_proto::task::Priority::Normal,
            acceptance_criteria: vec![],
            depends_on: vec![],
            origin: TaskOrigin::plain(),
        })
        .unwrap();
    let task = created
        .iter()
        .find_map(|event| match &event.event {
            orgbot_proto::Event::TaskCreated { task } => Some(task.id),
            _ => None,
        })
        .unwrap();
    for command in [
        Command::OpenTask { task },
        Command::ClaimTask { task, by: lead },
        Command::StartTask { task, by: lead },
    ] {
        room.engine_mut().apply(command).unwrap();
    }
    (room, lead, task)
}

fn ask_access(
    room: &mut Orchestrator,
    by: orgbot_proto::AgentId,
    task: orgbot_proto::TaskId,
    items: &[&str],
    action: orgbot_proto::decision::AccessAction,
) -> Result<Vec<orgbot_proto::EventEnvelope>, orgbot_engine::EngineError> {
    room.engine_mut().apply(orgbot_engine::Command::RequestAccess {
        by,
        task,
        items: items.iter().map(|item| item.to_string()).collect(),
        action,
        reason: "publish the verified build".into(),
    })
}

fn pending_count(room: &Orchestrator) -> usize {
    room.engine().decisions().pending().count()
}

#[tokio::test]
async fn each_access_mode_approves_what_it_should_and_asks_the_rest() {
    use orgbot_proto::decision::AccessAction::*;
    use orgbot_proto::room::AccessMode;

    // Approval: everything is asked, and the task waits on it.
    let (mut room, lead, task) = access_room(AccessMode::Approval);
    ask_access(&mut room, lead, task, &["docs"], Inspect).unwrap();
    assert_eq!(pending_count(&room), 1, "approval mode asks even for regular requests");
    assert!(room.engine().lent_paths_for(lead).is_empty());

    // Half approval: regular at once, risky asked.
    let (mut room, lead, task) = access_room(AccessMode::HalfApproval);
    ask_access(&mut room, lead, task, &["docs"], Inspect).unwrap();
    assert_eq!(pending_count(&room), 0, "a regular request is approved at once");
    assert_eq!(room.engine().lent_paths_for(lead), vec![".config/docs-reader".to_string()]);
    assert!(room.engine().decisions().decisions.iter().any(|d| d
        .answer
        .as_ref()
        .is_some_and(|a| a.note.starts_with("Approved automatically"))), "and recorded as an answered decision");
    ask_access(&mut room, lead, task, &["wrangler"], Change).unwrap();
    assert_eq!(pending_count(&room), 1, "a risky sign-in is asked");

    // Auto: risky at once, critical asked, whatever the sign-in's own rating.
    let (mut room, lead, task) = access_room(AccessMode::Auto);
    ask_access(&mut room, lead, task, &["wrangler"], Release).unwrap();
    assert_eq!(pending_count(&room), 0, "auto approves a risky release");
    assert!(room.engine().lent_paths_for(lead).contains(&".config/.wrangler".to_string()));
    ask_access(&mut room, lead, task, &["docs"], Destroy).unwrap();
    assert_eq!(pending_count(&room), 1, "a critical action is asked even with a regular sign-in");
    let (mut room, lead, task) = access_room(AccessMode::Auto);
    ask_access(&mut room, lead, task, &["server"], Change).unwrap();
    assert!(room.engine().server_access_for(lead), "the server key is lent like a sign-in");

    // Full write: even critical requests are approved.
    let (mut room, lead, task) = access_room(AccessMode::FullWrite);
    ask_access(&mut room, lead, task, &["wrangler"], Destroy).unwrap();
    assert_eq!(pending_count(&room), 0);
    assert!(!room.engine().lent_paths_for(lead).is_empty());

    // Lent only while the task is under way.
    room.engine_mut()
        .apply(orgbot_engine::Command::ReleaseTask { task, by: lead, reason: "done for now".into() })
        .unwrap();
    assert!(room.engine().lent_paths_for(lead).is_empty(), "a released task takes the loan away");

    // Asking for something the owner does not lend says what is lent.
    let (mut room, lead, task) = access_room(AccessMode::FullWrite);
    let refused = ask_access(&mut room, lead, task, &["aws"], Inspect).unwrap_err();
    assert!(refused.to_string().contains("wrangler"), "{refused}");
}

#[tokio::test]
async fn an_owner_answer_lends_a_sign_in_for_the_task() {
    use orgbot_proto::decision::{AccessAction, ACCESS_ALLOW};
    let (mut room, lead, task) = access_room(orgbot_proto::room::AccessMode::Approval);
    let asked = ask_access(&mut room, lead, task, &["wrangler"], AccessAction::Release).unwrap();
    let decision = asked
        .iter()
        .find_map(|event| match &event.event {
            orgbot_proto::Event::DecisionAsked { decision } => Some(decision.clone()),
            _ => None,
        })
        .unwrap();
    assert!(decision.options.contains(&ACCESS_ALLOW.to_string()));
    assert!(decision.because.contains("rated risky"), "{}", decision.because);
    room.engine_mut()
        .apply(orgbot_engine::Command::AnswerDecision {
            decision: decision.id,
            answer: orgbot_proto::decision::Answer {
                chosen: ACCESS_ALLOW.into(),
                note: String::new(),
                at: orgbot_proto::Timestamp::now(),
            },
        })
        .unwrap();
    assert_eq!(room.engine().lent_paths_for(lead), vec![".config/.wrangler".to_string()]);
    assert_eq!(room.engine().lendable_paths().len(), 2, "every lent path stays hidden from other turns");
    let situation = orgbot_runtime::prompt::situation(&room.engine(), lead, orgbot_proto::event::Seq::ZERO);
    assert!(situation.contains("Cloudflare (Wrangler) (in ~/.config/.wrangler)"), "{situation}");
}

/// Says it is about to change the code, every turn, and changes nothing: the
/// live room's looping bot. Counts the fresh sessions it is asked to start.
#[derive(Debug)]
struct TalksInsteadOfActing {
    fresh_sessions: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl Provider for TalksInsteadOfActing {
    fn name(&self) -> &str {
        "talks"
    }

    async fn complete(
        &self,
        _request: orgbot_providers::CompletionRequest,
        _sink: Option<mpsc::Sender<orgbot_providers::StreamEvent>>,
    ) -> orgbot_providers::Result<orgbot_providers::CompletionResponse> {
        Ok(orgbot_providers::scripted::says(
            "The late-party path still floors the roster. I'm changing that count now.",
        ))
    }

    fn last_turn_activity(&self) -> Option<usize> {
        Some(0)
    }

    fn start_fresh_session(&self) {
        self.fresh_sessions.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[tokio::test]
async fn a_bot_that_keeps_talking_instead_of_acting_is_rested_and_the_coordinators_told() {
    // From the live room: a bot holding an urgent fix took 26 turns in an
    // hour saying it was about to change the code, and changed nothing.
    use orgbot_engine::Command;
    let mut room = Orchestrator::new(RoomSpec {
        name: "release".into(),
        goal: "ship".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
        access: Default::default(),
    });
    room.seat(spec("lead", &["coordinator"], Autonomy::default()), Arc::new(ScriptedProvider::new(vec![])))
        .unwrap();
    let fresh_sessions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let worker = room
        .seat(
            spec("worker", &["engineer"], Autonomy::default()),
            Arc::new(TalksInsteadOfActing { fresh_sessions: fresh_sessions.clone() }),
        )
        .unwrap();
    room.start().unwrap();
    let created = room
        .engine_mut()
        .apply(Command::CreateTask {
            title: "Fix late party squad admission".into(),
            brief: String::new(),
            by: None,
            required_role: None,
            priority: orgbot_proto::task::Priority::Urgent,
            acceptance_criteria: vec![],
            depends_on: vec![],
            origin: TaskOrigin::plain(),
        })
        .unwrap();
    let task = created
        .iter()
        .find_map(|event| match &event.event {
            orgbot_proto::Event::TaskCreated { task } => Some(task.id),
            _ => None,
        })
        .unwrap();
    for command in [
        Command::OpenTask { task },
        Command::ClaimTask { task, by: worker },
        Command::StartTask { task, by: worker },
    ] {
        room.engine_mut().apply(command).unwrap();
    }

    for turn in 1..=3 {
        assert!(room.is_actionable(worker), "woken for its work on turn {turn}");
        let report = room.step(None).await.expect("the worker takes a turn");
        assert_eq!(report.agent, worker);
    }
    assert!(!room.is_actionable(worker), "after three turns of talk it is not woken for that work again");
    assert_eq!(fresh_sessions.load(std::sync::atomic::Ordering::SeqCst), 1, "its next turn starts fresh");
    let told = room.engine().transcript().iter().any(|message| {
        message.body.contains("@worker has ended 3 turns in a row")
            && matches!(&message.audience, orgbot_proto::Audience::Roles { roles } if roles.iter().any(|role| role == "coordinator"))
    });
    assert!(told, "the coordinators are told");

    // Talking to it wakes it again.
    owner_asks(&room.shared_engine(), Some(worker), "@worker are you stuck on the squad count?");
    assert!(room.is_actionable(worker));
}
