//! What a person sees instead of the board.
//!
//! A room with sixteen bots produced ninety-six work records for twelve real
//! objectives, and the headline number a person saw was `Tasks · 96`. That
//! number went up as the room made progress, because finishing something
//! produced reviews, and reviews produced remediations. The board got busier
//! the closer it got to done.
//!
//! This is the other half of that fix. Bots keep the full operational graph —
//! every review, every evidence gap, every superseded record, all of it
//! auditable. A person gets objectives, progress, and the decisions only they
//! can make. Nothing is deleted to achieve it: [`RoomDigest`] is a view over
//! the same tasks, computed fresh.
//!
//! Two counts are deliberately *not* what they look like:
//!
//! - **Progress ignores supporting records.** Ten review cycles under one
//!   objective must not make the room look less complete than it was before
//!   anyone reviewed anything. Only work that carries the outcome counts
//!   toward the denominator.
//! - **`needs_you` is the operator's whole world.** It counts tasks whose
//!   authorization genuinely waits on a person, and nothing else. An internal
//!   blocker is progress, not a request.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use orgbot_proto::graph::Readiness;
use orgbot_proto::ids::{AgentId, TaskId};
use orgbot_proto::task::{Task, TaskState};

/// Counts a person can act on, in the order they matter.
///
/// `raw_records` is last and named as it is on purpose: it is the number that
/// used to lead, and it belongs in diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BoardCounts {
    /// Independently owned outcomes.
    pub objectives: u32,
    /// Objectives with work happening right now.
    pub active: u32,
    /// Objectives the room is unblocking by itself.
    pub waiting_internally: u32,
    /// Objectives that genuinely need a person. Usually zero.
    pub needs_you: u32,
    /// Objectives finished.
    pub delivered: u32,
    /// Every live work record behind those objectives. Diagnostics only.
    pub raw_records: u32,
}

/// How much of an objective is done.
///
/// Counted in work that carries the outcome, never in supporting records.
/// Splitting a task in two changes this; reviewing it does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Progress {
    /// Carrying work delivered.
    pub done: u32,
    /// Carrying work in total.
    pub total: u32,
}

impl Progress {
    /// Completion as a fraction, or `None` when there is nothing to count.
    pub fn fraction(self) -> Option<f32> {
        (self.total > 0).then(|| f32::from(self.done as u16) / f32::from(self.total as u16))
    }

    /// `7 / 10 stages complete`, or a plain state when there are no stages.
    pub fn label(self) -> String {
        if self.total == 0 {
            return "not broken down yet".into();
        }
        format!("{} / {} stages complete", self.done, self.total)
    }
}

/// One outcome, as a person should see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectiveCard {
    /// The objective itself.
    pub task: TaskId,
    /// What it is.
    pub title: String,
    /// How far along, counted in carrying work.
    pub progress: Progress,
    /// A short state line: "internal remediation", "waiting", "needs you".
    pub status: String,
    /// What is holding it up right now, if anything.
    pub blocker: Option<String>,
    /// The bots working on it.
    pub owners: Vec<AgentId>,
    /// Whether a person has to do something. Shown on every card, because the
    /// absence of this was what made the board exhausting to read.
    pub needs_operator: bool,
    /// Every live record beneath it, for the expanded view.
    pub records: Vec<TaskId>,
}

/// A decision waiting on a person, with everything needed to answer it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingDecision {
    /// The task that cannot proceed.
    pub task: TaskId,
    /// What that task is.
    pub title: String,
    /// Completes "Operator decision required because: ___".
    pub because: String,
    /// The answers offered, for a one-tap reply.
    pub options: Vec<String>,
    /// The objective this decision unblocks, when it serves one.
    pub objective: Option<TaskId>,
}

/// Signals that the room is coordinating more than it is working.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    /// Live records per objective. Around one is healthy; eight means the
    /// room spent most of its effort describing its own work.
    pub inflation_ratio: f32,
    /// Records that exist only to support other records.
    pub supporting_records: u32,
    /// Work replaced or retired, which is fine in itself but tells you how
    /// much churn there has been.
    pub closed_as_duplicate: u32,
    /// Whether the record count is growing much faster than the outcomes.
    pub coordination_expansion: bool,
}

impl Health {
    /// Above this, the room is producing far more coordination than work.
    ///
    /// Four records per objective — an implementation, a review, a
    /// remediation and a re-review — is an ordinary hard task. Beyond that
    /// the graph is describing itself.
    pub const EXPANSION_THRESHOLD: f32 = 4.0;
}

