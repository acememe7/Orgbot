//! End-to-end room behaviour.
//!
//! These drive the engine exactly as a bot's tool calls would, and assert on
//! the rules that matter when several bots share one board: only the assignee
//! acts on a task, nobody inspects their own work, a blocked sender is woken
//! when its handoff settles, and a mutual wait is detected instead of hanging.

use orgbot_engine::{Command, EngineError};
use orgbot_engine::engine::Engine;
use orgbot_proto::agent::{Autonomy, ProviderRef};
use orgbot_proto::artifact::ArtifactBody;
use orgbot_proto::event::{DeadlockResolution, Event, EventEnvelope};
use orgbot_proto::handoff::{HandoffKind, HandoffState, Recipient};
use orgbot_proto::review::Finding;
use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
use orgbot_proto::task::{BlockReason, Priority, TaskState};
use orgbot_proto::{
    AgentId, AgentSpec, ArtifactId, ChatMessage, HandoffId, MessageIntent, TaskId,
};
use orgbot_proto::Timestamp;

// ------------------------------------------------------------------ harness

/// A room with a coordinator, two workers, and a dedicated inspector.
struct Fixture {
    engine: Engine,
    coordinator: AgentId,
    worker: AgentId,
    worker2: AgentId,
    inspector: AgentId,
}

fn provider() -> ProviderRef {
    ProviderRef::Scripted {
        script: "test".into(),
    }
}

fn fixture() -> Fixture {
    let mut engine = Engine::new(RoomSpec {
        name: "release".into(),
        goal: "ship the release notes".into(),
        roster: vec![],
        completion: CompletionRule::AllTasksTerminal,
        limits: RoomLimits::default(),
    });

    let coordinator = AgentSpec::new("lead", "Lead", provider())
        .with_roles(["coordinator"])
        .with_autonomy(Autonomy::coordinator());
    let worker = AgentSpec::new("w1", "Worker One", provider()).with_roles(["writer"]);
    let worker2 = AgentSpec::new("w2", "Worker Two", provider()).with_roles(["writer"]);
    let inspector = AgentSpec::new("qa", "Inspector", provider())
        .with_roles(["reviewer"])
        .with_autonomy(Autonomy::inspector());

    let ids = (coordinator.id, worker.id, worker2.id, inspector.id);
    for spec in [coordinator, worker, worker2, inspector] {
        engine
            .apply(Command::JoinAgent {
                spec: Box::new(spec),
            })
            .expect("join");
    }
    engine.apply(Command::StartRoom).expect("start");

    Fixture {
        engine,
        coordinator: ids.0,
        worker: ids.1,
        worker2: ids.2,
        inspector: ids.3,
    }
}

impl Fixture {
    fn apply(&mut self, command: Command) -> Vec<EventEnvelope> {
        self.engine
            .apply(command.clone())
            .unwrap_or_else(|e| panic!("`{}` was rejected: {e}", command.name()))
    }

    fn reject(&mut self, command: Command) -> EngineError {
        let name = command.name();
        self.engine
            .apply(command)
            .expect_err(&format!("`{name}` should have been rejected"))
    }

    /// Create an open, claimable task.
    fn open_task(&mut self, title: &str, role: Option<&str>) -> TaskId {
        let events = self.apply(Command::CreateTask {
            title: title.into(),
            brief: format!("please do: {title}"),
            by: Some(self.coordinator),
            required_role: role.map(Into::into),
            priority: Priority::Normal,
            acceptance_criteria: vec!["it is correct".into()],
            depends_on: vec![],
            origin: TaskOrigin::plain(),
        });
        let task = created_task(&events);
        self.apply(Command::OpenTask { task });
        task
    }

    /// Take a task all the way to `InProgress` under `agent`.
    fn start_task(&mut self, title: &str, agent: AgentId) -> TaskId {
        let task = self.open_task(title, None);
        self.apply(Command::ClaimTask { task, by: agent });
        self.apply(Command::StartTask { task, by: agent });
        task
    }

    fn add_artifact(&mut self, task: TaskId, author: AgentId) -> ArtifactId {
        let events = self.apply(Command::AddArtifact {
            task,
            author,
            name: "notes.md".into(),
            body: ArtifactBody::Text {
                text: "the release notes".into(),
            },
            authority_bearing: false,
        });
        added_artifact(&events)
    }

    fn state(&self, task: TaskId) -> TaskState {
        self.engine.task(task).expect("task exists").state
    }
}

fn created_task(events: &[EventEnvelope]) -> TaskId {
    events
        .iter()
        .find_map(|e| match &e.event {
            Event::TaskCreated { task } => Some(task.id),
            _ => None,
        })
        .expect("a task_created event")
}

fn added_artifact(events: &[EventEnvelope]) -> ArtifactId {
    events
        .iter()
        .find_map(|e| match &e.event {
            Event::ArtifactAdded { artifact } => Some(artifact.id),
            _ => None,
        })
        .expect("an artifact_added event")
}

fn offered_handoff(events: &[EventEnvelope]) -> HandoffId {
    events
        .iter()
        .find_map(|e| match &e.event {
            Event::HandoffOffered { handoff } => Some(handoff.id),
            _ => None,
        })
        .expect("a handoff_offered event")
}

fn deadlocks(events: &[EventEnvelope]) -> Vec<(Vec<TaskId>, DeadlockResolution)> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            Event::DeadlockDetected {
                cycle, resolution, ..
            } => Some((cycle.clone(), resolution.clone())),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------- the main happy path

#[test]
fn a_task_is_worked_inspected_and_submitted() {
    let mut f = fixture();
    let (worker, inspector, lead) = (f.worker, f.inspector, f.coordinator);

    let task = f.start_task("write the release notes", worker);
    assert_eq!(f.state(task), TaskState::InProgress);

    let artifact = f.add_artifact(task, worker);
    f.apply(Command::SubmitForReview {
        task,
        by: worker,
        artifacts: vec![artifact],
    });
    assert_eq!(f.state(task), TaskState::InReview);

    // A clean inspection approves and moves the task on by itself.
    f.apply(Command::RecordReview {
        task,
        inspector,
        findings: vec![],
        summary: "matches the criteria".into(),
        criteria_met: vec!["it is correct".into()],
        criteria_failed: vec![],
    });
    assert_eq!(f.state(task), TaskState::Approved);

    f.apply(Command::SubmitTask { task, by: lead });
    assert_eq!(f.state(task), TaskState::Submitted);
}

#[test]
fn a_blocking_finding_sends_the_work_back_and_costs_a_revision() {
    let mut f = fixture();
    let (worker, inspector) = (f.worker, f.inspector);

    let task = f.start_task("write the release notes", worker);
    let artifact = f.add_artifact(task, worker);
    f.apply(Command::SubmitForReview {
        task,
        by: worker,
        artifacts: vec![artifact],
    });
    f.apply(Command::RecordReview {
        task,
        inspector,
        findings: vec![Finding::major("the version number is wrong")],
        summary: "needs another pass".into(),
        criteria_met: vec![],
        criteria_failed: vec!["it is correct".into()],
    });

    assert_eq!(f.state(task), TaskState::InProgress, "rework returns to the author");
    assert_eq!(f.engine.task(task).unwrap().revision, 1);
}

// --------------------------------------------------- separation of duties

#[test]
fn a_bot_cannot_inspect_its_own_work() {
    let mut f = fixture();
    // A bot that is allowed to inspect in general, holding the task itself.
    let solo = AgentSpec::new("solo", "Solo", provider())
        .with_roles(["writer", "reviewer"])
        .with_autonomy(Autonomy::inspector());
    let solo_id = solo.id;
    f.apply(Command::JoinAgent {
        spec: Box::new(solo),
    });

    let task = f.start_task("write the release notes", solo_id);
    let artifact = f.add_artifact(task, solo_id);
    f.apply(Command::SubmitForReview {
        task,
        by: solo_id,
        artifacts: vec![artifact],
    });

    let err = f.reject(Command::RecordReview {
        task,
        inspector: solo_id,
        findings: vec![],
        summary: "looks great to me".into(),
        criteria_met: vec![],
        criteria_failed: vec![],
    });
    assert_eq!(
        err,
        EngineError::SelfInspection {
            task,
            agent: solo_id
        }
    );
    assert_eq!(f.state(task), TaskState::InReview, "the task must not advance");
}

#[test]
fn a_worker_cannot_record_a_review_at_all() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.start_task("write the release notes", worker);
    let artifact = f.add_artifact(task, worker);
    f.apply(Command::SubmitForReview {
        task,
        by: worker,
        artifacts: vec![artifact],
    });

    let err = f.reject(Command::RecordReview {
        task,
        inspector: worker2,
        findings: vec![],
        summary: "fine by me".into(),
        criteria_met: vec![],
        criteria_failed: vec![],
    });
    assert!(matches!(err, EngineError::NotPermitted { .. }));
}

