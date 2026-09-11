//! One answer about who may claim what, and what happens when it goes wrong.
//!
//! Two separate things are tested here and they are worth keeping apart.
//!
//! The first is **agreement**: what a bot is told it can take, what a listing
//! returns, and what a claim accepts must be the same decision, because they
//! are now literally the same function. A test that forces them apart — by
//! corrupting the cached projection a display path reads — should find that
//! every path still follows the board rather than the cache.
//!
//! The second is **repair**. A task's state says whether somebody is working
//! on it; the holder and the claim say who, and until when. Those can come
//! apart, and every way they could come apart used to be permanent: the sweep
//! that returns abandoned work to the pool only looked at claims that had an
//! expiry recorded, so a task held with no expiry — which is how every room
//! saved before claims had one comes back — was held forever by a bot that
//! might be long gone. A live room had eight of them.
//!
//! The one that cost the most was not a claim at all. A task blocked on
//! another task recorded the wait and then nobody watched for it to end, so
//! six tasks in that room were blocked on work that had already finished, and
//! the five bots holding them were parked for good.

use orgbot_engine::eligibility::{LeaseState, Reason};
use orgbot_engine::engine::Engine;
use orgbot_engine::{Command, EngineError};
use orgbot_proto::agent::{Autonomy, ProviderRef};
use orgbot_proto::event::{Event, EventEnvelope};
use orgbot_proto::graph::{DependencyState, TaskOrigin};
use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
use orgbot_proto::task::{BlockReason, Priority, TaskState};
use orgbot_proto::{AgentId, AgentSpec, TaskId, Timestamp};

// ------------------------------------------------------------------ harness

struct Room {
    engine: Engine,
    lead: AgentId,
    a: AgentId,
    b: AgentId,
    qa: AgentId,
}

fn provider() -> ProviderRef {
    ProviderRef::Scripted {
        script: "test".into(),
    }
}

fn room() -> Room {
    let mut engine = Engine::new(RoomSpec {
        name: "eligibility".into(),
        goal: "one answer, everywhere".into(),
        roster: vec![],
        completion: CompletionRule::AllTasksTerminal,
        limits: RoomLimits::default(),
    });
    let lead = AgentSpec::new("lead", "Lead", provider())
        .with_roles(["coordinator"])
        .with_autonomy(Autonomy::coordinator());
    let a = AgentSpec::new("a", "Worker A", provider()).with_roles(["writer"]);
    let b = AgentSpec::new("b", "Worker B", provider()).with_roles(["writer"]);
    let qa = AgentSpec::new("qa", "Inspector", provider())
        .with_roles(["reviewer"])
        .with_autonomy(Autonomy::inspector());
    let ids = (lead.id, a.id, b.id, qa.id);
    for spec in [lead, a, b, qa] {
        engine
            .apply(Command::JoinAgent {
                spec: Box::new(spec),
            })
            .expect("join");
    }
    engine.apply(Command::StartRoom).expect("start");
    Room {
        engine,
        lead: ids.0,
        a: ids.1,
        b: ids.2,
        qa: ids.3,
    }
}

impl Room {
    fn apply(&mut self, command: Command) -> Vec<EventEnvelope> {
        self.engine
            .apply(command.clone())
            .unwrap_or_else(|e| panic!("`{}` was rejected: {e}", command.name()))
    }

    fn open(&mut self, title: &str, depends_on: Vec<TaskId>) -> TaskId {
        let events = self.apply(Command::CreateTask {
            title: title.into(),
            brief: format!("do: {title}"),
            by: Some(self.lead),
            required_role: Some("writer".into()),
            priority: Priority::Normal,
            acceptance_criteria: vec![],
            depends_on,
            origin: TaskOrigin::plain(),
        });
        let task = events
            .iter()
            .find_map(|e| match &e.event {
                Event::TaskCreated { task } => Some(task.id),
                _ => None,
            })
            .expect("a task was created");
        self.apply(Command::OpenTask { task });
        task
    }

    fn tick(&mut self) {
        let _ = self.engine.apply(Command::Tick {
            now: Timestamp::now(),
        });
    }

    fn state(&self, task: TaskId) -> TaskState {
        self.engine.task(task).expect("the task exists").state
    }

    fn reason(&self, task: TaskId, agent: AgentId) -> Reason {
        self.engine
            .evaluate_claim(task, agent, Timestamp::now())
            .reason
    }

    /// Every task this bot would be told about, from the one function that
    /// decides. The prompt and `list_tasks(claimable)` both read this.
    fn advertised(&self, agent: AgentId) -> Vec<TaskId> {
        self.engine.claimable_by(agent)
    }
}

// -------------------------------------------------- the layers cannot differ