/// Everything a person needs, and nothing they do not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomDigest {
    /// The outcomes, most urgent first.
    pub objectives: Vec<ObjectiveCard>,
    /// The headline numbers.
    pub counts: BoardCounts,
    /// The operator's inbox. Usually empty.
    pub needs_you: Vec<PendingDecision>,
    /// The chain currently deciding when the room finishes.
    pub critical_path: Vec<TaskId>,
    /// Whether the room is coordinating more than it is working.
    pub health: Health,
}

impl RoomDigest {
    /// A one-paragraph status, the way a person would write it.
    ///
    /// This is what replaces `96 tasks / 13 in flight / 27 blocked / 31 ready`,
    /// which said nothing about whether anything was expected of the reader.
    pub fn summary(&self, room: &str) -> String {
        let c = &self.counts;
        let mut lines = vec![
            room.to_string(),
            String::new(),
            format!("{}/{} objectives complete", c.delivered, c.objectives),
        ];
        if c.active > 0 {
            lines.push(format!("{} in flight", c.active));
        }
        if c.waiting_internally > 0 {
            lines.push(format!("{} waiting on the room itself", c.waiting_internally));
        }
        lines.push(match c.needs_you {
            0 => "0 decisions needed from you".into(),
            1 => "1 decision needed from you".into(),
            n => format!("{n} decisions needed from you"),
        });

        let live: Vec<&ObjectiveCard> = self
            .objectives
            .iter()
            .filter(|card| card.progress.done < card.progress.total || card.blocker.is_some())
            .take(5)
            .collect();
        if !live.is_empty() {
            lines.push(String::new());
            lines.push("Current:".into());
            for card in live {
                match &card.blocker {
                    Some(blocker) => lines.push(format!("• {} — {blocker}", card.title)),
                    None => lines.push(format!("• {} — {}", card.title, card.status)),
                }
            }
        }
        lines.join("\n")
    }
}

/// Build the digest from a room's tasks.
///
/// Takes an iterator rather than the engine so it can be tested on a handful
/// of tasks without standing a room up, and so the same computation can serve
/// a snapshot as easily as a live room.
pub fn digest<'a>(tasks: impl Iterator<Item = &'a Task>) -> RoomDigest {
    let tasks: Vec<&Task> = tasks.collect();
    let by_id: HashMap<TaskId, &Task> = tasks.iter().map(|t| (t.id, *t)).collect();

    // Records grouped under the outcome they serve. A record that names no
    // objective stands as its own outcome, which is how a room that never
    // declares objectives still reads sensibly.
    let mut children: HashMap<TaskId, Vec<&Task>> = HashMap::new();
    let mut roots: Vec<&Task> = Vec::new();
    for task in &tasks {
        match objective_of(task, &by_id) {
            Some(objective) if objective != task.id => {
                children.entry(objective).or_default().push(task);
            }
            _ => roots.push(task),
        }
    }
    // A record whose objective is itself is a root; so is an orphan.
    roots.sort_by_key(|t| t.created_at);

    let mut objectives: Vec<ObjectiveCard> = roots
        .iter()
        .filter(|task| !is_closed(task))
        .map(|task| card_for(task, children.get(&task.id).map_or(&[][..], Vec::as_slice)))
        .collect();

    // Anything needing a person first, then anything blocked, then the rest.
    objectives.sort_by_key(|card| {
        (
            !card.needs_operator,
            card.blocker.is_none(),
            card.title.clone(),
        )
    });

    let live: Vec<&&Task> = tasks.iter().filter(|task| !is_closed(task)).collect();
    let counts = BoardCounts {
        objectives: objectives.len() as u32,
        active: objectives
            .iter()
            .filter(|c| !c.owners.is_empty() && !c.needs_operator)
            .count() as u32,
        waiting_internally: objectives
            .iter()
            .filter(|c| c.blocker.is_some() && !c.needs_operator)
            .count() as u32,
        needs_you: objectives.iter().filter(|c| c.needs_operator).count() as u32,
        delivered: roots
            .iter()
            .filter(|t| t.state == TaskState::Submitted)
            .count() as u32,
        raw_records: live.len() as u32,
    };

    let needs_you = tasks
        .iter()
        .filter(|task| !is_closed(task))
        .filter_map(|task| match task.readiness() {
            Readiness::BlockedOperator { because, options } => Some(PendingDecision {
                task: task.id,
                title: task.title.clone(),
                because,
                options,
                objective: task.lineage.root_objective,
            }),
            _ => None,
        })
        .collect();

    let supporting = live.iter().filter(|t| t.kind.is_supporting()).count() as u32;
    let outcomes = counts.objectives.max(1) as f32;
    let inflation = counts.raw_records as f32 / outcomes;

    RoomDigest {
        objectives,
        needs_you,
        critical_path: critical_path(&tasks, &by_id),
        health: Health {
            inflation_ratio: inflation,
            supporting_records: supporting,
            closed_as_duplicate: tasks
                .iter()
                .filter(|t| {
                    matches!(t.state, TaskState::Superseded | TaskState::Retired)
                })
                .count() as u32,
            coordination_expansion: counts.objectives > 0
                && inflation > Health::EXPANSION_THRESHOLD,
        },
        counts,
    }
}

