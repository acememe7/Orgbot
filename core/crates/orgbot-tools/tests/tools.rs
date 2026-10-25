//! The tool surface, exercised the way a model experiences it.
//!
//! Bots only ever see JSON in and text out, so these tests drive `dispatch`
//! with JSON and read ids back out of the rendered strings rather than reaching
//! into the engine. If an id cannot be recovered from what a bot is told, a
//! real bot could not continue either.

use orgbot_engine::{Command, Engine};
use orgbot_proto::agent::{Autonomy, ProviderRef};
use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
use orgbot_proto::{AgentId, AgentSpec, ChatMessage, TaskState};
use orgbot_tools::{definitions_for, dispatch, ToolOutcome};
use serde_json::{json, Value};

struct Fixture {
    engine: Engine,
    lead: AgentId,
    worker: AgentId,
    worker2: AgentId,
    inspector: AgentId,
}

fn fixture() -> Fixture {
    let mut engine = Engine::new(RoomSpec {
        name: "release".into(),
        goal: "ship the release notes".into(),
        roster: vec![],
        completion: CompletionRule::AllTasksTerminal,
        limits: RoomLimits::default(),
    });
    let provider = || ProviderRef::Scripted {
        script: "test".into(),
    };

    let lead = AgentSpec::new("lead", "Lead", provider())
        .with_roles(["coordinator"])
        .with_autonomy(Autonomy::coordinator());
    let worker = AgentSpec::new("w1", "Worker One", provider()).with_roles(["writer"]);
    let worker2 = AgentSpec::new("w2", "Worker Two", provider()).with_roles(["writer"]);
    let inspector = AgentSpec::new("qa", "Inspector", provider())
        .with_roles(["reviewer"])
        .with_autonomy(Autonomy::inspector());

    let ids = (lead.id, worker.id, worker2.id, inspector.id);
    for spec in [lead, worker, worker2, inspector] {
        engine
            .apply(Command::JoinAgent {
                spec: Box::new(spec),
            })
            .unwrap();
    }
    engine.apply(Command::StartRoom).unwrap();

    Fixture {
        engine,
        lead: ids.0,
        worker: ids.1,
        worker2: ids.2,
        inspector: ids.3,
    }
}

impl Fixture {
    fn call(&mut self, actor: AgentId, tool: &str, input: Value) -> ToolOutcome {
        dispatch(&mut self.engine, actor, tool, &input)
    }

    /// Call a tool and require it to succeed.
    fn ok(&mut self, actor: AgentId, tool: &str, input: Value) -> String {
        let outcome = self.call(actor, tool, input);
        assert!(!outcome.is_error, "`{tool}` failed: {}", outcome.content);
        outcome.content
    }

    /// Call a tool and require it to fail.
    fn err(&mut self, actor: AgentId, tool: &str, input: Value) -> String {
        let outcome = self.call(actor, tool, input);
        assert!(
            outcome.is_error,
            "`{tool}` should have failed, got: {}",
            outcome.content
        );
        outcome.content
    }
}

/// Recover the first id with `prefix` from text a bot was shown.
fn id_in(text: &str, prefix: &str) -> String {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .find(|word| word.starts_with(&format!("{prefix}_")) && word.len() > prefix.len() + 8)
        .unwrap_or_else(|| panic!("no `{prefix}_` id in: {text}"))
        .to_string()
}

// ------------------------------------------------------- capability filtering

#[test]
fn a_worker_is_not_offered_the_tools_of_other_roles() {
    let f = fixture();
    let names: Vec<String> = definitions_for(f.engine.agent(f.worker).unwrap())
        .into_iter()
        .map(|t| t.name)
        .collect();

    assert!(names.contains(&"claim_task".to_string()));
    assert!(names.contains(&"offer_handoff".to_string()));
    for withheld in ["record_review", "submit_task", "create_task"] {
        assert!(
            !names.contains(&withheld.to_string()),
            "a worker must not even see `{withheld}`"
        );
    }
}

#[test]
fn each_role_sees_exactly_the_extra_tool_it_needs() {
    let f = fixture();
    let names = |agent| -> Vec<String> {
        definitions_for(f.engine.agent(agent).unwrap())
            .into_iter()
            .map(|t| t.name)
            .collect()
    };
    let inspector = names(f.inspector);
    assert!(inspector.contains(&"record_review".to_string()));
    assert!(!inspector.contains(&"submit_task".to_string()));

    let lead = names(f.lead);
    assert!(lead.contains(&"submit_task".to_string()));
    assert!(lead.contains(&"create_task".to_string()));
    assert!(
        !lead.contains(&"record_review".to_string()),
        "a coordinator must not sign off on its own plan"
    );
}