/// TEST 1 — what a bot is offered, it can take.
#[test]
fn work_a_bot_is_offered_is_work_it_can_actually_claim() {
    let mut r = room();
    let task = r.open("write the notes", vec![]);

    assert!(r.advertised(r.a).contains(&task), "it should be offered");
    assert_eq!(r.reason(task, r.a), Reason::ReadyNow);
    r.apply(Command::ClaimTask { task, by: r.a });
    assert_eq!(r.state(task), TaskState::Claimed);
}

/// TEST 3 — the listing and the advertisement are the same set, by
/// construction: both are `claimable_by`, which filters on the evaluator.
#[test]
fn the_listing_and_the_advertisement_are_the_same_decision() {
    let mut r = room();
    let open = r.open("open", vec![]);
    let upstream = r.open("upstream", vec![]);
    let blocked = r.open("waits on upstream", vec![upstream]);
    r.apply(Command::ClaimTask {
        task: upstream,
        by: r.b,
    });

    let advertised = r.advertised(r.a);
    let evaluated: Vec<TaskId> = [open, upstream, blocked]
        .into_iter()
        .filter(|task| {
            r.engine
                .evaluate_claim(*task, r.a, Timestamp::now())
                .eligible
        })
        .collect();
    assert_eq!(
        advertised, evaluated,
        "the list a bot is shown must be exactly what the evaluator allows"
    );
    assert_eq!(advertised, vec![open]);
}

/// TEST 12 — the display cache is not the authority.
///
/// `dependency` is a projection of the `depends_on` edges, kept for the board
/// to render. Force it to disagree, and every path must still follow the
/// edges. This is the divergence that used to be possible in principle: one
/// path read the cache, the other walked the edges.
#[test]
fn a_stale_projection_does_not_make_blocked_work_claimable() {
    let mut r = room();
    let upstream = r.open("upstream", vec![]);
    let dependent = r.open("waits on upstream", vec![upstream]);
    r.apply(Command::ClaimTask {
        task: upstream,
        by: r.b,
    });

    // Corrupt the projection: claim the dependency is met when it is not.
    r.engine.task_mut(dependent).unwrap().dependency = DependencyState::Satisfied;

    assert!(
        !r.advertised(r.a).contains(&dependent),
        "advertisement must read the board, not the cache"
    );
    assert_eq!(r.reason(dependent, r.a), Reason::DependencyBlocked);
    let err = r
        .engine
        .apply(Command::ClaimTask {
            task: dependent,
            by: r.a,
        })
        .expect_err("the claim must be refused too");
    assert!(matches!(err, EngineError::DependencyNotMet { .. }), "{err:?}");
}

/// TEST 11 — work waiting its turn is never offered.
#[test]
fn a_dependent_task_is_not_offered_until_its_upstream_lands() {
    let mut r = room();
    let upstream = r.open("upstream", vec![]);
    let dependent = r.open("dependent", vec![upstream]);
    assert_eq!(r.reason(dependent, r.a), Reason::DependencyBlocked);
    assert!(!r.advertised(r.a).contains(&dependent));
}

/// TEST 10 — replaced and retired work is never offered and never claimable.
#[test]
fn replaced_and_retired_work_is_offered_to_nobody() {
    let mut r = room();
    let original = r.open("the original", vec![]);
    let replacement = r.open("the replacement", vec![]);
    r.apply(Command::SupersedeTask {
        task: original,
        replacement,
        reason: "rewritten".into(),
    });

    assert_eq!(r.reason(original, r.a), Reason::Superseded);
    assert!(!r.advertised(r.a).contains(&original));
    let err = r
        .engine
        .apply(Command::ClaimTask {
            task: original,
            by: r.a,
        })
        .expect_err("replaced work cannot be claimed");
    assert!(matches!(err, EngineError::NotReady { .. }), "{err:?}");
}

// --------------------------------------------------------- races and holders

/// TEST 2 and TEST 9 — a lost race is a clean, named refusal.
///
/// This is what the reported symptom actually was. A bot is told a task is
/// open, spends a minute thinking, and by the time it claims, somebody else
/// has it. That is not a broken board, and the refusal now says so: the code
/// is `ALREADY_CLAIMED` and the sentence names the holder, rather than the
/// state machine's "cannot claim a task that is in_progress", which reads like
/// the room contradicting itself.
#[test]
fn losing_a_race_says_who_won_rather_than_looking_like_a_contradiction() {
    let mut r = room();
    let task = r.open("contested", vec![]);
    assert!(r.advertised(r.a).contains(&task), "both are offered it");
    assert!(r.advertised(r.b).contains(&task));

    r.apply(Command::ClaimTask { task, by: r.b });

    assert_eq!(r.reason(task, r.a), Reason::AlreadyClaimed);
    let verdict = r.engine.evaluate_claim(task, r.a, Timestamp::now());
    assert_eq!(verdict.holder, Some(r.b), "it names the winner");
    assert_eq!(verdict.lease, LeaseState::Active);
    assert!(!r.advertised(r.a).contains(&task), "and stops offering it");

    let err = r
        .engine
        .apply(Command::ClaimTask { task, by: r.a })
        .expect_err("the second claim must fail");
    assert!(matches!(err, EngineError::Transition(_)), "{err:?}");
    assert_eq!(
        r.engine.task(task).unwrap().assignee,
        Some(r.b),
        "and the winner keeps it"
    );
}