/// Whether a record has left the live board.
///
/// Delivered work has left it too: it is finished, and showing it as active
/// would be wrong. It still counts toward progress — see [`counts_toward`].
fn is_closed(task: &Task) -> bool {
    task.state.is_terminal() || task.lineage.is_superseded()
}

/// Whether a record still counts toward how much of an objective is done.
///
/// Delivered work obviously does; that is the whole point of the number.
/// Replaced, retired and cancelled work does not, because counting it would
/// mean an objective could never reach complete once anything was abandoned.
fn counts_toward(task: &Task) -> bool {
    !task.lineage.is_superseded()
        && !task.state.is_permanent()
        && task.state != TaskState::Cancelled
}

/// The outcome a record serves, following its own link or walking up.
fn objective_of(task: &Task, by_id: &HashMap<TaskId, &Task>) -> Option<TaskId> {
    if let Some(root) = task.lineage.root_objective {
        return Some(root);
    }
    // A record created before objectives existed still has a parent chain.
    let mut current = task.lineage.parent.or_else(|| task.lineage.subject())?;
    for _ in 0..32 {
        let parent = by_id.get(&current)?;
        if parent.kind.is_headline() {
            return Some(current);
        }
        if let Some(root) = parent.lineage.root_objective {
            return Some(root);
        }
        match parent.lineage.parent.or_else(|| parent.lineage.subject()) {
            Some(next) if next != current => current = next,
            _ => return Some(current),
        }
    }
    None
}

/// Build one objective's card from it and everything beneath it.
fn card_for(objective: &Task, records: &[&Task]) -> ObjectiveCard {
    let live: Vec<&&Task> = records.iter().filter(|t| !is_closed(t)).collect();

    // Progress counts only work that carries the outcome. Reviews and
    // remediations are real work and stay on the board, but a room does not
    // become less finished because it inspected itself.
    //
    // Counted over everything that still stands rather than over what is
    // still open — otherwise finishing a stage would remove it from the
    // denominator and the fraction would never move.
    let carrying: Vec<&&Task> = records
        .iter()
        .filter(|t| counts_toward(t) && !t.kind.is_supporting())
        .collect();
    let progress = Progress {
        done: carrying
            .iter()
            .filter(|t| t.state == TaskState::Submitted)
            .count() as u32,
        total: carrying.len() as u32,
    };

    let mut owners: Vec<AgentId> = live.iter().filter_map(|t| t.assignee).collect();
    if let Some(assignee) = objective.assignee {
        owners.push(assignee);
    }
    let mut seen = HashSet::new();
    owners.retain(|id| seen.insert(*id));

    // What is actually holding this up: the objective's own state first, then
    // the most pressing thing beneath it.
    let readiness = objective.readiness();
    let needs_operator = readiness.needs_operator()
        || live.iter().any(|t| t.readiness().needs_operator());

    let blocker = blocking_line(&readiness).or_else(|| {
        live.iter()
            .map(|t| t.readiness())
            .find(|r| r.needs_operator())
            .or_else(|| {
                live.iter()
                    .map(|t| t.readiness())
                    .find(|r| !matches!(r, Readiness::ReadyNow | Readiness::Underway { .. }))
            })
            .and_then(|r| blocking_line(&r))
    });

    let status = if needs_operator {
        "needs you".into()
    } else if blocker.is_some() {
        "internal remediation".into()
    } else if !owners.is_empty() {
        "in progress".into()
    } else {
        readiness.label().to_lowercase()
    };

    ObjectiveCard {
        task: objective.id,
        title: objective.title.clone(),
        progress,
        status,
        blocker,
        owners,
        needs_operator,
        records: live.iter().map(|t| t.id).collect(),
    }
}

