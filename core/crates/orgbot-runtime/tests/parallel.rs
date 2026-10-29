//! Several bots working at once, without treading on each other.
//!
//! Two things have to hold at the same time and they pull against each other.
//! Turns must genuinely overlap, or the room is no faster than it was and
//! seating twenty bots is theatre. And the board has to survive that overlap:
//! whatever two bots do at the same instant, the result must be a board one of
//! them could have produced alone, never a mixture of both.
//!
//! The second is the one worth testing hard, because the failure is quiet. A
//! lost claim does not crash anything; it produces two bots convinced they own
//! the same work, and you find out days later when their results disagree.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use orgbot_engine::{Command, Engine};
use orgbot_proto::agent::{Autonomy, ProviderRef};
use orgbot_proto::graph::TaskOrigin;
use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
use orgbot_proto::{AgentSpec, TaskId, TaskState};
use orgbot_providers::types::{
    CompletionRequest, CompletionResponse, ContentBlock, StopReason, StreamEvent, Usage,
};
use orgbot_providers::Provider;
use orgbot_runtime::{Bot, Orchestrator, SharedEngine};
use serde_json::json;
use tokio::sync::mpsc;

// ------------------------------------------------------------------- harness

/// A provider that will not answer until `parties` of them are waiting.
///
/// This is what makes "concurrent" mean something. If turns are taken one at a
/// time, the first bot into this never returns, because the second is never
/// started — so a test that finishes at all has proved the overlap, and one
/// that has been quietly serialised times out instead of passing.
struct Rendezvous {
    barrier: Arc<tokio::sync::Barrier>,
    reply: CompletionResponse,
    arrived: Arc<AtomicUsize>,
}

/// Whether this request is a continuation — the bot has already acted once
/// this turn and is being shown the result.
fn is_follow_up(request: &CompletionRequest) -> bool {
    request.messages.iter().any(|turn| {
        turn.content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
    })
}

impl std::fmt::Debug for Rendezvous {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Rendezvous")
    }
}

#[async_trait]
impl Provider for Rendezvous {
    fn name(&self) -> &str {
        "rendezvous"
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        _sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> orgbot_providers::Result<CompletionResponse> {
        // A turn is a loop, and a bot that answered "claim that" would
        // otherwise answer it again on every step. Act once, then stop, as a
        // model does once it has seen the result.
        if is_follow_up(&request) {
            return Ok(CompletionResponse {
                model: "test".into(),
                content: vec![ContentBlock::text("done")],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            });
        }
        self.arrived.fetch_add(1, Ordering::SeqCst);
        self.barrier.wait().await;
        Ok(self.reply.clone())
    }
}

/// A provider that answers immediately with a fixed reply.
#[derive(Debug)]
struct Fixed(CompletionResponse);

#[async_trait]
impl Provider for Fixed {
    fn name(&self) -> &str {
        "fixed"
    }

    async fn complete(
        &self,
        _request: CompletionRequest,
        _sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> orgbot_providers::Result<CompletionResponse> {
        Ok(self.0.clone())
    }
}

fn claim(task: TaskId) -> CompletionResponse {
    CompletionResponse {
        model: "test".into(),
        content: vec![ContentBlock::ToolUse {
            id: "toolu_claim".into(),
            name: "claim_task".into(),
            input: json!({ "task": task.to_string() }),
        }],
        stop_reason: StopReason::ToolUse,
        usage: Usage::default(),
    }
}

fn worker(handle: &str) -> AgentSpec {
    AgentSpec::new(
        handle,
        handle,
        ProviderRef::Scripted {
            script: "test".into(),
        },
    )
    .with_roles(vec!["writer"])
    .with_autonomy(Autonomy::default())
}

fn room_spec(parallel: u32) -> RoomSpec {
    RoomSpec {
        name: "parallel".into(),
        goal: "several bots at once".into(),
        roster: vec![],
        completion: CompletionRule::AllTasksTerminal,
        limits: RoomLimits {
            max_parallel_turns: parallel,
            ..RoomLimits::default()
        },
    }
}

/// Seat `count` writers, all sharing one provider.
fn seat_writers(room: &mut Orchestrator, count: usize, provider: Arc<dyn Provider>) {
    for index in 0..count {
        room.seat(worker(&format!("w{index}")), provider.clone())
            .unwrap();
    }
}

/// The owner asks `to` something directly.
fn owner_asks(room: &mut Orchestrator, to: orgbot_proto::AgentId, body: &str) {
    use orgbot_proto::chat::{Author, ChatMessage, MessageIntent};
    let mut engine = room.engine_mut();
    let mut message = ChatMessage::say(engine.room().id, to, body)
        .to_agents(vec![to])
        .with_intent(MessageIntent::Ask);
    message.author = Author::Human {
        name: "Aece".into(),
    };
    engine.apply(Command::PostMessage { message: Box::new(message) }).unwrap();
}

/// Put an open, claimable task on the board.
fn open_task(engine: &mut Engine, title: &str) -> TaskId {
    let events = engine
        .apply(Command::CreateTask {
            title: title.into(),
            brief: "work".into(),
            by: None,
            required_role: Some("writer".into()),
            priority: orgbot_proto::task::Priority::Normal,
            acceptance_criteria: vec![],
            depends_on: vec![],
            origin: TaskOrigin::plain(),
        })
        .unwrap();
    let task = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            orgbot_proto::Event::TaskCreated { task } => Some(task.id),
            _ => None,
        })
        .expect("the task was created");
    engine.apply(Command::OpenTask { task }).unwrap();
    task
}

