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
use orgbot_proto::handoff::{HandoffKind, HandoffState, HandoffTransition, Recipient};
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
        access: Default::default(),
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
fn an_artifact_author_cannot_review_after_delegating_the_task() {
    let mut f = fixture();
    let (author, recipient) = (f.inspector, f.worker2);
    let second_author = AgentSpec::new("qa2", "Second Author", provider())
        .with_roles(["reviewer"])
        .with_autonomy(Autonomy::inspector());
    let second_author_id = second_author.id;
    f.apply(Command::JoinAgent {
        spec: Box::new(second_author),
    });
    let independent = AgentSpec::new("qa3", "Independent Inspector", provider())
        .with_roles(["reviewer"])
        .with_autonomy(Autonomy::inspector());
    let independent_id = independent.id;
    f.apply(Command::JoinAgent {
        spec: Box::new(independent),
    });

    let task = f.start_task("write the release notes", author);
    let artifact = f.add_artifact(task, author);
    f.add_artifact(task, second_author_id);
    let events = f.apply(Command::OfferHandoff {
        task,
        from: author,
        to: Recipient::Agent { agent: recipient },
        kind: HandoffKind::Delegate,
        brief: "finish the release notes".into(),
        expects: vec![],
        context: vec![artifact],
        blocks_sender: None,
        timeout_ms: None,
    });
    f.apply(Command::AcceptHandoff {
        handoff: offered_handoff(&events),
        by: recipient,
    });
    f.apply(Command::SubmitForReview {
        task,
        by: recipient,
        artifacts: vec![artifact],
    });

    assert!(!f.engine.review_queue_for(author).contains(&task));
    assert!(!f.engine.review_queue_for(second_author_id).contains(&task));
    assert_eq!(f.engine.reviewer_for(task), Some(independent_id));
    let event_count = f.engine.head();
    assert_eq!(
        f.reject(Command::RecordReview {
            task,
            inspector: author,
            findings: vec![],
            summary: "looks good".into(),
            criteria_met: vec![],
            criteria_failed: vec![],
        }),
        EngineError::SelfInspection { task, agent: author }
    );
    assert_eq!(f.engine.head(), event_count);
    assert_eq!(f.state(task), TaskState::InReview);
    assert_eq!(
        f.reject(Command::RecordReview {
            task,
            inspector: second_author_id,
            findings: vec![],
            summary: "also looks good".into(),
            criteria_met: vec![],
            criteria_failed: vec![],
        }),
        EngineError::SelfInspection {
            task,
            agent: second_author_id,
        }
    );
    assert_eq!(f.engine.head(), event_count);

    f.apply(Command::RecordReview {
        task,
        inspector: independent_id,
        findings: vec![],
        summary: "independent review passed".into(),
        criteria_met: vec![],
        criteria_failed: vec![],
    });
    assert_eq!(f.state(task), TaskState::Approved);
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

/// From the live room: @opus offered @grok-quick a delegation while
/// @grok-quick was in a turn that had begun before the offer and ran fifteen
/// minutes more. The offer's five minutes ran out before that turn ended, so
/// it expired without ever being shown to the bot it was for.
#[test]
fn an_offer_waits_for_a_busy_recipient_to_have_a_turn_before_it_can_expire() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.start_task("write the release notes", worker);

    // worker2 is mid-turn on something else when the offer arrives.
    f.apply(Command::BeginTurn { agent: worker2 });
    let events = f.apply(Command::OfferHandoff {
        task,
        from: worker,
        to: Recipient::Agent { agent: worker2 },
        kind: HandoffKind::Delegate,
        brief: "you own this now".into(),
        expects: vec![],
        context: vec![],
        blocks_sender: None,
        timeout_ms: Some(100),
    });
    let handoff = offered_handoff(&events);

    // That turn outlasts the timeout. The orchestrator vouches for a bot
    // mid-turn every second, and that is what keeps the offer open.
    std::thread::sleep(std::time::Duration::from_millis(150));
    f.apply(Command::KeepAlive { agent: worker2 });
    f.apply(Command::Tick {
        now: Timestamp::now(),
    });
    assert_eq!(
        f.engine.handoff(handoff).unwrap().state,
        HandoffState::Offered,
        "an offer its recipient has not yet been shown must not run out"
    );

    // Its next turn shows it the offer, and from then the timeout runs, however
    // busy the bot stays.
    f.apply(Command::BeginTurn { agent: worker2 });
    f.apply(Command::KeepAlive { agent: worker2 });
    f.apply(Command::Tick {
        now: Timestamp::now().plus_millis(150),
    });
    assert_eq!(
        f.engine.handoff(handoff).unwrap().state,
        HandoffState::Expired,
        "an offer that was read and left unanswered still expires"
    );
}

/// From the live room: @mira accepted a delegation four seconds after its
/// deadline had passed, before the room had ticked. The accept went through,
/// and the next tick expired a handoff that was already under way.
#[test]
fn accepting_a_handoff_restarts_its_timeout() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.start_task("write the release notes", worker);
    let events = f.apply(Command::OfferHandoff {
        task,
        from: worker,
        to: Recipient::Agent { agent: worker2 },
        kind: HandoffKind::Assist,
        brief: "check the dates".into(),
        expects: vec![],
        context: vec![],
        blocks_sender: None,
        timeout_ms: Some(100),
    });
    let handoff = offered_handoff(&events);

    std::thread::sleep(std::time::Duration::from_millis(150));
    f.apply(Command::AcceptHandoff {
        handoff,
        by: worker2,
    });
    f.apply(Command::Tick {
        now: Timestamp::now(),
    });
    assert_eq!(
        f.engine.handoff(handoff).unwrap().state,
        HandoffState::Accepted,
        "the time it waited to be answered is not time to do the work in"
    );
}

/// From the live room: @ollama-glm accepted an assist for @mira in a turn
/// that ran on for fifteen minutes doing it. The assist expired five minutes
/// after it was offered, @mira was released without the result, and the
/// finished run had nowhere to go.
#[test]
fn an_accepted_handoff_does_not_expire_during_the_turn_that_took_it_on() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.start_task("write the release notes", worker);
    let events = f.apply(Command::OfferHandoff {
        task,
        from: worker,
        to: Recipient::Agent { agent: worker2 },
        kind: HandoffKind::Assist,
        brief: "run the checks".into(),
        expects: vec![],
        context: vec![],
        blocks_sender: None,
        timeout_ms: Some(100),
    });
    let handoff = offered_handoff(&events);
    f.apply(Command::BeginTurn { agent: worker2 });
    f.apply(Command::AcceptHandoff {
        handoff,
        by: worker2,
    });

    // The work takes longer than the timeout, all within that turn.
    std::thread::sleep(std::time::Duration::from_millis(150));
    f.apply(Command::KeepAlive { agent: worker2 });
    f.apply(Command::Tick {
        now: Timestamp::now(),
    });
    assert_eq!(f.engine.handoff(handoff).unwrap().state, HandoffState::Accepted);
    assert_eq!(f.state(task), TaskState::Blocked, "the sender is still waiting for it");

    // A later turn that does not finish it does not keep the sender waiting.
    f.apply(Command::BeginTurn { agent: worker2 });
    f.apply(Command::KeepAlive { agent: worker2 });
    f.apply(Command::Tick {
        now: Timestamp::now().plus_millis(150),
    });
    assert_eq!(f.engine.handoff(handoff).unwrap().state, HandoffState::Expired);
    assert_eq!(f.state(task), TaskState::InProgress, "and the sender is released");
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