/// A short phrase naming what is holding something up, or `None` when nothing
/// is.
fn blocking_line(readiness: &Readiness) -> Option<String> {
    match readiness {
        Readiness::ReadyNow | Readiness::Underway { .. } | Readiness::NotYetOpen => None,
        Readiness::WaitingDependency { reason, .. } => Some(format!("waiting on {reason}")),
        Readiness::BlockedInternal { reason } => Some(reason.clone()),
        Readiness::HeldByPolicy { policy, .. } => Some(format!("held by the {policy} rule")),
        Readiness::BlockedOperator { because, .. } => Some(because.clone()),
        Readiness::Closed { .. } => None,
    }
}

/// The longest chain of unfinished work that must happen in order.
///
/// More useful than any count: it is the sequence that decides when the room
/// can finish, so it is the thing worth unblocking first.
fn critical_path(tasks: &[&Task], by_id: &HashMap<TaskId, &Task>) -> Vec<TaskId> {
    let mut longest: HashMap<TaskId, Vec<TaskId>> = HashMap::new();
    let mut best: Vec<TaskId> = Vec::new();

    for task in tasks {
        if is_closed(task) {
            continue;
        }
        let chain = chain_to(task.id, by_id, &mut longest, &mut HashSet::new());
        if chain.len() > best.len() {
            best = chain;
        }
    }
    best
}

/// The longest unfinished dependency chain ending at `id`, memoised.
///
/// `visiting` guards against a dependency cycle: the engine refuses to create
/// one, but a room file loaded from disk has made no such promise, and a
/// digest must never hang the caller.
fn chain_to(
    id: TaskId,
    by_id: &HashMap<TaskId, &Task>,
    memo: &mut HashMap<TaskId, Vec<TaskId>>,
    visiting: &mut HashSet<TaskId>,
) -> Vec<TaskId> {
    if let Some(known) = memo.get(&id) {
        return known.clone();
    }
    if !visiting.insert(id) {
        return Vec::new();
    }

    let mut longest: Vec<TaskId> = Vec::new();
    if let Some(task) = by_id.get(&id) {
        for dep in &task.depends_on {
            let Some(upstream) = by_id.get(dep) else {
                continue;
            };
            if is_closed(upstream) {
                continue;
            }
            let candidate = chain_to(*dep, by_id, memo, visiting);
            if candidate.len() > longest.len() {
                longest = candidate;
            }
        }
    }
    longest.push(id);

    visiting.remove(&id);
    memo.insert(id, longest.clone());
    longest
}

#[cfg(test)]
mod tests {
    use super::*;
    use orgbot_proto::graph::{AuthorizationState, DependencyState, Lineage, TaskKind};
    use orgbot_proto::ids::RoomId;

    fn room() -> RoomId {
        RoomId::new()
    }

    fn objective(room: RoomId, title: &str) -> Task {
        let mut task = Task::draft(room, title, "an outcome");
        task.kind = TaskKind::Objective;
        task.state = TaskState::Ready;
        task
    }

    fn under(room: RoomId, title: &str, kind: TaskKind, objective: TaskId) -> Task {
        let mut task = Task::draft(room, title, "work");
        task.kind = kind;
        task.state = TaskState::Ready;
        task.lineage = Lineage {
            root_objective: Some(objective),
            parent: Some(objective),
            ..Default::default()
        };
        task
    }

    /// The headline: many records, few outcomes.
    #[test]
    fn a_crowded_board_reads_as_a_few_objectives() {
        let room = room();
        let a = objective(room, "repair the harness");
        let b = objective(room, "ship the release");
        let mut all = vec![a.clone(), b.clone()];
        for i in 0..8 {
            all.push(under(room, &format!("step {i}"), TaskKind::Subtask, a.id));
            all.push(under(room, &format!("review {i}"), TaskKind::Review, a.id));
        }

        let view = digest(all.iter());
        assert_eq!(view.counts.objectives, 2, "two outcomes, not eighteen");
        assert_eq!(view.counts.raw_records, 18, "every record is still tracked");
        assert_eq!(view.counts.needs_you, 0);
    }

