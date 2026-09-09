//! What the room did, read back afterwards.
//!
//! A finished room leaves a full event log and a board nobody will read. The
//! useful question is not what happened but where the effort went that should
//! not have: which objectives multiplied into records, where review went round
//! more than twice, how long anything sat waiting on a person, and how much of
//! the roster was idle while work queued.
//!
//! Each observation names something that could be done differently next time.
//! A report that only describes is a second thing to read; this one is meant to
//! change the next room's shape.

use std::collections::BTreeMap;

use orgbot_proto::ids::TaskId;
use orgbot_proto::task::TaskState;
use serde::{Deserialize, Serialize};

use crate::digest::Health;
use crate::engine::Engine;

/// One thing worth doing differently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    /// A short stable name for the pattern.
    pub pattern: String,
    /// What was seen, with numbers.
    pub finding: String,
    /// What to change. Never omitted: an observation with no suggestion is a
    /// complaint.
    pub suggestion: String,
}

/// What a finished room has to say for itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Retrospective {
    /// The room's name.
    pub room: String,
    /// Objectives delivered out of those attempted.
    pub delivered: u32,
    /// Objectives attempted.
    pub objectives: u32,
    /// Live and closed records behind them.
    pub records: u32,
    /// The health figures at the end.
    pub health: Health,
    /// Tasks that went round review more than twice, worst first.
    pub review_loops: Vec<(TaskId, usize)>,
    /// How many questions reached a person, and how many a standing rule or a
    /// previous answer settled without them.
    pub asked: u32,
    /// Questions the room answered from what it already knew.
    pub spared: u32,
    /// What to change.
    pub observations: Vec<Observation>,
}

impl Retrospective {
    /// The report as a person would read it.
    pub fn render(&self) -> String {
        let mut out = format!(
            "{}\n\n{}/{} objectives delivered, from {} work records.\n",
            self.room, self.delivered, self.objectives, self.records
        );
        if self.asked > 0 || self.spared > 0 {
            out.push_str(&format!(
                "{} question(s) reached you; {} were settled without you.\n",
                self.asked, self.spared
            ));
        }
        if self.observations.is_empty() {
            out.push_str("\nNothing stands out to change.");
            return out;
        }
        out.push_str("\nWorth changing:\n");
        for observation in &self.observations {
            out.push_str(&format!(
                "\n• {}\n  {}\n",
                observation.finding, observation.suggestion
            ));
        }
        out
    }
}

/// How many review cycles counts as going round too often.
const REVIEW_LOOP_THRESHOLD: usize = 2;