/// The reason given when the room withdrew `handoff`, if it did.
fn withdrawal_reason(events: &[EventEnvelope], handoff: HandoffId) -> Option<String> {
    events.iter().find_map(|e| match &e.event {
        Event::HandoffTransitioned {
            handoff: id,
            transition,
            ..
        } if *id == handoff => match transition.as_ref() {
            HandoffTransition::Withdraw { reason, .. } => Some(reason.clone()),
            _ => None,
        },
        _ => None,
    })
}

/// From the live room: @opus offered @grok-quick a delegation and then
/// released the task. Accepting needs the sender to hold the task, so the
/// offer could never be taken. @grok-quick tried, claimed the task, tried
/// again, and rejected the offer: "the card was unowned, then held by me
/// rather than the sender."
#[test]
fn releasing_a_delegated_task_withdraws_the_offer() {
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

    let events = f.apply(Command::ReleaseTask {
        task,
        by: worker,
        reason: "this is theirs to do".into(),
    });
    assert_eq!(
        f.engine.handoff(handoff).unwrap().state,
        HandoffState::Withdrawn,
        "an offer nobody can accept must not stay open"
    );
    let reason = withdrawal_reason(&events, handoff).expect("withdrawn with the release");
    assert!(
        reason.contains("no longer holds") && reason.contains("back on the board"),
        "the reason says what happened: {reason}"
    );

    // The recipient takes the work the ordinary way.
    assert!(f.engine.offers_for(worker2).is_empty());
    f.apply(Command::ClaimTask { task, by: worker2 });
    assert_eq!(f.engine.task(task).unwrap().assignee, Some(worker2));
}

/// Losing the task counts as much as letting it go.
#[test]
fn a_delegation_is_withdrawn_when_the_senders_claim_lapses() {
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
        timeout_ms: Some(60 * 60 * 1_000),
    });
    let handoff = offered_handoff(&events);

    // Past the claim's window, well inside the offer's.
    let events = f.apply(Command::Tick {
        now: Timestamp::now().plus_millis(DEFAULT_LEASE_MILLIS + 60_000),
    });
    assert_eq!(f.engine.task(task).unwrap().assignee, None, "the claim lapsed");
    assert_eq!(
        f.engine.handoff(handoff).unwrap().state,
        HandoffState::Withdrawn
    );
    assert!(withdrawal_reason(&events, handoff).is_some());
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
            permanent: false,
        });
    }

    assert_eq!(
        f.engine.status(worker),
        orgbot_proto::agent::AgentStatus::Faulted
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
fn retrying_an_answer_after_a_lost_ack_is_idempotent() {
    let mut f = fixture();
    let task = f.open_task("provision the VPS", None);
    let decision = asked_decision(&f.gate(task, "it provisions paid infrastructure"));
    f.approve(decision);

    let retry = f.apply(Command::AnswerDecision {
        decision,
        answer: Answer {
            chosen: "Decline".into(),
            note: String::new(),
            at: Timestamp::now(),
        },
    });
    assert!(retry.is_empty(), "a duplicate answer must not wake anyone twice");
    assert_eq!(f.engine.task(task).unwrap().authorization, AuthorizationState::Authorized);
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
    // Anything added to the engine since; extend this as fields are added.
    fields.remove("decisions");

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

// --------------------------------------------- the rest of the lifecycle
//
// Two readings the board could not previously make, plus the scoping and
// authority properties the doc asks for.

/// "In progress" hides whether work is advancing or going back over ground.
#[test]
fn rework_reads_differently_from_first_pass_work() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.start_task("write the notes", worker);

    assert!(matches!(
        f.engine.task(task).unwrap().readiness(),
        Readiness::Underway { .. }
    ));

    f.reject_with(task, worker, "the parser drops commas", "parse.rs");

    match f.engine.task(task).unwrap().readiness() {
        Readiness::ReviewRemediation { round } => assert_eq!(round, 1),
        other => panic!("expected rework, got {other:?}"),
    }
    // Still the room's own work, and still progressing.
    assert!(f.engine.task(task).unwrap().readiness().is_progressing());
    assert!(!f.engine.task(task).unwrap().needs_operator());
}

/// Something outstanding that does not strictly block should not stop work.
#[test]
fn a_soft_dependency_lets_work_start_and_says_it_is_provisional() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.open_task("draft the notes", None);
    let upstream = TaskId::new();

    f.engine.task_mut(task).unwrap().dependency = DependencyState::Conditional {
        on: upstream,
        reason: "the final numbers".into(),
    };

    let readiness = f.engine.task(task).unwrap().readiness();
    assert!(readiness.is_claimable(), "work can start");
    assert!(matches!(readiness, Readiness::ConditionallyReady { .. }));
    assert!(
        readiness.summary().contains("may mean revisiting"),
        "and the board says the start is provisional: {}",
        readiness.summary()
    );
    assert!(f.engine.claimable_by(worker).contains(&task));
}

/// An exact input that has been replaced is a reason to look again.
#[test]
fn work_built_on_a_replaced_input_is_flagged() {
    let mut f = fixture();
    let worker = f.worker;
    let source = f.start_task("write the schema", worker);
    let original = f.add_artifact(source, worker);
    let corrected = f.add_artifact(source, worker);

    let dependent = f.open_task("build on the schema", None);
    f.engine.task_mut(dependent).unwrap().requires_artifacts = vec![original];
    assert_eq!(f.engine.inconsistencies(), vec![]);

    // The input is corrected; the work built on it is now suspect.
    f.engine.artifact_mut(original).unwrap().superseded_by = Some(corrected);

    let found = f.engine.inconsistencies();
    assert!(
        found.iter().any(|v| v.rule == "input-is-current"),
        "got {found:?}"
    );
}

