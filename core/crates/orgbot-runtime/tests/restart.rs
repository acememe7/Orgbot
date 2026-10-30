//! Surviving being killed.
//!
//! A phone kills backgrounded apps whenever it likes, and it will not wait for
//! a batch of turns to finish. So the question is not whether a room gets
//! interrupted mid-work — it will, routinely — but what it looks like when it
//! comes back: the same work, held by the same bots, with nothing lost and
//! nothing done twice.
//!
//! The dangerous shape is a task left half-claimed by a turn that never
//! returned. Before ownership reconciliation that could strand the task for
//! the life of the room; these check that it either comes back to its holder
//! or comes back to the pool, and never to both.

use std::sync::Arc;

use async_trait::async_trait;
use orgbot_engine::{Command, Engine};
use orgbot_proto::agent::{Autonomy, ProviderRef};
use orgbot_proto::graph::TaskOrigin;
use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
use orgbot_proto::task::TaskState;
use orgbot_proto::{AgentSpec, TaskId, Timestamp};
use orgbot_providers::types::{
    CompletionRequest, CompletionResponse, ContentBlock, StopReason, StreamEvent, Usage,
};
use orgbot_providers::Provider;
use orgbot_runtime::Orchestrator;
use serde_json::json;
use tokio::sync::mpsc;

/// Claims the first task it is offered, once, then stops.
///
/// Reads the id out of the prompt rather than being handed one, the way a real
/// bot does — which also means it does not need to exist before the board.
#[derive(Debug)]
struct Claimer;

