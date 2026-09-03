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
use gptbots_proto::agent::{Autonomy, ProviderRef};
use gptbots_proto::room::{CompletionRule, RoomLimits, RoomSpec};
use gptbots_proto::{AgentSpec, TaskState};
use gptbots_providers::types::{
    CompletionRequest, CompletionResponse, ContentBlock, Role, StopReason, StreamEvent, Usage,
};
use gptbots_providers::{Provider, ScriptedProvider};
use gptbots_runtime::{Orchestrator, RunOutcome};
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
        f.debug_struct("Reactive").field("name", &self.name).finish()
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
    ) -> gptbots_providers::Result<CompletionResponse> {
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
fn text_of(turn: &gptbots_providers::types::Turn) -> String {
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
    })
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
    assert_eq!(outcome, RunOutcome::Completed, "the room should finish its goal");

    // Exactly one task, delivered.
    let tasks: Vec<_> = room.engine().tasks().collect();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].state, TaskState::Submitted);

    // The work itself exists, and was inspected by someone other than its author.
    assert_eq!(tasks[0].artifacts.len(), 1);
    let reviews = room.engine().reviews();
    assert_eq!(reviews.len(), 1);
    assert_ne!(
        Some(reviews[0].inspector),
        tasks[0].assignee,
        "the inspector must not be the author"
    );

    // And the run is reconstructible from its log.
    let log = room.engine().log();
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
    let tasks: Vec<_> = room.engine().tasks().collect();
    assert_eq!(
        tasks[0].state,
        TaskState::Failed,
        "a worker and an inspector that never agree must terminate"
    );
    assert!(
        tasks[0].revision <= tasks[0].max_revisions,
        "the rework budget must be respected, saw revision {}",
        tasks[0].revision
    );
    assert!(
        room.turns_used() < room.engine().room().spec.limits.max_total_turns,
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
        .apply(gptbots_engine::Command::PauseRoom {
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
        ) -> gptbots_providers::Result<CompletionResponse> {
            Err(gptbots_providers::ProviderError::Unauthorized {
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
    room.seat(spec("w1", &["writer"], Autonomy::default()), Arc::new(Broken))
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
        gptbots_proto::AgentStatus::Faulted
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
        .apply(gptbots_engine::Command::PostMessage {
            message: Box::new(
                gptbots_proto::ChatMessage {
                    id: gptbots_proto::MessageId::new(),
                    room: room_id,
                    author: gptbots_proto::Author::Human { name: "Alex".into() },
                    audience: gptbots_proto::Audience::Everyone,
                    intent: gptbots_proto::MessageIntent::Ask,
                    body: "anyone there?".into(),
                    task: None,
                    handoff: None,
                    reply_to: None,
                    attachments: vec![],
                    sent_at: gptbots_proto::Timestamp::now(),
                },
            ),
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
async fn an_ordinary_broadcast_statement_does_not_wake_the_room() {
    let mut room = Orchestrator::new(RoomSpec {
        name: "chat".into(),
        goal: "stay quiet".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
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
        .apply(gptbots_engine::Command::PostMessage {
            message: Box::new(gptbots_proto::ChatMessage::say(
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