/// TEST 4 — work in hand, with a live claim, is nobody else's.
#[test]
fn work_underway_with_a_live_claim_is_not_claimable() {
    let mut r = room();
    let task = r.open("underway", vec![]);
    r.apply(Command::ClaimTask { task, by: r.b });
    r.apply(Command::StartTask { task, by: r.b });

    let verdict = r.engine.evaluate_claim(task, r.a, Timestamp::now());
    assert!(!verdict.eligible);
    assert_eq!(verdict.reason, Reason::AlreadyClaimed);
    assert_eq!(verdict.lease, LeaseState::Active);
    assert!(!verdict.repair_required, "a healthy claim is not a fault");

    r.tick();
    assert_eq!(
        r.state(task),
        TaskState::InProgress,
        "reconciliation must not disturb work that is going fine"
    );
    assert_eq!(r.engine.task(task).unwrap().assignee, Some(r.b));
}

/// TEST 8 — released work becomes claimable, once.
#[test]
fn released_work_returns_to_the_pool_for_exactly_one_taker() {
    let mut r = room();
    let task = r.open("handed back", vec![]);
    r.apply(Command::ClaimTask { task, by: r.a });
    r.apply(Command::ReleaseTask {
        task,
        by: r.a,
        reason: "not mine after all".into(),
    });

    assert_eq!(r.reason(task, r.b), Reason::ReadyNow);
    r.apply(Command::ClaimTask { task, by: r.b });
    assert_eq!(r.reason(task, r.a), Reason::AlreadyClaimed);
    assert!(
        r.engine
            .apply(Command::ClaimTask { task, by: r.a })
            .is_err(),
        "only one taker"
    );
}

// ------------------------------------------------------------------- repair

/// TEST 5 — a working state with nobody working it is repaired, not stuck.
///
/// Not reachable through the commands, which is exactly why it was never
/// noticed: it arrives through a save from an older version, or a partial
/// handoff. The board rendered it as "closed" and no sweep would touch it.
#[test]
fn a_task_underway_with_no_holder_is_returned_to_the_pool() {
    let mut r = room();
    let task = r.open("abandoned", vec![]);
    r.apply(Command::ClaimTask { task, by: r.a });
    r.apply(Command::StartTask { task, by: r.a });

    // The holder vanishes, as a half-finished handoff would leave it.
    {
        let record = r.engine.task_mut(task).unwrap();
        record.assignee = None;
        record.lease_expires_at = None;
    }
    assert_eq!(r.reason(task, r.b), Reason::ZombieInProgress);
    assert!(
        r.engine
            .evaluate_claim(task, r.b, Timestamp::now())
            .repair_required
    );

    r.tick();
    assert_eq!(r.state(task), TaskState::Ready, "the room repairs it");
    assert_eq!(r.reason(task, r.b), Reason::ReadyNow);
    r.apply(Command::ClaimTask { task, by: r.b });
}

/// A claim with no expiry recorded is reclaimable, eventually.
///
/// Every room saved before claims carried an expiry comes back this way. The
/// sweep compared against an expiry, so with none there was nothing to
/// compare and the task stayed held for the life of the room. The repair
/// grants a claim rather than seizing the task, so a holder that is still
/// alive keeps its work and only a holder that is gone loses it.
#[test]
fn a_holder_with_no_recorded_claim_is_given_one_rather_than_stripped() {
    let mut r = room();
    let task = r.open("held since before leases", vec![]);
    r.apply(Command::ClaimTask { task, by: r.a });
    r.engine.task_mut(task).unwrap().lease_expires_at = None;

    assert_eq!(
        r.engine
            .evaluate_claim(task, r.b, Timestamp::now())
            .lease,
        LeaseState::Missing
    );

    r.tick();
    let record = r.engine.task(task).unwrap();
    assert_eq!(record.assignee, Some(r.a), "the holder keeps it for now");
    assert!(
        record.lease_expires_at.is_some(),
        "but it now has a claim that can lapse, so it cannot be held forever"
    );
}