/// Read a finished — or running — room back.
pub fn retrospective(engine: &Engine) -> Retrospective {
    let view = engine.digest();

    let mut review_loops: Vec<(TaskId, usize)> = engine
        .tasks()
        .map(|task| (task.id, engine.review_cycles(task.id).len()))
        .filter(|(_, cycles)| *cycles > REVIEW_LOOP_THRESHOLD)
        .collect();
    review_loops.sort_by(|(a_id, a), (b_id, b)| b.cmp(a).then(a_id.cmp(b_id)));

    let decisions = engine.decisions();
    let asked = decisions.decisions.len() as u32;
    // Every task holding an operator gate that never became its own question
    // was settled by something the room already knew.
    let spared = decisions
        .decisions
        .iter()
        .filter(|decision| decision.is_authoritative())
        .count() as u32;

    let records = engine.tasks().count() as u32;
    let mut observations = Vec::new();

    if view.health.coordination_expansion {
        observations.push(Observation {
            pattern: "coordination-expansion".into(),
            finding: format!(
                "{records} records for {} objectives — {:.1} per outcome, {} of \
                 them reviews and follow-ups.",
                view.counts.objectives,
                view.health.inflation_ratio,
                view.health.supporting_records,
            ),
            suggestion: "Attach supporting work to the objective it serves \
                         rather than raising it top-level, and fold repeated \
                         remediations into one."
                .into(),
        });
    }

    if !review_loops.is_empty() {
        let (worst, cycles) = review_loops[0];
        observations.push(Observation {
            pattern: "review-loops".into(),
            finding: format!(
                "{} task(s) went round review more than {REVIEW_LOOP_THRESHOLD} \
                 times; the worst ({worst}) took {cycles}.",
                review_loops.len()
            ),
            suggestion: "Ask reviewers for the full blocking set in one pass, \
                         and treat a third failure on one invariant as a signal \
                         to fix the thing underneath."
                .into(),
        });
    }

    if view.utilization.is_wasting_capacity() {
        observations.push(Observation {
            pattern: "idle-while-queued".into(),
            finding: format!(
                "{} bot(s) were free while {} piece(s) of work sat unclaimed.",
                view.utilization.idle, view.utilization.unclaimed
            ),
            suggestion: "Widen the roles those tasks require, or declare \
                         fallbacks so a free bot can stand in."
                .into(),
        });
    }

    if view.health.stand_in_records > 0 {
        observations.push(Observation {
            pattern: "roster-mismatch".into(),
            finding: format!(
                "{} record(s) were done by a bot standing in for the role asked \
                 for.",
                view.health.stand_in_records
            ),
            suggestion: "The roster does not match the work. Add a bot holding \
                         that role, or stop requiring it."
                .into(),
        });
    }

    if view.health.closed_as_duplicate > 0 {
        observations.push(Observation {
            pattern: "duplicated-work".into(),
            finding: format!(
                "{} record(s) were replaced or retired rather than delivered.",
                view.health.closed_as_duplicate
            ),
            suggestion: "Search for existing work before raising more; the \
                         duplicates cost a turn each to notice and close."
                .into(),
        });
    }

    let stuck: Vec<&str> = engine
        .tasks()
        .filter(|task| task.state == TaskState::Failed)
        .map(|task| task.title.as_str())
        .collect();
    if !stuck.is_empty() {
        observations.push(Observation {
            pattern: "abandoned-work".into(),
            finding: format!("{} task(s) were abandoned: {}.", stuck.len(), stuck.join(", ")),
            suggestion: "Check whether the rework budget ran out mid-argument \
                         rather than the work being genuinely impossible."
                .into(),
        });
    }

    Retrospective {
        room: engine.room().spec.name.clone(),
        delivered: view.counts.delivered,
        objectives: view.counts.objectives,
        records,
        health: view.health,
        review_loops,
        asked,
        spared,
        observations,
    }
}