#[test]
fn an_inspector_cannot_perform_the_final_submit() {
    let mut f = fixture();
    let (worker, inspector) = (f.worker, f.inspector);
    let task = f.start_task("write the release notes", worker);
    let artifact = f.add_artifact(task, worker);
    f.apply(Command::SubmitForReview {
        task,
        by: worker,
        artifacts: vec![artifact],
    });
    f.apply(Command::RecordReview {
        task,
        inspector,
        findings: vec![],
        summary: "good".into(),
        criteria_met: vec![],
        criteria_failed: vec![],
    });

    let err = f.reject(Command::SubmitTask {
        task,
        by: inspector,
    });
    assert!(matches!(err, EngineError::NotPermitted { .. }));
    assert_eq!(f.state(task), TaskState::Approved);
}

#[test]
fn only_the_assignee_may_act_on_a_task() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.start_task("write the release notes", worker);

    let err = f.reject(Command::SubmitForReview {
        task,
        by: worker2,
        artifacts: vec![],
    });
    assert!(matches!(err, EngineError::NotAssignee { .. }));
}

#[test]
fn a_task_cannot_be_claimed_by_a_bot_without_the_role() {
    let mut f = fixture();
    let inspector = f.inspector;
    let task = f.open_task("write the release notes", Some("writer"));

    let err = f.reject(Command::ClaimTask {
        task,
        by: inspector,
    });
    assert!(matches!(err, EngineError::RoleMismatch { .. }));
}

#[test]
fn two_bots_cannot_claim_the_same_task() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.open_task("write the release notes", None);

    f.apply(Command::ClaimTask { task, by: worker });
    let err = f.reject(Command::ClaimTask {
        task,
        by: worker2,
    });
    assert!(matches!(err, EngineError::Transition(_)));
    assert_eq!(f.engine.task(task).unwrap().assignee, Some(worker));
}

#[test]
fn work_cannot_reach_submitted_without_passing_inspection() {
    let mut f = fixture();
    let (worker, lead) = (f.worker, f.coordinator);
    let task = f.start_task("write the release notes", worker);

    let err = f.reject(Command::SubmitTask { task, by: lead });
    assert!(matches!(err, EngineError::Transition(_)));
    assert_eq!(f.state(task), TaskState::InProgress);
}

// ------------------------------------------------------------------ handoffs

#[test]
fn a_blocking_handoff_parks_the_sender_and_completing_it_wakes_them() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.start_task("write the release notes", worker);

    let events = f.apply(Command::OfferHandoff {
        task,
        from: worker,
        to: Recipient::Agent { agent: worker2 },
        kind: HandoffKind::Assist,
        brief: "pull the changelog since v1.2".into(),
        expects: vec!["a bullet list".into()],
        context: vec![],
        blocks_sender: None,
        timeout_ms: None,
    });
    let handoff = offered_handoff(&events);
    assert_eq!(
        f.state(task),
        TaskState::Blocked,
        "an assist blocks the sender without it having to remember to"
    );

    f.apply(Command::AcceptHandoff {
        handoff,
        by: worker2,
    });
    assert_eq!(f.state(task), TaskState::Blocked, "still waiting");

    f.apply(Command::CompleteHandoff {
        handoff,
        by: worker2,
        artifacts: vec![],
        summary: "12 entries since v1.2".into(),
    });
    assert_eq!(
        f.state(task),
        TaskState::InProgress,
        "completing the handoff wakes the sender"
    );
}

#[test]
fn a_rejected_handoff_also_wakes_the_sender() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.start_task("write the release notes", worker);

    let events = f.apply(Command::OfferHandoff {
        task,
        from: worker,
        to: Recipient::Agent { agent: worker2 },
        kind: HandoffKind::Assist,
        brief: "help".into(),
        expects: vec![],
        context: vec![],
        blocks_sender: None,
        timeout_ms: None,
    });
    let handoff = offered_handoff(&events);
    assert_eq!(f.state(task), TaskState::Blocked);

    f.apply(Command::RejectHandoff {
        handoff,
        by: worker2,
        reason: "not my area".into(),
    });
    assert_eq!(
        f.state(task),
        TaskState::InProgress,
        "a refusal must not strand the sender"
    );
}

#[test]
fn an_unanswered_handoff_expires_and_frees_the_sender() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.start_task("write the release notes", worker);

    let events = f.apply(Command::OfferHandoff {
        task,
        from: worker,
        to: Recipient::Agent { agent: worker2 },
        kind: HandoffKind::Assist,
        brief: "help".into(),
        expects: vec![],
        context: vec![],
        blocks_sender: None,
        timeout_ms: Some(1_000),
    });
    let handoff = offered_handoff(&events);
    assert_eq!(f.state(task), TaskState::Blocked);

    // Before the deadline nothing happens.
    f.apply(Command::Tick {
        now: Timestamp::now(),
    });
    assert_eq!(f.state(task), TaskState::Blocked);

    f.apply(Command::Tick {
        now: Timestamp::now().plus_millis(2_000),
    });
    assert_eq!(
        f.engine.handoff(handoff).unwrap().state,
        HandoffState::Expired
    );
    assert_eq!(f.state(task), TaskState::InProgress);
}

#[test]
fn a_delegation_moves_the_task_to_whoever_accepts() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.start_task("write the release notes", worker);

    let events = f.apply(Command::OfferHandoff {
        task,
        from: worker,
        to: Recipient::Agent { agent: worker2 },
        kind: HandoffKind::Delegate,
        brief: "you own this now".into(),
        expects: vec![],
        context: vec![],
        blocks_sender: None,
        timeout_ms: None,
    });
    let handoff = offered_handoff(&events);
    assert_eq!(
        f.state(task),
        TaskState::InProgress,
        "a delegation does not block the sender"
    );

    f.apply(Command::AcceptHandoff {
        handoff,
        by: worker2,
    });
    assert_eq!(f.engine.task(task).unwrap().assignee, Some(worker2));
}

#[test]
fn a_role_addressed_offer_can_be_taken_by_any_holder() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.start_task("write the release notes", worker);

    let events = f.apply(Command::OfferHandoff {
        task,
        from: worker,
        to: Recipient::Role {
            role: "writer".into(),
        },
        kind: HandoffKind::Assist,
        brief: "anyone free?".into(),
        expects: vec![],
        context: vec![],
        blocks_sender: None,
        timeout_ms: None,
    });
    let handoff = offered_handoff(&events);

    assert!(f.engine.offers_for(worker2).contains(&handoff));
    assert!(
        !f.engine.offers_for(f.inspector).contains(&handoff),
        "a bot without the role sees no offer"
    );
    f.apply(Command::AcceptHandoff {
        handoff,
        by: worker2,
    });
    assert_eq!(f.engine.handoff(handoff).unwrap().accepted_by, Some(worker2));
}

#[test]
fn a_bot_cannot_hand_off_to_itself() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.start_task("write the release notes", worker);

    let err = f.reject(Command::OfferHandoff {
        task,
        from: worker,
        to: Recipient::Agent { agent: worker },
        kind: HandoffKind::Assist,
        brief: "help me".into(),
        expects: vec![],
        context: vec![],
        blocks_sender: None,
        timeout_ms: None,
    });
    assert_eq!(err, EngineError::SelfHandoff);
}

#[test]
fn fan_out_is_capped_by_the_bots_handoff_budget() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let limit = f.engine.agent(worker).unwrap().autonomy.max_open_handoffs;

    // Non-blocking delegations, so the sender stays free to keep offering.
    let task = f.start_task("write the release notes", worker);
    for _ in 0..limit {
        f.apply(Command::OfferHandoff {
            task,
            from: worker,
            to: Recipient::Agent { agent: worker2 },
            kind: HandoffKind::Delegate,
            brief: "take a piece".into(),
            expects: vec![],
            context: vec![],
            blocks_sender: Some(false),
            timeout_ms: None,
        });
    }
    let err = f.reject(Command::OfferHandoff {
        task,
        from: worker,
        to: Recipient::Agent { agent: worker2 },
        kind: HandoffKind::Delegate,
        brief: "one too many".into(),
        expects: vec![],
        context: vec![],
        blocks_sender: Some(false),
        timeout_ms: None,
    });
    assert!(matches!(err, EngineError::TooManyOpenHandoffs { .. }));
}

// ------------------------------------------------------------------ deadlock