#[test]
fn review_only_bots_get_read_tools_and_cannot_dispatch_mutations() {
    let mut f = fixture();
    let mut reviewer = f.engine.agent(f.inspector).unwrap().clone();
    reviewer.autonomy.review_only = true;
    f.engine.update_agent(reviewer.clone());

    let names: Vec<_> = definitions_for(&reviewer)
        .into_iter()
        .map(|tool| tool.name)
        .collect();
    for allowed in [
        "room_status",
        "list_tasks",
        "read_artifact",
        "record_review",
        "add_artifact",
        "submit_for_review",
    ] {
        assert!(
            names.iter().any(|name| name == allowed),
            "missing {allowed}"
        );
    }
    for blocked in [
        "offer_handoff",
        "accept_handoff",
        "create_task",
        "submit_task",
    ] {
        assert!(
            !names.iter().any(|name| name == blocked),
            "offered {blocked}"
        );
        let outcome = f.call(f.inspector, blocked, json!({}));
        assert!(outcome.is_error, "dispatch must reject {blocked}");
        assert!(outcome.content.contains("Review-only"));
    }

    let message = f.call(
        f.inspector,
        "post_message",
        json!({"body":"Review underway"}),
    );
    assert!(
        !message.is_error,
        "reviewers must still send actionable messages"
    );

    let ordinary = f.make(json!({"title":"implementation", "brief":"ordinary task"}));
    let not_assigned = f.call(
        f.inspector,
        "add_artifact",
        json!({"task":ordinary, "name":"report.md", "content":"not allowed"}),
    );
    assert!(
        not_assigned.is_error,
        "unassigned ordinary-task artifact refused"
    );

    let review_task = id_in(
        &f.ok(
            f.lead,
            "create_task",
            json!({
                "title":"review-only proof",
                "brief":"inspect the implementation and report findings",
                "kind":"review",
                "of":ordinary,
                "required_role":"reviewer",
            }),
        ),
        "task",
    );
    f.ok(f.inspector, "claim_task", json!({"task":review_task}));
    f.ok(f.inspector, "start_task", json!({"task":review_task}));
    let artifact = id_in(
        &f.ok(
            f.inspector,
            "add_artifact",
            json!({"task":review_task, "name":"review.md", "content":"Checked criteria and recorded findings."}),
        ),
        "art",
    );
    f.ok(
        f.inspector,
        "submit_for_review",
        json!({"task":review_task, "artifacts":[artifact]}),
    );
    let self_review = f.call(
        f.inspector,
        "record_review",
        json!({"task":review_task, "summary":"self approval"}),
    );
    assert!(
        self_review.is_error,
        "reviewer cannot approve its own report"
    );

    let second_reviewer = AgentSpec::new(
        "qa-second",
        "Independent Reviewer",
        ProviderRef::Scripted {
            script: "test".into(),
        },
    )
    .with_roles(["reviewer"])
    .with_autonomy(Autonomy::inspector());
    let second_reviewer_id = second_reviewer.id;
    f.engine
        .apply(Command::JoinAgent {
            spec: Box::new(second_reviewer),
        })
        .unwrap();
    let review = f.ok(
        second_reviewer_id,
        "record_review",
        json!({"task":review_task, "summary":"independent review confirms the report"}),
    );
    assert!(
        review.contains("approved"),
        "review remained actionable: {review}"
    );
    f.ok(f.lead, "submit_task", json!({"task":review_task}));
}

#[test]
fn every_tool_schema_rejects_unknown_arguments() {
    let f = fixture();
    for tool in definitions_for(f.engine.agent(f.lead).unwrap()) {
        assert_eq!(
            tool.input_schema["additionalProperties"], false,
            "`{}` would silently drop an invented argument",
            tool.name
        );
        assert!(
            !tool.description.is_empty(),
            "`{}` has no description for the model to read",
            tool.name
        );
    }
}

// --------------------------------------------------------- the whole workflow

/// The headline case: a task is created, worked, inspected, and delivered
/// using nothing but tool calls.
#[test]
fn a_task_ships_end_to_end_through_tool_calls_alone() {
    let mut f = fixture();
    let (lead, worker, inspector) = (f.lead, f.worker, f.inspector);

    let created = f.ok(
        lead,
        "create_task",
        json!({
            "title": "write the release notes",
            "brief": "summarise everything since v1.2",
            "acceptance_criteria": ["covers every merged PR", "no internal links"],
            "required_role": "writer",
            "priority": "high"
        }),
    );
    let task = id_in(&created, "task");

    // The worker finds it, takes it, and starts.
    let board = f.ok(worker, "list_tasks", json!({"filter": "claimable"}));
    assert!(board.contains(&task), "the worker must be able to find it");
    f.ok(worker, "claim_task", json!({"task": task}));
    f.ok(worker, "start_task", json!({"task": task}));

    // The work itself lands as an artifact, not as chat.
    let recorded = f.ok(
        worker,
        "add_artifact",
        json!({
            "task": task,
            "name": "notes.md",
            "content": "## v1.3\n- fixed the thing"
        }),
    );
    let artifact = id_in(&recorded, "art");

    let sent = f.ok(
        worker,
        "submit_for_review",
        json!({"task": task, "artifacts": [artifact]}),
    );
    assert!(
        sent.contains("cannot approve this yourself"),
        "the tool should say why it is waiting: {sent}"
    );

    // The inspector reads the work and finds nothing blocking.
    let read = f.ok(inspector, "read_artifact", json!({"artifact": artifact}));
    assert!(read.contains("fixed the thing"));

    let reviewed = f.ok(
        inspector,
        "record_review",
        json!({
            "task": task,
            "summary": "covers everything",
            "findings": [],
            "criteria_met": ["covers every merged PR", "no internal links"]
        }),
    );
    assert!(reviewed.contains("approved"), "got: {reviewed}");

    f.ok(lead, "submit_task", json!({"task": task}));
    let id: orgbot_proto::TaskId = task.parse().unwrap();
    assert_eq!(f.engine.task(id).unwrap().state, TaskState::Submitted);
}