/// Everything a room owns is scoped to it.
#[test]
fn a_rooms_work_carries_its_own_identity() {
    let mut f = fixture();
    let worker = f.worker;
    let room = f.engine.room_id();
    let task = f.start_task("write the notes", worker);
    let artifact = f.add_artifact(task, worker);
    let gate = f.open_task("provision the VPS", None);
    f.gate(gate, "it provisions paid infrastructure");

    assert_eq!(f.engine.task(task).unwrap().room, room);
    assert_eq!(f.engine.artifact(artifact).unwrap().room, Some(room));
    assert!(f
        .engine
        .decisions()
        .decisions
        .iter()
        .all(|decision| decision.room == room));
}

/// A rule in the room's state beats anything said in chat.
#[test]
fn a_policy_hold_outranks_conversation() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.open_task("deploy it", None);

    f.apply(Command::SetAuthorization {
        task,
        authorization: AuthorizationState::HeldByPolicy {
            policy: "no-deploys-on-friday".into(),
            reason: "the freeze is in force".into(),
        },
    });

    // Anyone may say anything; the board does not care.
    f.apply(Command::PostMessage {
        message: Box::new(ChatMessage {
            id: orgbot_proto::MessageId::new(),
            room: f.engine.room_id(),
            author: orgbot_proto::chat::Author::Human {
                name: "someone".into(),
            },
            audience: orgbot_proto::chat::Audience::Everyone,
            intent: MessageIntent::Say,
            body: "go ahead and deploy it anyway".into(),
            task: Some(task),
            handoff: None,
            reply_to: None,
            attachments: vec![],
            files: vec![],
            sent_at: Timestamp::now(),
        }),
    });

    assert!(!f.engine.claimable_by(worker).contains(&task));
    let err = f.reject(Command::ClaimTask { task, by: worker });
    assert!(matches!(err, EngineError::NotReady { .. }), "got {err:?}");
}

/// "Assigned" and "being worked on" are different things, and the record says
/// which — without disturbing the structured link the engine uses to wake the
/// sender when the handoff completes.
#[test]
fn an_accepted_handoff_records_who_took_it() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.start_task("write the notes", worker);

    let events = f.apply(Command::OfferHandoff {
        task,
        from: worker,
        to: Recipient::Agent { agent: worker2 },
        kind: HandoffKind::Assist,
        brief: "please draft the summary".into(),
        expects: vec![],
        context: vec![],
        blocks_sender: Some(true),
        timeout_ms: None,
    });
    let handoff = offered_handoff(&events);

    // Offered: nobody has it yet.
    let offered = f.engine.handoff(handoff).unwrap();
    assert_eq!(offered.state, HandoffState::Offered);
    assert_eq!(offered.accepted_by, None);

    f.apply(Command::AcceptHandoff {
        handoff,
        by: worker2,
    });

    // Accepted: the record names who took it, so a board can say so rather
    // than reporting only that a handoff exists.
    let accepted = f.engine.handoff(handoff).unwrap();
    assert_eq!(accepted.state, HandoffState::Accepted);
    assert_eq!(accepted.accepted_by, Some(worker2));

    // And the sender is still linked to it, so completing it still wakes them.
    assert!(f
        .engine
        .task(task)
        .unwrap()
        .blocked_on
        .iter()
        .any(|r| matches!(r, BlockReason::AwaitingHandoff { handoff: h } if *h == handoff)));

    f.apply(Command::CompleteHandoff {
        handoff,
        by: worker2,
        artifacts: vec![],
        summary: "done".into(),
    });
    assert_eq!(f.state(task), TaskState::InProgress, "the sender wakes");
}

// ------------------------------------------- why a room stops doing anything
//
// A real room sat with nine bots free and eight tasks open for eight hours.
// Half the work required roles whose only holder was busy; the other half was
// open to a bot that kept replying "nothing actionable for me". Two different
// faults that look identical from a count of idle bots beside open tasks.

/// Work nobody free can do is a roster problem, not a scheduling one.
#[test]
fn work_no_free_bot_can_cover_is_named_as_such() {
    let mut f = fixture();
    let worker = f.worker;

    // Both writers take something, then more writer work appears.
    let worker2 = f.worker2;
    let first = f.open_task("first writing job", Some("writer"));
    let second = f.open_task("second writing job", Some("writer"));
    f.apply(Command::ClaimTask { task: first, by: worker });
    f.apply(Command::ClaimTask {
        task: second,
        by: worker2,
    });
    f.open_task("third writing job", Some("writer"));
    f.open_task("fourth writing job", Some("writer"));

    let starved = f.engine.starved_roles();
    assert_eq!(
        starved,
        vec![("writer".to_string(), 2)],
        "two tasks want a writer and no free bot is one"
    );

    let utilization = f.engine.utilization();
    assert!(utilization.idle > 0, "other bots are free");
    assert!(
        utilization.is_short_handed(),
        "the roster is the problem here"
    );
    assert!(
        !utilization.is_wasting_capacity(),
        "and calling it a scheduling failure would send someone looking in \
         the wrong place"
    );
}

/// Declaring a stand-in turns a roster problem back into available work.
#[test]
fn a_fallback_role_relieves_the_starvation() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let first = f.open_task("first writing job", Some("writer"));
    let second = f.open_task("second writing job", Some("writer"));
    f.apply(Command::ClaimTask { task: first, by: worker });
    f.apply(Command::ClaimTask {
        task: second,
        by: worker2,
    });
    f.open_task("third writing job", Some("writer"));

    assert!(!f.engine.starved_roles().is_empty());

    f.engine.set_roles(RoleTable::new(vec![
        RoleSpec::new("writer").falling_back_to(["reviewer"])
    ]));

    assert_eq!(
        f.engine.starved_roles(),
        vec![],
        "the inspector can stand in, so nothing is starved"
    );
    assert!(f.engine.utilization().is_wasting_capacity());
}

/// Genuinely idle bots beside work they could take is a scheduling failure.
#[test]
fn free_bots_beside_work_they_can_take_is_wasting_capacity() {
    let mut f = fixture();
    f.open_task("anyone could do this", None);

    let utilization = f.engine.utilization();
    assert!(utilization.is_wasting_capacity());
    assert!(!utilization.is_short_handed());
}

// ------------------------------------------------- a bot that cannot run
//
// One bot in a real room was configured with a model its account is not
// offered. Every turn it took was rejected identically, three times before it
// faulted, and in between it sat in the roster looking free while being unable
// to do anything at all.

/// A misconfiguration does not get three goes.
#[test]
fn a_bot_that_cannot_run_faults_on_the_first_attempt() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.open_task("write the notes", None);
    f.apply(Command::ClaimTask { task, by: worker });

    f.apply(Command::ReportError {
        agent: worker,
        task: Some(task),
        error: "codex rejected the request: the model `gpt-5.4-mini` is not \
                supported when using Codex with a ChatGPT account"
            .into(),
        permanent: true,
    });

    assert_eq!(
        f.engine.status(worker),
        orgbot_proto::agent::AgentStatus::Faulted,
        "a request that can never succeed must not be retried"
    );
    assert_eq!(
        f.state(task),
        TaskState::Ready,
        "and whatever it held goes back to the pool"
    );
}

