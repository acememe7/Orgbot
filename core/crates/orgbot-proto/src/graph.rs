//! The work graph: what a task *is*, what it descends from, and whether it is
//! genuinely ready.
//!
//! A room's board used to carry one axis of truth — [`TaskState`] — and one
//! meaning of "ready": an unclaimed record exists. A sixteen-bot run showed
//! what that costs. Reviews, remediations and evidence gathering each became
//! another top-level card, eleven future graph nodes all advertised themselves
//! as ready at once, superseded work stayed claimable, and every internal gap
//! reached the operator as though only a human could clear it.
//!
//! So readiness is not stored here. It is *derived*, every time, from three
//! independent dimensions that must not be collapsed into one badge:
//!
//! - **workflow** — where the task sits in its own lifecycle ([`TaskState`])
//! - **dependency** — whether the work it needs has actually landed
//!   ([`DependencyState`])
//! - **authorization** — whether it is permitted to run at all
//!   ([`AuthorizationState`])
//!
//! A task authorized by the operator may still be waiting on a dependency; a
//! dependency-free task may still be held by security. Neither is "blocked" in
//! any sense a person should be interrupted for. [`Task::readiness`] resolves
//! all three in the order the scheduler must respect and returns one verdict
//! that says both what is happening and, crucially, whether a human is needed.

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, TaskId};

/// What kind of work record this is.
///
/// The distinction exists so the board can stop treating a review cycle as a
/// peer of the objective it reviews. Only [`TaskKind::Objective`] is a
/// top-level outcome; everything else hangs off one and is folded away until
/// someone asks for detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// An independently owned outcome. The unit a person cares about.
    Objective,
    /// Ordinary work under an objective. The default, so rooms written before
    /// kinds existed keep the meaning they had.
    #[default]
    Task,
    /// A piece of a task, split for parallelism or clarity.
    Subtask,
    /// An inspection of another task's work.
    Review,
    /// Work to address what a review found.
    Remediation,
    /// Gathering proof that something holds. Not itself a deliverable.
    Evidence,
    /// Restoring something the room produced and then lost or truncated.
    Recovery,
    /// Continuing work that was cut short.
    Continuation,
}

impl TaskKind {
    /// Whether this belongs on the board as a top-level card.
    ///
    /// Everything else is coordination detail: real work, tracked and audited,
    /// but not an outcome in its own right. This is the single check that
    /// keeps ninety-six records reading as twelve objectives.
    pub const fn is_headline(self) -> bool {
        matches!(self, Self::Objective)
    }

    /// Whether this record exists to support another task rather than to
    /// deliver something on its own.
    pub const fn is_supporting(self) -> bool {
        matches!(
            self,
            Self::Review | Self::Remediation | Self::Evidence | Self::Recovery | Self::Continuation
        )
    }

    /// Stable lowercase name, matching the serde representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Objective => "objective",
            Self::Task => "task",
            Self::Subtask => "subtask",
            Self::Review => "review",
            Self::Remediation => "remediation",
            Self::Evidence => "evidence",
            Self::Recovery => "recovery",
            Self::Continuation => "continuation",
        }
    }
}

/// Where a task came from, and what it replaces.
///
/// Several records commonly describe one real objective — the implementation,
/// the reviews of it, the remediation of those reviews. Without durable links
/// between them the board cannot tell a second review cycle from a second
/// objective, and a person watching sees the work multiply as it progresses.
///
/// Every field defaults to `None`, so a room written before this existed loads
/// with its relationships honestly unknown rather than guessed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Lineage {
    /// The objective this ultimately serves. Progress is counted against
    /// these, not against raw records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_objective: Option<TaskId>,
    /// The record this hangs directly beneath.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<TaskId>,
    /// The task this one reviews.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_of: Option<TaskId>,
    /// The task whose review findings this addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation_of: Option<TaskId>,
    /// The task whose lost or truncated output this restores.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_for: Option<TaskId>,
    /// The task this carries on from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_of: Option<TaskId>,
    /// The task this one replaces. Set on the replacement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<TaskId>,
    /// The task that replaced this one. Set on the original, which then
    /// becomes permanently non-actionable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<TaskId>,
}