#[async_trait]
impl Provider for Claimer {
    fn name(&self) -> &str {
        "claimer"
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        _sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> orgbot_providers::Result<CompletionResponse> {
        let done = CompletionResponse {
            model: "test".into(),
            content: vec![ContentBlock::text("done")],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        let text: String = request
            .messages
            .iter()
            .flat_map(|turn| turn.content.iter())
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                ContentBlock::ToolResult { content, .. } => Some(content.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        // Already acted this turn: say something and stop.
        if request.messages.iter().any(|turn| {
            turn.content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
        }) {
            return Ok(done);
        }
        let Some(task) = text
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .find(|word| word.starts_with("task_") && word.len() > 13)
        else {
            return Ok(done);
        };
        Ok(CompletionResponse {
            model: "test".into(),
            content: vec![ContentBlock::ToolUse {
                id: "toolu_1".into(),
                name: "claim_task".into(),
                input: json!({ "task": task }),
            }],
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
        })
    }
}

fn spec(handle: &str) -> AgentSpec {
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

fn room_spec() -> RoomSpec {
    RoomSpec {
        name: "restart".into(),
        goal: "survive being killed".into(),
        roster: vec![],
        completion: CompletionRule::AllTasksTerminal,
        limits: RoomLimits::default(),
        access: Default::default(),
    }
}

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
        .find_map(|e| match &e.event {
            orgbot_proto::Event::TaskCreated { task } => Some(task.id),
            _ => None,
        })
        .unwrap();
    engine.apply(Command::OpenTask { task }).unwrap();
    task
}

fn claiming() -> Arc<dyn Provider> {
    Arc::new(Claimer)
}

/// A running room, one bot, one task it is scripted to claim.
///
/// The board has to be opened after the room starts, so the provider cannot be
/// built with the task id at seating time; it is swapped in once the id
/// exists.
fn started() -> (Orchestrator, AgentSpec, TaskId) {
    let mut room = Orchestrator::new(room_spec());
    let worker = spec("w1");
    room.seat(worker.clone(), claiming()).unwrap();
    room.start().unwrap();
    let task = {
        let mut engine = room.engine_mut();
        open_task(&mut engine, "the work in hand")
    };
    (room, worker, task)
}

/// Work in hand comes back in hand.
///
/// The app is killed after a bot has claimed a task and before it finishes.
/// On reopening, the claim is still that bot's: its lease has not lapsed, so
/// nothing should have taken the work off it.
#[tokio::test]
async fn a_room_killed_mid_turn_reopens_with_its_work_intact() {
    let (mut room, worker, task) = started();
    let handle = worker.handle.clone();
    room.step_many(1, None).await;

    let holder = room.engine().task(task).unwrap().assignee;
    assert!(holder.is_some(), "the bot took the task before the kill");

    // The kill: nothing but the last checkpoint survives.
    let saved = room.checkpoint();
    drop(room);

    let mut reopened = Orchestrator::restore(saved, room_spec(), vec![(worker, claiming())])
        .expect("a saved room must reopen");

    let record = reopened.engine().task(task).unwrap().clone();
    assert_eq!(record.state, TaskState::Claimed, "the work is still claimed");
    assert_eq!(record.assignee, holder, "and by the same bot");
    assert_eq!(
        reopened.engine().agent_by_handle(&handle).map(|s| s.id),
        holder,
        "whose identity survived the restart"
    );

    // Reconciliation on reopening must not take live work away.
    reopened.engine_mut().apply(Command::Tick {
        now: Timestamp::now(),
    })
    .unwrap();
    assert_eq!(reopened.engine().task(task).unwrap().assignee, holder);
}

/// A room reopens paused, so an interrupted operation is never replayed blind.
#[tokio::test]
async fn a_reopened_room_waits_to_be_told_to_carry_on() {
    let (mut room, worker, task) = started();
    room.step_many(1, None).await;

    let reopened =
        Orchestrator::restore(room.checkpoint(), room_spec(), vec![(worker, claiming())])
            .expect("reopen");

    assert_eq!(
        reopened.engine().room().phase,
        orgbot_proto::room::RoomPhase::Paused,
        "an interrupted room must not resume itself and repeat what it was doing"
    );
}

#[tokio::test]
async fn replacing_the_team_releases_old_claims_without_deleting_history() {
    let (mut room, old, task) = started();
    room.step_many(1, None).await;
    let head = room.engine().head();
    let replacement = spec("new_worker");
    let reopened = Orchestrator::restore(
        room.checkpoint(),
        room_spec(),
        vec![(replacement.clone(), claiming())],
    )
    .unwrap();
    let engine = reopened.engine();
    assert!(engine.agent_by_handle(&old.handle).is_none());
    assert!(engine.agent_by_handle(&replacement.handle).is_some());
    assert_eq!(engine.task(task).unwrap().assignee, None);
    assert_eq!(engine.task(task).unwrap().state, TaskState::Ready);
    assert!(
        engine.head() > head,
        "old history remains, with departure events appended"
    );
    assert_eq!(engine.room().phase, orgbot_proto::room::RoomPhase::Paused);
    assert!(engine.health().is_ok(), "{}", engine.health().report());
}

#[tokio::test]
async fn tightening_cli_policy_discards_previous_conversation_context() {
    use orgbot_proto::agent::{AgentPolicy, CliKind};
    let mut room = Orchestrator::new(room_spec());
    let mut worker = spec("reviewer");
    worker.provider = ProviderRef::CliAgent {
        cli: CliKind::Codex,
        model: "test".into(),
        workdir: ".".into(),
        policy: AgentPolicy::WorkspaceWrite,
        mcp: false,
        effort: String::new(),
    };
    room.seat(worker.clone(), claiming()).unwrap();
    room.start().unwrap();
    open_task(&mut room.engine_mut(), "old work");
    room.step_many(1, None).await;
    let saved = room.checkpoint();
    let same = Orchestrator::restore(
        saved.clone(),
        room_spec(),
        vec![(worker.clone(), claiming())],
    )
    .unwrap();
    let key = worker.id.to_string();
    assert!(!same.checkpoint()["bots"][&key]["history"]
        .as_array()
        .unwrap()
        .is_empty());
    if let ProviderRef::CliAgent { policy, .. } = &mut worker.provider {
        *policy = AgentPolicy::ReadOnly;
    }
    worker.autonomy.review_only = true;
    worker.autonomy.may_inspect = true;
    let tightened = Orchestrator::restore(saved, room_spec(), vec![(worker, claiming())]).unwrap();
    assert!(tightened.checkpoint()["bots"][&key]["history"]
        .as_array()
        .unwrap()
        .is_empty());
}

/// Nothing is lost and nothing is duplicated across the restart.
#[tokio::test]
async fn the_board_is_the_same_board_after_a_restart() {
    let (mut room, worker, first) = started();
    let second = {
        let mut engine = room.engine_mut();
        open_task(&mut engine, "a second, quite different piece of work")
    };
    room.step_many(1, None).await;

    let before: Vec<(TaskId, TaskState)> = room
        .engine()
        .tasks()
        .map(|task| (task.id, task.state))
        .collect();
    let head = room.engine().head();

    let reopened =
        Orchestrator::restore(room.checkpoint(), room_spec(), vec![(worker, claiming())])
            .expect("reopen");

    let after: Vec<(TaskId, TaskState)> = reopened
        .engine()
        .tasks()
        .map(|task| (task.id, task.state))
        .collect();
    assert_eq!(before, after, "the board came back exactly as it was");
    assert_eq!(reopened.engine().head(), head, "and so did its history");
    assert!(
        reopened.engine().health().is_ok(),
        "a restart must not leave the room in a state it has to repair: {}",
        reopened.engine().health().report()
    );
    let _ = second;
}

/// A turn that never returned does not strand its task.
///
/// The bot claimed the work and the process died before it did anything else.
/// Its claim lapses like any other, and the work goes back to the pool rather
/// than sitting assigned to a bot that is never coming back.
#[tokio::test]
async fn work_claimed_by_a_turn_that_never_finished_is_recovered() {
    let (mut room, worker, task) = started();
    room.step_many(1, None).await;

    let mut reopened =
        Orchestrator::restore(room.checkpoint(), room_spec(), vec![(worker, claiming())])
            .expect("reopen");

    // A reopened room comes back paused, and nothing is reclaimed while it is
    // paused; the owner resumes it, which renews the claim. Then time passes
    // with the bot never coming back.
    reopened.engine_mut().apply(Command::ResumeRoom).unwrap();
    reopened.engine_mut().task_mut(task).unwrap().lease_expires_at =
        Some(Timestamp::now().plus_millis(-1));
    reopened
        .engine_mut()
        .apply(Command::Tick {
            now: Timestamp::now(),
        })
        .unwrap();

    let engine = reopened.engine();
    let record = engine.task(task).unwrap();
    assert_eq!(record.state, TaskState::Ready, "back on the board");
    assert_eq!(record.assignee, None, "and held by nobody");
}

// ------------------------------------------------------------ context weight
//
// A board snapshot is the biggest thing a bot is sent and the shortest-lived
// thing it is told: by the next turn it describes a board that has moved. They
// were all kept, so a bot carried thirty-odd of them and re-sent every one on
// every step of every turn. In a real room that was two megabytes a bot —
// roughly half a million tokens per request, against 368 thousand output
// tokens across the entire room. Almost all of the spend was re-reading.

/// Only the newest board snapshot is carried.
#[tokio::test]
async fn a_bot_does_not_re_send_every_board_it_has_ever_seen() {
    let (mut room, _worker, _task) = started();

    // Several turns, each of which would add a snapshot.
    for _ in 0..4 {
        room.step_many(1, None).await;
        let mut engine = room.engine_mut();
        open_task(&mut engine, &format!("more work {}", orgbot_proto::TaskId::new()));
    }

    let memory = room.checkpoint();
    let history = memory["bots"]
        .as_object()
        .and_then(|bots| bots.values().next())
        .and_then(|m| m["history"].as_array())
        .expect("the bot kept a history");

    let boards = history
        .iter()
        .flat_map(|turn| turn["content"].as_array().into_iter().flatten())
        .filter_map(|block| block["text"].as_str())
        .filter(|text| text.starts_with(orgbot_runtime::prompt::SITUATION_MARKER))
        .count();

    assert!(
        boards <= 1,
        "a bot should carry the current board and no older one, found {boards}"
    );
    assert!(
        history.len() > 1,
        "and must still have its conversation: {} turns",
        history.len()
    );
}

/// What replaces a snapshot says why it is gone.
#[tokio::test]
async fn a_dropped_board_leaves_a_note_rather_than_a_hole() {
    let (mut room, _worker, _task) = started();
    room.step_many(1, None).await;
    {
        let mut engine = room.engine_mut();
        open_task(&mut engine, "something new to notice");
    }
    room.step_many(1, None).await;

    let memory = room.checkpoint();
    let text = memory["bots"].to_string();
    assert!(
        text.contains("out of date and has been dropped"),
        "the model should be told why the earlier board is missing"
    );
}

/// A bot is told that the computer may carry playbooks of its own.
///
/// The Hermes machines already hold skills written for them, and the tools to
/// read those have worked all along — `workspace_list` and `workspace_read`.
/// What was missing was any reason to look: a bot cannot use what it does not
/// know is there, and nothing in its instructions had ever mentioned it.
#[test]
fn the_workspace_brief_points_at_the_computers_own_playbooks() {
    let workspace = orgbot_runtime::workspace::Workspace::new(
        orgbot_runtime::workspace::WorkspaceSpec {
            label: "hermes".into(),
            root: "/srv/app".into(),
            writable: true,
            commands: true,
        },
        tokio::sync::mpsc::channel(1).0,
    );
    let brief = workspace.instruction();
    assert!(brief.contains("skills"), "{brief}");
    assert!(brief.contains("SKILL.md"), "{brief}");
    assert!(
        brief.contains("workspace_read"),
        "and say which tool reads one: {brief}"
    );
}