#[test]
fn a_blocking_finding_sends_the_work_back_without_the_inspector_choosing_a_verdict() {
    let mut f = fixture();
    let (lead, worker, inspector) = (f.lead, f.worker, f.inspector);

    let task = id_in(
        &f.ok(
            lead,
            "create_task",
            json!({"title": "notes", "brief": "write them"}),
        ),
        "task",
    );
    f.ok(worker, "claim_task", json!({"task": task}));
    f.ok(worker, "start_task", json!({"task": task}));
    let artifact = id_in(
        &f.ok(
            worker,
            "add_artifact",
            json!({"task": task, "name": "notes.md", "content": "draft"}),
        ),
        "art",
    );
    f.ok(
        worker,
        "submit_for_review",
        json!({"task": task, "artifacts": [artifact]}),
    );

    let reviewed = f.ok(
        inspector,
        "record_review",
        json!({
            "task": task,
            "summary": "not ready",
            "findings": [{"severity": "major", "summary": "the version is wrong"}]
        }),
    );
    assert!(reviewed.contains("in_progress"), "got: {reviewed}");
}

// -------------------------------------------------------------- handoffs

#[test]
fn a_handoff_round_trips_through_tool_calls() {
    let mut f = fixture();
    let (lead, worker, worker2) = (f.lead, f.worker, f.worker2);

    let task = id_in(
        &f.ok(
            lead,
            "create_task",
            json!({"title": "notes", "brief": "write them"}),
        ),
        "task",
    );
    f.ok(worker, "claim_task", json!({"task": task}));
    f.ok(worker, "start_task", json!({"task": task}));

    let offered = f.ok(
        worker,
        "offer_handoff",
        json!({
            "task": task,
            "to_agent": "w2",
            "kind": "assist",
            "brief": "pull the changelog",
            "expects": ["a bullet list"]
        }),
    );
    assert!(
        offered.contains("blocked") && offered.contains("do not poll"),
        "the sender must be told it is parked: {offered}"
    );
    let handoff = id_in(&offered, "ho");

    // The recipient sees it in their queue.
    let queue = f.ok(worker2, "list_handoffs", json!({}));
    assert!(queue.contains(&handoff));

    f.ok(worker2, "accept_handoff", json!({"handoff": handoff}));
    let delivered = f.ok(
        worker2,
        "complete_handoff",
        json!({"handoff": handoff, "summary": "12 entries since v1.2"}),
    );
    assert!(delivered.contains("unblocked"));

    let id: orgbot_proto::TaskId = task.parse().unwrap();
    assert_eq!(f.engine.task(id).unwrap().state, TaskState::InProgress);
}

#[test]
fn declining_a_handoff_unblocks_the_sender() {
    let mut f = fixture();
    let (lead, worker, worker2) = (f.lead, f.worker, f.worker2);
    let task = id_in(
        &f.ok(lead, "create_task", json!({"title": "t", "brief": "b"})),
        "task",
    );
    f.ok(worker, "claim_task", json!({"task": task}));
    f.ok(worker, "start_task", json!({"task": task}));
    let handoff = id_in(
        &f.ok(
            worker,
            "offer_handoff",
            json!({"task": task, "to_agent": "w2", "kind": "assist", "brief": "help"}),
        ),
        "ho",
    );

    let declined = f.ok(
        worker2,
        "reject_handoff",
        json!({"handoff": handoff, "reason": "not my area"}),
    );
    assert!(declined.contains("unblocked"));
    let id: orgbot_proto::TaskId = task.parse().unwrap();
    assert_eq!(f.engine.task(id).unwrap().state, TaskState::InProgress);
}

#[test]
fn a_bot_can_be_addressed_by_handle_rather_than_by_id() {
    let mut f = fixture();
    let (lead, worker) = (f.lead, f.worker);
    let task = id_in(
        &f.ok(lead, "create_task", json!({"title": "t", "brief": "b"})),
        "task",
    );
    f.ok(worker, "claim_task", json!({"task": task}));
    f.ok(worker, "start_task", json!({"task": task}));

    // `w2` is a handle, not an id — a model will reach for the handle it saw
    // in room_status.
    f.ok(
        worker,
        "offer_handoff",
        json!({"task": task, "to_agent": "w2", "kind": "delegate", "brief": "take it"}),
    );
    f.ok(
        worker,
        "post_message",
        json!({"body": "over to you", "to_agents": ["w2"], "intent": "say"}),
    );
}

#[test]
fn chat_mentions_and_replies_target_only_the_addressed_bot() {
    let mut f = fixture();
    f.ok(
        f.worker,
        "post_message",
        json!({"body": "@w2 can you check this?"}),
    );
    let mention = f.engine.transcript().last().unwrap();
    assert_eq!(
        mention.audience,
        orgbot_proto::Audience::Agents {
            agents: vec![f.worker2]
        }
    );
    assert!(mention.wakes(f.worker2, &["writer".into()]));
    assert!(!mention.wakes(f.worker, &["writer".into()]));

    let parent = ChatMessage::say(f.engine.room_id(), f.worker2, "What should I check?");
    let parent_id = parent.id;
    f.engine
        .apply(Command::PostMessage {
            message: Box::new(parent),
        })
        .unwrap();
    f.ok(
        f.worker,
        "post_message",
        json!({"body": "Reviewing your question.", "reply_to": parent_id.to_string()}),
    );
    let reply = f.engine.transcript().last().unwrap();
    assert_eq!(reply.reply_to, Some(parent_id));
    assert_eq!(
        reply.audience,
        orgbot_proto::Audience::Agents {
            agents: vec![f.worker2]
        }
    );
    assert!(reply.wakes(f.worker2, &["writer".into()]));
    assert!(!reply.wakes(f.lead, &["coordinator".into()]));
}