    /// Reviewing your work must not make the room look less finished.
    #[test]
    fn adding_review_cycles_does_not_reduce_progress() {
        let room = room();
        let goal = objective(room, "repair the harness");
        let mut work: Vec<Task> = (0..4)
            .map(|i| under(room, &format!("stage {i}"), TaskKind::Subtask, goal.id))
            .collect();
        for task in work.iter_mut().take(2) {
            task.state = TaskState::Submitted;
        }

        let mut all = vec![goal.clone()];
        all.extend(work.clone());
        let before = digest(all.iter()).objectives[0].progress;
        assert_eq!(before, Progress { done: 2, total: 4 });

        // Ten reviews and remediations later, the same amount is done.
        for i in 0..10 {
            let kind = if i % 2 == 0 {
                TaskKind::Review
            } else {
                TaskKind::Remediation
            };
            all.push(under(room, &format!("cycle {i}"), kind, goal.id));
        }
        let after = digest(all.iter()).objectives[0].progress;
        assert_eq!(after, before, "supporting work does not move the goalposts");
        assert_eq!(after.label(), "2 / 4 stages complete");
    }

    /// The operator inbox holds exactly the things a person must answer.
    #[test]
    fn needs_you_counts_only_real_decisions() {
        let room = room();
        let goal = objective(room, "deploy the service");
        let mut waiting = under(room, "provision the VPS", TaskKind::Subtask, goal.id);
        waiting.authorization = AuthorizationState::AwaitingOperator {
            because: "it provisions paid infrastructure".into(),
            options: vec!["Approve".into(), "Decline".into()],
        };
        let mut internal = under(room, "gather evidence", TaskKind::Evidence, goal.id);
        internal.authorization = AuthorizationState::SecurityHold {
            finding: "need filesystem classification".into(),
            material: false,
        };
        let mut blocked = under(room, "rebuild", TaskKind::Subtask, goal.id);
        blocked.dependency = DependencyState::Waiting {
            on: TaskId::new(),
            reason: "source recovery".into(),
            owner: None,
        };

        let view = digest([goal, waiting, internal, blocked].iter());
        assert_eq!(view.needs_you.len(), 1);
        assert_eq!(view.needs_you[0].because, "it provisions paid infrastructure");
        assert_eq!(view.needs_you[0].options.len(), 2);
        assert_eq!(view.counts.needs_you, 1);
    }

    /// A room busy with its own work asks for nothing.
    #[test]
    fn a_busy_room_needing_nothing_says_so_plainly() {
        let room = room();
        let goal = objective(room, "repair the harness");
        let mut working = under(room, "implement", TaskKind::Subtask, goal.id);
        working.state = TaskState::InProgress;
        working.assignee = Some(AgentId::new());
        let mut remediating = under(room, "fix the finding", TaskKind::Remediation, goal.id);
        remediating.authorization = AuthorizationState::SecurityHold {
            finding: "awaiting evidence".into(),
            material: false,
        };

        let view = digest([goal, working, remediating].iter());
        assert_eq!(view.counts.needs_you, 0);
        assert!(view.needs_you.is_empty());
        let summary = view.summary("Leo room repair");
        assert!(
            summary.contains("0 decisions needed from you"),
            "got:\n{summary}"
        );
    }

    #[test]
    fn a_card_names_its_blocker_and_its_owners() {
        let room = room();
        let goal = objective(room, "repair the harness");
        let bot = AgentId::new();
        let mut working = under(room, "implement", TaskKind::Subtask, goal.id);
        working.state = TaskState::InProgress;
        working.assignee = Some(bot);
        let mut waiting = under(room, "rebuild", TaskKind::Subtask, goal.id);
        waiting.dependency = DependencyState::Waiting {
            on: TaskId::new(),
            reason: "B3-C source recovery".into(),
            owner: Some(bot),
        };

        let view = digest([goal, working, waiting].iter());
        let card = &view.objectives[0];
        assert_eq!(card.owners, vec![bot]);
        assert_eq!(
            card.blocker.as_deref(),
            Some("waiting on B3-C source recovery")
        );
        assert!(!card.needs_operator);
        assert_eq!(card.status, "internal remediation");
    }