/// TEST 6 and TEST 14 — a faulted bot's work comes back, all of it.
#[test]
fn every_task_a_faulted_bot_held_comes_back_exactly_once() {
    let mut r = room();
    let first = r.open("first", vec![]);
    let second = r.open("second", vec![]);
    for task in [first, second] {
        r.apply(Command::ClaimTask { task, by: r.a });
        r.apply(Command::StartTask { task, by: r.a });
    }
    // The bot stops and its claims lapse.
    for task in [first, second] {
        r.engine.task_mut(task).unwrap().lease_expires_at =
            Some(Timestamp::now().plus_millis(-1));
    }

    r.tick();
    for task in [first, second] {
        assert_eq!(r.state(task), TaskState::Ready, "returned to the pool");
        assert_eq!(r.engine.task(task).unwrap().assignee, None);
    }
    // And exactly one bot ends up with each, with no double ownership.
    r.apply(Command::ClaimTask { task: first, by: r.b });
    r.apply(Command::ClaimTask {
        task: second,
        by: r.b,
    });
    assert_eq!(r.engine.task(first).unwrap().assignee, Some(r.b));
    assert_eq!(r.engine.task(second).unwrap().assignee, Some(r.b));
}

/// TEST 7 — a bot that comes back cannot write to work it no longer holds.
///
/// The fence is the holder's identity, which every mutation is checked
/// against; the epoch records that the holding changed, so the two holdings
/// can be told apart in a log.
#[test]
fn a_returning_bot_cannot_touch_work_that_moved_on_without_it() {
    let mut r = room();
    let task = r.open("reassigned", vec![]);
    r.apply(Command::ClaimTask { task, by: r.a });
    r.apply(Command::StartTask { task, by: r.a });
    let epoch_before = r.engine.task(task).unwrap().ownership_epoch;

    r.engine.task_mut(task).unwrap().lease_expires_at = Some(Timestamp::now().plus_millis(-1));
    r.tick();
    r.apply(Command::ClaimTask { task, by: r.b });

    let record = r.engine.task(task).unwrap();
    assert_eq!(record.assignee, Some(r.b));
    assert!(
        record.ownership_epoch > epoch_before,
        "the holding changed, so the epoch must have moved: {} -> {}",
        epoch_before,
        record.ownership_epoch
    );

    // The old holder comes back and tries to finish what it thinks is its work.
    let err = r
        .engine
        .apply(Command::SubmitForReview {
            task,
            by: r.a,
            artifacts: vec![],
        })
        .expect_err("a bot that no longer holds the task must be refused");
    assert!(matches!(err, EngineError::NotAssignee { .. }), "{err:?}");
    assert_eq!(
        r.engine.task(task).unwrap().assignee,
        Some(r.b),
        "and the current holder is untouched"
    );
}

/// TEST 15 — repair never produces two owners, or none.
#[test]
fn repair_leaves_exactly_one_owner_or_none_at_all() {
    let mut r = room();
    let task = r.open("interrupted mid-handoff", vec![]);
    r.apply(Command::ClaimTask { task, by: r.a });
    r.apply(Command::StartTask { task, by: r.a });
    // A handoff that got half way: the holder is gone, the claim remains.
    r.engine.task_mut(task).unwrap().assignee = None;

    r.tick();
    let record = r.engine.task(task).unwrap();
    assert_eq!(record.state, TaskState::Ready);
    assert_eq!(record.assignee, None, "nobody is left holding it");
    assert!(
        record.lease_expires_at.is_none(),
        "and no claim is left behind on unheld work"
    );

    r.apply(Command::ClaimTask { task, by: r.b });
    assert_eq!(r.engine.task(task).unwrap().assignee, Some(r.b));
}

/// Work held by a bot that is no longer in the room comes back.
#[test]
fn work_held_by_a_bot_that_left_returns_to_the_pool() {
    let mut r = room();
    let task = r.open("held by a ghost", vec![]);
    r.apply(Command::ClaimTask { task, by: r.a });
    r.engine.task_mut(task).unwrap().assignee = Some(AgentId::new());

    r.tick();
    assert_eq!(r.state(task), TaskState::Ready);
    assert_eq!(r.reason(task, r.b), Reason::ReadyNow);
}

// ------------------------------------------- the one that cost the most