#[test]
fn chat_mentions_accept_a_unique_display_name_but_reject_ambiguous_names() {
    let mut f = fixture();
    let mut worker = f.engine.agent(f.worker2).unwrap().clone();
    worker.display_name = "Julian".into();
    f.engine.update_agent(worker);
    f.ok(
        f.worker,
        "post_message",
        json!({"body": "@Julian please review"}),
    );
    assert_eq!(
        f.engine.transcript().last().unwrap().audience,
        orgbot_proto::Audience::Agents {
            agents: vec![f.worker2]
        }
    );

    let mut worker = f.engine.agent(f.worker).unwrap().clone();
    worker.display_name = "Julian".into();
    f.engine.update_agent(worker);
    let error = f.err(
        f.lead,
        "post_message",
        json!({"body": "@Julian please review"}),
    );
    assert!(error.contains("ambiguous"));
    assert_eq!(f.engine.transcript().len(), 1);
}

#[test]
fn post_message_rejects_unknown_reply_ids() {
    let mut f = fixture();
    let result = f.err(
        f.worker,
        "post_message",
        json!({"body": "This reply must not be broadcast.", "reply_to": "msg_00000000000000000000000000000000"}),
    );
    assert!(result.contains("no message with that id in this room"));
    assert!(f.engine.transcript().is_empty());
}

// ---------------------------------------------------- errors as instructions

#[test]
fn acting_on_someone_elses_task_explains_what_to_do_instead() {
    let mut f = fixture();
    let (lead, worker, worker2) = (f.lead, f.worker, f.worker2);
    let task = id_in(
        &f.ok(lead, "create_task", json!({"title": "t", "brief": "b"})),
        "task",
    );
    f.ok(worker, "claim_task", json!({"task": task}));

    let message = f.err(
        worker2,
        "submit_for_review",
        json!({"task": task, "artifacts": []}),
    );
    assert!(
        message.contains("Claim the task first") || message.contains("handoff"),
        "the error must tell the model what to do: {message}"
    );
}

#[test]
fn reviewing_your_own_work_explains_the_rule() {
    let mut f = fixture();
    let solo = AgentSpec::new("solo", "Solo", ProviderRef::Scripted { script: "s".into() })
        .with_roles(["writer", "reviewer"])
        .with_autonomy(Autonomy::inspector());
    let solo_id = solo.id;
    f.engine
        .apply(Command::JoinAgent {
            spec: Box::new(solo),
        })
        .unwrap();

    let lead = f.lead;
    let task = id_in(
        &f.ok(lead, "create_task", json!({"title": "t", "brief": "b"})),
        "task",
    );
    f.ok(solo_id, "claim_task", json!({"task": task}));
    f.ok(solo_id, "start_task", json!({"task": task}));
    let artifact = id_in(
        &f.ok(
            solo_id,
            "add_artifact",
            json!({"task": task, "name": "x", "content": "y"}),
        ),
        "art",
    );
    f.ok(
        solo_id,
        "submit_for_review",
        json!({"task": task, "artifacts": [artifact]}),
    );

    let message = f.err(
        solo_id,
        "record_review",
        json!({"task": task, "summary": "looks fine to me"}),
    );
    assert!(message.contains("different bot"), "got: {message}");
}

#[test]
fn a_malformed_id_says_what_the_right_shape_looks_like() {
    let mut f = fixture();
    let worker = f.worker;
    let message = f.err(worker, "claim_task", json!({"task": "the first one"}));
    assert!(message.contains("task_"), "got: {message}");
}

#[test]
fn a_missing_required_argument_names_the_field() {
    let mut f = fixture();
    let worker = f.worker;
    assert!(f.err(worker, "post_message", json!({})).contains("`body`"));
    assert!(f
        .err(worker, "claim_task", json!({"task": ""}))
        .contains("`task`"));
}

#[test]
fn an_unknown_bot_handle_points_at_room_status() {
    let mut f = fixture();
    let worker = f.worker;
    let message = f.err(
        worker,
        "post_message",
        json!({"body": "hi", "to_agents": ["nobody"]}),
    );
    assert!(message.contains("room_status"), "got: {message}");
}

#[test]
fn an_unknown_tool_name_is_reported_rather_than_ignored() {
    let mut f = fixture();
    let worker = f.worker;
    let outcome = f.call(worker, "delete_everything", json!({}));
    assert!(outcome.is_error);
    assert!(outcome.content.contains("no tool called"));
}

#[test]
fn a_handoff_with_no_recipient_says_which_field_to_use() {
    let mut f = fixture();
    let (lead, worker) = (f.lead, f.worker);
    let task = id_in(
        &f.ok(lead, "create_task", json!({"title": "t", "brief": "b"})),
        "task",
    );
    f.ok(worker, "claim_task", json!({"task": task}));
    f.ok(worker, "start_task", json!({"task": task}));

    let message = f.err(
        worker,
        "offer_handoff",
        json!({"task": task, "kind": "assist", "brief": "help"}),
    );
    assert!(message.contains("to_agent") && message.contains("to_role"));
}

#[test]
fn invalid_json_content_suggests_switching_to_text() {
    let mut f = fixture();
    let (lead, worker) = (f.lead, f.worker);
    let task = id_in(
        &f.ok(lead, "create_task", json!({"title": "t", "brief": "b"})),
        "task",
    );
    f.ok(worker, "claim_task", json!({"task": task}));
    f.ok(worker, "start_task", json!({"task": task}));

    let message = f.err(
        worker,
        "add_artifact",
        json!({"task": task, "name": "data", "content": "{not json", "kind": "json"}),
    );
    assert!(message.contains("\"text\""), "got: {message}");
}

// ------------------------------------------------------------------ visibility