// ------------------------------------------------------- the overlap is real

/// The headline claim: turns run at the same time.
///
/// Three bots each refuse to answer until all three have arrived. Under the
/// old one-at-a-time loop this cannot complete — bot one waits forever for
/// bots that were never started — so reaching the assertion *is* the proof.
#[tokio::test]
async fn three_bots_take_their_turns_at_the_same_moment() {
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let arrived = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(Rendezvous {
        barrier,
        arrived: arrived.clone(),
        reply: CompletionResponse {
            model: "test".into(),
            content: vec![ContentBlock::text("here")],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        },
    });

    let mut room = Orchestrator::new(room_spec(3));
    seat_writers(&mut room, 3, provider);
    room.start().unwrap();
    // A task each, so all three are woken for work of their own.
    for index in 0..3 {
        open_task(&mut room.engine_mut(), &format!("task {index}"));
    }

    let reports = tokio::time::timeout(std::time::Duration::from_secs(10), room.step_many(3, None))
        .await
        .expect("three bots must be in flight together, or this never returns");

    assert_eq!(reports.len(), 3, "all three took a turn");
    assert_eq!(
        arrived.load(Ordering::SeqCst),
        3,
        "all three reached the model call before any of them finished"
    );
}

/// Parallelism is capped by the room's setting, not by what is available.
#[tokio::test]
async fn a_room_set_to_two_runs_two_even_with_more_to_do() {
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let arrived = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(Rendezvous {
        barrier,
        arrived: arrived.clone(),
        reply: CompletionResponse {
            model: "test".into(),
            content: vec![ContentBlock::text("here")],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        },
    });

    let mut room = Orchestrator::new(room_spec(2));
    seat_writers(&mut room, 5, provider);
    room.start().unwrap();
    for index in 0..5 {
        open_task(&mut room.engine_mut(), &format!("task {index}"));
    }

    assert_eq!(room.batch_size(), 2, "the room was set to two");
    let reports = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        room.step_many(room.batch_size(), None),
    )
    .await
    .expect("two bots run together");
    assert_eq!(reports.len(), 2, "and only two");
}

// --------------------------------------------------- and the board holds up