/// A claim survives a pause and a long turn: bots cannot report progress while
/// the room is paused, and an agent CLI can work for a long time on files
/// without touching the board.
#[test]
fn claims_do_not_lapse_across_a_pause_or_a_long_turn() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.start_task("finish the web game", worker);
    let at = |minutes: i64| Timestamp::now().plus_millis(minutes * 60_000);

    // Paused past the lease: nothing is reclaimed while paused.
    f.apply(Command::PauseRoom { reason: "owner".into() });
    f.apply(Command::Tick { now: at(20) });
    assert_eq!(f.state(task), TaskState::InProgress, "kept while paused");

    // Resuming gives the claim a fresh window.
    f.apply(Command::ResumeRoom);
    f.apply(Command::Tick { now: at(10) });
    assert_eq!(f.state(task), TaskState::InProgress, "renewed on resume");

    // A turn that ends without touching the board still renews the claim.
    f.apply(Command::RecordTurn {
        agent: worker,
        usage: Default::default(),
    });
    f.apply(Command::Tick { now: at(14) });
    assert_eq!(f.state(task), TaskState::InProgress, "renewed by a finished turn");

    // A holder that really goes quiet still loses it.
    f.apply(Command::Tick { now: at(16) });
    assert_eq!(f.state(task), TaskState::Ready, "a lapsed claim returns to the pool");
}

/// From the live room: Miles blocked "finish the web game" on a teammate's
/// handoff. The claim kept counting down while it waited; the handoff came
/// back fourteen minutes later, every slot was busy, and the room took the
/// task away before Miles got a turn. Waiting on someone else is not walking
/// away from the work.
#[test]
fn a_claim_does_not_lapse_while_its_holder_waits_on_a_teammate() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let task = f.start_task("finish the web game", worker);
    let at = |minutes: i64| Timestamp::now().plus_millis(minutes * 60_000);
    let claimed_until = f.engine.task(task).unwrap().lease_expires_at.unwrap();

    let events = f.apply(Command::OfferHandoff {
        task,
        from: worker,
        to: Recipient::Agent { agent: worker2 },
        kind: HandoffKind::Assist,
        brief: "finish the party queue first".into(),
        expects: vec![],
        context: vec![],
        blocks_sender: None,
        timeout_ms: Some(60 * 60_000),
    });
    let handoff = offered_handoff(&events);
    f.apply(Command::AcceptHandoff {
        handoff,
        by: worker2,
    });
    assert_eq!(f.state(task), TaskState::Blocked);
    f.apply(Command::Tick { now: at(20) });
    assert_eq!(f.state(task), TaskState::Blocked, "kept while waiting");
    assert_eq!(f.engine.task(task).unwrap().assignee, Some(worker));

    // The teammate delivers and the room unblocks the task by itself; the
    // holder gets a whole new window to come back to it.
    std::thread::sleep(std::time::Duration::from_millis(5));
    f.apply(Command::CompleteHandoff {
        handoff,
        by: worker2,
        artifacts: vec![],
        summary: "party queue done".into(),
    });
    assert_eq!(f.state(task), TaskState::InProgress);
    assert!(
        f.engine.task(task).unwrap().lease_expires_at.unwrap() > claimed_until,
        "unblocking starts the claim fresh"
    );
}

/// A bot whose CLI sign-in expired comes back by itself once the owner signs
/// in again, instead of staying faulted until the room is paused and resumed.
#[test]
fn a_faulted_bot_is_retried_on_a_growing_schedule() {
    use orgbot_proto::agent::AgentStatus;
    let mut f = fixture();
    let worker = f.worker;
    let expired = || Command::ReportError {
        agent: worker,
        task: None,
        error: "CLI reported an error: Failed to authenticate: OAuth session expired".into(),
        permanent: true,
    };
    let tick = |f: &mut Fixture, minutes: i64| {
        f.apply(Command::Tick {
            now: Timestamp::now().plus_millis(minutes * 60_000 + 5_000),
        });
    };

    f.apply(expired());
    assert_eq!(f.engine.status(worker), AgentStatus::Faulted);
    tick(&mut f, 0);
    assert_eq!(f.engine.status(worker), AgentStatus::Faulted, "not before a minute");
    tick(&mut f, 1);
    assert_ne!(f.engine.status(worker), AgentStatus::Faulted, "retried after a minute");

    // Still signed out: one attempt, then faulted again and a longer wait.
    f.apply(expired());
    assert_eq!(f.engine.status(worker), AgentStatus::Faulted);
    tick(&mut f, 1);
    assert_eq!(f.engine.status(worker), AgentStatus::Faulted, "the second wait is longer");
    tick(&mut f, 2);
    assert_ne!(f.engine.status(worker), AgentStatus::Faulted);

    // Signed in again: a successful turn clears the schedule.
    f.apply(Command::RecordTurn {
        agent: worker,
        usage: Default::default(),
    });
    f.apply(expired());
    tick(&mut f, 1);
    assert_ne!(
        f.engine.status(worker),
        AgentStatus::Faulted,
        "after a success the next fault waits a minute again"
    );
}

/// A network blip still gets its retries.
#[test]
fn a_transient_failure_is_still_retried() {
    let mut f = fixture();
    let worker = f.worker;

    f.apply(Command::ReportError {
        agent: worker,
        task: None,
        error: "transport error talking to codex: request timed out".into(),
        permanent: false,
    });

    assert_ne!(
        f.engine.status(worker),
        orgbot_proto::agent::AgentStatus::Faulted,
        "one timeout is not a reason to give up on a bot"
    );
}

/// A faulted bot is not free capacity, however idle it looks.
#[test]
fn a_faulted_bot_is_not_counted_as_available() {
    let mut f = fixture();
    let worker = f.worker;
    f.open_task("anyone could do this", None);

    let before = f.engine.utilization();
    assert!(before.bots.iter().any(|bot| bot.free));

    f.apply(Command::ReportError {
        agent: worker,
        task: None,
        error: "the model does not exist".into(),
        permanent: true,
    });

    let after = f.engine.utilization();
    let state = after
        .bots
        .iter()
        .find(|bot| bot.agent == worker)
        .expect("still on the roster");
    assert_eq!(state.doing, "faulted");
    assert!(
        !state.free,
        "a bot that cannot take a turn is not spare capacity"
    );
}

// ------------------------------------------------------------ server access

fn answer(f: &mut Fixture, decision: orgbot_proto::DecisionId, chosen: &str) {
    f.apply(Command::AnswerDecision {
        decision,
        answer: Answer {
            chosen: chosen.into(),
            note: String::new(),
            at: Timestamp::now(),
        },
    });
}