#[test]
fn a_mutual_wait_is_detected_and_broken() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);

    let t1 = f.start_task("write the notes", worker);
    let t2 = f.start_task("collect the metrics", worker2);

    // w1 parks on w2...
    f.apply(Command::OfferHandoff {
        task: t1,
        from: worker,
        to: Recipient::Agent { agent: worker2 },
        kind: HandoffKind::Assist,
        brief: "need the metrics".into(),
        expects: vec![],
        context: vec![],
        blocks_sender: None,
        timeout_ms: None,
    });
    assert_eq!(f.state(t1), TaskState::Blocked);

    // ...and w2 parks on w1. Nothing errors; both are politely waiting.
    let events = f.apply(Command::OfferHandoff {
        task: t2,
        from: worker2,
        to: Recipient::Agent { agent: worker },
        kind: HandoffKind::Assist,
        brief: "need the notes".into(),
        expects: vec![],
        context: vec![],
        blocks_sender: None,
        timeout_ms: None,
    });

    let found = deadlocks(&events);
    assert_eq!(found.len(), 1, "the cycle should be reported exactly once");
    let (cycle, resolution) = &found[0];
    assert_eq!(cycle.len(), 2);
    assert!(cycle.contains(&t1) && cycle.contains(&t2));
    assert!(
        matches!(resolution, DeadlockResolution::WithdrewHandoff { .. }),
        "withdrawing the blocking handoff is preferred over dropping the task, got {resolution:?}"
    );

    // Exactly one side is freed, and it keeps its task rather than losing it.
    let states = [f.state(t1), f.state(t2)];
    assert!(
        states.contains(&TaskState::InProgress) && states.contains(&TaskState::Blocked),
        "one side must be freed to break the jam, got {states:?}"
    );
    let freed = if f.state(t1) == TaskState::InProgress { t1 } else { t2 };
    assert!(
        f.engine.task(freed).unwrap().assignee.is_some(),
        "the freed task stays with its owner"
    );
    assert!(!f.engine.wait_graph().has_cycle());
}

#[test]
fn a_dependency_cycle_between_tasks_is_broken_by_release() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let t1 = f.start_task("a", worker);
    let t2 = f.start_task("b", worker2);

    f.apply(Command::BlockTask {
        task: t1,
        by: worker,
        reason: BlockReason::AwaitingTask { task: t2 },
    });
    let events = f.apply(Command::BlockTask {
        task: t2,
        by: worker2,
        reason: BlockReason::AwaitingTask { task: t1 },
    });

    let found = deadlocks(&events);
    assert_eq!(found.len(), 1);
    assert!(
        matches!(found[0].1, DeadlockResolution::ReleasedTask { .. }),
        "with no handoff to withdraw, releasing is the fallback"
    );
    assert!(!f.engine.wait_graph().has_cycle());
}

#[test]
fn independent_waits_are_not_reported_as_deadlocks() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let t1 = f.start_task("a", worker);
    let _t2 = f.start_task("b", worker2);

    // A one-way wait is ordinary collaboration, not a jam.
    let events = f.apply(Command::OfferHandoff {
        task: t1,
        from: worker,
        to: Recipient::Agent { agent: worker2 },
        kind: HandoffKind::Assist,
        brief: "need input".into(),
        expects: vec![],
        context: vec![],
        blocks_sender: None,
        timeout_ms: None,
    });
    assert!(deadlocks(&events).is_empty());
    assert!(!f.engine.wait_graph().has_cycle());
}

// ------------------------------------------------------------ fault handling

#[test]
fn a_bot_that_keeps_failing_is_faulted_and_gives_its_work_back() {
    let mut f = fixture();
    let worker = f.worker;
    let limit = f.engine.room().spec.limits.max_consecutive_errors;
    let task = f.start_task("write the release notes", worker);

    for _ in 0..limit {
        f.apply(Command::ReportError {
            agent: worker,
            task: Some(task),
            error: "provider timed out".into(),
        });
    }

    assert_eq!(
        f.engine.status(worker),
        orgbot_proto::AgentStatus::Faulted
    );
    assert_eq!(
        f.state(task),
        TaskState::Ready,
        "a faulted bot's task returns to the pool"
    );
    assert!(f.engine.task(task).unwrap().assignee.is_none());
    f.apply(Command::PauseRoom { reason: "retry after reconnecting".into() });
    let events = f.apply(Command::ResumeRoom);
    assert_eq!(f.engine.status(worker), orgbot_proto::AgentStatus::Idle);
    assert_eq!(f.engine.error_streak(worker), 0);
    assert_eq!(f.state(task), TaskState::Ready);
    assert!(events.iter().any(|e| matches!(e.event, Event::AgentStatusChanged { agent, to: orgbot_proto::AgentStatus::Idle, .. } if agent == worker)));
    let restored = Engine::from_archive(f.engine.room().clone(), f.engine.log().to_vec()).unwrap();
    assert_eq!(restored.error_streak(worker), 0);
    f.apply(Command::ClaimTask { task, by: worker });
}

#[test]
fn a_bot_leaving_returns_its_work_and_withdraws_its_requests() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.start_task("write the release notes", worker);

    let events = f.apply(Command::OfferHandoff {
        task,
        from: worker,
        to: Recipient::Agent { agent: worker2 },
        kind: HandoffKind::Assist,
        brief: "help".into(),
        expects: vec![],
        context: vec![],
        blocks_sender: None,
        timeout_ms: None,
    });
    let handoff = offered_handoff(&events);

    f.apply(Command::LeaveAgent {
        agent: worker,
        reason: "shutdown".into(),
    });

    assert_eq!(f.state(task), TaskState::Ready);
    assert_eq!(
        f.engine.handoff(handoff).unwrap().state,
        HandoffState::Withdrawn,
        "nobody should be left waiting on a bot that has gone"
    );
}

#[test]
fn rework_stops_once_the_revision_budget_is_spent() {
    let mut f = fixture();
    let (worker, inspector) = (f.worker, f.inspector);
    let task = f.start_task("write the release notes", worker);
    let budget = f.engine.task(task).unwrap().max_revisions;

    // Each round: resubmit, get a blocking finding back.
    for round in 0..budget {
        let artifact = f.add_artifact(task, worker);
        f.apply(Command::SubmitForReview {
            task,
            by: worker,
            artifacts: vec![artifact],
        });
        f.apply(Command::RecordReview {
            task,
            inspector,
            findings: vec![Finding::major("still wrong")],
            summary: "again".into(),
            criteria_met: vec![],
            criteria_failed: vec![],
        });
        assert_eq!(
            f.state(task),
            TaskState::InProgress,
            "round {round} should still allow rework"
        );
    }

    // One more rejection exhausts the budget.
    let artifact = f.add_artifact(task, worker);
    f.apply(Command::SubmitForReview {
        task,
        by: worker,
        artifacts: vec![artifact],
    });
    f.apply(Command::RecordReview {
        task,
        inspector,
        findings: vec![Finding::major("still wrong")],
        summary: "giving up".into(),
        criteria_met: vec![],
        criteria_failed: vec![],
    });
    assert_eq!(
        f.state(task),
        TaskState::Failed,
        "a worker and an inspector who disagree must not loop forever"
    );
}

// ------------------------------------------------------- dependencies & board

#[test]
fn a_task_cannot_start_before_its_dependency_is_submitted() {
    let mut f = fixture();
    let (worker, worker2, inspector, lead) =
        (f.worker, f.worker2, f.inspector, f.coordinator);

    let first = f.open_task("gather input", None);
    let events = f.apply(Command::CreateTask {
        title: "write it up".into(),
        brief: "needs the input".into(),
        by: Some(lead),
        required_role: None,
        priority: Priority::Normal,
        acceptance_criteria: vec![],
        depends_on: vec![first],
        origin: TaskOrigin::plain(),
    });
    let second = created_task(&events);
    f.apply(Command::OpenTask { task: second });

    let err = f.reject(Command::ClaimTask {
        task: second,
        by: worker2,
    });
    assert!(matches!(err, EngineError::DependencyNotMet { .. }));
    assert!(
        !f.engine.claimable_by(worker2).contains(&second),
        "a blocked-by-dependency task must not appear in the claimable list"
    );

    // Drive the dependency to Submitted, then the second task opens up.
    f.apply(Command::ClaimTask { task: first, by: worker });
    f.apply(Command::StartTask { task: first, by: worker });
    let artifact = f.add_artifact(first, worker);
    f.apply(Command::SubmitForReview {
        task: first,
        by: worker,
        artifacts: vec![artifact],
    });
    f.apply(Command::RecordReview {
        task: first,
        inspector,
        findings: vec![],
        summary: "good".into(),
        criteria_met: vec![],
        criteria_failed: vec![],
    });
    f.apply(Command::SubmitTask {
        task: first,
        by: lead,
    });

    assert!(f.engine.claimable_by(worker2).contains(&second));
    f.apply(Command::ClaimTask {
        task: second,
        by: worker2,
    });
}