/// Two bots, one task, both told to claim it at the same instant.
///
/// The scheduler tries not to arrange this (see below), but it cannot rule it
/// out: a bot decides what to claim from a prompt built minutes earlier. So
/// the engine has to be the thing that settles it. Exactly one bot ends up
/// holding the task, and the board says so unambiguously — the loser has not
/// half-claimed anything, and has not overwritten the winner.
#[tokio::test]
async fn two_bots_reaching_for_one_task_produce_one_holder() {
    let mut engine = Engine::new(room_spec(2));
    let first = worker("w0");
    let second = worker("w1");
    for spec in [&first, &second] {
        engine
            .apply(Command::JoinAgent {
                spec: Box::new(spec.clone()),
            })
            .unwrap();
    }
    engine.apply(Command::StartRoom).unwrap();
    let task = open_task(&mut engine, "the contested task");
    let engine = SharedEngine::new(engine);

    // Both arrive at the model together and both answer "claim that one".
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let arrived = Arc::new(AtomicUsize::new(0));
    let mut bots = [first, second].map(|spec| {
        Bot::new(
            spec,
            Arc::new(Rendezvous {
                barrier: barrier.clone(),
                arrived: arrived.clone(),
                reply: claim(task),
            }) as Arc<dyn Provider>,
        )
    });
    let [ref mut one, ref mut two] = bots;

    let both = futures::future::join(one.take_turn(&engine, None), two.take_turn(&engine, None));
    let (first_report, second_report) = tokio::time::timeout(std::time::Duration::from_secs(10), both)
        .await
        .expect("both turns ran together");
    let reports = [first_report.unwrap(), second_report.unwrap()];

    let record = engine.with(|engine| engine.task(task).cloned()).unwrap();
    assert_eq!(
        record.state,
        TaskState::Claimed,
        "the task is claimed once, not left in some in-between state"
    );
    assert!(record.assignee.is_some(), "and somebody holds it");

    for report in &reports {
        assert_eq!(report.tool_calls, 1, "each bot tried exactly once");
    }
    let winners: Vec<_> = reports
        .iter()
        .filter(|report| report.tool_errors == 0)
        .collect();
    assert_eq!(winners.len(), 1, "exactly one claim succeeded: {reports:?}");
    assert_eq!(
        reports.iter().filter(|r| r.tool_errors == 1).count(),
        1,
        "and the other was refused rather than silently dropped"
    );

    // The holder is the bot whose claim succeeded, not whoever wrote last.
    let winner = winners[0].agent;
    assert_eq!(
        record.assignee,
        Some(winner),
        "the board must agree with whoever was told they succeeded"
    );
}

/// The log is still a sequence, whatever happened concurrently.
///
/// Ordering may vary between runs; being gapless may not. A torn write would
/// show up here as a duplicated or missing sequence number.
#[tokio::test]
async fn concurrent_turns_still_produce_a_gapless_log() {
    let barrier = Arc::new(tokio::sync::Barrier::new(4));
    let provider = Arc::new(Rendezvous {
        barrier,
        arrived: Arc::new(AtomicUsize::new(0)),
        reply: CompletionResponse {
            model: "test".into(),
            content: vec![ContentBlock::text("reporting in")],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        },
    });

    let mut room = Orchestrator::new(room_spec(4));
    seat_writers(&mut room, 4, provider);
    room.start().unwrap();
    for index in 0..4 {
        open_task(&mut room.engine_mut(), &format!("task {index}"));
    }

    tokio::time::timeout(std::time::Duration::from_secs(10), room.step_many(4, None))
        .await
        .expect("four turns together");

    let engine = room.engine();
    let log = engine.log();
    assert!(log.len() >= 8, "four bots posted and recorded turns");
    for (index, envelope) in log.iter().enumerate() {
        assert_eq!(
            envelope.seq.0 as usize,
            index + 1,
            "sequence numbers must not repeat or skip under concurrency"
        );
    }
}

// ------------------------------------------------- not staging races at all

/// Free bots are woken for distinct work, not sent to fight over one task.
///
/// Losing a race is safe but it still costs a turn — and a turn is a model
/// call. With nine bots free and eight tasks open, waking the ninth buys
/// nothing but a refusal.
#[tokio::test]
async fn nine_free_bots_and_eight_open_tasks_wake_eight() {
    let provider = Arc::new(Fixed(CompletionResponse {
        model: "test".into(),
        content: vec![ContentBlock::text("idle")],
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    })) as Arc<dyn Provider>;

    let mut room = Orchestrator::new(room_spec(16));
    seat_writers(&mut room, 9, provider);
    room.start().unwrap();
    for index in 0..8 {
        open_task(&mut room.engine_mut(), &format!("task {index}"));
    }

    assert_eq!(
        room.next_actors(16).len(),
        8,
        "one free bot per open task, and no more"
    );
}

/// One open task wakes one bot, however many are free.
#[tokio::test]
async fn one_task_wakes_one_bot() {
    let provider = Arc::new(Fixed(CompletionResponse {
        model: "test".into(),
        content: vec![ContentBlock::text("idle")],
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    })) as Arc<dyn Provider>;

    let mut room = Orchestrator::new(room_spec(8));
    seat_writers(&mut room, 6, provider);
    room.start().unwrap();
    open_task(&mut room.engine_mut(), "the only task");

    assert_eq!(room.next_actors(8).len(), 1);
}

