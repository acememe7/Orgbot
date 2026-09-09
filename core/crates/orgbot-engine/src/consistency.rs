//! Things that must never be true at once.
//!
//! Most of the rules in this engine are enforced where the change happens: a
//! transition table refuses an illegal move, a claim checks its gates. That
//! catches everything going through the front door. It does not catch a room
//! restored from a save written by an older build, a state reached by a
//! sequence nobody anticipated, or a bug in one of those checks.
//!
//! So the invariants are also stated once, in one place, as questions that can
//! be asked of a whole room. Each one is a pair that must not both hold —
//! "superseded" and "claimable", "archived" and "needs a person" — and each
//! names the record it found so the answer is actionable rather than a bare
//! count.
//!
//! This is a check, not a repair. Silently fixing an inconsistent room hides
//! the bug that produced it, and the next one is harder to find.

use orgbot_proto::ids::TaskId;
use orgbot_proto::task::{Task, TaskState};

use crate::engine::Engine;

/// One rule that does not hold, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// A short name for the rule, stable enough to grep for.
    pub rule: &'static str,
    /// The record at fault.
    pub task: TaskId,
    /// What is wrong, in a sentence.
    pub detail: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}: {}", self.rule, self.task, self.detail)
    }
}

/// Check every invariant against a room.
///
/// An empty result means the room is coherent. Anything returned is a bug
/// somewhere upstream, not a state to work around.
pub fn check(engine: &Engine) -> Vec<Violation> {
    let mut found = Vec::new();
    for task in engine.tasks() {
        superseded_work_is_never_offered(task, &mut found);
        waiting_work_is_never_held(task, &mut found);
        closed_work_never_asks_for_a_person(task, &mut found);
        replacement_links_agree(engine, task, &mut found);
        reviewed_work_has_something_to_review(engine, task, &mut found);
        a_lease_belongs_to_a_holder(task, &mut found);
        required_inputs_are_current(engine, task, &mut found);
    }
    dependencies_resolve(engine, &mut found);
    found
}

/// A task that has been replaced must not still look like open work.
///
/// Checked against the stored state rather than the derived verdict.
/// `readiness` already refuses to offer a replaced task, which is the right
/// runtime behaviour and also means it can never expose this: the fields
/// underneath still disagree, and that is the bug worth reporting.
fn superseded_work_is_never_offered(task: &Task, found: &mut Vec<Violation>) {
    if task.lineage.is_superseded() && !task.state.is_terminal() {
        found.push(Violation {
            rule: "superseded-not-claimable",
            task: task.id,
            detail: format!(
                "it has been replaced but its state is still `{}`",
                task.state.as_str()
            ),
        });
    }
}

/// A task waiting on something must not be held by a bot.
fn waiting_work_is_never_held(task: &Task, found: &mut Vec<Violation>) {
    if !task.dependency.is_satisfied() && task.state == TaskState::InProgress {
        found.push(Violation {
            rule: "waiting-not-in-progress",
            task: task.id,
            detail: "it is being worked on while an upstream dependency is unmet".into(),
        });
    }
}

/// Work that is closed must not be holding a question for anyone.
///
/// Again against the stored authorization: `readiness` reports a terminal task
/// as closed whatever gate it carries, so the contradiction survives out of
/// sight until something reads the field directly.
fn closed_work_never_asks_for_a_person(task: &Task, found: &mut Vec<Violation>) {
    if task.state.is_terminal() && task.authorization.needs_operator() {
        found.push(Violation {
            rule: "closed-not-asking",
            task: task.id,
            detail: format!(
                "it is {} but still counted as needing a decision",
                task.state.as_str()
            ),
        });
    }
}

/// A replacement link must point somewhere, and only one way.
fn replacement_links_agree(engine: &Engine, task: &Task, found: &mut Vec<Violation>) {
    let Some(replacement) = task.lineage.superseded_by else {
        return;
    };
    if replacement == task.id {
        found.push(Violation {
            rule: "no-self-supersession",
            task: task.id,
            detail: "it is recorded as replacing itself".into(),
        });
        return;
    }
    match engine.task(replacement) {
        None => found.push(Violation {
            rule: "replacement-exists",
            task: task.id,
            detail: format!("it points at {replacement}, which is not in this room"),
        }),
        // Two records each claiming to replace the other leaves nothing
        // actionable: both are closed and neither carries the work.
        Some(other) if other.lineage.superseded_by == Some(task.id) => {
            found.push(Violation {
                rule: "one-live-replacement",
                task: task.id,
                detail: format!("it and {replacement} each claim to replace the other"),
            });
        }
        Some(_) => {}
    }
}