/// A wait on another task ends when that task lands.
///
/// Blocking on a handoff released its waiters when the handoff settled.
/// Blocking on a *task* recorded the wait and then nobody watched for it to
/// end, so the task stayed blocked after its dependency was delivered — and
/// the bot holding it stayed parked. In a room that had been running for days
/// this had happened six times.
#[test]
fn a_task_blocked_on_work_that_has_landed_does_not_stay_blocked() {
    let mut r = room();
    let upstream = r.open("the thing being waited on", vec![]);
    let waiting = r.open("the waiter", vec![]);

    r.apply(Command::ClaimTask {
        task: waiting,
        by: r.a,
    });
    r.apply(Command::StartTask {
        task: waiting,
        by: r.a,
    });
    r.apply(Command::BlockTask {
        task: waiting,
        by: r.a,
        reason: BlockReason::AwaitingTask { task: upstream },
    });
    assert_eq!(r.state(waiting), TaskState::Blocked);

    // The upstream is finished, properly.
    r.apply(Command::ClaimTask {
        task: upstream,
        by: r.b,
    });
    r.apply(Command::StartTask {
        task: upstream,
        by: r.b,
    });
    r.apply(Command::SubmitForReview {
        task: upstream,
        by: r.b,
        artifacts: vec![],
    });
    r.apply(Command::RecordReview {
        task: upstream,
        inspector: r.qa,
        findings: vec![],
        summary: "fine".into(),
        criteria_met: vec![],
        criteria_failed: vec![],
    });
    r.apply(Command::SubmitTask {
        task: upstream,
        by: r.lead,
    });
    assert_eq!(r.state(upstream), TaskState::Submitted);

    r.tick();
    assert_eq!(
        r.state(waiting),
        TaskState::InProgress,
        "the wait is over, so the waiter should be working again"
    );
    assert_eq!(
        r.engine.task(waiting).unwrap().assignee,
        Some(r.a),
        "and it is still the same bot's work"
    );
}

/// A wait on work that failed is left alone, deliberately.
///
/// That dependency is not going to arrive, but deciding what should happen
/// instead is a judgement — carry on without it, replace it, abandon the
/// dependent — and guessing silently is worse than leaving it visible.
#[test]
fn a_wait_on_work_that_failed_is_left_for_someone_to_decide() {
    let mut r = room();
    let upstream = r.open("doomed", vec![]);
    let waiting = r.open("waiter", vec![]);
    r.apply(Command::ClaimTask {
        task: waiting,
        by: r.a,
    });
    r.apply(Command::StartTask {
        task: waiting,
        by: r.a,
    });
    r.apply(Command::BlockTask {
        task: waiting,
        by: r.a,
        reason: BlockReason::AwaitingTask { task: upstream },
    });
    r.apply(Command::FailTask {
        task: upstream,
        reason: "could not be done".into(),
    });

    r.tick();
    assert_eq!(
        r.state(waiting),
        TaskState::Blocked,
        "a failed dependency is not a cleared wait"
    );
    assert_eq!(r.reason(waiting, r.b), Reason::BlockedInternal);
}

// ------------------------------------------------------------- bot health

/// A faulted bot is offered nothing, and told why if it asks.
#[test]
fn a_faulted_bot_is_offered_no_work() {
    let mut r = room();
    let task = r.open("open work", vec![]);
    for _ in 0..5 {
        let _ = r.engine.apply(Command::ReportError {
            agent: r.a,
            task: None,
            error: "the model refuses this configuration".into(),
            permanent: true,
        });
    }

    assert_eq!(r.reason(task, r.a), Reason::BotFaulted);
    assert!(r.advertised(r.a).is_empty(), "a faulted bot gets nothing");
    assert!(
        r.advertised(r.b).contains(&task),
        "and everyone else is unaffected"
    );
}

/// A bot mid-turn is busy, not unavailable.
///
/// `accepts_work` means "is free"; being part-way through a turn is not the
/// same as being unable to hold work. Reading one as the other stops a busy
/// room dead.
#[test]
fn a_bot_part_way_through_a_turn_can_still_take_a_task() {
    let mut r = room();
    let first = r.open("first", vec![]);
    let second = r.open("second", vec![]);
    r.apply(Command::ClaimTask {
        task: first,
        by: r.a,
    });
    r.apply(Command::StartTask {
        task: first,
        by: r.a,
    });

    assert_eq!(
        r.reason(second, r.a),
        Reason::ReadyNow,
        "working is not the same as unavailable"
    );
}

/// Stranded work reads as recovering, not as finished.
///
/// The board used to call this "closed", which is the one label guaranteed to
/// stop anybody looking at it: the work was not finished, it was abandoned
/// mid-flight, and it said so nowhere.
#[test]
fn work_left_with_nobody_on_it_reads_as_recovering_not_closed() {
    use orgbot_proto::graph::Readiness;

    let mut r = room();
    let task = r.open("stranded", vec![]);
    r.apply(Command::ClaimTask { task, by: r.a });
    r.apply(Command::StartTask { task, by: r.a });
    r.engine.task_mut(task).unwrap().assignee = None;

    let readiness = r.engine.task(task).unwrap().readiness();
    assert!(
        matches!(readiness, Readiness::Recovering { .. }),
        "expected Recovering, got {readiness:?}"
    );
    assert_eq!(readiness.label(), "Recovering");
    assert!(
        !readiness.needs_operator(),
        "the room fixes this itself; it must not interrupt anyone"
    );
}