/// An empty board is planned by one bot, not by every coordinator at once.
///
/// This one is not merely wasteful. Two coordinators drawing up a plan for the
/// same goal at the same moment both succeed — nothing in the engine refuses a
/// second set of tasks — and the room wakes up with the work listed twice.
#[tokio::test]
async fn an_empty_board_is_planned_by_one_bot() {
    let provider = Arc::new(Fixed(CompletionResponse {
        model: "test".into(),
        content: vec![ContentBlock::text("planning")],
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    })) as Arc<dyn Provider>;

    let mut room = Orchestrator::new(room_spec(8));
    for index in 0..3 {
        let spec = AgentSpec::new(
            format!("lead{index}"),
            "Lead",
            ProviderRef::Scripted {
                script: "test".into(),
            },
        )
        .with_roles(vec!["coordinator"])
        .with_autonomy(Autonomy {
            may_create_tasks: true,
            ..Autonomy::default()
        });
        room.seat(spec, provider.clone()).unwrap();
    }
    room.start().unwrap();

    assert_eq!(
        room.next_actors(8).len(),
        1,
        "three coordinators and an empty board must not produce three plans"
    );
}

#[tokio::test]
async fn coordinators_plan_before_workers_and_questions_reserve_the_planning_turn() {
    use orgbot_proto::{Author, ChatMessage, MessageIntent};
    let provider = Arc::new(Fixed(CompletionResponse {
        model: "test".into(),
        content: vec![ContentBlock::text("planning")],
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    })) as Arc<dyn Provider>;
    let mut room = Orchestrator::new(room_spec(8));
    let mut ids = Vec::new();
    for (handle, role) in [
        ("worker", "engineer"),
        ("astra", "coordinator"),
        ("opus", "coordinator"),
    ] {
        let spec = AgentSpec::new(
            handle,
            handle,
            ProviderRef::Scripted {
                script: "test".into(),
            },
        )
        .with_roles([role])
        .with_autonomy(Autonomy {
            may_create_tasks: true,
            ..Autonomy::default()
        });
        ids.push(spec.id);
        room.seat(spec, provider.clone()).unwrap();
    }
    room.start().unwrap();
    assert_eq!(
        room.next_actors(8),
        vec![ids[1]],
        "a worker seated first must not take strategic planning"
    );

    let mut engine = room.engine_mut();
    let mut message = ChatMessage::say(engine.room().id, ids[0], "Plan the app")
        .to_agents(vec![ids[2]])
        .with_intent(MessageIntent::Ask);
    message.author = Author::Human {
        name: "Owner".into(),
    };
    engine.apply(Command::PostMessage { message: Box::new(message) }).unwrap();
    drop(engine);
    assert_eq!(
        room.next_actors(8),
        vec![ids[2]],
        "answering the request must not also wake another planner"
    );
}

/// A batch cannot spend more turns than the budget has left.
#[tokio::test]
async fn the_last_batch_narrows_to_what_the_budget_allows() {
    let provider = Arc::new(Fixed(CompletionResponse {
        model: "test".into(),
        content: vec![ContentBlock::text("idle")],
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    })) as Arc<dyn Provider>;

    let mut spec = room_spec(8);
    spec.limits.max_total_turns = 2;
    let mut room = Orchestrator::new(spec);
    seat_writers(&mut room, 8, provider);
    room.start().unwrap();

    assert_eq!(room.batch_size(), 2, "only two turns are left to spend");
}

/// A room asking for no ceiling still runs a full batch.
#[tokio::test]
async fn an_unlimited_budget_does_not_shrink_the_batch() {
    let provider = Arc::new(Fixed(CompletionResponse {
        model: "test".into(),
        content: vec![ContentBlock::text("idle")],
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    })) as Arc<dyn Provider>;

    let mut spec = room_spec(4);
    spec.limits.max_total_turns = 0;
    let mut room = Orchestrator::new(spec);
    seat_writers(&mut room, 8, provider);
    room.start().unwrap();

    assert_eq!(room.batch_size(), 4);
}

/// Zero means one, not "everyone at once".
#[test]
fn a_parallelism_of_zero_is_read_as_one() {
    let limits = RoomLimits {
        max_parallel_turns: 0,
        ..RoomLimits::default()
    };
    assert_eq!(limits.parallel_turns(), 1);
}