#[test]
fn the_claimable_list_is_ordered_by_priority_then_age() {
    let mut f = fixture();
    let (worker, lead) = (f.worker, f.coordinator);

    let mut make = |title: &str, priority: Priority, f: &mut Fixture| {
        let events = f.apply(Command::CreateTask {
            title: title.into(),
            brief: String::new(),
            by: Some(lead),
            required_role: None,
            priority,
            acceptance_criteria: vec![],
            depends_on: vec![],
            origin: TaskOrigin::plain(),
        });
        let task = created_task(&events);
        f.apply(Command::OpenTask { task });
        task
    };
    let normal = make("normal", Priority::Normal, &mut f);
    let urgent = make("urgent", Priority::Urgent, &mut f);
    let low = make("low", Priority::Low, &mut f);

    assert_eq!(f.engine.claimable_by(worker), vec![urgent, normal, low]);
}

#[test]
fn the_review_queue_excludes_the_inspectors_own_work() {
    let mut f = fixture();
    let (worker, inspector) = (f.worker, f.inspector);

    let theirs = f.start_task("someone else's task", worker);
    let artifact = f.add_artifact(theirs, worker);
    f.apply(Command::SubmitForReview {
        task: theirs,
        by: worker,
        artifacts: vec![artifact],
    });
    assert_eq!(f.engine.review_queue_for(inspector), vec![theirs]);

    // A worker has no review queue at all.
    assert!(f.engine.review_queue_for(worker).is_empty());
}

// ----------------------------------------------------------------- the log

#[test]
fn the_event_log_is_gapless_and_replayable_from_any_point() {
    let mut f = fixture();
    let (worker, inspector, lead) = (f.worker, f.inspector, f.coordinator);

    let task = f.start_task("write the release notes", worker);
    let artifact = f.add_artifact(task, worker);
    f.apply(Command::SubmitForReview {
        task,
        by: worker,
        artifacts: vec![artifact],
    });
    f.apply(Command::RecordReview {
        task,
        inspector,
        findings: vec![],
        summary: "good".into(),
        criteria_met: vec![],
        criteria_failed: vec![],
    });
    f.apply(Command::SubmitTask { task, by: lead });

    let log = f.engine.log();
    assert!(log.len() > 10);
    for (index, envelope) in log.iter().enumerate() {
        assert_eq!(
            envelope.seq.0 as usize,
            index + 1,
            "sequence numbers must be gapless"
        );
        assert_eq!(envelope.room, f.engine.room_id());
    }

    // A client that saw the first half can catch up with the rest.
    let midpoint = log[log.len() / 2].seq;
    let tail: Vec<_> = f.engine.events_since(midpoint).collect();
    assert_eq!(tail.len(), log.len() - (midpoint.0 as usize));
    assert!(tail.iter().all(|e| e.seq > midpoint));

    // And every event survives the wire format.
    for envelope in log {
        let line = envelope.to_jsonl().expect("serialize");
        assert_eq!(&EventEnvelope::from_jsonl(&line).expect("parse"), envelope);
    }
}

#[test]
fn a_direct_question_wakes_only_its_target() {
    let mut f = fixture();
    let (worker, worker2, room) = (f.worker, f.worker2, f.engine.room_id());

    let message = ChatMessage::say(room, worker, "what's the version number?")
        .to_agents(vec![worker2])
        .with_intent(MessageIntent::Ask);
    f.apply(Command::PostMessage {
        message: Box::new(message.clone()),
    });

    assert_eq!(f.engine.transcript().len(), 1);
    assert!(message.wakes(worker2, &["writer".into()]));
    assert!(!message.wakes(f.inspector, &["reviewer".into()]));
}

#[test]
fn commands_are_rejected_while_the_room_is_paused() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.open_task("write the release notes", None);

    f.apply(Command::PauseRoom {
        reason: "operator stepped in".into(),
    });
    let err = f.reject(Command::ClaimTask { task, by: worker });
    assert!(matches!(err, EngineError::RoomNotRunning { .. }));

    f.apply(Command::ResumeRoom);
    f.apply(Command::ClaimTask { task, by: worker });
}

#[test]
fn a_rejected_command_changes_nothing() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.start_task("write the release notes", worker);

    let before = f.engine.head();
    let snapshot = f.engine.task(task).cloned();

    f.reject(Command::SubmitForReview {
        task,
        by: worker2,
        artifacts: vec![],
    });

    assert_eq!(f.engine.head(), before, "a refusal must not append events");
    assert_eq!(f.engine.task(task).cloned(), snapshot);
}

// ------------------------------------------------------- the work graph
//
// A sixteen-bot run put ninety-six records on a board where twelve objectives
// existed, showed eleven future nodes as simultaneously ready, and kept
// offering work that had already been replaced. These cover the engine side of
// that: what the scheduler is allowed to offer, and what happens on its own
// when an upstream task lands.

use orgbot_proto::graph::{AuthorizationState, DependencyState, Readiness, TaskOrigin};

impl Fixture {
    /// An open task that cannot start until `blockers` are submitted.
    fn open_task_after(&mut self, title: &str, blockers: Vec<TaskId>) -> TaskId {
        let events = self.apply(Command::CreateTask {
            title: title.into(),
            brief: format!("please do: {title}"),
            by: Some(self.coordinator),
            required_role: None,
            priority: Priority::Normal,
            acceptance_criteria: vec![],
            depends_on: blockers,
            origin: TaskOrigin::plain(),
        });
        let task = created_task(&events);
        self.apply(Command::OpenTask { task });
        task
    }

    /// Drive a task all the way to delivered.
    fn deliver(&mut self, task: TaskId, worker: AgentId) {
        self.apply(Command::ClaimTask { task, by: worker });
        self.apply(Command::StartTask { task, by: worker });
        self.apply(Command::SubmitForReview {
            task,
            by: worker,
            artifacts: vec![],
        });
        self.apply(Command::RecordReview {
            task,
            inspector: self.inspector,
            findings: vec![],
            summary: "fine".into(),
            criteria_met: vec![],
            criteria_failed: vec![],
        });
        self.apply(Command::SubmitTask {
            task,
            by: self.coordinator,
        });
    }

    fn readiness(&self, task: TaskId) -> Readiness {
        self.engine.task(task).expect("task exists").readiness()
    }
}

/// The headline failure: a chain of future work all advertising itself as
/// ready at once, because each was an unclaimed record.
#[test]
fn a_dependency_chain_offers_only_its_first_link() {
    let mut f = fixture();
    let worker = f.worker;

    let c0 = f.open_task("recover the source", None);
    let c1 = f.open_task_after("rebuild from source", vec![c0]);
    let c2 = f.open_task_after("re-run the checks", vec![c1]);

    let claimable = f.engine.claimable_by(worker);
    assert_eq!(
        claimable,
        vec![c0],
        "only the head of the chain is workable; the rest are waiting"
    );
    assert!(matches!(
        f.readiness(c1),
        Readiness::WaitingDependency { .. }
    ));
    assert!(matches!(
        f.readiness(c2),
        Readiness::WaitingDependency { .. }
    ));
}

/// When upstream lands, downstream becomes eligible on its own — no
/// coordinator has to notice and announce it.
#[test]
fn finishing_upstream_frees_the_next_link_automatically() {
    let mut f = fixture();
    let worker = f.worker;

    let first = f.open_task("recover the source", None);
    let second = f.open_task_after("rebuild from source", vec![first]);

    assert!(matches!(
        f.readiness(second),
        Readiness::WaitingDependency { .. }
    ));

    f.deliver(first, worker);

    assert_eq!(
        f.readiness(second),
        Readiness::ReadyNow,
        "the dependent should free itself the moment its blocker is delivered"
    );
    assert!(f.engine.claimable_by(worker).contains(&second));
}

/// One missing source is one blocker, named identically by everything waiting
/// on it — not five separate cards saying the same thing.
#[test]
fn many_dependents_name_one_shared_blocker() {
    let mut f = fixture();
    let recovery = f.open_task("recover the B3-C source", None);
    let dependents: Vec<TaskId> = (0..5)
        .map(|i| f.open_task_after(&format!("downstream {i}"), vec![recovery]))
        .collect();

    let blockers: std::collections::HashSet<TaskId> = dependents
        .iter()
        .map(|task| match f.readiness(*task) {
            Readiness::WaitingDependency { on, .. } => on,
            other => panic!("expected a wait, got {other:?}"),
        })
        .collect();

    assert_eq!(blockers, std::collections::HashSet::from([recovery]));
    assert!(
        dependents.iter().all(|t| !f.engine.task(*t).unwrap().needs_operator()),
        "waiting on the room's own work is never a question for a person"
    );
}

