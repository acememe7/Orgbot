//! The room that caused all of this, rebuilt as a fixture.
//!
//! A real run put sixteen bots on a dozen objectives and ended with ninety-six
//! work records, eleven future tasks all showing as ready, superseded work
//! still claimable, and a person being asked for files the room itself had
//! written. Every mechanism in this crate exists because of something in that
//! list.
//!
//! So the run is reconstructed here at roughly its real size and shape, and
//! the assertions are the properties that failed then. A change that quietly
//! reintroduces any of them fails here rather than on someone's phone.
//!
//! This is a shape test, not a simulation. No models run; the point is what
//! the board looks like to a person once the coordination has happened.

use orgbot_engine::{analytics, Command, Engine};
use orgbot_proto::agent::{AgentSpec, Autonomy, ProviderRef};
use orgbot_proto::decision::{Envelope, Policy};
use orgbot_proto::event::Event;
use orgbot_proto::graph::{AuthorizationState, TaskKind, TaskOrigin};
use orgbot_proto::review::{Finding, Severity};
use orgbot_proto::roles::{RoleSpec, RoleTable};
use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
use orgbot_proto::task::Priority;
use orgbot_proto::{AgentId, TaskId};

/// Sixteen bots, as the real room had.
const ROSTER: usize = 16;
/// Twelve real outcomes.
const OBJECTIVES: usize = 12;

struct Room {
    engine: Engine,
    lead: AgentId,
    workers: Vec<AgentId>,
    inspectors: Vec<AgentId>,
}

fn provider() -> ProviderRef {
    ProviderRef::Scripted {
        script: "stress".into(),
    }
}

fn room() -> Room {
    let mut engine = Engine::new(RoomSpec {
        name: "Leo room repair".into(),
        goal: "repair the Leo coding harness".into(),
        roster: vec![],
        completion: CompletionRule::AllTasksTerminal,
        limits: RoomLimits::default(),
    });

    // Roles that can cover for one another, as a sixteen-bot room needs.
    engine.set_roles(RoleTable::new(vec![
        RoleSpec::new("engineer")
            .able_to(["write", "fix"])
            .falling_back_to(["architect"]),
        RoleSpec::new("architect").able_to(["write", "design", "review"]),
        RoleSpec::new("reviewer")
            .able_to(["review", "audit"])
            .falling_back_to(["architect"]),
    ]));

    let lead = AgentSpec::new("lead", "Lead", provider())
        .with_roles(["coordinator"])
        .with_autonomy(Autonomy::coordinator());
    let lead_id = lead.id;
    engine
        .apply(Command::JoinAgent {
            spec: Box::new(lead),
        })
        .expect("lead joins");

    let mut workers = Vec::new();
    let mut inspectors = Vec::new();
    for index in 1..ROSTER {
        let reviewer = index % 4 == 0;
        let spec = if reviewer {
            AgentSpec::new(format!("qa{index}"), format!("Inspector {index}"), provider())
                .with_roles(["reviewer"])
                .with_autonomy(Autonomy::inspector())
        } else {
            AgentSpec::new(format!("eng{index}"), format!("Engineer {index}"), provider())
                .with_roles(["engineer"])
        };
        let id = spec.id;
        engine
            .apply(Command::JoinAgent {
                spec: Box::new(spec),
            })
            .expect("bot joins");
        if reviewer {
            inspectors.push(id);
        } else {
            workers.push(id);
        }
    }
    engine.apply(Command::StartRoom).expect("start");

    Room {
        engine,
        lead: lead_id,
        workers,
        inspectors,
    }
}

impl Room {
    fn create(&mut self, title: &str, origin: TaskOrigin, depends_on: Vec<TaskId>) -> TaskId {
        let events = self
            .engine
            .apply(Command::CreateTask {
                title: title.into(),
                brief: format!("brief for {title}"),
                by: Some(self.lead),
                required_role: None,
                priority: Priority::Normal,
                acceptance_criteria: vec!["it is correct".into()],
                depends_on,
                origin,
            })
            .unwrap_or_else(|e| panic!("creating `{title}` was refused: {e}"));
        let id = events
            .iter()
            .find_map(|e| match &e.event {
                Event::TaskCreated { task } => Some(task.id),
                _ => None,
            })
            .expect("a task was created");
        self.engine
            .apply(Command::OpenTask { task: id })
            .expect("open");
        id
    }

    fn objective(&mut self, title: &str) -> TaskId {
        self.create(
            title,
            TaskOrigin {
                kind: TaskKind::Objective,
                ..Default::default()
            },
            vec![],
        )
    }
}