fn ask_for_server(f: &mut Fixture, task: TaskId, by: AgentId) -> orgbot_proto::DecisionId {
    let events = f.apply(Command::RequestServerAccess {
        by,
        task,
        reason: "deploy the reviewed build to the game server".into(),
    });
    asked_decision(&events)
}

/// From the live room: a bot installed a server release over SSH while the
/// rest of the room had that release on hold, because every sandbox carried
/// the bot server key. Now only the owner grants it, for one bot and one
/// task, and only while that task is under way.
#[test]
fn the_server_key_is_granted_by_the_owner_for_one_task_while_it_is_under_way() {
    use orgbot_proto::decision::SERVER_ACCESS_ALLOW;
    let mut f = fixture();
    let (worker, worker2, inspector) = (f.worker, f.worker2, f.inspector);
    let task = f.start_task("deploy the release", worker);
    assert!(!f.engine.server_access_for(worker), "nobody has the key by default");

    let decision = ask_for_server(&mut f, task, worker);
    assert_eq!(f.state(task), TaskState::Blocked, "the task waits on the owner");
    assert!(!f.engine.server_access_for(worker), "asking is not being allowed");
    let again = f.apply(Command::RequestServerAccess {
        by: worker,
        task,
        reason: "deploy the reviewed build to the game server".into(),
    });
    assert!(
        !again.iter().any(|e| matches!(e.event, Event::DecisionAsked { .. })),
        "the same request is put to the owner once"
    );

    answer(&mut f, decision, SERVER_ACCESS_ALLOW);
    assert_eq!(f.state(task), TaskState::InProgress);
    assert!(f.engine.server_access_for(worker));
    assert!(!f.engine.server_access_for(worker2), "only the bot that was allowed");
    assert!(!f.engine.server_access_for(inspector));

    // A restored room grants exactly what the owner allowed.
    let restored = Engine::from_archive(f.engine.room().clone(), f.engine.log().to_vec())
        .expect("history replays");
    assert!(restored.server_access_for(worker));
    assert!(!restored.server_access_for(worker2));

    // Blocked, the key goes; unblocked, it comes back.
    f.apply(Command::BlockTask {
        task,
        by: worker,
        reason: BlockReason::Other {
            detail: "waiting for the database backup".into(),
        },
    });
    assert!(!f.engine.server_access_for(worker));
    f.apply(Command::UnblockTask { task });
    assert!(f.engine.server_access_for(worker));

    // A hold takes it away too.
    f.apply(Command::SetAuthorization {
        task,
        authorization: AuthorizationState::HeldByPolicy {
            policy: "release freeze".into(),
            reason: "the coordinator put the release on hold".into(),
        },
    });
    assert!(!f.engine.server_access_for(worker));
    f.apply(Command::SetAuthorization {
        task,
        authorization: AuthorizationState::Authorized,
    });
    assert!(f.engine.server_access_for(worker));

    // In review the work is done with: no key.
    let artifact = f.add_artifact(task, worker);
    f.apply(Command::SubmitForReview {
        task,
        by: worker,
        artifacts: vec![artifact],
    });
    assert!(!f.engine.server_access_for(worker));
}

#[test]
fn a_denied_request_gives_no_server_key_and_lets_the_work_go_on() {
    use orgbot_proto::decision::SERVER_ACCESS_DENY;
    let mut f = fixture();
    let worker = f.worker;
    let task = f.start_task("deploy the release", worker);
    let decision = ask_for_server(&mut f, task, worker);
    answer(&mut f, decision, SERVER_ACCESS_DENY);
    assert_eq!(f.state(task), TaskState::InProgress);
    assert!(!f.engine.server_access_for(worker));
}

#[test]
fn only_the_bot_holding_the_task_may_ask_for_the_server_key() {
    let mut f = fixture();
    let (worker, worker2, inspector) = (f.worker, f.worker2, f.inspector);
    let task = f.start_task("deploy the release", worker);
    let reason = || "deploy the reviewed build to the game server".to_string();
    f.reject(Command::RequestServerAccess {
        by: worker2,
        task,
        reason: reason(),
    });
    f.reject(Command::RequestServerAccess {
        by: inspector,
        task,
        reason: reason(),
    });
    f.reject(Command::RequestServerAccess {
        by: worker,
        task,
        reason: "  ".into(),
    });
}

/// From the live room: a Codex bot faulted on a temporary capacity error and
/// waited eleven minutes, behind other bots' long turns, for a retry due
/// after one. The run now asks whether a retry is due and ticks when it is.
#[test]
fn a_faulted_bot_says_when_its_retry_is_due() {
    let mut f = fixture();
    let worker = f.worker;
    let now = Timestamp::now();
    assert!(!f.engine.retry_due(now), "nobody is faulted");
    f.apply(Command::ReportError {
        agent: worker,
        task: None,
        error: "selected model is at capacity".into(),
        permanent: true,
    });
    assert_eq!(f.engine.status(worker), orgbot_proto::AgentStatus::Faulted);
    assert!(!f.engine.retry_due(Timestamp::now()), "not before its wait is over");
    let later = Timestamp::now().plus_millis(61_000);
    assert!(f.engine.retry_due(later));
    f.apply(Command::Tick { now: later });
    assert_eq!(f.engine.status(worker), orgbot_proto::AgentStatus::Idle);
    assert!(!f.engine.retry_due(later), "once retried, nothing is due");
}

// ------------------------------------------------------------ retiring tasks

/// From the live room: an obsolete task stayed open and woke idle bots every
/// turn, and the bots asked the owner to archive it in an app with no way to.
#[test]
fn a_coordinator_proposes_retiring_a_task_and_the_owners_answer_retires_it() {
    use orgbot_proto::decision::RETIRE_TASK_ALLOW;
    let mut f = fixture();
    let lead = f.coordinator;
    let task = f.open_task("try a shadow experiment for the stutter", None);
    let events = f.apply(Command::ProposeRetire {
        by: lead,
        task,
        reason: "the owner chose to keep High, which replaced this experiment".into(),
    });
    let decision = asked_decision(&events);
    assert_eq!(f.state(task), TaskState::Ready, "nothing changes until the owner answers");

    answer(&mut f, decision, RETIRE_TASK_ALLOW);
    assert_eq!(f.state(task), TaskState::Retired);
    let restored = Engine::from_archive(f.engine.room().clone(), f.engine.log().to_vec())
        .expect("history replays");
    assert_eq!(restored.task(task).unwrap().state, TaskState::Retired);
    f.reject(Command::ProposeRetire {
        by: lead,
        task,
        reason: "again".into(),
    });
}