/// A waiting task names who is expected to clear it, so nobody has to ask.
#[test]
fn a_wait_names_the_bot_who_owns_the_blocker() {
    let mut f = fixture();
    let worker = f.worker;
    let blocker = f.open_task("recover the source", None);
    let waiting = f.open_task_after("rebuild", vec![blocker]);

    f.apply(Command::ClaimTask {
        task: blocker,
        by: worker,
    });

    match f.readiness(waiting) {
        Readiness::WaitingDependency { owner, reason, .. } => {
            assert_eq!(owner, Some(worker));
            assert_eq!(reason, "recover the source");
        }
        other => panic!("expected a wait, got {other:?}"),
    }
}

/// Replaced work must leave the scheduler entirely, immediately.
#[test]
fn a_superseded_task_is_never_offered_or_claimable() {
    let mut f = fixture();
    let worker = f.worker;

    let original = f.open_task("the first attempt", None);
    let replacement = f.open_task("the canonical task", None);

    f.apply(Command::SupersedeTask {
        task: original,
        replacement,
        reason: "merged into the canonical task".into(),
    });

    assert!(
        !f.engine.claimable_by(worker).contains(&original),
        "replaced work must not be offered"
    );
    let err = f.reject(Command::ClaimTask {
        task: original,
        by: worker,
    });
    assert!(
        matches!(err, EngineError::NotReady { .. }),
        "claiming replaced work should be refused, got {err:?}"
    );
    assert!(matches!(f.readiness(original), Readiness::Closed { .. }));
}

/// Dependents of a replaced task should follow to its replacement rather than
/// waiting forever on a record nobody will finish.
#[test]
fn a_dependent_follows_its_blocker_to_the_replacement() {
    let mut f = fixture();
    let worker = f.worker;

    let original = f.open_task("the first attempt", None);
    let dependent = f.open_task_after("downstream work", vec![original]);
    let replacement = f.open_task("the canonical task", None);

    f.apply(Command::SupersedeTask {
        task: original,
        replacement,
        reason: "merged into the canonical task".into(),
    });

    match f.readiness(dependent) {
        Readiness::WaitingDependency { on, .. } => assert_eq!(
            on, replacement,
            "the wait should move to whatever now carries the work"
        ),
        other => panic!("expected a wait on the replacement, got {other:?}"),
    }

    f.deliver(replacement, worker);
    assert_eq!(
        f.readiness(dependent),
        Readiness::ReadyNow,
        "delivering the replacement should free the dependent"
    );
}

/// Missing evidence is work for the room. It must not be offered as claimable,
/// and must not reach the operator.
#[test]
fn a_security_hold_pauses_work_without_asking_anyone() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.open_task("ship the container", None);

    f.apply(Command::SetAuthorization {
        task,
        authorization: AuthorizationState::SecurityHold {
            finding: "need merged-filesystem classification".into(),
            material: false,
        },
    });

    assert!(!f.engine.claimable_by(worker).contains(&task));
    assert!(!f.engine.task(task).unwrap().needs_operator());
    assert!(matches!(
        f.readiness(task),
        Readiness::BlockedInternal { .. }
    ));
}

/// A genuine operator gate does stop the room, and says why.
#[test]
fn an_operator_gate_stops_the_task_and_names_the_decision() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.open_task("provision the VPS", None);

    f.apply(Command::SetAuthorization {
        task,
        authorization: AuthorizationState::AwaitingOperator {
            because: "it provisions paid infrastructure".into(),
            options: vec!["Approve".into(), "Decline".into()],
        },
    });

    assert!(!f.engine.claimable_by(worker).contains(&task));
    assert!(f.engine.task(task).unwrap().needs_operator());

    let err = f.reject(Command::ClaimTask { task, by: worker });
    match err {
        EngineError::NotReady { reason, .. } => assert!(
            reason.contains("paid infrastructure"),
            "the refusal should carry the decision, got {reason}"
        ),
        other => panic!("expected NotReady, got {other:?}"),
    }
}

/// The error a bot sees must be written for a bot. The operator-facing copy
/// reassures the reader that nothing is needed from them, which is precisely
/// the wrong thing to tell an agent it cannot claim.
#[test]
fn a_refusal_tells_the_bot_what_to_do_instead() {
    let mut f = fixture();
    let worker = f.worker;
    let original = f.open_task("the first attempt", None);
    let replacement = f.open_task("the canonical task", None);
    f.apply(Command::SupersedeTask {
        task: original,
        replacement,
        reason: "merged into the canonical task".into(),
    });

    match f.reject(Command::ClaimTask {
        task: original,
        by: worker,
    }) {
        EngineError::NotReady { reason, .. } => {
            assert!(reason.contains("work that instead"), "got {reason}");
            assert!(!reason.contains("No action needed from you"));
        }
        other => panic!("expected NotReady, got {other:?}"),
    }
}

/// Two bots racing for one task still get the precise state-machine error;
/// the graph checks must not swallow it into a vaguer one.
#[test]
fn the_graph_checks_do_not_mask_a_plain_race() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.open_task("write the release notes", None);

    f.apply(Command::ClaimTask { task, by: worker });
    let err = f.reject(Command::ClaimTask { task, by: worker2 });
    assert!(
        matches!(err, EngineError::Transition(_)),
        "a taken task is a workflow error, not a graph one; got {err:?}"
    );
}

/// A dependency block keeps its own precise error rather than the generic one.
#[test]
fn a_dependency_block_keeps_its_specific_error() {
    let mut f = fixture();
    let worker = f.worker;
    let blocker = f.open_task("recover the source", None);
    let waiting = f.open_task_after("rebuild", vec![blocker]);

    match f.reject(Command::ClaimTask {
        task: waiting,
        by: worker,
    }) {
        EngineError::DependencyNotMet { blocker: named, .. } => assert_eq!(named, blocker),
        other => panic!("expected DependencyNotMet, got {other:?}"),
    }
}

/// A room written before the graph existed keeps working exactly as it did.
#[test]
fn a_plain_task_with_no_graph_state_is_still_ready() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.open_task("ordinary work", None);

    let record = f.engine.task(task).expect("task exists");
    assert_eq!(record.dependency, DependencyState::Satisfied);
    assert_eq!(record.authorization, AuthorizationState::Authorized);
    assert_eq!(f.readiness(task), Readiness::ReadyNow);
    assert!(f.engine.claimable_by(worker).contains(&task));
}

/// The guard that stops the room routing its own gaps to a person. Every one
/// of these was a real escalation from a run where the room asked the operator
/// for something the room itself had lost or simply not done yet.
#[test]
fn the_engine_refuses_an_operator_gate_that_is_really_internal_work() {
    let mut f = fixture();
    let task = f.open_task("rebuild the manifest", None);

    for because in [
        "the B3-C source is missing",
        "the artifact was truncated",
        "we need to regenerate the manifest",
        "evidence is required for the security review",
    ] {
        let err = f.reject(Command::SetAuthorization {
            task,
            authorization: AuthorizationState::AwaitingOperator {
                because: because.into(),
                options: vec![],
            },
        });
        assert!(
            matches!(err, EngineError::NotReady { .. }),
            "{because:?} should have been kept inside the room, got {err:?}"
        );
    }

    // The task is untouched by a refused hold.
    assert_eq!(f.readiness(task), Readiness::ReadyNow);
    assert!(!f.engine.task(task).unwrap().needs_operator());
}

#[test]
fn the_engine_accepts_a_genuine_operator_gate() {
    let mut f = fixture();
    let task = f.open_task("deploy to production", None);

    f.apply(Command::SetAuthorization {
        task,
        authorization: AuthorizationState::AwaitingOperator {
            because: "deploying to production needs your sign-off".into(),
            options: vec!["Approve".into(), "Defer".into()],
        },
    });
    assert!(f.engine.task(task).unwrap().needs_operator());
}

/// Lifting a hold puts the task straight back in the queue.
#[test]
fn lifting_a_hold_returns_the_task_to_the_queue() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.open_task("ship it", None);

    f.apply(Command::SetAuthorization {
        task,
        authorization: AuthorizationState::HeldByPolicy {
            policy: "no-new-machines".into(),
            reason: "pending the confined-docker check".into(),
        },
    });
    assert!(!f.engine.claimable_by(worker).contains(&task));

    f.apply(Command::SetAuthorization {
        task,
        authorization: AuthorizationState::Authorized,
    });
    assert!(f.engine.claimable_by(worker).contains(&task));
}