/// Records per objective, for a quick read of where a room's effort went.
pub fn inflation_by_objective(engine: &Engine) -> BTreeMap<TaskId, usize> {
    let mut counts: BTreeMap<TaskId, usize> = BTreeMap::new();
    for task in engine.tasks() {
        if let Some(objective) = task.lineage.root_objective {
            *counts.entry(objective).or_default() += 1;
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use orgbot_proto::agent::{AgentSpec, Autonomy, ProviderRef};
    use orgbot_proto::graph::{TaskKind, TaskOrigin};
    use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
    use orgbot_proto::task::Priority;
    use orgbot_proto::{AgentId, Event};

    use crate::Command;

    fn room() -> (Engine, AgentId) {
        let mut engine = Engine::new(RoomSpec {
            name: "Leo room repair".into(),
            goal: "repair the harness".into(),
            roster: vec![],
            completion: CompletionRule::AllTasksTerminal,
            limits: RoomLimits::default(),
        });
        let lead = AgentSpec::new("lead", "Lead", ProviderRef::Scripted { script: "x".into() })
            .with_roles(["coordinator"])
            .with_autonomy(Autonomy::coordinator());
        let id = lead.id;
        engine
            .apply(Command::JoinAgent {
                spec: Box::new(lead),
            })
            .unwrap();
        engine.apply(Command::StartRoom).unwrap();
        (engine, id)
    }

    fn add(engine: &mut Engine, lead: AgentId, title: &str, origin: TaskOrigin) -> TaskId {
        let events = engine
            .apply(Command::CreateTask {
                title: title.into(),
                brief: "work".into(),
                by: Some(lead),
                required_role: None,
                priority: Priority::Normal,
                acceptance_criteria: vec![],
                depends_on: vec![],
                origin,
            })
            .unwrap();
        events
            .iter()
            .find_map(|e| match &e.event {
                Event::TaskCreated { task } => Some(task.id),
                _ => None,
            })
            .unwrap()
    }

    /// A room that behaved well has nothing to change.
    #[test]
    fn a_healthy_room_has_nothing_to_report() {
        let (mut engine, lead) = room();
        let objective = add(&mut engine, lead, "repair the harness", {
            let mut origin = TaskOrigin::plain();
            origin.kind = TaskKind::Objective;
            origin
        });
        add(
            &mut engine,
            lead,
            "do the work",
            TaskOrigin::under(TaskKind::Subtask, objective),
        );

        let report = retrospective(&engine);
        assert_eq!(report.observations, vec![]);
        assert!(report.render().contains("Nothing stands out to change"));
    }

    /// The pattern the whole upgrade exists to prevent, seen after the fact.
    #[test]
    fn a_room_that_multiplied_its_work_says_so_and_says_why() {
        let (mut engine, lead) = room();
        let objective = add(&mut engine, lead, "repair the harness", {
            let mut origin = TaskOrigin::plain();
            origin.kind = TaskKind::Objective;
            origin
        });
        for i in 0..12 {
            add(
                &mut engine,
                lead,
                &format!("review pass {i}"),
                TaskOrigin::under(TaskKind::Review, objective),
            );
        }

        let report = retrospective(&engine);
        let expansion = report
            .observations
            .iter()
            .find(|o| o.pattern == "coordination-expansion")
            .expect("it should be noticed");
        assert!(expansion.finding.contains("13 records"), "{expansion:?}");
        assert!(expansion.suggestion.contains("Attach supporting work"));
    }

    /// Every observation has to say what to do about it.
    #[test]
    fn no_observation_is_a_bare_complaint() {
        let (mut engine, lead) = room();
        let objective = add(&mut engine, lead, "one outcome", {
            let mut origin = TaskOrigin::plain();
            origin.kind = TaskKind::Objective;
            origin
        });
        for i in 0..12 {
            add(
                &mut engine,
                lead,
                &format!("review {i}"),
                TaskOrigin::under(TaskKind::Review, objective),
            );
        }

        let report = retrospective(&engine);
        assert!(!report.observations.is_empty());
        for observation in &report.observations {
            assert!(
                !observation.suggestion.is_empty(),
                "{} says what happened but not what to change",
                observation.pattern
            );
        }
    }

    #[test]
    fn replaced_work_is_counted_and_explained() {
        let (mut engine, lead) = room();
        let original = add(&mut engine, lead, "first attempt", TaskOrigin::plain());
        let replacement = add(&mut engine, lead, "canonical version", TaskOrigin::plain());
        engine
            .apply(Command::SupersedeTask {
                task: original,
                replacement,
                reason: "merged".into(),
            })
            .unwrap();

        let report = retrospective(&engine);
        assert!(report
            .observations
            .iter()
            .any(|o| o.pattern == "duplicated-work"));
    }

    #[test]
    fn the_report_reads_as_prose() {
        let (mut engine, lead) = room();
        add(&mut engine, lead, "one outcome", TaskOrigin::plain());

        let rendered = retrospective(&engine).render();
        assert!(rendered.starts_with("Leo room repair"));
        assert!(rendered.contains("objectives delivered"));
    }

    #[test]
    fn records_are_attributed_to_the_objective_they_served() {
        let (mut engine, lead) = room();
        let objective = add(&mut engine, lead, "repair the harness", {
            let mut origin = TaskOrigin::plain();
            origin.kind = TaskKind::Objective;
            origin
        });
        for i in 0..3 {
            add(
                &mut engine,
                lead,
                &format!("step {i}"),
                TaskOrigin::under(TaskKind::Subtask, objective),
            );
        }

        let counts = inflation_by_objective(&engine);
        assert_eq!(counts.get(&objective), Some(&3));
    }
}