#[test]
fn keeping_a_task_proposed_for_retirement_leaves_it_as_it_was() {
    use orgbot_proto::decision::RETIRE_TASK_KEEP;
    let mut f = fixture();
    let (lead, worker) = (f.coordinator, f.worker);
    let task = f.start_task("try a shadow experiment for the stutter", worker);
    let events = f.apply(Command::ProposeRetire {
        by: lead,
        task,
        reason: "looks superseded".into(),
    });
    answer(&mut f, asked_decision(&events), RETIRE_TASK_KEEP);
    assert_eq!(f.state(task), TaskState::InProgress);
    assert_eq!(f.engine.task(task).unwrap().assignee, Some(worker));
}

#[test]
fn only_a_coordinator_may_propose_retiring_a_task() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.open_task("try a shadow experiment for the stutter", None);
    f.reject(Command::ProposeRetire {
        by: worker,
        task,
        reason: "I think it is stale".into(),
    });
}

/// From the live room: a bot's commits stayed in its own copy, so it handed
/// the owner a bundle and a script to run.
#[test]
fn landing_commits_needs_the_owners_yes_for_exactly_those_commits() {
    use orgbot_proto::decision::{LAND_COMMITS_ALLOW, LAND_COMMITS_DENY};
    let mut f = fixture();
    let (worker, worker2, inspector) = (f.worker, f.worker2, f.inspector);
    let task = f.start_task("commit the release in separate commits", worker);
    let (base, head, other) = ("a".repeat(40), "b".repeat(40), "c".repeat(40));
    let request = |by: AgentId, branch: &str, head: &str| Command::RequestLanding {
        by,
        task,
        branch: branch.into(),
        base: "a".repeat(40),
        head: head.into(),
        commits: vec!["bbbbbbb tests: add the squad test [skip ci]".into()],
        changes: "1 file changed, 40 insertions(+)".into(),
        reason: "the owner asked for separate commits".into(),
    };
    let asked = f.apply(request(worker, "main", &head));
    let question = f.engine.decisions().pending().next().unwrap().because.clone();
    assert!(question.contains("land 1 commit on main") && question.contains("Nothing is pushed") && question.contains("add the squad test"), "{question}");
    assert!(!f.engine.landing_approved(task, worker, "main", &base, &head), "not before the owner answers");
    assert_eq!(
        f.engine.digest().needs_you[0].grants_with.as_deref(),
        Some(LAND_COMMITS_ALLOW),
        "landing takes a second tap in the app"
    );

    answer(&mut f, asked_decision(&asked), LAND_COMMITS_ALLOW);
    assert!(f.engine.landing_approved(task, worker, "main", &base, &head));
    assert!(!f.engine.landing_approved(task, worker, "main", &base, &other), "only those commits");
    assert!(!f.engine.landing_approved(task, worker, "release", &base, &head), "only onto that branch");
    assert!(!f.engine.landing_approved(task, worker2, "main", &base, &head), "only for that bot");
    assert!(f.apply(request(worker, "main", &head)).is_empty(), "asking again for the same is not a new question");

    let refused = f.apply(request(worker, "main", &other));
    answer(&mut f, asked_decision(&refused), LAND_COMMITS_DENY);
    assert!(!f.engine.landing_approved(task, worker, "main", &base, &other));

    let not_holding = f.reject(request(inspector, "main", &head));
    assert!(matches!(not_holding, EngineError::NotAssignee { .. }), "only the task's holder may ask: {not_holding:?}");
    f.reject(request(worker, "../main", &head));
    f.reject(request(worker, "main", &base));
}

/// From the live room: one release reached the owner as three cards in a row.
#[test]
fn a_bot_asks_one_question_per_task_at_a_time() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.start_task("publish Release 2", worker);
    let ask = |question: &str| Command::AskOwner {
        by: worker,
        task: Some(task),
        question: question.into(),
        options: vec!["Yes".into(), "Not yet".into()],
    };
    let first = asked_decision(&f.apply(ask("Publish Release 2 now?")));
    let refused = f.reject(ask("Approve the publication checks?"));
    assert!(refused.to_string().contains("already have a question about this task"), "{refused}");
    f.apply(ask("Publish Release 2 now?"));
    assert_eq!(f.engine.decisions().pending().count(), 1, "the same question joins the one asked");

    answer(&mut f, first, "Yes");
    asked_decision(&f.apply(ask("Approve the publication checks?")));
}

/// From the live room: a gate written into a brief held nothing back, so a
/// bot claimed the task, could not start it, and was woken eleven times.
#[test]
fn a_coordinator_makes_a_task_wait_for_others_with_amend_task() {
    let mut f = fixture();
    let (lead, worker) = (f.coordinator, f.worker);
    let docs = f.open_task("record the docs", None);
    let commit = f.open_task("commit the docs and Renderer cleanup", None);
    f.apply(Command::AmendTask {
        by: lead,
        task: commit,
        brief: None,
        acceptance_criteria: None,
        depends_on: Some(vec![docs]),
        reason: "commit only after the docs card is approved".into(),
    });
    assert!(f.engine.waits_on_dependency(commit));
    assert!(!f.engine.claimable_by(worker).contains(&commit), "nobody can claim it yet");
    f.reject(Command::ClaimTask { task: commit, by: worker });
    assert!(f.engine.transcript().iter().any(|m| m.body.contains("It now waits for \"record the docs\"")));

    // Waiting for itself, or for a task that waits for it, is refused.
    let cycle = f.reject(Command::AmendTask {
        by: lead,
        task: docs,
        brief: None,
        acceptance_criteria: None,
        depends_on: Some(vec![commit]),
        reason: "backwards".into(),
    });
    assert!(cycle.to_string().contains("never end"), "{cycle}");

    // A saved room keeps the dependency.
    let restored = Engine::from_archive(f.engine.room().clone(), f.engine.log().to_vec()).unwrap();
    assert!(restored.waits_on_dependency(commit));
}

/// From the live room: every message a CLI bot posted ended with a reminder
/// that file removals awaited the owner, appended by OrgBots itself.
#[test]
fn pending_removals_are_counted_for_the_app_and_only_changes_are_announced() {
    let mut f = fixture();
    let worker = f.worker;
    let record = |count: u32| Command::RecordPendingRemovals { agent: worker, copy: "room tools".into(), count };
    assert_eq!(f.apply(record(1549)).len(), 1);
    assert!(f.apply(record(1549)).is_empty(), "the same count is not announced again");
    f.apply(Command::RecordPendingRemovals { agent: worker, copy: "its own CLI".into(), count: 3 });
    let digest = f.engine.digest();
    assert_eq!(digest.pending_removals.len(), 2);
    assert_eq!(digest.pending_removals.iter().map(|row| row.count).sum::<u32>(), 1552);
    assert!(digest.pending_removals.iter().all(|row| row.handle == "w1"));

    let restored = Engine::from_archive(f.engine.room().clone(), f.engine.log().to_vec()).unwrap();
    assert_eq!(restored.digest().pending_removals.len(), 2);

    f.apply(record(0));
    assert_eq!(f.engine.digest().pending_removals.len(), 1, "zero clears that copy");
    f.reject(Command::RecordPendingRemovals { agent: worker, copy: "somewhere".into(), count: 1 });
}