#[test]
fn a_supersession_cycle_is_refused() {
    let mut f = fixture();
    let first = f.open_task("first", None);
    let second = f.open_task("second", None);

    f.apply(Command::SupersedeTask {
        task: first,
        replacement: second,
        reason: "merged".into(),
    });
    // Pointing the replacement back at what it replaced would make the
    // scheduler chase its own tail.
    let err = f.reject(Command::SupersedeTask {
        task: second,
        replacement: first,
        reason: "undo".into(),
    });
    assert!(matches!(err, EngineError::NotReady { .. }), "got {err:?}");

    let err = f.reject(Command::SupersedeTask {
        task: first,
        replacement: first,
        reason: "itself".into(),
    });
    assert!(matches!(err, EngineError::NotReady { .. }), "got {err:?}");
}

/// Supersession records both directions, so history reads either way.
#[test]
fn supersession_is_recorded_on_both_records() {
    let mut f = fixture();
    let original = f.open_task("first attempt", None);
    let replacement = f.open_task("canonical", None);

    f.apply(Command::SupersedeTask {
        task: original,
        replacement,
        reason: "merged".into(),
    });

    assert_eq!(
        f.engine.task(original).unwrap().lineage.superseded_by,
        Some(replacement)
    );
    assert_eq!(
        f.engine.task(replacement).unwrap().lineage.supersedes,
        Some(original)
    );
    assert_eq!(f.state(original), TaskState::Superseded);
}

/// Retired work stays retired. An old incident task that keeps coming back is
/// how a settled decision turns into live work again.
#[test]
fn a_retired_task_cannot_be_revived() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.open_task("an old incident", None);

    f.apply(Command::RetireTask {
        task,
        reason: "no longer relevant".into(),
    });

    assert_eq!(f.state(task), TaskState::Retired);
    assert!(!f.engine.claimable_by(worker).contains(&task));
    assert!(f.engine.task(task).unwrap().state.is_permanent());
    // Nothing puts it back on the board.
    let err = f.reject(Command::ClaimTask { task, by: worker });
    assert!(matches!(err, EngineError::Transition(_)), "got {err:?}");
}

// ------------------------------------------------------- operator decisions
//
// A run kept putting the same question to the same person: the room had
// approved "same VPS, confined Docker" once, and every rebuild inside that
// boundary came back for approval again. These cover the engine side of asking
// once.

use orgbot_proto::decision::Answer;

impl Fixture {
    fn gate(&mut self, task: TaskId, because: &str) -> Vec<EventEnvelope> {
        self.apply(Command::SetAuthorization {
            task,
            authorization: AuthorizationState::AwaitingOperator {
                because: because.into(),
                options: vec!["Approve".into(), "Decline".into()],
            },
        })
    }

    fn approve(&mut self, decision: orgbot_proto::DecisionId) {
        self.apply(Command::AnswerDecision {
            decision,
            answer: Answer {
                chosen: "Approve".into(),
                note: String::new(),
                at: Timestamp::now(),
            },
        });
    }
}

fn asked_decision(events: &[EventEnvelope]) -> orgbot_proto::DecisionId {
    events
        .iter()
        .find_map(|e| match &e.event {
            Event::DecisionAsked { decision } => Some(decision.id),
            _ => None,
        })
        .expect("a decision_asked event")
}

#[test]
fn asking_the_operator_raises_exactly_one_question() {
    let mut f = fixture();
    let task = f.open_task("provision the VPS", None);
    let events = f.gate(task, "it provisions paid infrastructure");

    assert_eq!(f.engine.decisions().pending().count(), 1);
    assert!(f.engine.task(task).unwrap().needs_operator());
    let _ = asked_decision(&events);
}

/// The behaviour the run most needed: the same question, asked again, does not
/// reach the person a second time.
#[test]
fn the_same_question_is_not_put_twice() {
    let mut f = fixture();
    let first = f.open_task("rebuild the container", None);
    let second = f.open_task("rebuild it again", None);

    f.gate(first, "this rebuilds on the approved VPS");
    f.gate(second, "This rebuilds on the approved VPS.");

    assert_eq!(
        f.engine.decisions().pending().count(),
        1,
        "one prompt, however many tasks hit it"
    );
}

/// Answering once clears every task waiting on that question.
#[test]
fn one_answer_releases_everything_it_unblocks() {
    let mut f = fixture();
    let worker = f.worker;
    let first = f.open_task("rebuild the container", None);
    let second = f.open_task("rebuild the sidecar", None);

    let events = f.gate(first, "this rebuilds on the approved VPS");
    let decision = asked_decision(&events);
    f.gate(second, "this rebuilds on the approved VPS");

    assert!(f.engine.task(first).unwrap().needs_operator());
    assert!(f.engine.task(second).unwrap().needs_operator());

    f.approve(decision);

    assert!(!f.engine.task(first).unwrap().needs_operator());
    assert!(
        !f.engine.task(second).unwrap().needs_operator(),
        "the person answered the question, not the card"
    );
    assert!(f.engine.claimable_by(worker).contains(&first));
    assert!(f.engine.claimable_by(worker).contains(&second));
    assert_eq!(f.engine.decisions().pending().count(), 0);
}

/// Once answered, the same question asked again is applied, not re-raised.
#[test]
fn a_settled_question_applies_itself_to_later_work() {
    let mut f = fixture();
    let worker = f.worker;
    let first = f.open_task("rebuild the container", None);
    let decision = asked_decision(&f.gate(first, "this rebuilds on the approved VPS"));
    f.approve(decision);

    // Much later, another bot hits the same gate.
    let later = f.open_task("rebuild it once more", None);
    f.gate(later, "this rebuilds on the approved VPS");

    assert_eq!(
        f.engine.decisions().pending().count(),
        0,
        "nobody should be asked again"
    );
    assert!(!f.engine.task(later).unwrap().needs_operator());
    assert!(f.engine.claimable_by(worker).contains(&later));
}

/// A different question still gets through.
#[test]
fn a_genuinely_new_question_still_reaches_the_operator() {
    let mut f = fixture();
    let first = f.open_task("rebuild the container", None);
    let decision = asked_decision(&f.gate(first, "this rebuilds on the approved VPS"));
    f.approve(decision);

    let other = f.open_task("delete the database", None);
    f.gate(other, "this destroys production data");

    assert_eq!(f.engine.decisions().pending().count(), 1);
    assert!(f.engine.task(other).unwrap().needs_operator());
}

#[test]
fn answering_something_already_settled_is_refused() {
    let mut f = fixture();
    let task = f.open_task("provision the VPS", None);
    let decision = asked_decision(&f.gate(task, "it provisions paid infrastructure"));
    f.approve(decision);

    let err = f.reject(Command::AnswerDecision {
        decision,
        answer: Answer {
            chosen: "Decline".into(),
            note: String::new(),
            at: Timestamp::now(),
        },
    });
    assert!(matches!(err, EngineError::NotReady { .. }), "got {err:?}");
}

/// The room is rebuilt from its own log, so what it has already asked has to
/// survive a replay — otherwise a restarted room starts pestering again.
#[test]
fn decisions_survive_a_replay_of_the_log() {
    let mut f = fixture();
    let task = f.open_task("provision the VPS", None);
    let decision = asked_decision(&f.gate(task, "it provisions paid infrastructure"));
    f.approve(decision);

    let replayed = Engine::from_archive(f.engine.room().clone(), f.engine.log().to_vec())
        .expect("the log should replay");

    assert_eq!(replayed.decisions().pending().count(), 0);
    assert_eq!(
        replayed.decisions().decisions.len(),
        1,
        "the question is remembered"
    );
    assert!(!replayed.task(task).unwrap().needs_operator());
}

/// A room saved before an upgrade must still open.
///
/// This is not hypothetical: adding the decision log to the engine without a
/// default made every existing saved room fail to restore with "missing field
/// `decisions`". The file survived, but the room would not open. Any field
/// added to the engine from here has to clear this test.
#[test]
fn a_room_saved_before_this_version_still_loads() {
    let mut f = fixture();
    let task = f.open_task("some earlier work", None);
    f.apply(Command::ClaimTask { task, by: f.worker });

    let saved = serde_json::to_value(&f.engine).expect("the engine serialises");

    // Strip everything a room saved by an older build would not have had.
    let mut older = saved.clone();
    let fields = older.as_object_mut().expect("an object");
    for added_since in ["decisions"] {
        fields.remove(added_since);
    }

    let restored: Engine =
        serde_json::from_value(older).expect("an older save must still open");
    assert_eq!(restored.tasks().count(), 1);
    assert_eq!(restored.decisions().decisions.len(), 0);
    assert!(restored.task(task).is_some());
}

/// And a room saved by *this* build round-trips unchanged.
#[test]
fn a_saved_room_round_trips() {
    let mut f = fixture();
    let task = f.open_task("provision the VPS", None);
    let decision = asked_decision(&f.gate(task, "it provisions paid infrastructure"));
    f.approve(decision);

    let saved = serde_json::to_string(&f.engine).expect("serialises");
    let restored: Engine = serde_json::from_str(&saved).expect("deserialises");

    assert_eq!(restored.decisions().decisions.len(), 1);
    assert_eq!(restored.decisions().pending().count(), 0);
    assert!(!restored.task(task).unwrap().needs_operator());
}