#[test]
fn room_status_shows_the_roster_the_goal_and_the_board() {
    let mut f = fixture();
    let (lead, worker) = (f.lead, f.worker);
    f.ok(
        lead,
        "create_task",
        json!({"title": "write the notes", "brief": "b"}),
    );

    let status = f.ok(worker, "room_status", json!({}));
    assert!(status.contains("ship the release notes"), "the goal");
    assert!(
        status.contains("@lead") && status.contains("@qa"),
        "the roster"
    );
    assert!(status.contains("← you"), "which bot is reading it");
    assert!(status.contains("write the notes"), "the board");
}

#[test]
fn a_deadlock_broken_during_a_block_is_reported_back_to_the_bot() {
    let mut f = fixture();
    let (lead, worker, worker2) = (f.lead, f.worker, f.worker2);

    let mut start = |title: &str, who: AgentId, f: &mut Fixture| {
        let task = id_in(
            &f.ok(lead, "create_task", json!({"title": title, "brief": "b"})),
            "task",
        );
        f.ok(who, "claim_task", json!({"task": task}));
        f.ok(who, "start_task", json!({"task": task}));
        task
    };
    let t1 = start("a", worker, &mut f);
    let t2 = start("b", worker2, &mut f);

    f.ok(
        worker,
        "block_on",
        json!({"task": t1, "waiting_for_task": t2}),
    );
    // This closes the loop, and the bot needs to know its situation changed.
    let message = f.ok(
        worker2,
        "block_on",
        json!({"task": t2, "waiting_for_task": t1}),
    );
    assert!(
        message.contains("circular wait"),
        "the bot must be told, or it will sit there: {message}"
    );
}

// --------------------------------------------------------- attaching work
//
// A board went from twelve objectives to ninety-six records because every
// review, fix and evidence gap became another top-level card. The tool now
// makes attaching the easy path, and refuses the silent fallback that made
// piling up the easy one.

use orgbot_proto::graph::{Lineage, TaskKind};

impl Fixture {
    /// Create a record through the tool, as a bot would, and recover its id
    /// from what the bot was told.
    fn make(&mut self, input: Value) -> String {
        let told = self.ok(self.lead, "create_task", input);
        id_in(&told, "task")
    }
}

/// A supporting record must say what it is about. Silently creating it as a
/// top-level card is the exact behaviour that inflated the board.
#[test]
fn a_review_without_a_subject_is_refused_with_instructions() {
    let mut f = fixture();
    let told = f.err(
        f.lead,
        "create_task",
        json!({ "title": "review the notes", "brief": "check them", "kind": "review" }),
    );
    assert!(
        told.contains("`of`"),
        "the refusal should say how to fix it: {told}"
    );
}

#[test]
fn a_review_attaches_to_the_task_it_reviews() {
    let mut f = fixture();
    let subject = f.make(json!({ "title": "write the notes", "brief": "write them" }));

    f.ok(
        f.lead,
        "create_task",
        json!({
            "title": "review the notes",
            "brief": "check them",
            "kind": "review",
            "of": subject,
        }),
    );

    let review = f
        .engine
        .tasks()
        .find(|t| t.kind == TaskKind::Review)
        .expect("the review exists");
    assert_eq!(review.lineage.review_of.unwrap().to_string(), subject);
    assert_eq!(review.lineage.parent.unwrap().to_string(), subject);
    assert!(
        !review.kind.is_headline(),
        "a review is not a headline card"
    );
}

/// However deep it sits, attached work knows which outcome it serves — so
/// progress can be counted against objectives rather than raw records.
#[test]
fn attached_work_inherits_the_objective_it_serves() {
    let mut f = fixture();
    let objective = f.make(json!({
        "title": "repair the harness",
        "brief": "the whole objective",
        "kind": "objective",
    }));
    let implementation = f.make(json!({
        "title": "implement it",
        "brief": "the work",
        "kind": "subtask",
        "of": objective,
    }));
    // Two levels down, and still attributed to the objective.
    f.ok(
        f.lead,
        "create_task",
        json!({
            "title": "review it",
            "brief": "check",
            "kind": "review",
            "of": implementation,
        }),
    );

    for task in f.engine.tasks() {
        if task.id.to_string() == objective {
            assert_eq!(
                task.lineage.root_objective, None,
                "an objective is its own root"
            );
        } else {
            assert_eq!(
                task.lineage.root_objective.map(|id| id.to_string()),
                Some(objective.clone()),
                "`{}` should serve the objective",
                task.title
            );
        }
    }
}

/// Only one record in that room is a headline, however many exist.
#[test]
fn one_objective_with_work_under_it_is_one_headline() {
    let mut f = fixture();
    let objective = f.make(json!({
        "title": "repair the harness",
        "brief": "everything",
        "kind": "objective",
    }));
    for i in 0..6 {
        f.make(json!({
            "title": format!("step {i}"),
            "brief": "work",
            "kind": "subtask",
            "of": objective,
        }));
    }

    let headlines = f.engine.tasks().filter(|t| t.kind.is_headline()).count();
    assert_eq!(headlines, 1, "seven records, one outcome");
    assert_eq!(
        f.engine.tasks().count(),
        7,
        "and all seven are still tracked"
    );
}

/// An objective belongs to nothing, by definition.
#[test]
fn an_objective_cannot_belong_to_another_task() {
    let mut f = fixture();
    let other = f.make(json!({ "title": "something else", "brief": "x" }));
    let told = f.err(
        f.lead,
        "create_task",
        json!({
            "title": "a second objective",
            "brief": "x",
            "kind": "objective",
            "of": other,
        }),
    );
    assert!(told.contains("top-level outcome"), "{told}");
}