// ---------------------------------------------- the room reporting on itself

/// A room that cannot get its work moving says so, once.
///
/// The failure this exists to prevent has a shape: the room keeps running, the
/// board looks busy, and nobody has any reason to look. One ran four days like
/// that. The engine could already work out what was wrong — nothing asked it.
#[test]
fn a_room_carrying_work_that_cannot_move_raises_it_exactly_once() {
    use orgbot_proto::event::Event;

    let mut r = room();
    let upstream = r.open("doomed upstream", vec![]);
    let waiting = r.open("waits on the doomed thing", vec![]);
    r.apply(Command::ClaimTask {
        task: waiting,
        by: r.a,
    });
    r.apply(Command::StartTask {
        task: waiting,
        by: r.a,
    });
    r.apply(Command::BlockTask {
        task: waiting,
        by: r.a,
        reason: BlockReason::AwaitingTask { task: upstream },
    });
    assert!(r.engine.health().is_ok(), "nothing is wrong yet");

    // The thing it waits on will never arrive.
    r.apply(Command::FailTask {
        task: upstream,
        reason: "cannot be done".into(),
    });

    let health = r.engine.health();
    assert!(!health.is_ok());
    assert_eq!(health.dead_waits.len(), 1);
    assert!(
        health.summary().contains("cannot arrive"),
        "the summary must say what is wrong: {}",
        health.summary()
    );

    // The room raises it on its next pass...
    let raised = r.engine.apply(Command::Tick {
        now: Timestamp::now(),
    });
    let count = |events: &[orgbot_proto::event::EventEnvelope]| {
        events
            .iter()
            .filter(|e| matches!(e.event, Event::RoomHealthChanged { ok: false, .. }))
            .count()
    };
    assert_eq!(count(&raised.unwrap()), 1, "it should be raised");

    // ...and not again on the next, because nothing changed. A room that
    // repeats itself every minute teaches its operator to ignore it.
    let again = r.engine.apply(Command::Tick {
        now: Timestamp::now(),
    });
    assert_eq!(count(&again.unwrap()), 0, "a standing problem is raised once");
}

/// A wait that can still end on its own is not raised at all.
#[test]
fn ordinary_waiting_is_not_reported_as_a_problem() {
    let mut r = room();
    let upstream = r.open("upstream, still going", vec![]);
    let waiting = r.open("waiter", vec![]);
    r.apply(Command::ClaimTask {
        task: waiting,
        by: r.a,
    });
    r.apply(Command::StartTask {
        task: waiting,
        by: r.a,
    });
    r.apply(Command::BlockTask {
        task: waiting,
        by: r.a,
        reason: BlockReason::AwaitingTask { task: upstream },
    });

    let health = r.engine.health();
    assert!(
        health.dead_waits.is_empty(),
        "work that is simply not its turn yet is the system working"
    );
    assert!(health.is_ok());
}

/// A halted bot is something a person has to restart, so it is raised.
#[test]
fn a_halted_bot_is_something_the_room_says_out_loud() {
    let mut r = room();
    for _ in 0..5 {
        let _ = r.engine.apply(Command::ReportError {
            agent: r.a,
            task: None,
            error: "the configured model does not exist".into(),
            permanent: true,
        });
    }
    let health = r.engine.health();
    assert!(!health.is_ok());
    assert_eq!(health.faulted, vec!["a".to_string()]);
    assert!(health.summary().contains('a'), "{}", health.summary());
}

/// Every eligible bot being busy is not a staffing problem.
///
/// The difference decides whether a notification is worth having. "Nobody is
/// free right now" is true constantly in a working room; "nobody here can do
/// this at all" is true until somebody changes the roster. Notifying on the
/// first would teach the operator to ignore the second.
#[test]
fn work_waiting_for_a_busy_bot_is_a_queue_not_a_staffing_gap() {
    let mut r = room();
    let first = r.open("first", vec![]);
    let second = r.open("second", vec![]);
    r.apply(Command::ClaimTask {
        task: first,
        by: r.a,
    });
    r.apply(Command::ClaimTask {
        task: second,
        by: r.b,
    });
    let third = r.open("third, with both writers busy", vec![]);

    assert!(
        !r.engine.starved_roles().is_empty(),
        "the board may fairly say nobody is free this minute"
    );
    assert!(
        r.engine.unstaffed_roles().is_empty(),
        "but the room is staffed for it, so nobody should be woken"
    );
    assert!(r.engine.health().is_ok());
    let _ = third;
}