// ------------------------------------------- roles, leases and delegation
//
// A real room stalled with work role-locked to a busy bot while others that
// could plainly have done it were not eligible, and with claims held forever
// by bots that had stopped. These cover both.

use orgbot_proto::roles::{RoleSpec, RoleTable};
use orgbot_proto::task::DEFAULT_LEASE_MILLIS;

#[test]
fn a_stand_in_can_take_role_locked_work_when_the_holder_is_busy() {
    let mut f = fixture();
    let (worker, inspector) = (f.worker, f.inspector);
    f.engine.set_roles(RoleTable::new(vec![
        RoleSpec::new("writer").falling_back_to(["reviewer"])
    ]));

    let locked = f.open_task("write the notes", Some("writer"));

    // The reviewer does not hold `writer`, but the room says it may stand in.
    assert!(f.engine.claimable_by(inspector).contains(&locked));
    f.apply(Command::ClaimTask {
        task: locked,
        by: inspector,
    });
    assert!(
        f.engine.task(locked).unwrap().stood_in,
        "standing in is recorded, so a roster that never fits is visible"
    );

    // And the actual role holder is still preferred where both could take it.
    let another = f.open_task("write more", Some("writer"));
    assert_eq!(f.engine.claimable_by(worker).first(), Some(&another));
}

#[test]
fn a_bot_with_no_path_to_the_role_still_cannot_claim() {
    let mut f = fixture();
    let inspector = f.inspector;
    let locked = f.open_task("write the notes", Some("writer"));

    assert!(!f.engine.claimable_by(inspector).contains(&locked));
    let err = f.reject(Command::ClaimTask {
        task: locked,
        by: inspector,
    });
    assert!(matches!(err, EngineError::RoleMismatch { .. }), "got {err:?}");
}

/// Work can ask for a capability rather than a named role.
#[test]
fn work_can_require_a_capability() {
    let mut f = fixture();
    let inspector = f.inspector;
    f.engine.set_roles(RoleTable::new(vec![
        RoleSpec::new("reviewer").able_to(["review", "audit"])
    ]));

    let audit = f.open_task("audit the output", Some("audit"));
    assert!(f.engine.claimable_by(inspector).contains(&audit));
}

/// A bot that stops holding up its end must not park the work forever.
#[test]
fn a_lapsed_claim_returns_the_work_to_the_pool() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.open_task("write the notes", None);

    f.apply(Command::ClaimTask { task, by: worker });
    assert_eq!(f.state(task), TaskState::Claimed);
    assert!(f.engine.task(task).unwrap().lease_expires_at.is_some());

    // Long enough later that the holder has plainly stopped.
    let later = Timestamp::now().plus_millis(DEFAULT_LEASE_MILLIS + 1_000);
    f.apply(Command::Tick { now: later });

    assert_eq!(f.state(task), TaskState::Ready, "it is claimable again");
    assert_eq!(f.engine.task(task).unwrap().assignee, None);
    assert!(f.engine.claimable_by(worker).contains(&task));
}

/// A bot working steadily never loses its claim.
#[test]
fn working_on_a_task_holds_its_claim() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.open_task("write the notes", None);
    f.apply(Command::ClaimTask { task, by: worker });

    let before = f.engine.task(task).unwrap().lease_expires_at;
    f.apply(Command::StartTask { task, by: worker });
    let after = f.engine.task(task).unwrap().lease_expires_at;

    assert!(after >= before, "acting on the task renews the lease");
    // And a sweep now leaves it alone.
    f.apply(Command::Tick {
        now: Timestamp::now(),
    });
    assert_eq!(f.state(task), TaskState::InProgress);
}

/// Delivered work holding an old lease is finished, not abandoned.
#[test]
fn a_finished_task_is_not_reclaimed() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.open_task("write the notes", None);
    f.deliver(task, worker);

    f.apply(Command::Tick {
        now: Timestamp::now().plus_millis(DEFAULT_LEASE_MILLIS * 10),
    });
    assert_eq!(f.state(task), TaskState::Submitted);
}

/// Nobody should be asked to pick a reviewer when one is available.
#[test]
fn a_reviewer_is_chosen_without_asking_anyone() {
    let mut f = fixture();
    let (worker, inspector) = (f.worker, f.inspector);
    let task = f.start_task("write the notes", worker);

    assert_eq!(
        f.engine.reviewer_for(task),
        Some(inspector),
        "the eligible independent bot should be found automatically"
    );
}

/// Independence is encoded, not left to convention.
#[test]
fn the_bot_that_did_the_work_is_never_its_reviewer() {
    let mut f = fixture();
    let inspector = f.inspector;
    // The inspector does the work itself.
    let task = f.open_task("write the notes", None);
    f.apply(Command::ClaimTask {
        task,
        by: inspector,
    });

    assert_ne!(f.engine.reviewer_for(task), Some(inspector));
}

/// Review must not queue behind one loaded inspector while others are free.
#[test]
fn review_goes_to_the_least_busy_eligible_bot() {
    let mut f = fixture();
    let (worker, inspector) = (f.worker, f.inspector);

    // Give the inspector two open tasks so it is the busiest.
    for i in 0..2 {
        let busywork = f.open_task(&format!("inspector work {i}"), None);
        f.apply(Command::ClaimTask {
            task: busywork,
            by: inspector,
        });
    }
    // A second bot that can also review.
    let spare = AgentSpec::new("qa2", "Second Inspector", provider())
        .with_roles(["reviewer"])
        .with_autonomy(Autonomy::inspector());
    let spare_id = spare.id;
    f.apply(Command::JoinAgent {
        spec: Box::new(spare),
    });

    let task = f.start_task("write the notes", worker);
    assert_eq!(
        f.engine.reviewer_for(task),
        Some(spare_id),
        "the free inspector should take it"
    );
}

#[test]
fn idle_bots_are_reported_so_work_can_be_spread() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.open_task("write the notes", None);
    f.apply(Command::ClaimTask { task, by: worker });

    let idle = f.engine.idle_agents();
    assert!(idle.contains(&worker2), "an unoccupied bot is idle");
    assert!(!idle.contains(&worker), "one holding work is not");
    assert_eq!(f.engine.workload(worker), 1);
    assert_eq!(f.engine.workload(worker2), 0);
}

/// A room that declares no roles behaves exactly as it did before.
#[test]
fn a_room_without_a_role_table_matches_by_name() {
    let mut f = fixture();
    let (worker, inspector) = (f.worker, f.inspector);
    let locked = f.open_task("write the notes", Some("writer"));

    assert!(f.engine.claimable_by(worker).contains(&locked));
    assert!(!f.engine.claimable_by(inspector).contains(&locked));
}

/// A delegation nobody accepts must not leave the sender waiting forever.
#[test]
fn an_unaccepted_handoff_frees_the_sender_when_it_expires() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.start_task("write the notes", worker);

    let events = f.apply(Command::OfferHandoff {
        task,
        from: worker,
        to: Recipient::Role {
            role: "nobody-holds-this".into(),
        },
        kind: HandoffKind::Assist,
        brief: "please help".into(),
        expects: vec![],
        context: vec![],
        blocks_sender: Some(true),
        timeout_ms: Some(1_000),
    });
    let handoff = offered_handoff(&events);
    assert_eq!(f.state(task), TaskState::Blocked, "an assist blocks the sender");

    f.apply(Command::Tick {
        now: Timestamp::now().plus_millis(5_000),
    });

    assert_eq!(
        f.engine.handoff(handoff).unwrap().state,
        HandoffState::Expired
    );
    assert_ne!(
        f.state(task),
        TaskState::Blocked,
        "the sender must be released, not left inert waiting on a dead offer"
    );
}

// ------------------------------------------------- review that gets somewhere
//
// A run produced cycles that each found one small thing, sent the work back,
// and found another next time — twenty fences where one choke point was
// needed, with nothing noticing the same problem had been reported four times.

use orgbot_proto::convergence::Convergence;
use orgbot_proto::review::Severity;

impl Fixture {
    /// Reject `task` with one blocking finding, then resubmit it.
    fn reject_with(&mut self, task: TaskId, worker: AgentId, summary: &str, locus: &str) {
        self.apply(Command::SubmitForReview {
            task,
            by: worker,
            artifacts: vec![],
        });
        self.apply(Command::RecordReview {
            task,
            inspector: self.inspector,
            findings: vec![Finding {
                severity: Severity::Major,
                summary: summary.into(),
                locus: Some(locus.into()),
                suggestion: None,
            }],
            summary: "needs work".into(),
            criteria_met: vec![],
            criteria_failed: vec![],
        });
    }
}