/// A room saved before this setting existed still opens, and opens faster.
#[test]
fn a_room_saved_without_a_parallelism_setting_still_loads() {
    let saved = json!({
        "max_total_turns": 500,
        "max_duration_ms": 3_600_000,
        "handoff_timeout_ms": 300_000,
        "max_consecutive_errors": 3
    });
    let limits: RoomLimits = serde_json::from_value(saved).expect("older rooms must still load");
    assert!(
        limits.parallel_turns() > 1,
        "an existing room should benefit from this without being rebuilt"
    );
}

/// Reopening saved work adopts the room's current setting.
///
/// This is the one that decides whether an existing room benefits. The room
/// that prompted all of this had been running for days; if reopening it kept
/// the concurrency it was saved with, the fix would only ever help rooms
/// created after it, and the room that needed it would stay slow.
#[test]
fn reopening_a_saved_room_picks_up_the_current_concurrency() {
    let mut engine = Engine::new(room_spec(1));
    engine.apply(Command::StartRoom).unwrap();
    assert_eq!(engine.room().spec.limits.parallel_turns(), 1);

    // What opening the saved room does: apply the spec as it is configured now.
    engine.configure_saved_room(room_spec(6));
    assert_eq!(
        engine.room().spec.limits.parallel_turns(),
        6,
        "days-old work should speed up on reopening, not stay as it was saved"
    );
}

/// Two inspectors are not sent at the same piece of work.
///
/// The review queue looked per-bot when the batch scheduler was written and is
/// not: every inspector sees the same work in review. Waking two for one task
/// meant the second arrived to find it already ruled on and sent back, and its
/// own review refused — which reads like the board contradicting itself rather
/// than like a race it lost. It cost a model turn every time.
#[tokio::test]
async fn two_inspectors_are_not_woken_for_the_same_review() {
    use orgbot_proto::agent::Autonomy;

    let provider = Arc::new(Fixed(CompletionResponse {
        model: "test".into(),
        content: vec![ContentBlock::text("looking")],
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    })) as Arc<dyn Provider>;

    let mut room = Orchestrator::new(room_spec(8));
    let author = worker("author");
    let author_id = room.seat(author, provider.clone()).unwrap();
    for index in 0..3 {
        let spec = AgentSpec::new(
            format!("qa{index}"),
            "Inspector",
            ProviderRef::Scripted {
                script: "test".into(),
            },
        )
        .with_roles(vec!["reviewer"])
        .with_autonomy(Autonomy::inspector());
        room.seat(spec, provider.clone()).unwrap();
    }
    room.start().unwrap();

    // One piece of work, submitted and waiting on inspection.
    let task = open_task(&mut room.engine_mut(), "the thing to inspect");
    for command in [
        Command::ClaimTask {
            task,
            by: author_id,
        },
        Command::StartTask {
            task,
            by: author_id,
        },
        Command::SubmitForReview {
            task,
            by: author_id,
            artifacts: vec![],
        },
    ] {
        room.engine_mut().apply(command).unwrap();
    }

    let woken = room.next_actors(8);
    assert_eq!(
        woken.len(),
        1,
        "one task in review must wake one inspector, not three"
    );
    assert_ne!(woken[0], author_id, "and not the bot that wrote it");
}

/// Reviews until the test lets it finish, counting how many it started.
#[derive(Debug)]
struct HeldReview {
    turns: AtomicUsize,
    release: tokio::sync::Semaphore,
}

#[async_trait]
impl Provider for HeldReview {
    fn name(&self) -> &str {
        "held review"
    }