/// Work no bot in the room could ever take is raised, because only a person
/// can fix it.
#[test]
fn work_no_bot_could_ever_take_is_raised() {
    let mut r = room();
    let events = r.apply(Command::CreateTask {
        title: "port the firmware".into(),
        brief: "needs an embedded engineer".into(),
        by: Some(r.lead),
        required_role: Some("embedded".into()),
        priority: orgbot_proto::task::Priority::Normal,
        acceptance_criteria: vec![],
        depends_on: vec![],
        origin: TaskOrigin::plain(),
    });
    let task = events
        .iter()
        .find_map(|e| match &e.event {
            Event::TaskCreated { task } => Some(task.id),
            _ => None,
        })
        .unwrap();
    r.apply(Command::OpenTask { task });

    let health = r.engine.health();
    assert_eq!(health.starved, vec![("embedded".to_string(), 1)]);
    assert!(!health.is_ok());
    assert!(health.summary().contains("embedded"), "{}", health.summary());
}

/// Letting go of a task lets go of the claim on it.
///
/// Found by pointing the health check at a real room: two tasks were carrying
/// a claim with nobody assigned. Harmless on its own — a claim with no holder
/// expires against nothing — but it is a standing inconsistency, and a room
/// that reports a standing inconsistency forever is a room whose reports get
/// ignored.
#[test]
fn releasing_a_task_releases_its_claim_too() {
    let mut r = room();
    let task = r.open("handed back", vec![]);
    r.apply(Command::ClaimTask { task, by: r.a });
    assert!(r.engine.task(task).unwrap().lease_expires_at.is_some());

    r.apply(Command::ReleaseTask {
        task,
        by: r.a,
        reason: "not mine".into(),
    });

    let record = r.engine.task(task).unwrap();
    assert_eq!(record.assignee, None);
    assert_eq!(record.lease_expires_at, None, "the claim went with the holder");
    assert!(r.engine.health().is_ok(), "{}", r.engine.health().report());
}

// ------------------------------------------------------------------- spend

/// A room can say what it has cost, by the unit an allowance is granted in.
///
/// Turns were always counted; tokens were reported by every turn and thrown
/// away. So the room could say it had taken 347 turns and nothing at all about
/// what that drew — which is how a four-fold rise in concurrency was argued
/// for on the strength of "no rate-limit errors today", against a weekly
/// allowance that does not fail that way.
#[test]
fn a_room_counts_what_it_draws_and_from_which_account() {
    use orgbot_proto::agent::ProviderRef;
    use orgbot_proto::spend::TokenUsage;

    let mut r = room();
    let cost = |input: u32, output: u32| TokenUsage {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    };

    // Two bots on one account, one on another.
    let heavy = AgentSpec::new(
        "heavy",
        "Heavy",
        ProviderRef::OpenAiCompatible {
            model: "gpt-6".into(),
            profile: "codex".into(),
        },
    )
    .with_roles(vec!["writer"]);
    let also_codex = AgentSpec::new(
        "also",
        "Also",
        ProviderRef::OpenAiCompatible {
            model: "gpt-6".into(),
            profile: "codex".into(),
        },
    )
    .with_roles(vec!["writer"]);
    let elsewhere = AgentSpec::new(
        "elsewhere",
        "Elsewhere",
        ProviderRef::DeepSeek {
            model: "deepseek-chat".into(),
        },
    )
    .with_roles(vec!["writer"]);
    let (heavy_id, also_id, other_id) = (heavy.id, also_codex.id, elsewhere.id);
    for spec in [heavy, also_codex, elsewhere] {
        r.apply(Command::JoinAgent {
            spec: Box::new(spec),
        });
    }

    r.apply(Command::RecordTurn {
        agent: heavy_id,
        usage: cost(1_000, 200),
    });
    r.apply(Command::RecordTurn {
        agent: also_id,
        usage: cost(500, 100),
    });
    r.apply(Command::RecordTurn {
        agent: other_id,
        usage: cost(10, 5),
    });

    assert_eq!(r.engine.spend_of(heavy_id).usage.total(), 1_200);
    assert_eq!(r.engine.spend_total().turns, 3);

    let by_backend = r.engine.spend_by_backend();
    assert_eq!(
        by_backend[0].0, "openai:codex",
        "the heaviest account comes first: {by_backend:?}"
    );
    assert_eq!(
        by_backend[0].1.usage.total(),
        1_800,
        "both bots on one account draw on one limit, and must be summed as one"
    );
    assert_eq!(by_backend[0].1.turns, 2);
    assert_eq!(by_backend[1].0, "deepseek");
}