/// From the live room: a bot asked to land commits for a task it had claimed
/// and not started. The card reached the owner, the task could not wait on it,
/// and the bot was told the request failed and was woken eleven times.
#[test]
fn asking_for_something_on_a_claimed_task_starts_it_and_waits_for_the_answer() {
    let mut f = fixture();
    let worker = f.worker;
    let task = f.open_task("commit the release in separate commits", None);
    f.apply(Command::ClaimTask { task, by: worker });
    f.apply(Command::RequestLanding {
        by: worker,
        task,
        branch: "main".into(),
        base: "a".repeat(40),
        head: "b".repeat(40),
        commits: vec!["bbbbbbb docs: record the commit set [skip ci]".into()],
        changes: "1 file changed".into(),
        reason: "the owner asked for separate commits".into(),
    });
    assert_eq!(f.state(task), TaskState::Blocked, "it waits on the owner, so nothing wakes it meanwhile");
    assert!(f.engine.awaiting_owner(worker));
    assert_eq!(f.engine.decisions().pending().count(), 1);

    // Work that cannot wait is refused before the owner is asked anything.
    let reviewed = f.start_task("release notes", worker);
    let artifact = f.add_artifact(reviewed, worker);
    f.apply(Command::SubmitForReview { task: reviewed, by: worker, artifacts: vec![artifact] });
    f.reject(Command::RequestLanding {
        by: worker,
        task: reviewed,
        branch: "main".into(),
        base: "a".repeat(40),
        head: "c".repeat(40),
        commits: vec!["ccccccc notes [skip ci]".into()],
        changes: "1 file changed".into(),
        reason: "notes".into(),
    });
    assert_eq!(f.engine.decisions().pending().count(), 1, "no orphan question");
}

/// From the live room: two coordinators woke to the same owner message, and
/// the one whose turn began first created five duplicates of the other's tasks
/// fifteen minutes later; two bots then took the same job.
#[test]
fn a_bot_creating_work_is_shown_once_what_others_added_during_its_turn() {
    let mut f = fixture();
    let lead = f.coordinator;
    let create = |title: &str, by: Option<AgentId>| Command::CreateTask {
        title: title.into(),
        brief: String::new(),
        by,
        required_role: None,
        priority: Priority::Normal,
        acceptance_criteria: vec![],
        depends_on: vec![],
        origin: TaskOrigin::plain(),
    };
    f.apply(Command::BeginTurn { agent: lead });
    f.apply(create("Split the release commit", Some(lead)));
    std::thread::sleep(std::time::Duration::from_millis(5));
    let theirs = created_task(&f.apply(create("Curfew: the closing cordon becomes a wall of tear gas", None)));

    let refused = f.reject(create("Redo the Curfew cordon as a tear-gas bank", Some(lead)));
    assert!(matches!(refused, EngineError::BoardChanged { .. }), "{refused:?}");
    let shown = refused.to_string();
    assert!(shown.contains(&theirs.to_string()) && shown.contains("wall of tear gas") && shown.contains("by the owner"), "{shown}");
    assert!(!shown.contains("Split the release commit"), "its own work is not news: {shown}");

    // Seen now: the same tasks never stop it twice, and a new turn starts clean.
    f.apply(create("Redo the Curfew cordon as a tear-gas bank", Some(lead)));
    f.apply(Command::BeginTurn { agent: lead });
    f.apply(create("Record Release 2", Some(lead)));
}

/// From the live room: the owner said to leave launch work parked, a builder
/// claimed it anyway, and the coordinator asked the owner about it again.
#[test]
fn work_the_owner_parks_is_left_alone_until_they_resume_it() {
    let mut f = fixture();
    let (worker, worker2, lead) = (f.worker, f.worker2, f.coordinator);
    let task = f.start_task("launch readiness: shop and ranked", worker);
    let asked = f.apply(Command::AskOwner {
        by: worker,
        task: Some(task),
        question: "Start the shop in sandbox, or keep it parked?".into(),
        options: vec!["Start the sandbox shop".into(), "Keep parked".into()],
    });
    let question = asked_decision(&asked);
    assert!(f.engine.awaiting_owner(worker));

    f.apply(Command::ParkTask {
        by: None,
        task,
        reason: "after the launch".into(),
    });
    let parked = f.engine.task(task).unwrap();
    assert_eq!(parked.state, TaskState::Parked);
    assert_eq!(parked.assignee, None, "nobody holds it, so nothing wakes for it");
    assert!(parked.blocked_on.is_empty());
    assert_eq!(f.engine.decisions().pending().count(), 0, "the question about it leaves Needs you");
    assert!(!f.engine.awaiting_owner(worker));
    assert!(f.engine.log().iter().any(|e| matches!(&e.event, Event::DecisionWithdrawn { decision, .. } if *decision == question)));

    // Nobody touches it, and nobody asks about it.
    for bot in [worker, worker2] {
        assert!(!f.engine.claimable_by(bot).contains(&task));
        let refused = f.reject(Command::ClaimTask { task, by: bot });
        assert!(refused.to_string().contains("parked"), "{refused}");
    }
    f.reject(Command::StartTask { task, by: worker });
    f.reject(Command::AddArtifact {
        task,
        author: worker,
        name: "notes.md".into(),
        body: ArtifactBody::Text { text: "still on it".into() },
        authority_bearing: false,
    });
    let asking = f.reject(Command::AskOwner {
        by: worker,
        task: Some(task),
        question: "Shall I start it now?".into(),
        options: vec![],
    });
    assert!(asking.to_string().contains("parked"), "{asking}");
    f.reject(Command::ProposeRetire { by: lead, task, reason: "it has waited a while".into() });
    f.reject(Command::ParkTask { by: None, task, reason: String::new() });

    // A saved room is still parked, with the question still withdrawn.
    let restored = Engine::from_archive(f.engine.room().clone(), f.engine.log().to_vec()).unwrap();
    assert_eq!(restored.task(task).unwrap().state, TaskState::Parked);
    assert_eq!(restored.decisions().pending().count(), 0);

    // The owner resumes it: open again, for whoever picks it up.
    f.apply(Command::ResumeTask { task });
    assert_eq!(f.state(task), TaskState::Ready);
    assert!(f.engine.claimable_by(worker2).contains(&task));
    f.reject(Command::ResumeTask { task });
}