/// The same invariant failing over and over should stop producing patches.
#[test]
fn a_recurring_invariant_is_reported_as_structural() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.start_task("fix the handler", worker);
    // A generous budget so the rework cap does not end this first.
    f.engine.set_max_revisions(task, 10);

    for wording in [
        "stale async mutation",
        "async mutation is stale",
        "mutation stale in the async path",
    ] {
        f.reject_with(task, worker, wording, "reducer");
    }

    let verdict = f.engine.convergence(task);
    assert!(
        matches!(verdict, Convergence::Structural { .. }),
        "three cycles on one invariant is a pattern, got {verdict:?}"
    );
    assert!(verdict.needs_escalation());

    // And the room was told, in the room, without anyone being asked.
    let said = f
        .engine
        .transcript()
        .iter()
        .any(|m| m.body.contains("Stop patching the symptom"));
    assert!(said, "the recommendation should reach the room");
    assert_eq!(
        f.engine.decisions().pending().count(),
        0,
        "going round in circles is the room's problem, not a question"
    );
}

/// One small finding per cycle turns a single pass into five.
#[test]
fn a_drip_fed_review_is_called_out() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.start_task("write the notes", worker);
    f.engine.set_max_revisions(task, 10);

    f.reject_with(task, worker, "the parser drops commas", "parse.rs");
    f.reject_with(task, worker, "the writer emits tabs", "write.rs");

    let said = f
        .engine
        .transcript()
        .iter()
        .any(|m| m.body.contains("full set of blocking findings"));
    assert!(said, "a second one-finding cycle should be called out");
}

/// A first review finding one thing is perfectly normal.
#[test]
fn a_first_review_is_not_accused_of_drip_feeding() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.start_task("write the notes", worker);

    f.reject_with(task, worker, "the parser drops commas", "parse.rs");

    let said = f
        .engine
        .transcript()
        .iter()
        .any(|m| m.body.contains("full set of blocking findings"));
    assert!(!said);
}

/// Rework loops without asking anyone whether to continue.
#[test]
fn rejection_sends_work_back_without_involving_a_person() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.start_task("write the notes", worker);

    f.reject_with(task, worker, "the parser drops commas", "parse.rs");

    assert_eq!(f.state(task), TaskState::InProgress, "back with its owner");
    assert_eq!(f.engine.task(task).unwrap().assignee, Some(worker));
    assert_eq!(f.engine.decisions().pending().count(), 0);
    assert!(!f.engine.task(task).unwrap().needs_operator());
}

/// Do not ask a person for something the room produced.
#[test]
fn a_lost_artifact_is_asked_of_the_bot_that_wrote_it() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.start_task("write the schema", worker);
    let artifact = f.add_artifact(task, worker);

    assert_eq!(f.engine.recovery_owner(artifact), Some(worker));
    assert_eq!(f.engine.decisions().pending().count(), 0);
}

/// Content that has to be exact may only be re-emitted by whoever produced it.
#[test]
fn authority_bearing_content_cannot_be_replaced_by_another_bot() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.start_task("mint the signing key", worker);

    let events = f.apply(Command::AddArtifact {
        task,
        author: worker,
        name: "public.pem".into(),
        body: ArtifactBody::Text {
            text: "-----BEGIN PUBLIC KEY-----".into(),
        },
        authority_bearing: true,
    });
    let key = added_artifact(&events);

    assert!(f.engine.may_replace(key, worker), "its author may re-emit it");
    assert!(
        !f.engine.may_replace(key, worker2),
        "a second author could only produce something that looks right"
    );
}

#[test]
fn ordinary_work_can_be_corrected_by_anyone() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.start_task("write the notes", worker);
    let notes = f.add_artifact(task, worker);

    assert!(f.engine.may_replace(notes, worker2));
}

// ------------------------------------------- standing rules and evidence
//
// Two ways a room asked for things it could have settled itself: it re-asked
// questions a standing rule had already answered, and it treated "we cannot
// prove this yet" as a decision for a person.

use orgbot_proto::decision::{Envelope, Policy, Request};

/// A rule answers a class of questions in advance, so they never get asked.
#[test]
fn a_standing_rule_answers_before_anyone_is_asked() {
    let mut f = fixture();
    f.engine.adopt_policy(
        Policy::new("confined-docker", "Docker on the approved VPS is fine.").granting(
            Envelope {
                allowed: vec!["container:*".into()],
                environment: "same-vps-confined-docker".into(),
                ..Default::default()
            },
        ),
    );

    let permitted = f
        .engine
        .policy_permits(&Request::new("container:rebuild", "same-vps-confined-docker"));
    assert_eq!(permitted.map(|p| p.name.as_str()), Some("confined-docker"));
    assert!(f
        .engine
        .policy_permits(&Request::new("container:rebuild", "somewhere-else"))
        .is_none());
}

/// A decision records the rules that governed it, so old work is not read
/// against rules adopted since.
#[test]
fn a_decision_records_the_rules_in_force_when_it_was_asked() {
    let mut f = fixture();
    f.engine
        .adopt_policy(Policy::new("author-recovers", "The author re-emits."));

    let task = f.open_task("provision the VPS", None);
    let decision = asked_decision(&f.gate(task, "it provisions paid infrastructure"));

    let recorded = f
        .engine
        .decisions()
        .decisions
        .iter()
        .find(|d| d.id == decision)
        .expect("the decision exists");
    assert!(
        recorded.governed_by.contains("author-recovers@1"),
        "got {:?}",
        recorded.governed_by
    );
}

/// "We cannot prove this yet" is work, not a question.
#[test]
fn a_security_hold_raises_the_evidence_work_itself() {
    let mut f = fixture();
    let task = f.open_task("ship the container", None);

    f.apply(Command::SetAuthorization {
        task,
        authorization: AuthorizationState::SecurityHold {
            finding: "need merged-filesystem classification".into(),
            material: false,
        },
    });

    let evidence: Vec<&orgbot_proto::task::Task> = f
        .engine
        .tasks()
        .filter(|t| t.kind == orgbot_proto::graph::TaskKind::Evidence)
        .collect();
    assert_eq!(evidence.len(), 1, "the errand should be raised once");
    assert_eq!(evidence[0].lineage.parent, Some(task));
    assert!(evidence[0].title.contains("merged-filesystem"));
    assert!(
        evidence[0].readiness().is_claimable(),
        "and be claimable, not left as a draft"
    );
    assert_eq!(
        f.engine.decisions().pending().count(),
        0,
        "nobody outside the room needs to be asked"
    );
}

/// A hold restated should not produce a second identical errand.
#[test]
fn evidence_work_is_not_raised_twice_for_one_hold() {
    let mut f = fixture();
    let task = f.open_task("ship the container", None);

    for wording in [
        "need merged-filesystem classification",
        "merged-filesystem classification is needed",
    ] {
        f.apply(Command::SetAuthorization {
            task,
            authorization: AuthorizationState::SecurityHold {
                finding: wording.into(),
                material: false,
            },
        });
    }

    let count = f
        .engine
        .tasks()
        .filter(|t| t.kind == orgbot_proto::graph::TaskKind::Evidence)
        .count();
    assert_eq!(count, 1, "one errand, however the hold is worded");
}

/// A material incident is a person's call and raises no errand.
#[test]
fn a_material_incident_does_not_become_an_errand() {
    let mut f = fixture();
    let task = f.open_task("ship the container", None);

    f.apply(Command::SetAuthorization {
        task,
        authorization: AuthorizationState::SecurityHold {
            finding: "a live credential was published".into(),
            material: true,
        },
    });

    assert_eq!(
        f.engine
            .tasks()
            .filter(|t| t.kind == orgbot_proto::graph::TaskKind::Evidence)
            .count(),
        0
    );
    assert!(f.engine.task(task).unwrap().needs_operator());
}

/// Everything above must leave the room coherent.
#[test]
fn the_room_stays_consistent_through_all_of_it() {
    let mut f = fixture();
    let worker = f.worker;
    let objective = f.open_task("repair the harness", None);
    let dependent = f.open_task_after("rebuild", vec![objective]);
    f.apply(Command::ClaimTask {
        task: objective,
        by: worker,
    });
    f.apply(Command::SetAuthorization {
        task: dependent,
        authorization: AuthorizationState::SecurityHold {
            finding: "need a classification".into(),
            material: false,
        },
    });
    let replaced = f.open_task("an old attempt", None);
    let replacement = f.open_task("the canonical attempt", None);
    f.apply(Command::SupersedeTask {
        task: replaced,
        replacement,
        reason: "merged".into(),
    });

    assert_eq!(
        f.engine.inconsistencies(),
        vec![],
        "a room built only through commands must never break its own rules"
    );
}