/// Build the room the real run produced: twelve objectives, a dependency
/// chain, review cycles, replaced work, a security hold, and a real decision.
fn leo_room() -> Room {
    let mut room = room();
    let mut objectives = Vec::new();

    for index in 0..OBJECTIVES {
        let objective = room.objective(&format!("N{index}: repair stage {index}"));
        objectives.push(objective);

        // Implementation, then the review and remediation it produced.
        let implementation = room.create(
            &format!("implement stage {index}"),
            TaskOrigin::under(TaskKind::Subtask, objective),
            vec![],
        );
        room.create(
            &format!("review stage {index}"),
            TaskOrigin::review_of(implementation),
            vec![],
        );
        room.create(
            &format!("fix what review {index} found"),
            TaskOrigin::remediation_of(implementation),
            vec![],
        );
        // Evidence for the security claim on this stage.
        room.create(
            &format!("gather classification evidence for stage {index}"),
            TaskOrigin::under(TaskKind::Evidence, objective),
            vec![],
        );
    }

    // The chain that made eleven future tasks look ready at once.
    let mut previous = objectives[0];
    for (step, objective) in objectives.iter().enumerate().skip(1) {
        previous = room.create(
            &format!("C{step}: chained work {step}"),
            TaskOrigin::under(TaskKind::Subtask, *objective),
            vec![previous],
        );
    }

    // Work replaced partway through, as the real run did repeatedly.
    let abandoned = room.create(
        "IR-0: an early approach",
        TaskOrigin::under(TaskKind::Subtask, objectives[0]),
        vec![],
    );
    let canonical = room.create(
        "IR-1: the approach that replaced it",
        TaskOrigin::under(TaskKind::Subtask, objectives[0]),
        vec![],
    );
    room.engine
        .apply(Command::SupersedeTask {
            task: abandoned,
            replacement: canonical,
            reason: "merged into the canonical task".into(),
        })
        .expect("supersede");

    // A security hold for want of proof, which the room settles itself.
    room.engine
        .apply(Command::SetAuthorization {
            task: objectives[3],
            authorization: AuthorizationState::SecurityHold {
                finding: "need merged-filesystem classification".into(),
                material: false,
            },
        })
        .expect("hold");

    // A standing rule, and one question that genuinely needs a person.
    room.engine.adopt_policy(
        Policy::new("confined-docker", "Docker on the approved VPS is fine.").granting(Envelope {
            allowed: vec!["container:*".into()],
            environment: "same-vps-confined-docker".into(),
            ..Default::default()
        }),
    );
    room.engine
        .apply(Command::SetAuthorization {
            task: objectives[5],
            authorization: AuthorizationState::AwaitingOperator {
                because: "it provisions a paid VPS".into(),
                options: vec!["Approve".into(), "Decline".into()],
            },
        })
        .expect("gate");

    room
}

/// The headline failure: a busy board must still read as a few outcomes.
#[test]
fn a_hundred_records_read_as_twelve_objectives() {
    let room = leo_room();
    let view = room.engine.digest();

    assert!(
        room.engine.tasks().count() >= 70,
        "the fixture should be the size of the run that caused this"
    );
    assert_eq!(
        view.counts.objectives, OBJECTIVES as u32,
        "a person sees outcomes, not records"
    );
    assert!(
        view.counts.raw_records > view.counts.objectives * 4,
        "and the records are all still there, tracked"
    );
}

/// Eleven future tasks all advertising themselves as ready at once.
#[test]
fn a_dependency_chain_offers_only_what_can_start() {
    let room = leo_room();
    let worker = room.workers[0];

    let claimable = room.engine.claimable_by(worker);
    let chained_and_ready = claimable
        .iter()
        .filter_map(|id| room.engine.task(*id))
        .filter(|task| task.title.starts_with("C"))
        .count();

    assert!(
        chained_and_ready <= 1,
        "at most the head of the chain is workable, found {chained_and_ready}"
    );
}

/// Superseded work stayed claimable, and kept being picked up again.
#[test]
fn replaced_work_is_offered_to_nobody() {
    let room = leo_room();
    let replaced: Vec<TaskId> = room
        .engine
        .tasks()
        .filter(|task| task.lineage.is_superseded())
        .map(|task| task.id)
        .collect();
    assert!(!replaced.is_empty(), "the fixture replaces some work");

    for worker in &room.workers {
        let offered = room.engine.claimable_by(*worker);
        for id in &replaced {
            assert!(
                !offered.contains(id),
                "{id} was replaced and must not be offered"
            );
        }
    }
}