/// A turn from a backend that reports nothing still counts as a turn.
#[test]
fn an_unmetered_backend_is_counted_but_not_presented_as_free() {
    use orgbot_proto::spend::TokenUsage;

    let mut r = room();
    r.apply(Command::RecordTurn {
        agent: r.a,
        usage: TokenUsage::default(),
    });
    let spend = r.engine.spend_of(r.a);
    assert_eq!(spend.turns, 1, "the turn happened");
    assert!(
        spend.usage.is_empty(),
        "and its cost is unknown, which is not the same as zero"
    );
}

/// A leftover claim on unheld work is cleared, not just prevented.
///
/// Releasing clears the holder and the claim together now. But a room that ran
/// before it did still carries the leftovers, and the health report names them
/// every time it is asked. An inconsistency the room reports and cannot clear
/// is worse than one it never noticed: it teaches the operator that the report
/// is noise, and then the real one goes unread with the rest. A live room had
/// two, raised once and standing.
#[test]
fn a_leftover_claim_on_unheld_work_is_cleaned_up() {
    let mut r = room();
    let task = r.open("carried over from an older build", vec![]);
    // The shape an older release left behind: back on the board, still
    // carrying the claim of whoever last held it.
    r.engine.task_mut(task).unwrap().lease_expires_at = Some(Timestamp::now());
    assert!(!r.engine.health().is_ok(), "the room should notice");

    r.tick();

    assert_eq!(r.engine.task(task).unwrap().lease_expires_at, None);
    assert!(
        r.engine.health().is_ok(),
        "and be able to say it is clear again: {}",
        r.engine.health().report()
    );
    assert_eq!(r.state(task), TaskState::Ready, "without disturbing the work");
}

/// A board stays readable however long the room runs.
///
/// The file has a ceiling — past it nothing can open it at all — and two of
/// the four things in it grew without limit. A four-day room reached
/// thirty-three megabytes of a sixty-four megabyte budget, eleven of them log.
#[test]
fn a_long_running_room_does_not_grow_its_board_without_limit() {
    let mut r = room();
    let task = r.open("chatter", vec![]);
    r.apply(Command::ClaimTask { task, by: r.a });

    // Far more traffic than the retention allows.
    for index in 0..4_200 {
        let message = orgbot_proto::ChatMessage::say(
            r.engine.room_id(),
            r.a,
            format!("progress note {index}"),
        );
        r.apply(Command::PostMessage {
            message: Box::new(message),
        });
    }
    assert!(
        r.engine.log().len() > 3_000,
        "the room holds everything until it is asked to tidy"
    );

    r.tick();

    assert!(
        r.engine.log().len() <= 3_000,
        "the log is bounded, got {}",
        r.engine.log().len()
    );
    assert!(
        r.engine.transcript().len() <= 1_000,
        "so is the transcript, got {}",
        r.engine.transcript().len()
    );
}

/// Trimming the record must not touch the work.
#[test]
fn the_board_itself_survives_the_tidying() {
    let mut r = room();
    let task = r.open("the work", vec![]);
    r.apply(Command::ClaimTask { task, by: r.a });
    r.apply(Command::StartTask { task, by: r.a });
    r.apply(Command::AddArtifact {
        task,
        author: r.a,
        name: "deliverable.md".into(),
        body: orgbot_proto::artifact::ArtifactBody::Text {
            text: "the thing that was built".into(),
        },
        authority_bearing: false,
    });

    for index in 0..4_200 {
        let message = orgbot_proto::ChatMessage::say(
            r.engine.room_id(),
            r.a,
            format!("noise {index}"),
        );
        r.apply(Command::PostMessage {
            message: Box::new(message),
        });
    }
    r.tick();

    assert_eq!(r.engine.tasks().count(), 1, "the task is still there");
    assert!(
        r.engine.task(task).unwrap().artifacts.len() == 1,
        "and so is what it produced — trimming a deliverable would throw away \
         the thing the room exists to make"
    );
    assert_eq!(r.engine.task(task).unwrap().assignee, Some(r.a));
}

/// A bot still gets the catch-up it is owed after a trim.
#[test]
fn what_a_bot_has_not_seen_is_still_there_to_be_read() {
    let mut r = room();
    for index in 0..3_500 {
        let message = orgbot_proto::ChatMessage::say(
            r.engine.room_id(),
            r.a,
            format!("said {index}"),
        );
        r.apply(Command::PostMessage {
            message: Box::new(message),
        });
    }
    r.tick();

    let head = r.engine.head();
    // A bot that acted a hundred events ago sees all hundred.
    let recently = orgbot_proto::Seq(head.0 - 100);
    assert_eq!(
        r.engine.events_since(recently).count(),
        100,
        "recent history is exactly what is kept"
    );
}