/// An unrecognised kind gets the real list back, not a schema error.
#[test]
fn an_unknown_kind_lists_the_real_ones() {
    let mut f = fixture();
    let told = f.err(
        f.lead,
        "create_task",
        json!({ "title": "x", "brief": "y", "kind": "epic" }),
    );
    assert!(told.contains("remediation"), "{told}");
}

/// Plain tasks still work exactly as they did before any of this.
#[test]
fn an_ordinary_task_needs_no_kind_at_all() {
    let mut f = fixture();
    f.make(json!({ "title": "ordinary work", "brief": "do it" }));

    let task = f
        .engine
        .tasks()
        .find(|t| t.title == "ordinary work")
        .expect("task exists");
    assert_eq!(task.kind, TaskKind::Task);
    assert_eq!(task.lineage, Lineage::default());
}

// ------------------------------------------------- artifacts arrive whole
//
// The board's worst quiet failure: read_artifact returned a preview, so an
// inspector could read two thirds of an architecture document, see no sign of
// that, and approve it.

/// A long deliverable must announce that it is partial.
#[test]
fn a_long_artifact_is_handed_over_in_named_parts() {
    let mut f = fixture();
    let task = f.make(json!({ "title": "write the design", "brief": "long doc" }));
    f.ok(f.worker, "claim_task", json!({ "task": task }));
    f.ok(f.worker, "start_task", json!({ "task": task }));
    let told = f.ok(
        f.worker,
        "add_artifact",
        json!({
            "task": task,
            "name": "architecture.md",
            "content": "x".repeat(20_000),
        }),
    );
    let artifact = id_in(&told, "art");

    let read = f.ok(f.worker, "read_artifact", json!({ "artifact": artifact }));
    assert!(
        read.contains("BEGIN PART 1/3"),
        "got: {}",
        &read[..read.len().min(200)]
    );
    assert!(
        read.contains("not the whole document"),
        "a partial read must say so"
    );
    assert!(read.contains("part=2"), "and say how to continue");
    assert!(
        !read.contains('…'),
        "never a bare ellipsis standing in for the rest"
    );
}

/// The last part says it is the last, so a reader knows when to stop.
#[test]
fn the_final_part_closes_the_document() {
    let mut f = fixture();
    let task = f.make(json!({ "title": "write the design", "brief": "long doc" }));
    f.ok(f.worker, "claim_task", json!({ "task": task }));
    f.ok(f.worker, "start_task", json!({ "task": task }));
    let told = f.ok(
        f.worker,
        "add_artifact",
        json!({
            "task": task,
            "name": "architecture.md",
            "content": "x".repeat(20_000),
        }),
    );
    let artifact = id_in(&told, "art");

    let last = f.ok(
        f.worker,
        "read_artifact",
        json!({ "artifact": artifact, "part": 3 }),
    );
    assert!(last.contains("END PART 3/3"));
    assert!(last.contains("That was the last part"));
}

/// A short one still reads as a single whole document.
#[test]
fn a_short_artifact_reads_as_one_piece() {
    let mut f = fixture();
    let task = f.make(json!({ "title": "write a note", "brief": "short" }));
    f.ok(f.worker, "claim_task", json!({ "task": task }));
    f.ok(f.worker, "start_task", json!({ "task": task }));
    let told = f.ok(
        f.worker,
        "add_artifact",
        json!({ "task": task, "name": "notes.md", "content": "all of it" }),
    );
    let artifact = id_in(&told, "art");

    let read = f.ok(f.worker, "read_artifact", json!({ "artifact": artifact }));
    assert!(read.contains("BEGIN ("));
    assert!(read.contains("all of it"));
    assert!(!read.contains("PART"), "no part machinery for a short doc");
}

/// What is being inspected must not change under the inspector.
#[test]
fn submitting_for_review_freezes_the_artifacts() {
    let mut f = fixture();
    let task = f.make(json!({ "title": "write the notes", "brief": "do it" }));
    f.ok(f.worker, "claim_task", json!({ "task": task }));
    f.ok(f.worker, "start_task", json!({ "task": task }));
    let told = f.ok(
        f.worker,
        "add_artifact",
        json!({ "task": task, "name": "notes.md", "content": "v1" }),
    );
    let artifact = id_in(&told, "art");

    f.ok(
        f.worker,
        "submit_for_review",
        json!({ "task": task, "artifacts": [artifact.clone()] }),
    );

    let id: orgbot_proto::ArtifactId = artifact.parse().expect("an artifact id");
    let sealed = f.engine.artifact(id).expect("artifact exists");
    assert!(sealed.sealed, "the reviewed version is frozen");
}

// ------------------------------------------------- not making the same card twice
//
// A board grows fastest when several bots independently notice the same gap
// and each raise it, because nothing links the records afterwards.

#[test]
fn work_that_already_exists_is_pointed_at_rather_than_duplicated() {
    let mut f = fixture();
    f.make(json!({
        "title": "restore the missing B3-C source",
        "brief": "get it back",
    }));

    let told = f.err(
        f.lead,
        "create_task",
        json!({
            "title": "the B3-C source, restore it",
            "brief": "same thing, different words",
        }),
    );
    assert!(told.contains("already exists"), "{told}");
    assert!(
        told.contains("B3-C"),
        "it should name the existing task: {told}"
    );
    assert!(told.contains("supersede"), "and say what to do: {told}");
}