/// The whole point of the upgrade: a person is asked almost nothing.
#[test]
fn only_the_genuine_decision_reaches_the_person() {
    let room = leo_room();
    let view = room.engine.digest();

    assert_eq!(
        view.counts.needs_you, 1,
        "one real decision out of everything in this room"
    );
    assert_eq!(view.needs_you.len(), 1);
    assert!(view.needs_you[0].because.contains("paid VPS"));

    // And the security hold, which is the room's own work, is not among them.
    assert!(
        !view
            .needs_you
            .iter()
            .any(|decision| decision.because.contains("classification")),
        "missing proof is work for the room"
    );
}

/// Every card says whether it needs anything, and almost none do.
#[test]
fn every_card_states_whether_it_needs_a_person() {
    let room = leo_room();
    let view = room.engine.digest();

    let needing = view
        .objectives
        .iter()
        .filter(|card| card.needs_operator)
        .count();
    assert_eq!(needing, 1, "one card, not twelve");
    for card in &view.objectives {
        assert!(!card.status.is_empty(), "each card says what state it is in");
    }
}

/// A room this size, built only through commands, must be coherent.
#[test]
fn the_whole_room_holds_its_invariants() {
    let room = leo_room();
    assert_eq!(
        room.engine.inconsistencies(),
        vec![],
        "a room built through the command surface must never break its own rules"
    );
}

/// The summary a person reads instead of the board.
#[test]
fn the_room_can_describe_itself_in_a_paragraph() {
    let room = leo_room();
    let summary = room.engine.digest().summary("Leo room repair");

    assert!(summary.starts_with("Leo room repair"));
    assert!(summary.contains("objectives complete"));
    assert!(
        summary.contains("1 decision needed from you"),
        "got:\n{summary}"
    );
}

/// Review going round on one invariant is caught at this scale too.
#[test]
fn a_recurring_invariant_is_still_caught_in_a_busy_room() {
    let mut room = leo_room();
    let worker = room.workers[0];
    let inspector = room.inspectors[0];

    let task = room.create(
        "the stubborn handler",
        TaskOrigin::plain(),
        vec![],
    );
    room.engine.set_max_revisions(task, 10);
    room.engine
        .apply(Command::ClaimTask { task, by: worker })
        .expect("claim");
    room.engine
        .apply(Command::StartTask { task, by: worker })
        .expect("start");

    for wording in [
        "stale async mutation",
        "async mutation is stale",
        "mutation stale in the async path",
    ] {
        room.engine
            .apply(Command::SubmitForReview {
                task,
                by: worker,
                artifacts: vec![],
            })
            .expect("submit");
        room.engine
            .apply(Command::RecordReview {
                task,
                inspector,
                findings: vec![Finding {
                    severity: Severity::Major,
                    summary: wording.into(),
                    locus: Some("reducer".into()),
                    suggestion: None,
                }],
                summary: "needs work".into(),
                criteria_met: vec![],
                criteria_failed: vec![],
            })
            .expect("review");
    }

    assert!(
        room.engine.convergence(task).needs_escalation(),
        "three cycles on one invariant is a pattern at any scale"
    );
    assert_eq!(
        room.engine.digest().counts.needs_you,
        1,
        "and it is still not a question for a person"
    );
}

/// Read back afterwards, the room says what it would do differently.
#[test]
fn the_retrospective_names_what_to_change() {
    let room = leo_room();
    let report = analytics::retrospective(&room.engine);

    assert_eq!(report.objectives, OBJECTIVES as u32);
    assert!(
        report
            .observations
            .iter()
            .any(|o| o.pattern == "duplicated-work"),
        "the replaced work should be noticed: {:?}",
        report.observations
    );
    for observation in &report.observations {
        assert!(!observation.suggestion.is_empty());
    }
    assert!(report.render().contains("Leo room repair"));
}

/// Sixteen bots and a full board should not leave the roster idle.
#[test]
fn a_full_room_puts_its_bots_to_work() {
    let room = leo_room();
    let utilization = room.engine.utilization();

    assert_eq!(utilization.bots.len(), ROSTER);
    assert!(
        utilization.unclaimed > 0,
        "there is work available in this fixture"
    );
    // Which means the room is visibly wasting capacity — the point of the
    // view is that this is now sayable rather than invisible.
    assert!(utilization.is_wasting_capacity());
}