impl Lineage {
    /// The task this record is *about*, when it is about another one.
    ///
    /// Reviews, remediations, recoveries and continuations all attach to a
    /// subject; this collapses them to one question so callers do not have to
    /// check four fields in the right order.
    pub const fn subject(&self) -> Option<TaskId> {
        if let Some(task) = self.review_of {
            Some(task)
        } else if let Some(task) = self.remediation_of {
            Some(task)
        } else if let Some(task) = self.recovery_for {
            Some(task)
        } else if let Some(task) = self.continuation_of {
            Some(task)
        } else {
            None
        }
    }

    /// Whether this record has been replaced.
    pub const fn is_superseded(&self) -> bool {
        self.superseded_by.is_some()
    }
}

/// What a task is and where it sits, supplied when it is created.
///
/// Bundled rather than passed as loose arguments because the two travel
/// together: a review with no `review_of` is just another top-level card, which
/// is the thing this whole model exists to stop.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TaskOrigin {
    /// What kind of record this is.
    #[serde(default)]
    pub kind: TaskKind,
    /// What it descends from and what it is about.
    #[serde(default)]
    pub lineage: Lineage,
}

impl TaskOrigin {
    /// An ordinary top-level task, as everything was before kinds existed.
    pub fn plain() -> Self {
        Self::default()
    }

    /// A record of `kind` hanging beneath `parent`.
    pub fn under(kind: TaskKind, parent: TaskId) -> Self {
        Self {
            kind,
            lineage: Lineage {
                parent: Some(parent),
                ..Default::default()
            },
        }
    }

    /// A review of `task`.
    pub fn review_of(task: TaskId) -> Self {
        Self {
            kind: TaskKind::Review,
            lineage: Lineage {
                parent: Some(task),
                review_of: Some(task),
                ..Default::default()
            },
        }
    }

    /// Work addressing what a review of `task` found.
    pub fn remediation_of(task: TaskId) -> Self {
        Self {
            kind: TaskKind::Remediation,
            lineage: Lineage {
                parent: Some(task),
                remediation_of: Some(task),
                ..Default::default()
            },
        }
    }
}

/// Whether the work a task needs has actually landed.
///
/// Distinct from the workflow state because a task can be perfectly healthy
/// and simply not its turn yet. Eleven such tasks previously all read as
/// "ready" at once, which told a person nothing about what to expect next.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DependencyState {
    /// Nothing outstanding.
    #[default]
    Satisfied,
    /// Waiting on something that would help but does not strictly block.
    ///
    /// Work can start; it may have to be revisited when the upstream lands.
    /// Distinguished from a hard wait so a room with spare capacity can make
    /// a start rather than sitting still, and so the board can say which kind
    /// of waiting it is.
    Conditional {
        /// What would help.
        on: TaskId,
        /// Why it would help.
        reason: String,
    },
    /// Waiting on another task, named so the board can say what and who.
    Waiting {
        /// The task being waited on. One upstream blocker is referenced by
        /// every dependent rather than copied into each of them.
        on: TaskId,
        /// Why, in words a person can read.
        reason: String,
        /// Who owns clearing it, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<AgentId>,
    },
}

impl DependencyState {
    /// The task being waited on, if any.
    pub const fn waiting_on(&self) -> Option<TaskId> {
        match self {
            Self::Satisfied => None,
            Self::Conditional { on, .. } | Self::Waiting { on, .. } => Some(*on),
        }
    }

    /// Whether every dependency has landed.
    pub const fn is_satisfied(&self) -> bool {
        matches!(self, Self::Satisfied)
    }