/// Genuinely separate work still gets through, once the caller says so.
#[test]
fn separate_work_can_be_created_deliberately() {
    let mut f = fixture();
    f.make(json!({"title": "restore the B3-C source", "brief": "x"}));

    f.ok(
        f.lead,
        "create_task",
        json!({
            "title": "restore the B3-C source",
            "brief": "a second, independent copy for the archive",
            "anyway": "true",
        }),
    );
    assert_eq!(f.engine.tasks().count(), 2);
}

/// Numbered work is not the same work.
#[test]
fn numbered_steps_are_not_duplicates_of_each_other() {
    let mut f = fixture();
    for i in 0..4 {
        f.make(json!({"title": format!("step {i}"), "brief": "work"}));
    }
    assert_eq!(f.engine.tasks().count(), 4);
}

/// Finished work does not block raising the same thing again later.
#[test]
fn a_delivered_task_does_not_block_new_work() {
    let mut f = fixture();
    let done = f.make(json!({"title": "restore the B3-C source", "brief": "x"}));
    let id: orgbot_proto::TaskId = done.parse().unwrap();
    f.engine
        .apply(Command::RetireTask {
            task: id,
            reason: "superseded by a new approach".into(),
        })
        .unwrap();

    f.ok(
        f.lead,
        "create_task",
        json!({"title": "restore the B3-C source", "brief": "again"}),
    );
}

/// One review finding must not spawn an open-ended chain of follow-ups.
#[test]
fn remediations_stop_multiplying_and_ask_to_be_consolidated() {
    let mut f = fixture();
    let subject = f.make(json!({"title": "the handler", "brief": "x"}));

    for i in 0..3 {
        f.ok(
            f.lead,
            "create_task",
            json!({
                "title": format!("fence the {i} case"),
                "brief": "patch",
                "kind": "remediation",
                "of": subject.clone(),
            }),
        );
    }

    let told = f.err(
        f.lead,
        "create_task",
        json!({
            "title": "fence the fourth case",
            "brief": "patch",
            "kind": "remediation",
            "of": subject,
        }),
    );
    assert!(told.contains("open remediations"), "{told}");
    assert!(told.contains("invariant they share"), "{told}");
}

// ------------------------------------------------------- artifact pagination
//
// Through `dispatch`, which is the path a bot actually takes. The reference
// tests beside the splitter check the splitting; these check that a request
// made the way a model makes one arrives at the splitter unchanged, which is
// where this went wrong: the schema declared `part` a string, the handler
// accepted only a number, and every value a model sent was silently discarded
// in favour of part 1. The splitter was correct the whole time and never saw
// the argument.

/// Create a task, claim it, and attach an artifact of `chars` characters.
fn artifact_of(f: &mut Fixture, chars: usize, title: &str) -> String {
    let worker = f.worker;
    // The board refuses work that looks like work it already has, which is
    // correct — so a test creating two tasks needs two genuinely different
    // subjects, not the same sentence with a number changed.
    let task = f.make(json!({"title": title, "brief": "a document to read back"}));
    f.ok(worker, "claim_task", json!({"task": task}));
    f.ok(worker, "start_task", json!({"task": task}));
    let body: String = (0..chars)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect();
    let recorded = f.ok(
        worker,
        "add_artifact",
        json!({"task": task, "name": "long.md", "content": body}),
    );
    id_in(&recorded, "art")
}

/// The reported defect, through the real tool: asking past the end must be an
/// error, never a repeat of the final chunk.
#[test]
fn reading_past_the_last_part_is_refused_not_answered_with_the_last_part() {
    let mut f = fixture();
    let worker = f.worker;
    let artifact = artifact_of(&mut f, 12_000, "the deployment runbook"); // two parts at 8k

    let first = f.ok(worker, "read_artifact", json!({"artifact": artifact}));
    assert!(
        first.contains("TRANSPORT part_index=1 total_parts=2"),
        "{first}"
    );

    let second = f.ok(
        worker,
        "read_artifact",
        json!({"artifact": artifact, "part": 2}),
    );
    assert!(
        second.contains("TRANSPORT part_index=2 total_parts=2"),
        "{second}"
    );
    assert!(second.contains("is_final_part=true"));

    let past = f.err(
        worker,
        "read_artifact",
        json!({"artifact": artifact, "part": 3}),
    );
    assert!(past.contains("PART_OUT_OF_RANGE"), "{past}");
    assert!(past.contains("requested=3"), "{past}");
    assert!(past.contains("total_parts=2"), "{past}");
    assert!(
        !past.contains("TRANSPORT"),
        "a refusal must not carry a chunk at all: {past}"
    );
}

/// The severe one: a `part` sent as a string used to be dropped, so every
/// request for a later part returned the first — and the tail of the document
/// was unreachable while looking, to the reader, like the end.
#[test]
fn a_part_number_sent_as_a_string_is_honoured_not_discarded() {
    let mut f = fixture();
    let worker = f.worker;
    let artifact = artifact_of(&mut f, 12_000, "the migration plan");

    let one = f.ok(
        worker,
        "read_artifact",
        json!({"artifact": artifact, "part": "1"}),
    );
    let two = f.ok(
        worker,
        "read_artifact",
        json!({"artifact": artifact, "part": "2"}),
    );

    assert!(one.contains("part_index=1"), "{one}");
    assert!(
        two.contains("part_index=2"),
        "a string part must not fall back to part 1: {two}"
    );
    assert_ne!(
        one, two,
        "part 1 and part 2 must not return byte-identical payloads"
    );

    // And out of range is still refused when it arrives as a string.
    let past = f.err(
        worker,
        "read_artifact",
        json!({"artifact": artifact, "part": "9"}),
    );
    assert!(past.contains("PART_OUT_OF_RANGE"), "{past}");
}