    /// Replaced and retired work leaves the person's view but stays countable.
    #[test]
    fn closed_records_leave_the_live_board() {
        let room = room();
        let goal = objective(room, "repair the harness");
        let live = under(room, "current attempt", TaskKind::Subtask, goal.id);
        let mut replaced = under(room, "first attempt", TaskKind::Subtask, goal.id);
        replaced.state = TaskState::Superseded;
        replaced.lineage.superseded_by = Some(live.id);

        let view = digest([goal, live, replaced].iter());
        assert_eq!(view.counts.raw_records, 2, "the replaced record is not live");
        assert_eq!(view.health.closed_as_duplicate, 1, "but it is still counted");
        assert_eq!(view.objectives[0].records.len(), 1);
    }

    /// The chain that decides when the room finishes.
    #[test]
    fn the_critical_path_is_the_longest_unfinished_chain() {
        let room = room();
        let a = objective(room, "recover the source");
        let mut b = objective(room, "rebuild");
        b.depends_on = vec![a.id];
        let mut c = objective(room, "re-run the checks");
        c.depends_on = vec![b.id];
        // A short branch that is not the constraint.
        let mut side = objective(room, "update the docs");
        side.depends_on = vec![a.id];

        let view = digest([a.clone(), b.clone(), c.clone(), side].iter());
        assert_eq!(view.critical_path, vec![a.id, b.id, c.id]);
    }

    /// A cycle in a loaded room file must not hang the digest.
    #[test]
    fn a_dependency_cycle_does_not_hang_the_critical_path() {
        let room = room();
        let mut a = objective(room, "a");
        let mut b = objective(room, "b");
        a.depends_on = vec![b.id];
        b.depends_on = vec![a.id];

        let view = digest([a, b].iter());
        assert!(view.critical_path.len() <= 2);
    }

    /// Coordination outrunning the work is worth saying out loud.
    #[test]
    fn runaway_coordination_is_flagged() {
        let room = room();
        let goal = objective(room, "one outcome");
        let mut all = vec![goal.clone()];
        for i in 0..12 {
            all.push(under(room, &format!("review {i}"), TaskKind::Review, goal.id));
        }

        let view = digest(all.iter());
        assert!(view.health.coordination_expansion);
        assert!(view.health.inflation_ratio > Health::EXPANSION_THRESHOLD);
        assert_eq!(view.health.supporting_records, 12);
    }

    #[test]
    fn an_ordinary_room_is_not_flagged() {
        let room = room();
        let goal = objective(room, "one outcome");
        let all = [
            goal.clone(),
            under(room, "implement", TaskKind::Subtask, goal.id),
            under(room, "review", TaskKind::Review, goal.id),
        ];
        let view = digest(all.iter());
        assert!(!view.health.coordination_expansion);
    }

    /// A room that never declares an objective still reads sensibly: each
    /// top-level task stands as its own outcome.
    #[test]
    fn a_room_with_no_objectives_still_produces_cards() {
        let room = room();
        let mut first = Task::draft(room, "write the notes", "do it");
        first.state = TaskState::Ready;
        let mut second = Task::draft(room, "ship them", "do it");
        second.state = TaskState::Submitted;

        let view = digest([first, second].iter());
        assert_eq!(view.counts.objectives, 1, "the delivered one has closed");
        assert_eq!(view.counts.delivered, 1);
        assert_eq!(view.counts.needs_you, 0);
    }

    /// The paragraph that replaces a wall of counts.
    #[test]
    fn the_summary_reads_like_a_person_wrote_it() {
        let room = room();
        let done = {
            let mut task = objective(room, "first objective");
            task.state = TaskState::Submitted;
            task
        };
        let goal = objective(room, "E0: blank preview reproduction");
        let mut waiting = under(room, "reproduce it", TaskKind::Subtask, goal.id);
        waiting.dependency = DependencyState::Waiting {
            on: TaskId::new(),
            reason: "B3-C source recovery".into(),
            owner: None,
        };

        let view = digest([done, goal, waiting].iter());
        let summary = view.summary("Leo room repair");
        assert!(summary.starts_with("Leo room repair"));
        assert!(summary.contains("objectives complete"), "got:\n{summary}");
        assert!(summary.contains("0 decisions needed from you"), "got:\n{summary}");
        assert!(
            summary.contains("B3-C source recovery"),
            "the current blocker should be named:\n{summary}"
        );
    }

    #[test]
    fn progress_with_nothing_broken_down_says_so() {
        assert_eq!(Progress::default().label(), "not broken down yet");
        assert_eq!(Progress::default().fraction(), None);
        assert_eq!(
            Progress { done: 1, total: 2 }.fraction(),
            Some(0.5)
        );
    }
}