    /// Whether work may start despite what is outstanding.
    pub const fn permits_a_start(&self) -> bool {
        matches!(self, Self::Satisfied | Self::Conditional { .. })
    }
}

/// Whether a task is permitted to proceed.
///
/// Separate from both workflow and dependency state, because approval,
/// security clearance and engineering progress move independently: an
/// operator may approve a whole class of action while security still holds one
/// artifact, and neither says anything about whether the upstream work is done.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AuthorizationState {
    /// Permitted, either by standing policy or by an explicit decision.
    #[default]
    Authorized,
    /// A standing room policy holds this task. Internal: the room resolves it.
    HeldByPolicy {
        /// Which policy, so the hold can be traced to a rule.
        policy: String,
        /// What would release it.
        reason: String,
    },
    /// Security is holding the work pending evidence. Internal by default:
    /// missing proof is work for the room, not a question for a person.
    SecurityHold {
        /// What is missing or at issue.
        finding: String,
        /// Whether this is severe enough that a person must accept the risk.
        #[serde(default)]
        material: bool,
    },
    /// Genuinely waiting on a human decision.
    ///
    /// The justification is not optional. Every escalation must name a
    /// decision only a person can make — new spend, destructive action,
    /// privilege expansion, production release, accepted risk. If that
    /// sentence cannot be written honestly, the task is not an operator block.
    AwaitingOperator {
        /// Completes "Operator decision required because: ___".
        because: String,
        /// The options offered, for a one-tap answer.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        options: Vec<String>,
    },
}

impl AuthorizationState {
    /// Whether a person must act before this task can proceed.
    ///
    /// A security hold counts only when it is material; ordinary missing
    /// evidence is work the room does for itself.
    pub const fn needs_operator(&self) -> bool {
        match self {
            Self::AwaitingOperator { .. } => true,
            Self::SecurityHold { material, .. } => *material,
            Self::Authorized | Self::HeldByPolicy { .. } => false,
        }
    }

    /// Whether the task may run as far as authorization is concerned.
    pub const fn is_authorized(&self) -> bool {
        matches!(self, Self::Authorized)
    }
}

/// Why an operator escalation was rejected as not genuinely human-only.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
pub enum EscalationError {
    /// The justification was blank or whitespace.
    #[error("an operator block must say why only a person can decide it")]
    NoJustification,
    /// The justification did not describe a decision, only a missing thing.
    #[error(
        "`{0}` describes work the room can do, not a decision only a person \
         can make; keep the task internal"
    )]
    NotHumanOnly(String),
}

/// Phrases that describe internal gaps rather than human decisions.
///
/// Drawn from the escalations a real run produced: every one of these was the
/// room asking a person for something the room itself had lost or not yet
/// done. Checked as whole words so "missing" catches the excuse without
/// catching "commissioning".
const INTERNAL_TELLS: &[&str] = &[
    "missing",
    "truncated",
    "incomplete",
    "regenerate",
    "re-emit",
    "reemit",
    "resend",
    "evidence",
    "rerun",
    "retry",
    "unavailable",
    "not found",
];

/// Check that an operator escalation names a decision only a person can make.
///
/// This is deliberately conservative: it rejects the shapes that went wrong in
/// practice and lets everything else through. It cannot judge intent, so it is
/// a guard rail on the obvious failure, not a proof of good faith.
pub fn validate_escalation(because: &str) -> Result<(), EscalationError> {
    let trimmed = because.trim();
    if trimmed.is_empty() {
        return Err(EscalationError::NoJustification);
    }
    let lowered = trimmed.to_lowercase();
    for tell in INTERNAL_TELLS {
        if contains_word(&lowered, tell) {
            return Err(EscalationError::NotHumanOnly(trimmed.to_string()));
        }
    }
    Ok(())
}