    async fn complete(
        &self,
        _request: CompletionRequest,
        _sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> orgbot_providers::Result<CompletionResponse> {
        self.turns.fetch_add(1, Ordering::SeqCst);
        let _ = self.release.acquire().await;
        Ok(CompletionResponse {
            model: "test".into(),
            content: vec![ContentBlock::text("reviewing")],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        })
    }
}

/// From the live room: Ellis was mid-review when another bot's turn ended,
/// and the freed slot woke Astra for the same review. Astra spent ten minutes
/// on work that was approved and delivered before it finished, and the room
/// refused its review.
#[tokio::test]
async fn a_freed_slot_does_not_wake_a_second_inspector_for_a_review_under_way() {
    let quick = Arc::new(Fixed(CompletionResponse {
        model: "test".into(),
        content: vec![ContentBlock::text("answered")],
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    })) as Arc<dyn Provider>;
    let review = Arc::new(HeldReview {
        turns: AtomicUsize::new(0),
        release: tokio::sync::Semaphore::new(0),
    });

    let mut room = Orchestrator::new(room_spec(3));
    let author = room.seat(worker("author"), quick).unwrap();
    for index in 0..2 {
        let spec = AgentSpec::new(
            format!("qa{index}"),
            "Inspector",
            ProviderRef::Scripted {
                script: "test".into(),
            },
        )
        .with_roles(vec!["reviewer"])
        .with_autonomy(Autonomy::inspector());
        room.seat(spec, review.clone() as Arc<dyn Provider>).unwrap();
    }
    room.start().unwrap();
    let task = open_task(&mut room.engine_mut(), "the thing to inspect");
    for command in [
        Command::ClaimTask { task, by: author },
        Command::StartTask { task, by: author },
        Command::SubmitForReview {
            task,
            by: author,
            artifacts: vec![],
        },
    ] {
        room.engine_mut().apply(command).unwrap();
    }
    // The author has a quick question to answer, so its slot frees at once.
    owner_asks(&mut room, author, "anything else?");

    let shared = room.shared_engine();
    let sinks = |_| None;
    let run = room.step_rolling(3, &sinks);
    tokio::pin!(run);
    let answered = async {
        while !shared.with(|engine| engine.transcript().iter().any(|m| m.body == "answered")) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    };
    tokio::select! {
        _ = &mut run => panic!("the run ended while the review was still going"),
        done = tokio::time::timeout(std::time::Duration::from_secs(5), answered) => {
            done.expect("the author should answer while the review runs");
        }
    }
    assert_eq!(
        review.turns.load(Ordering::SeqCst),
        1,
        "the freed slot must not start a second review of the same task"
    );
    review.release.add_permits(16);
    tokio::time::timeout(std::time::Duration::from_secs(5), run)
        .await
        .expect("the run should end once the review does");
}

/// Its first turn does real work (a room tool call that succeeds); every later
/// one only talks.
#[derive(Debug, Default)]
struct WorksOnce {
    turns: AtomicUsize,
}

#[async_trait]
impl Provider for WorksOnce {
    fn name(&self) -> &str {
        "works once"
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        _sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> orgbot_providers::Result<CompletionResponse> {
        let text = |body: &str| CompletionResponse {
            model: "test".into(),
            content: vec![ContentBlock::text(body)],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        // A bot keeps its conversation between turns, so a turn starts when
        // the last message is not a tool result.
        let continuing = request.messages.last().is_some_and(|turn| {
            turn.content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
        });
        if continuing {
            return Ok(text("read the board"));
        }
        if self.turns.fetch_add(1, Ordering::SeqCst) == 0 {
            return Ok(CompletionResponse {
                model: "test".into(),
                content: vec![ContentBlock::ToolUse {
                    id: "toolu_list".into(),
                    name: "list_tasks".into(),
                    input: json!({}),
                }],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            });
        }
        Ok(text("nothing new from me"))
    }
}

/// From the live room: Mira's turn ran for over an hour. A run lasts as long
/// as its slowest turn, and after its first ten minutes freed slots were no
/// longer filled, so Quinn and Devon sat idle beside ready work the whole
/// time. A bot whose turn got something done goes again; one that only
/// talked does not.
#[tokio::test]
async fn a_long_turn_does_not_idle_everyone_else_for_the_rest_of_the_run() {
    let slow = Arc::new(HeldReview {
        turns: AtomicUsize::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    let busy = Arc::new(WorksOnce::default());
    let mut room = Orchestrator::new(room_spec(2));
    // The slow bot cannot claim the open task, so it is woken only by the
    // owner's question and leaves the task to the other.
    let slow_spec = AgentSpec::new(
        "slow",
        "slow",
        ProviderRef::Scripted {
            script: "test".into(),
        },
    )
    .with_roles(vec!["reviewer"]);
    let slow_id = room.seat(slow_spec, slow.clone() as Arc<dyn Provider>).unwrap();
    room.seat(worker("busy"), busy.clone() as Arc<dyn Provider>).unwrap();
    room.start().unwrap();
    open_task(&mut room.engine_mut(), "ready work");
    owner_asks(&mut room, slow_id, "start the long job");

    let sinks = |_| None;
    let run = room.step_rolling(2, &sinks);
    tokio::pin!(run);
    let worked_again = async {
        while busy.turns.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    };
    tokio::select! {
        _ = &mut run => panic!("the run ended while the long turn was still going"),
        done = tokio::time::timeout(std::time::Duration::from_secs(5), worked_again) => {
            done.expect("after a productive turn the bot must go again while the long turn runs");
        }
    }
    assert_eq!(
        busy.turns.load(Ordering::SeqCst),
        2,
        "a turn that only talked does not earn another"
    );
    assert_eq!(slow.turns.load(Ordering::SeqCst), 1, "the long turn is still the same one");
    slow.release.add_permits(16);
    tokio::time::timeout(std::time::Duration::from_secs(5), run)
        .await
        .expect("the run should end once the long turn does");
}

/// Only ever talks, counting its turns.
#[derive(Debug, Default)]
struct OnlyTalks {
    turns: AtomicUsize,
}

#[async_trait]
impl Provider for OnlyTalks {
    fn name(&self) -> &str {
        "only talks"
    }

    async fn complete(
        &self,
        _request: CompletionRequest,
        _sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> orgbot_providers::Result<CompletionResponse> {
        self.turns.fetch_add(1, Ordering::SeqCst);
        Ok(CompletionResponse {
            model: "test".into(),
            content: vec![ContentBlock::text("thinking about it")],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        })
    }
}

/// From the live room: Julian and Finn each had a turn that achieved nothing,
/// and a run now lasts as long as there is work, so neither got another for
/// the rest of it. Both sat idle beside their own tasks until the claims
/// lapsed. Such a bot waits a while, not forever.
#[tokio::test(start_paused = true)]
async fn a_bot_whose_turn_achieved_nothing_tries_again_later_in_the_run() {
    let slow = Arc::new(HeldReview {
        turns: AtomicUsize::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    let talker = Arc::new(OnlyTalks::default());
    let mut room = Orchestrator::new(room_spec(2));
    let slow_spec = AgentSpec::new(
        "slow",
        "slow",
        ProviderRef::Scripted {
            script: "test".into(),
        },
    )
    .with_roles(vec!["reviewer"]);
    let slow_id = room.seat(slow_spec, slow.clone() as Arc<dyn Provider>).unwrap();
    room.seat(worker("talker"), talker.clone() as Arc<dyn Provider>).unwrap();
    room.start().unwrap();
    open_task(&mut room.engine_mut(), "ready work");
    owner_asks(&mut room, slow_id, "start the long job");

    let sinks = |_| None;
    let run = room.step_rolling(2, &sinks);
    tokio::pin!(run);
    let wait = |seconds| tokio::time::sleep(std::time::Duration::from_secs(seconds));
    tokio::select! {
        _ = &mut run => panic!("the run ended while the long turn was still going"),
        _ = wait(60) => {}
    }
    assert_eq!(talker.turns.load(Ordering::SeqCst), 1, "not at once after a turn that achieved nothing");
    tokio::select! {
        _ = &mut run => panic!("the run ended while the long turn was still going"),
        _ = wait(90) => {}
    }
    assert_eq!(talker.turns.load(Ordering::SeqCst), 2, "but again a little later, while it still has work");
    slow.release.add_permits(16);
    tokio::time::timeout(std::time::Duration::from_secs(600), run)
        .await
        .expect("the run should end once the long turn does");
}

/// From the live room: nothing was written to disk for the hour Mira's turn
/// ran, because the room was only saved when a run ended. A crash then would
/// have lost an hour of board changes.
#[tokio::test(start_paused = true)]
async fn the_room_is_saved_while_a_long_turn_runs() {
    let slow = Arc::new(HeldReview {
        turns: AtomicUsize::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    let mut room = Orchestrator::new(room_spec(1));
    let slow_id = room.seat(worker("slow"), slow.clone() as Arc<dyn Provider>).unwrap();
    room.start().unwrap();
    owner_asks(&mut room, slow_id, "start the long job");
    let saves = Arc::new(std::sync::Mutex::new(Vec::new()));
    let saved = saves.clone();
    room.attach_autosave(orgbot_runtime::Autosave(Arc::new(move |board| {
        saved.lock().unwrap().push(board);
    })));

    let sinks = |_| None;
    let run = room.step_rolling(1, &sinks);
    tokio::pin!(run);
    let saved_once = async {
        while saves.lock().unwrap().is_empty() {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    };
    tokio::select! {
        _ = &mut run => panic!("the run ended while the long turn was still going"),
        done = tokio::time::timeout(std::time::Duration::from_secs(10 * 60), saved_once) => {
            done.expect("the room must be saved while the turn runs");
        }
    }
    let board = saves.lock().unwrap()[0].clone();
    assert!(
        board["bots"].as_object().unwrap().contains_key(&slow_id.to_string()),
        "the bot in its turn is saved too, as it was when the turn began: {}",
        board["bots"]
    );
    slow.release.add_permits(16);
    tokio::time::timeout(std::time::Duration::from_secs(60), run)
        .await
        .expect("the run should end once the turn does");
}

/// Concurrency is not aimed all at one provider.
///
/// Parallel turns are parallel requests. Bots sharing a backend share its rate
/// limit, so starting six of them at once against one subscription spends six
/// turns to get two answers and four refusals. The room is then slower than if
/// it had waited, which is the opposite of the point.
#[tokio::test]
async fn one_backend_does_not_receive_the_whole_batch() {
    let provider = Arc::new(Fixed(CompletionResponse {
        model: "test".into(),
        content: vec![ContentBlock::text("idle")],
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    })) as Arc<dyn Provider>;

    let mut spec = room_spec(8);
    spec.limits.max_parallel_per_backend = 2;
    let mut room = Orchestrator::new(spec);

    // Six bots, all on the same account.
    for index in 0..6 {
        let bot = AgentSpec::new(
            format!("w{index}"),
            "Worker",
            ProviderRef::Anthropic {
                model: "claude-opus-5".into(),
            },
        )
        .with_roles(vec!["writer"]);
        room.seat(bot, provider.clone()).unwrap();
    }
    room.start().unwrap();
    for index in 0..6 {
        open_task(&mut room.engine_mut(), &format!("task {index}"));
    }

    assert_eq!(
        room.next_actors(8).len(),
        2,
        "six bots on one subscription must not all be started at once"
    );
}

/// Different accounts do not contend, so they are not held back.
#[tokio::test]
async fn bots_on_separate_backends_all_run_together() {
    let provider = Arc::new(Fixed(CompletionResponse {
        model: "test".into(),
        content: vec![ContentBlock::text("idle")],
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    })) as Arc<dyn Provider>;

    let mut spec = room_spec(8);
    spec.limits.max_parallel_per_backend = 2;
    let mut room = Orchestrator::new(spec);

    let backends = [
        ProviderRef::Anthropic { model: "a".into() },
        ProviderRef::DeepSeek { model: "d".into() },
        ProviderRef::OpenAiCompatible {
            model: "g".into(),
            profile: "grok".into(),
        },
        ProviderRef::OpenAiCompatible {
            model: "c".into(),
            profile: "codex".into(),
        },
    ];
    for (index, backend) in backends.into_iter().enumerate() {
        let bot = AgentSpec::new(format!("w{index}"), "Worker", backend).with_roles(vec!["writer"]);
        room.seat(bot, provider.clone()).unwrap();
    }
    room.start().unwrap();
    for index in 0..4 {
        open_task(&mut room.engine_mut(), &format!("task {index}"));
    }

    assert_eq!(
        room.next_actors(8).len(),
        4,
        "four separate accounts do not compete and must not be throttled"
    );
}

/// The per-backend cap is off unless somebody turns it on.
///
/// It guards against a risk that may not exist for a given roster, and it
/// costs throughput that certainly does. A room of fifteen bots on one account
/// running four at a time, with no rate limiting all day, is evidence — and a
/// cap set tighter than the evidence is just a slower room.
#[tokio::test]
async fn a_roster_on_one_account_is_not_throttled_by_default() {
    let provider = Arc::new(Fixed(CompletionResponse {
        model: "test".into(),
        content: vec![ContentBlock::text("idle")],
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    })) as Arc<dyn Provider>;

    let mut room = Orchestrator::new(room_spec(4));
    for index in 0..8 {
        let bot = AgentSpec::new(
            format!("w{index}"),
            "Worker",
            ProviderRef::OpenAiCompatible {
                model: "gpt-6-astra".into(),
                profile: "codex".into(),
            },
        )
        .with_roles(vec!["writer"]);
        room.seat(bot, provider.clone()).unwrap();
    }
    room.start().unwrap();
    for index in 0..8 {
        open_task(&mut room.engine_mut(), &format!("task {index}"));
    }

    // Through `batch_size`, which is what actually drives a batch: the
    // room-wide setting is the ceiling, and nothing narrows it further.
    assert_eq!(room.batch_size(), 4);
    assert_eq!(
        room.next_actors(room.batch_size()).len(),
        4,
        "the room's own concurrency is the only limit until one is asked for"
    );
}