/// Approving work with no deliverable means the verdict describes nothing.
fn reviewed_work_has_something_to_review(
    engine: &Engine,
    task: &Task,
    found: &mut Vec<Violation>,
) {
    if task.state != TaskState::Approved && task.state != TaskState::Submitted {
        return;
    }
    let missing: Vec<String> = task
        .artifacts
        .iter()
        .filter(|id| engine.artifact(**id).is_none())
        .map(ToString::to_string)
        .collect();
    if !missing.is_empty() {
        found.push(Violation {
            rule: "reviewed-artifacts-exist",
            task: task.id,
            detail: format!(
                "it passed inspection citing {} that no longer exists",
                missing.join(", ")
            ),
        });
    }
}

/// A lease without a holder is a claim on nobody.
fn a_lease_belongs_to_a_holder(task: &Task, found: &mut Vec<Violation>) {
    if task.lease_expires_at.is_some() && task.assignee.is_none() {
        found.push(Violation {
            rule: "lease-has-a-holder",
            task: task.id,
            detail: "it holds a lease with nobody assigned".into(),
        });
    }
}

/// Work must not be built on an input that has been replaced.
///
/// Naming an exact artifact is what stops "the schema Savage wrote" quietly
/// meaning a later, different schema. That only helps if something notices
/// when the named one is superseded.
fn required_inputs_are_current(engine: &Engine, task: &Task, found: &mut Vec<Violation>) {
    if task.state.is_terminal() {
        return;
    }
    for input in &task.requires_artifacts {
        match engine.artifact(*input) {
            None => found.push(Violation {
                rule: "input-exists",
                task: task.id,
                detail: format!("it requires {input}, which is not in this room"),
            }),
            Some(artifact) if !artifact.is_current() => found.push(Violation {
                rule: "input-is-current",
                task: task.id,
                detail: format!(
                    "it is built on `{}`, which has since been replaced",
                    artifact.name
                ),
            }),
            Some(_) => {}
        }
    }
}

/// Every dependency must name a task in this room, and none may be circular.
fn dependencies_resolve(engine: &Engine, found: &mut Vec<Violation>) {
    for task in engine.tasks() {
        for dependency in &task.depends_on {
            if engine.task(*dependency).is_none() {
                found.push(Violation {
                    rule: "dependency-exists",
                    task: task.id,
                    detail: format!("it waits on {dependency}, which is not in this room"),
                });
            }
        }
        if reaches(engine, task.id, task.id) {
            found.push(Violation {
                rule: "dependencies-acyclic",
                task: task.id,
                detail: "its dependencies lead back to itself".into(),
            });
        }
    }
}