#[test]
fn a_coordinator_parks_work_on_the_owners_word_and_only_before_review() {
    let mut f = fixture();
    let (worker, lead) = (f.worker, f.coordinator);
    let task = f.open_task("ranked hosted verification", None);
    let refused = f.reject(Command::ParkTask { by: Some(worker), task, reason: "the owner said later".into() });
    assert!(matches!(refused, EngineError::NotPermitted { .. }), "{refused:?}");
    let unexplained = f.reject(Command::ParkTask { by: Some(lead), task, reason: "  ".into() });
    assert!(matches!(unexplained, EngineError::InvalidOwnerQuestion { .. }), "{unexplained:?}");
    f.apply(Command::ParkTask { by: Some(lead), task, reason: "Aece: leave it parked".into() });
    assert_eq!(f.state(task), TaskState::Parked);
    assert!(f.engine.transcript().iter().any(|m| m.body.contains("@lead parked \"ranked hosted verification\": Aece: leave it parked")));

    // Work already in review is not parked.
    let reviewed = f.start_task("release notes", worker);
    let artifact = f.add_artifact(reviewed, worker);
    f.apply(Command::SubmitForReview { task: reviewed, by: worker, artifacts: vec![artifact] });
    f.reject(Command::ParkTask { by: None, task: reviewed, reason: String::new() });

    // The owner may park and resume while the room is paused.
    let later = f.open_task("store screenshots", None);
    f.apply(Command::PauseRoom { reason: "lunch".into() });
    f.apply(Command::ParkTask { by: None, task: later, reason: String::new() });
    f.apply(Command::ResumeTask { task: later });
}

#[test]
fn parking_one_task_keeps_a_question_another_live_task_waits_on() {
    let mut f = fixture();
    let (worker, worker2) = (f.worker, f.worker2);
    let first = f.start_task("rebuild the container", worker);
    let second = f.start_task("rebuild the container again", worker2);
    let question = "Rebuild on the approved VPS?";
    for (task, by) in [(first, worker), (second, worker2)] {
        f.apply(Command::AskOwner {
            by,
            task: Some(task),
            question: question.into(),
            options: vec!["Yes".into(), "No".into()],
        });
    }
    assert_eq!(f.engine.decisions().pending().count(), 1);
    f.apply(Command::ParkTask { by: None, task: first, reason: String::new() });
    assert_eq!(f.engine.decisions().pending().count(), 1, "the other task still needs the answer");
    assert!(f.engine.awaiting_owner(worker2));
}

#[test]
fn a_coordinator_amends_what_a_task_asks_for_and_the_reviewer_sees_it() {
    // From the live room: a coordinator waived a requirement in chat, the task
    // still listed it, and the reviewer sent the work back for it again.
    let mut f = fixture();
    let task = f.start_task("fix the squad count", f.worker);
    f.apply(Command::AmendTask {
        by: f.coordinator,
        task,
        brief: None,
        acceptance_criteria: Some(vec![
            "late parties are seated".into(),
            "a direct test covers the last-resort donor".into(),
        ]),
        depends_on: None,
        reason: "the join-level test is waived: the branch cannot be reached by a real join".into(),
    });
    assert_eq!(
        f.engine.task(task).unwrap().acceptance_criteria,
        vec!["late parties are seated".to_string(), "a direct test covers the last-resort donor".to_string()]
    );
    let told = f.engine.transcript().iter().any(|message| {
        message.body.contains("amended \"fix the squad count\"")
            && message.body.contains("2. a direct test covers the last-resort donor")
            && matches!(&message.audience, orgbot_proto::Audience::Agents { agents } if agents.contains(&f.worker))
    });
    assert!(told, "the holder is told what the task asks for now");

    // A saved room replays the amendment.
    let restored = Engine::from_archive(f.engine.room().clone(), f.engine.log().to_vec()).unwrap();
    assert_eq!(restored.task(task).unwrap().acceptance_criteria.len(), 2);

    // Only a coordinator may, and only with a reason and something to change.
    let refused = f.reject(Command::AmendTask {
        by: f.worker,
        task,
        brief: None,
        acceptance_criteria: Some(vec!["anything goes".into()]),
        depends_on: None,
        reason: "I would like an easier bar".into(),
    });
    assert!(matches!(refused, EngineError::NotPermitted { .. }), "{refused:?}");
    let empty = f.reject(Command::AmendTask {
        by: f.coordinator,
        task,
        brief: None,
        acceptance_criteria: None,
        depends_on: None,
        reason: "nothing really".into(),
    });
    assert!(matches!(empty, EngineError::InvalidOwnerQuestion { .. }), "{empty:?}");
}

#[test]
fn an_approved_goal_is_ready_to_deliver_only_once_nothing_under_it_is_unfinished() {
    // From the live room: an approved goal waited on one subtask still in
    // progress, and the delivering coordinators were woken every few minutes
    // only to say nothing needed doing.
    use orgbot_proto::graph::{TaskKind, TaskOrigin};
    let mut f = fixture();
    let (worker, worker2, inspector, lead) = (f.worker, f.worker2, f.inspector, f.coordinator);
    let goal = f.start_task("finish the web game", worker);

    let created = f.apply(Command::CreateTask {
        title: "Release 2".into(),
        brief: String::new(),
        by: None,
        required_role: None,
        priority: orgbot_proto::task::Priority::Normal,
        acceptance_criteria: vec![],
        depends_on: vec![],
        origin: TaskOrigin::under(TaskKind::Subtask, goal),
    });
    let child = created
        .iter()
        .find_map(|event| match &event.event {
            Event::TaskCreated { task } => Some(task.id),
            _ => None,
        })
        .unwrap();
    for command in [
        Command::OpenTask { task: child },
        Command::ClaimTask { task: child, by: worker2 },
        Command::StartTask { task: child, by: worker2 },
    ] {
        f.apply(command);
    }

    let approve = |f: &mut Fixture, task, by| {
        let artifact = f.add_artifact(task, by);
        f.apply(Command::SubmitForReview { task, by, artifacts: vec![artifact] });
        f.apply(Command::RecordReview {
            task,
            inspector,
            findings: vec![],
            summary: "matches the criteria".into(),
            criteria_met: vec!["it is correct".into()],
            criteria_failed: vec![],
        });
    };
    approve(&mut f, goal, worker);
    assert_eq!(f.state(goal), TaskState::Approved);
    assert_eq!(f.engine.awaiting_delivery(), vec![goal]);
    assert!(f.engine.ready_for_delivery().is_empty(), "not while its subtask is unfinished");
    assert_eq!(f.engine.delivery_blocker(goal).as_deref(), Some("Release 2"));

    approve(&mut f, child, worker2);
    f.apply(Command::SubmitTask { task: child, by: lead });
    assert_eq!(f.engine.ready_for_delivery(), vec![goal], "ready once its subtask has landed");
}