/// Whether `needle` appears in `haystack` bounded by non-alphanumeric edges.
///
/// Substring matching would flag "commissioning" for containing "missing",
/// which would push real decisions back into the room.
fn contains_word(haystack: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(found) = haystack[from..].find(needle) {
        let start = from + found;
        let end = start + needle.len();
        let before_ok = start == 0
            || !haystack[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
        let after_ok = end == haystack.len()
            || !haystack[end..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
        if from >= haystack.len() {
            break;
        }
    }
    false
}

/// One verdict on whether a task can be worked right now, and if not, why.
///
/// Replaces the question "is this record unclaimed?" with the question the
/// scheduler and the board both actually need answered. Only
/// [`Readiness::ReadyNow`] may be claimed, and only
/// [`Readiness::BlockedOperator`] may interrupt a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "readiness", rename_all = "snake_case")]
pub enum Readiness {
    /// Every gate is satisfied. Claimable.
    ReadyNow,
    /// Claimable, but something outstanding may mean revisiting it.
    ///
    /// Kept apart from `ReadyNow` so a coordinator with a choice takes the
    /// unconditional work first, and so a person reading the board can tell
    /// which starts are provisional.
    ConditionallyReady {
        /// What is still outstanding.
        on: TaskId,
        /// Why it may need revisiting.
        reason: String,
    },
    /// Still a draft; not offered to bots yet.
    NotYetOpen,
    /// Someone already holds it.
    Underway {
        /// The bot holding it.
        by: AgentId,
    },
    /// Waiting on upstream work. The room continues without anyone's help.
    WaitingDependency {
        /// What it waits on.
        on: TaskId,
        /// Why, in readable words.
        reason: String,
        /// Who owns clearing it, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<AgentId>,
    },
    /// Sent back from review and being fixed.
    ///
    /// Its own reading because it is the most common thing a busy room is
    /// doing, and "in progress" hides whether work is advancing or going back
    /// over ground.
    ReviewRemediation {
        /// Which round of rework this is.
        round: u32,
    },
    /// Stopped on something the room resolves itself — remediation, evidence,
    /// recovery, a retry. Explicitly not a reason to notify anyone.
    BlockedInternal {
        /// What is being resolved.
        reason: String,
    },
    /// Held by a standing room policy.
    HeldByPolicy {
        /// Which rule.
        policy: String,
        /// What would release it.
        reason: String,
    },
    /// Genuinely waiting on a person.
    BlockedOperator {
        /// Completes "Operator decision required because: ___".
        because: String,
        /// Offered answers, for a one-tap decision.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        options: Vec<String>,
    },
    /// Finished, abandoned, replaced or retired. Never actionable.
    Closed {
        /// The terminal state, for display.
        state: &'static str,
    },
}

impl Readiness {
    /// Whether a bot may claim this task now.
    pub const fn is_claimable(&self) -> bool {
        matches!(self, Self::ReadyNow | Self::ConditionallyReady { .. })
    }

    /// Whether a person has to do something.
    ///
    /// This is the only thing that should ever put an item in front of the
    /// operator, and the only thing "Needs you" counts.
    pub const fn needs_operator(&self) -> bool {
        matches!(self, Self::BlockedOperator { .. })
    }

    /// Whether the room is still moving this forward on its own.
    pub const fn is_progressing(&self) -> bool {
        matches!(
            self,
            Self::ReadyNow
                | Self::ConditionallyReady { .. }
                | Self::Underway { .. }
                | Self::ReviewRemediation { .. }
                | Self::WaitingDependency { .. }
                | Self::BlockedInternal { .. }
        )
    }

    /// A short label for the board.
    pub const fn label(&self) -> &'static str {
        match self {
            Self::ReadyNow => "Ready",
            Self::ConditionallyReady { .. } => "Ready (provisional)",
            Self::ReviewRemediation { .. } => "Rework",
            Self::NotYetOpen => "Draft",
            Self::Underway { .. } => "In progress",
            Self::WaitingDependency { .. } => "Waiting",
            Self::BlockedInternal { .. } => "Internal remediation",
            Self::HeldByPolicy { .. } => "Held by policy",
            Self::BlockedOperator { .. } => "Needs you",
            Self::Closed { .. } => "Closed",
        }
    }

    /// One line a person can read without knowing the schema.
    ///
    /// The board used to show `awaiting_task`, which told nobody whether they
    /// had to do anything. Every line here either names a decision or says, in
    /// so many words, that the room has it covered.
    pub fn summary(&self) -> String {
        match self {
            Self::ReadyNow => "Ready to start. No action needed from you.".into(),
            Self::ConditionallyReady { reason, .. } => format!(
                "Can start now, though {reason} may mean revisiting it. \
                 No action needed from you."
            ),
            Self::ReviewRemediation { round } => format!(
                "Being reworked after review (round {round}). \
                 No action needed from you."
            ),
            Self::NotYetOpen => "Still a draft. No action needed from you.".into(),
            Self::Underway { .. } => "Being worked on. No action needed from you.".into(),
            Self::WaitingDependency { reason, .. } => {
                format!("Waiting on {reason}. No action needed from you.")
            }
            Self::BlockedInternal { reason } => {
                format!("The room is resolving this: {reason}. No action needed from you.")
            }
            Self::HeldByPolicy { policy, reason } => {
                format!("Held by the `{policy}` rule: {reason}. No action needed from you.")
            }
            Self::BlockedOperator { because, .. } => {
                format!("Operator decision required because: {because}")
            }
            Self::Closed { state } => format!("Closed ({state})."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_objective_is_a_headline() {
        assert!(TaskKind::Objective.is_headline());
        for kind in [
            TaskKind::Task,
            TaskKind::Subtask,
            TaskKind::Review,
            TaskKind::Remediation,
            TaskKind::Evidence,
            TaskKind::Recovery,
            TaskKind::Continuation,
        ] {
            assert!(!kind.is_headline(), "{kind:?} must not be a top-level card");
        }
    }

    /// A room written before kinds existed must keep the meaning it had.
    #[test]
    fn an_unlabelled_record_is_an_ordinary_task() {
        assert_eq!(TaskKind::default(), TaskKind::Task);
        let decoded: TaskKind = serde_json::from_str(r#""review""#).unwrap();
        assert_eq!(decoded, TaskKind::Review);
    }

    #[test]
    fn lineage_collapses_the_four_subject_links_to_one_question() {
        let subject = TaskId::new();
        let review = Lineage {
            review_of: Some(subject),
            ..Default::default()
        };
        assert_eq!(review.subject(), Some(subject));

        let recovery = Lineage {
            recovery_for: Some(subject),
            ..Default::default()
        };
        assert_eq!(recovery.subject(), Some(subject));

        assert_eq!(Lineage::default().subject(), None);
    }

    /// Relationships that were never recorded stay unknown rather than guessed.
    #[test]
    fn a_legacy_record_decodes_with_empty_lineage() {
        let lineage: Lineage = serde_json::from_str("{}").unwrap();
        assert_eq!(lineage, Lineage::default());
        assert!(!lineage.is_superseded());
        // Absent links are omitted rather than written as nulls, so a room
        // file stays readable.
        assert_eq!(serde_json::to_string(&lineage).unwrap(), "{}");
    }

    #[test]
    fn missing_evidence_is_not_a_reason_to_interrupt_anyone() {
        let hold = AuthorizationState::SecurityHold {
            finding: "need merged-filesystem classification".into(),
            material: false,
        };
        assert!(!hold.needs_operator());

        let incident = AuthorizationState::SecurityHold {
            finding: "credentials were exposed".into(),
            material: true,
        };
        assert!(incident.needs_operator());
    }

    #[test]
    fn a_policy_hold_is_the_rooms_problem_not_a_persons() {
        let held = AuthorizationState::HeldByPolicy {
            policy: "no-new-machines".into(),
            reason: "waiting on the confined-docker check".into(),
        };
        assert!(!held.needs_operator());
        assert!(!held.is_authorized());
    }

    #[test]
    fn a_real_decision_passes_the_escalation_check() {
        for because in [
            "deploying to production needs your sign-off",
            "this provisions a paid VPS",
            "it deletes the customer database",
            "it expands the bot's privileges beyond the approved envelope",
            "accepting the residual risk is your call",
        ] {
            assert!(
                validate_escalation(because).is_ok(),
                "{because:?} should be allowed through"
            );
        }
    }

    /// Every one of these came from a run where the room asked a person for
    /// something the room itself had lost or simply had not done yet.
    #[test]
    fn an_internal_gap_is_refused_as_an_operator_block() {
        for because in [
            "the B3-C source is missing",
            "the artifact was truncated",
            "we need to regenerate the manifest",
            "please re-emit the file",
            "evidence is required for the security review",
            "the test needs a rerun",
        ] {
            assert!(
                matches!(
                    validate_escalation(because),
                    Err(EscalationError::NotHumanOnly(_))
                ),
                "{because:?} should have been kept internal"
            );
        }
    }

    #[test]
    fn an_empty_justification_is_refused() {
        assert_eq!(
            validate_escalation("   "),
            Err(EscalationError::NoJustification)
        );
    }

    /// Substring matching would read "commissioning" as "missing" and push a
    /// real decision back into the room.
    #[test]
    fn a_word_inside_another_word_is_not_a_tell() {
        assert!(validate_escalation("commissioning new hardware is your call").is_ok());
        assert!(validate_escalation("the retry budget policy is yours to set").is_err());
    }

    #[test]
    fn only_an_operator_block_asks_for_a_person() {
        let waiting = Readiness::WaitingDependency {
            on: TaskId::new(),
            reason: "source recovery".into(),
            owner: None,
        };
        assert!(!waiting.needs_operator());
        assert!(waiting.is_progressing());

        let internal = Readiness::BlockedInternal {
            reason: "review remediation".into(),
        };
        assert!(!internal.needs_operator());
        assert!(internal.is_progressing());

        let operator = Readiness::BlockedOperator {
            because: "it costs money".into(),
            options: vec!["Approve".into(), "Decline".into()],
        };
        assert!(operator.needs_operator());
        assert!(!operator.is_progressing());
    }

    #[test]
    fn only_ready_now_can_be_claimed() {
        assert!(Readiness::ReadyNow.is_claimable());
        for readiness in [
            Readiness::NotYetOpen,
            Readiness::Underway { by: AgentId::new() },
            Readiness::BlockedInternal {
                reason: "x".into(),
            },
            Readiness::Closed { state: "superseded" },
        ] {
            assert!(!readiness.is_claimable(), "{readiness:?} must not be claimable");
        }
    }

    /// The board said `awaiting_task`, which told nobody whether they had to
    /// act. Every non-operator line now says so outright.
    #[test]
    fn every_line_says_whether_a_person_is_needed() {
        let quiet = [
            Readiness::ReadyNow,
            Readiness::NotYetOpen,
            Readiness::Underway { by: AgentId::new() },
            Readiness::WaitingDependency {
                on: TaskId::new(),
                reason: "source recovery".into(),
                owner: None,
            },
            Readiness::BlockedInternal {
                reason: "review remediation".into(),
            },
            Readiness::HeldByPolicy {
                policy: "p".into(),
                reason: "r".into(),
            },
        ];
        for readiness in quiet {
            assert!(
                readiness.summary().contains("No action needed from you"),
                "{readiness:?} should reassure: {}",
                readiness.summary()
            );
        }

        let loud = Readiness::BlockedOperator {
            because: "it deletes production data".into(),
            options: Vec::new(),
        };
        assert_eq!(
            loud.summary(),
            "Operator decision required because: it deletes production data"
        );
    }
}