/// Whether `from` can reach `target` through dependency edges.
///
/// Bounded rather than memoised: a room's board is small, and a cycle must not
/// hang the check that exists to find cycles.
fn reaches(engine: &Engine, from: TaskId, target: TaskId) -> bool {
    let mut frontier: Vec<TaskId> = engine
        .task(from)
        .map(|task| task.depends_on.clone())
        .unwrap_or_default();
    let mut seen: Vec<TaskId> = vec![from];
    let mut steps = 0;
    while let Some(next) = frontier.pop() {
        steps += 1;
        if steps > 4_096 {
            return false;
        }
        if next == target {
            return true;
        }
        if seen.contains(&next) {
            continue;
        }
        seen.push(next);
        if let Some(task) = engine.task(next) {
            frontier.extend(task.depends_on.iter().copied());
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use orgbot_proto::agent::{AgentSpec, Autonomy, ProviderRef};
    use orgbot_proto::graph::{AuthorizationState, DependencyState};
    use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
    use orgbot_proto::{AgentId, Timestamp};

    use crate::Command;

    fn engine() -> (Engine, AgentId) {
        let mut engine = Engine::new(RoomSpec {
            name: "check".into(),
            goal: "stay coherent".into(),
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

    fn task(engine: &mut Engine, lead: AgentId, title: &str) -> TaskId {
        let events = engine
            .apply(Command::CreateTask {
                title: title.into(),
                brief: "work".into(),
                by: Some(lead),
                required_role: None,
                priority: orgbot_proto::task::Priority::Normal,
                acceptance_criteria: vec![],
                depends_on: vec![],
                origin: orgbot_proto::graph::TaskOrigin::plain(),
            })
            .unwrap();
        let id = events
            .iter()
            .find_map(|e| match &e.event {
                orgbot_proto::event::Event::TaskCreated { task } => Some(task.id),
                _ => None,
            })
            .unwrap();
        engine.apply(Command::OpenTask { task: id }).unwrap();
        id
    }

    /// A healthy room says nothing.
    #[test]
    fn an_ordinary_room_is_coherent() {
        let (mut engine, lead) = engine();
        let first = task(&mut engine, lead, "first");
        let _second = task(&mut engine, lead, "second");
        engine
            .apply(Command::ClaimTask {
                task: first,
                by: lead,
            })
            .unwrap();

        assert_eq!(check(&engine), vec![]);
    }

    #[test]
    fn a_room_that_supersedes_properly_stays_coherent() {
        let (mut engine, lead) = engine();
        let original = task(&mut engine, lead, "first attempt");
        let replacement = task(&mut engine, lead, "canonical version");
        engine
            .apply(Command::SupersedeTask {
                task: original,
                replacement,
                reason: "merged".into(),
            })
            .unwrap();

        assert_eq!(check(&engine), vec![]);
    }

    /// The state a save from an older build could arrive in.
    #[test]
    fn replaced_work_still_being_offered_is_caught() {
        let (mut engine, lead) = engine();
        let original = task(&mut engine, lead, "first");
        let replacement = task(&mut engine, lead, "second");
        // Set the link without the transition, as a partial write would.
        engine.task_mut(original).unwrap().lineage.superseded_by = Some(replacement);

        let found = check(&engine);
        assert!(
            found.iter().any(|v| v.rule == "superseded-not-claimable"),
            "got {found:?}"
        );
        assert!(found[0].to_string().contains("still `ready`"));
    }

    #[test]
    fn two_records_each_replacing_the_other_is_caught() {
        let (mut engine, lead) = engine();
        let first = task(&mut engine, lead, "first");
        let second = task(&mut engine, lead, "second");
        engine.task_mut(first).unwrap().lineage.superseded_by = Some(second);
        engine.task_mut(second).unwrap().lineage.superseded_by = Some(first);

        assert!(check(&engine)
            .iter()
            .any(|v| v.rule == "one-live-replacement"));
    }

    #[test]
    fn a_replacement_that_is_not_in_the_room_is_caught() {
        let (mut engine, lead) = engine();
        let orphan = task(&mut engine, lead, "first");
        engine.task_mut(orphan).unwrap().lineage.superseded_by = Some(TaskId::new());

        assert!(check(&engine).iter().any(|v| v.rule == "replacement-exists"));
    }

    #[test]
    fn work_in_progress_on_an_unmet_dependency_is_caught() {
        let (mut engine, lead) = engine();
        let blocked = task(&mut engine, lead, "downstream");
        {
            let record = engine.task_mut(blocked).unwrap();
            record.state = TaskState::InProgress;
            record.assignee = Some(lead);
            record.dependency = DependencyState::Waiting {
                on: TaskId::new(),
                reason: "source recovery".into(),
                owner: None,
            };
        }

        assert!(check(&engine)
            .iter()
            .any(|v| v.rule == "waiting-not-in-progress"));
    }

    /// A finished task must not keep a person's attention.
    #[test]
    fn closed_work_that_still_asks_for_a_person_is_caught() {
        let (mut engine, lead) = engine();
        let done = task(&mut engine, lead, "finished");
        {
            let record = engine.task_mut(done).unwrap();
            record.state = TaskState::Cancelled;
            record.authorization = AuthorizationState::AwaitingOperator {
                because: "it costs money".into(),
                options: vec![],
            };
        }

        assert!(check(&engine).iter().any(|v| v.rule == "closed-not-asking"));
    }

    #[test]
    fn a_lease_with_nobody_holding_it_is_caught() {
        let (mut engine, lead) = engine();
        let orphaned = task(&mut engine, lead, "abandoned");
        {
            let record = engine.task_mut(orphaned).unwrap();
            record.assignee = None;
            record.lease_expires_at = Some(Timestamp::now());
        }

        assert!(check(&engine).iter().any(|v| v.rule == "lease-has-a-holder"));
    }

    #[test]
    fn approving_work_citing_a_vanished_artifact_is_caught() {
        let (mut engine, lead) = engine();
        let approved = task(&mut engine, lead, "delivered");
        {
            let record = engine.task_mut(approved).unwrap();
            record.state = TaskState::Approved;
            record.artifacts = vec![orgbot_proto::ArtifactId::new()];
        }

        assert!(check(&engine)
            .iter()
            .any(|v| v.rule == "reviewed-artifacts-exist"));
    }

    #[test]
    fn a_dependency_on_nothing_is_caught() {
        let (mut engine, lead) = engine();
        let dangling = task(&mut engine, lead, "downstream");
        engine.task_mut(dangling).unwrap().depends_on = vec![TaskId::new()];

        assert!(check(&engine).iter().any(|v| v.rule == "dependency-exists"));
    }

    /// The engine refuses to create one, but a loaded save has made no promise.
    #[test]
    fn a_dependency_cycle_is_caught_without_hanging() {
        let (mut engine, lead) = engine();
        let first = task(&mut engine, lead, "first");
        let second = task(&mut engine, lead, "second");
        engine.task_mut(first).unwrap().depends_on = vec![second];
        engine.task_mut(second).unwrap().depends_on = vec![first];

        let found = check(&engine);
        assert!(found.iter().any(|v| v.rule == "dependencies-acyclic"));
    }

    #[test]
    fn a_violation_reads_as_a_sentence() {
        let violation = Violation {
            rule: "superseded-not-claimable",
            task: TaskId::new(),
            detail: "it has been replaced".into(),
        };
        let rendered = violation.to_string();
        assert!(rendered.starts_with("[superseded-not-claimable]"));
        assert!(rendered.ends_with("it has been replaced"));
    }
}
