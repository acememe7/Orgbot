//! Whether the room is in a state it can get itself out of.
//!
//! A room that has stopped making progress and says nothing is worse than one
//! that fails: the operator has no reason to look, so nobody looks. One room
//! ran for four days with six tasks blocked on work that had already been
//! delivered and five bots parked holding them, and reported nothing at all —
//! the board rendered every one of them as ordinary waiting.
//!
//! The engine already knew. `consistency::check` computes broken invariants
//! and nothing ever called it; `starved_roles` computes work nobody free can
//! take and only the digest looked. This gathers what the room can already
//! work out about itself into one answer, so there is something to notice.
//!
//! Everything here is a *determinate* condition rather than a heuristic about
//! elapsed time. "This task has not moved in six hours" is a guess — a bot may
//! be mid-turn on it, and a room that guesses wrong teaches its operator to
//! ignore it. "This task waits on a task that failed" is a fact, and it will
//! still be a fact tomorrow. Only facts are worth waking someone for.

use orgbot_proto::task::TaskState;
use orgbot_proto::TaskId;

use crate::consistency::Violation;
use crate::engine::Engine;

/// Work waiting on something that is never going to arrive.
///
/// Reconciliation clears a wait when its upstream lands, and deliberately does
/// not when the upstream failed: carrying on without it, replacing it, or
/// abandoning the dependent are all defensible and the room should not pick
/// one silently. That makes this the one case it cannot resolve alone, so it
/// is the one case worth raising.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadWait {
    /// The task that is stuck.
    pub task: TaskId,
    /// Its title, so the report reads without a lookup.
    pub title: String,
    /// What it is waiting for.
    pub blocker: TaskId,
    /// Why that will never arrive.
    pub why: String,
}

/// What the room can work out about its own condition.
///
/// Not serialized: a `Violation` names its rule with a `&'static str`, and in
/// any case this is a reading taken now rather than something to store and
/// trust later. What travels is the event it produces.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Health {
    /// Invariants the board breaks. Always a bug, never a state to work around.
    pub violations: Vec<Violation>,
    /// Work waiting on something that cannot happen.
    pub dead_waits: Vec<DeadWait>,
    /// Roles with claimable work and nobody in the room who could do it.
    ///
    /// Deliberately not "nobody free": every eligible bot being mid-turn is
    /// ordinary, momentary, and not worth waking anyone for. This is the
    /// durable version — the roster does not match the work.
    pub starved: Vec<(String, u32)>,
    /// Bots halted after repeated failures, by handle.
    pub faulted: Vec<String>,
}

impl Health {
    /// Whether the room can carry on without anyone.
    pub fn is_ok(&self) -> bool {
        self.violations.is_empty()
            && self.dead_waits.is_empty()
            && self.starved.is_empty()
            && self.faulted.is_empty()
    }

    /// A stable identity for this condition.
    ///
    /// Compared against the last one reported so a standing problem is raised
    /// once rather than on every tick. A room that repeats itself every minute
    /// trains its operator to swipe it away, and then the new thing goes with
    /// the rest.
    pub fn fingerprint(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for violation in &self.violations {
            parts.push(format!("v:{}:{}", violation.rule, violation.task));
        }
        for wait in &self.dead_waits {
            parts.push(format!("w:{}:{}", wait.task, wait.blocker));
        }
        for (role, count) in &self.starved {
            parts.push(format!("s:{role}:{count}"));
        }
        for handle in &self.faulted {
            parts.push(format!("f:{handle}"));
        }
        parts.sort();
        parts.join("|")
    }

    /// One line, for a notification.
    pub fn summary(&self) -> String {
        let mut said: Vec<String> = Vec::new();
        if !self.dead_waits.is_empty() {
            said.push(format!(
                "{} task(s) waiting on work that cannot arrive",
                self.dead_waits.len()
            ));
        }
        if !self.faulted.is_empty() {
            said.push(format!("{} bot(s) halted: {}", self.faulted.len(), self.faulted.join(", ")));
        }
        if !self.starved.is_empty() {
            let roles: Vec<String> = self
                .starved
                .iter()
                .map(|(role, count)| format!("{count} for {role}"))
                .collect();
            said.push(format!("work nobody free can take ({})", roles.join(", ")));
        }
        if !self.violations.is_empty() {
            said.push(format!("{} board inconsistency(ies)", self.violations.len()));
        }
        if said.is_empty() {
            return "The room is working normally.".into();
        }
        said.join("; ")
    }