/// Anything that is not a part number is refused out loud. What must never
/// happen is an argument being supplied and quietly ignored.
#[test]
fn an_unusable_part_argument_is_refused_rather_than_defaulted() {
    let mut f = fixture();
    let worker = f.worker;
    let artifact = artifact_of(&mut f, 12_000, "the incident timeline");

    for bad in [
        json!(0),
        json!(-1),
        json!("two"),
        json!(1.5),
        json!(true),
        json!([2]),
    ] {
        let out = f.call(
            worker,
            "read_artifact",
            json!({"artifact": artifact, "part": bad}),
        );
        assert!(
            out.is_error,
            "part={bad} should be refused, got: {}",
            out.content
        );
        assert!(
            out.content.contains("PART_INVALID") || out.content.contains("PART_OUT_OF_RANGE"),
            "part={bad} needs a reason code, got: {}",
            out.content
        );
    }

    // An absent part is not an error: it means the first.
    let default = f.ok(worker, "read_artifact", json!({"artifact": artifact}));
    assert!(default.contains("part_index=1"), "{default}");
    let null = f.ok(
        worker,
        "read_artifact",
        json!({"artifact": artifact, "part": null}),
    );
    assert!(null.contains("part_index=1"), "{null}");
}

/// Every part is identified by something the author did not write.
///
/// A document can say "PART 2/2" in its own text and be wrong, or be lying.
/// The transport line is the only claim about which chunk this is that the
/// author had no hand in.
#[test]
fn each_part_carries_transport_owned_identity_a_reader_can_check() {
    let mut f = fixture();
    let worker = f.worker;
    let artifact = artifact_of(&mut f, 20_000, "the architecture review"); // three parts

    let mut digests = Vec::new();
    let mut reassembled = 0usize;
    for part in 1..=3 {
        let out = f.ok(
            worker,
            "read_artifact",
            json!({"artifact": artifact, "part": part}),
        );
        assert!(
            out.contains(&format!("part_index={part} total_parts=3")),
            "{out}"
        );
        assert!(out.contains("artifact_sha256="), "{out}");
        let digest = out
            .split("chunk_sha256=")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .expect("a chunk digest")
            .to_string();
        let bytes: usize = out
            .split("bytes=")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .expect("a byte count");
        reassembled += bytes;
        digests.push(digest);
        assert_eq!(
            out.contains("is_final_part=true"),
            part == 3,
            "only the last part may claim to be the last: {out}"
        );
    }
    assert_eq!(
        reassembled, 20_000,
        "the parts account for the whole document"
    );
    digests.sort();
    digests.dedup();
    assert_eq!(digests.len(), 3, "no two parts may present the same digest");
}

/// The first read hands over the reading order, so a reader knows what the
/// complete set is before deciding they have all of it.
#[test]
fn the_first_part_comes_with_the_manifest_for_the_whole_document() {
    let mut f = fixture();
    let worker = f.worker;
    let artifact = artifact_of(&mut f, 20_000, "the security assessment");

    let first = f.ok(worker, "read_artifact", json!({"artifact": artifact}));
    assert!(first.contains("MANIFEST"), "{first}");
    assert!(first.contains("3 parts"), "{first}");

    // A document that arrives whole has no reading order to publish.
    let short = artifact_of(&mut f, 200, "a one line changelog");
    let whole = f.ok(worker, "read_artifact", json!({"artifact": short}));
    assert!(!whole.contains("MANIFEST"), "{whole}");
    assert!(whole.contains("total_parts=1"), "{whole}");
}

// ------------------------------------------------------------------- skills
//
// A room of capable models still does poor work if every bot reasons out how
// to approach a problem from scratch every time. A longer system prompt is the
// wrong fix — it is paid for on every request by every bot whether the advice
// applies or not. So a skill's one-line description is always in view and its
// body is fetched only by the bot that decided it applied.

fn with_skills(f: &mut Fixture, skills: Vec<(&str, &str, &str)>) {
    f.engine.set_skills(
        skills
            .into_iter()
            .map(|(name, description, body)| orgbot_proto::skill::Skill {
                name: name.into(),
                description: description.into(),
                body: body.into(),
            })
            .collect(),
    );
}

#[test]
fn a_bot_can_load_a_playbook_by_name() {
    let mut f = fixture();
    with_skills(
        &mut f,
        vec![(
            "ponytail",
            "the laziest thing that works",
            "Question the task.",
        )],
    );
    let out = f.ok(f.worker, "use_skill", json!({"skill": "ponytail"}));
    assert!(out.contains("Question the task."), "{out}");
}

/// The name is how a model refers to it, and models are inconsistent about case.
#[test]
fn asking_for_a_skill_is_not_case_sensitive() {
    let mut f = fixture();
    with_skills(&mut f, vec![("ponytail", "d", "body text")]);
    assert!(f
        .ok(f.worker, "use_skill", json!({"skill": "Ponytail"}))
        .contains("body text"));
}

/// A refusal names what there is, so the next call succeeds.
#[test]
fn asking_for_a_skill_that_is_not_there_says_what_is() {
    let mut f = fixture();
    with_skills(&mut f, vec![("ponytail", "d", "b"), ("skillops", "d", "b")]);
    let out = f.err(f.worker, "use_skill", json!({"skill": "nonesuch"}));
    assert!(
        out.contains("ponytail") && out.contains("skillops"),
        "{out}"
    );
}

#[test]
fn a_room_with_no_skills_says_so_plainly() {
    let mut f = fixture();
    let out = f.err(f.worker, "use_skill", json!({"skill": "anything"}));
    assert!(out.contains("no skills"), "{out}");
}