    /// The whole picture, for a person reading it deliberately.
    pub fn report(&self) -> String {
        if self.is_ok() {
            return "The room is working normally.".into();
        }
        let mut out = String::new();
        if !self.dead_waits.is_empty() {
            out.push_str("Waiting on work that cannot arrive:\n");
            for wait in &self.dead_waits {
                out.push_str(&format!(
                    "  {} — {}\n    waits on {}, which {}\n",
                    wait.task, wait.title, wait.blocker, wait.why
                ));
            }
        }
        if !self.faulted.is_empty() {
            out.push_str(&format!("Halted bots: {}\n", self.faulted.join(", ")));
        }
        if !self.starved.is_empty() {
            out.push_str("Work nobody free can take:\n");
            for (role, count) in &self.starved {
                out.push_str(&format!("  {count} task(s) need `{role}`\n"));
            }
        }
        if !self.violations.is_empty() {
            out.push_str("Board inconsistencies:\n");
            for violation in &self.violations {
                out.push_str(&format!("  {violation}\n"));
            }
        }
        out
    }
}

impl Engine {
    /// What the room can tell about its own condition right now.
    pub fn health(&self) -> Health {
        let mut dead_waits = Vec::new();
        for task in self.tasks() {
            if task.state != TaskState::Blocked {
                continue;
            }
            for reason in &task.blocked_on {
                let orgbot_proto::task::BlockReason::AwaitingTask { task: blocker } = reason else {
                    continue;
                };
                let why = match self.task(*blocker) {
                    None => "is not in this room".to_string(),
                    Some(upstream) if upstream.state == TaskState::Submitted => continue,
                    Some(upstream) if upstream.state.is_terminal() => {
                        format!("is {} and will not be worked again", upstream.state.as_str())
                    }
                    // Still live: this wait can still end on its own.
                    Some(_) => continue,
                };
                dead_waits.push(DeadWait {
                    task: task.id,
                    title: task.title.clone(),
                    blocker: *blocker,
                    why,
                });
            }
        }

        let faulted = self
            .agents()
            .filter(|spec| self.status(spec.id) == orgbot_proto::AgentStatus::Faulted)
            .map(|spec| spec.handle.clone())
            .collect();

        Health {
            violations: self.inconsistencies(),
            dead_waits,
            starved: self.unstaffed_roles(),
            faulted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A standing problem has a stable identity, so it is raised once.
    #[test]
    fn the_same_condition_fingerprints_the_same_way() {
        let wait = DeadWait {
            task: TaskId::new(),
            title: "a".into(),
            blocker: TaskId::new(),
            why: "failed".into(),
        };
        let first = Health {
            dead_waits: vec![wait.clone()],
            faulted: vec!["kitara".into()],
            ..Health::default()
        };
        let reordered = Health {
            faulted: vec!["kitara".into()],
            dead_waits: vec![wait],
            ..Health::default()
        };
        assert_eq!(first.fingerprint(), reordered.fingerprint());
        assert_ne!(first.fingerprint(), Health::default().fingerprint());
    }

    #[test]
    fn a_healthy_room_says_so_and_raises_nothing() {
        let health = Health::default();
        assert!(health.is_ok());
        assert_eq!(health.fingerprint(), "");
        assert!(health.report().contains("working normally"));
    }

    /// The summary is what a person sees on a locked phone; it has to name the
    /// thing, not the count alone.
    #[test]
    fn the_summary_names_what_is_wrong() {
        let health = Health {
            faulted: vec!["kitara".into()],
            starved: vec![("engineer".into(), 4)],
            ..Health::default()
        };
        let summary = health.summary();
        assert!(summary.contains("kitara"), "{summary}");
        assert!(summary.contains("engineer"), "{summary}");
    }
}
