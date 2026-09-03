//! Handoffs: how one bot passes work to another.
//!
//! A handoff is a first-class object rather than a chat message, because the
//! sender frequently needs to *wait* on it. Modelling it explicitly gives the
//! engine an edge in the wait-for graph, which is what makes deadlock
//! detection possible (see `gptbots_engine::deadlock`).
//!
//! ```text
//!   Offered ──accept──> Accepted ──complete──> Completed
//!      │                    │
//!      ├──reject──> Rejected│──fail──> Failed
//!      ├──withdraw──> Withdrawn
//!      └──expire──> Expired <──expire──┘
//! ```

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, ArtifactId, HandoffId, TaskId};
use crate::time::Timestamp;

/// Who a handoff is addressed to.
///
/// Addressing by role keeps a group chat working when the specific bot that
/// would normally take the work is busy or offline: any bot holding the role
/// may accept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Recipient {
    /// One named bot, and only that bot, may accept.
    Agent { agent: AgentId },
    /// Any bot holding this role may accept; first come, first served.
    Role { role: String },
    /// A person must accept.
    Human,
}

impl Recipient {
    /// Whether `agent`, holding `roles`, is allowed to accept.
    pub fn admits(&self, agent: AgentId, roles: &[String]) -> bool {
        match self {
            Self::Agent { agent: target } => *target == agent,
            Self::Role { role } => roles.iter().any(|held| held == role),
            Self::Human => false,
        }
    }
}

/// What the sender is asking the recipient to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffKind {
    /// Transfer ownership of the task outright. The sender stops working it.
    Delegate,
    /// Ask for a sub-deliverable. The sender keeps the task and usually waits.
    Assist,
    /// Ask for inspection of finished work.
    Review,
    /// Push the decision up to a supervisor or a human.
    Escalate,
}

impl HandoffKind {
    /// Whether accepting this handoff moves the task's assignee to the
    /// recipient. Only a delegation changes ownership; an assist or a review
    /// leaves the sender holding the task.
    pub const fn transfers_ownership(self) -> bool {
        matches!(self, Self::Delegate)
    }

    /// Whether the sender is expected to stop and wait for the result.
    pub const fn blocks_sender_by_default(self) -> bool {
        matches!(self, Self::Assist | Self::Review)
    }
}

/// Where a handoff sits in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffState {
    /// Offered, awaiting a decision from the recipient.
    Offered,
    /// Taken up; the recipient is working it.
    Accepted,
    /// Terminal: the recipient delivered.
    Completed,
    /// Terminal: the recipient declined to take it.
    Rejected,
    /// Terminal: the recipient took it and could not finish.
    Failed,
    /// Terminal: the sender pulled it back.
    Withdrawn,
    /// Terminal: nobody responded inside the deadline.
    Expired,
}

impl HandoffState {
    /// Whether the handoff has reached a final state.
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Rejected | Self::Failed | Self::Withdrawn | Self::Expired
        )
    }

    /// Whether a waiting sender should still be waiting.
    pub const fn is_pending(self) -> bool {
        matches!(self, Self::Offered | Self::Accepted)
    }

    /// Whether the handoff delivered what was asked for.
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Completed)
    }

    /// Stable lowercase name, matching the serde representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Offered => "offered",
            Self::Accepted => "accepted",
            Self::Completed => "completed",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
            Self::Withdrawn => "withdrawn",
            Self::Expired => "expired",
        }
    }

    /// Apply a transition, yielding the next state.
    pub fn apply(self, transition: &HandoffTransition) -> Result<HandoffState, HandoffError> {
        use HandoffState::*;
        use HandoffTransition as T;

        let next = match (self, transition) {
            (Offered, T::Accept { .. }) => Accepted,
            (Offered, T::Reject { .. }) => Rejected,
            (Offered, T::Withdraw { .. }) => Withdrawn,
            (Accepted, T::Complete { .. }) => Completed,
            (Accepted, T::Fail { .. }) => Failed,
            // A sender may pull back work the recipient has already started;
            // the recipient learns about it through the event stream.
            (Accepted, T::Withdraw { .. }) => Withdrawn,
            (Offered | Accepted, T::Expire) => Expired,
            (from, transition) => {
                return Err(HandoffError::IllegalTransition {
                    from,
                    transition: transition.name().to_string(),
                })
            }
        };
        Ok(next)
    }
}

/// Something went wrong driving a handoff.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum HandoffError {
    #[error("cannot `{transition}` a handoff that is `{}`", from.as_str())]
    IllegalTransition {
        from: HandoffState,
        transition: String,
    },
    #[error("bot {agent} is not an eligible recipient for this handoff")]
    NotAddressedTo { agent: AgentId },
    #[error("only the sender may withdraw a handoff")]
    NotTheSender,
    #[error("a bot cannot hand off to itself")]
    SelfHandoff,
}

/// An event that moves a handoff from one state to the next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transition", rename_all = "snake_case")]
pub enum HandoffTransition {
    /// The recipient takes the work.
    Accept { by: AgentId },
    /// The recipient declines.
    Reject { by: AgentId, reason: String },
    /// The recipient delivers.
    Complete {
        by: AgentId,
        /// What was produced.
        artifacts: Vec<ArtifactId>,
        /// A short prose result the sender reads directly.
        summary: String,
    },
    /// The recipient took the work and could not finish it.
    Fail { by: AgentId, reason: String },
    /// The sender pulls the offer back.
    Withdraw { by: AgentId, reason: String },
    /// The deadline passed with no resolution.
    Expire,
}

impl HandoffTransition {
    /// A stable name for logs and error messages.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Accept { .. } => "accept",
            Self::Reject { .. } => "reject",
            Self::Complete { .. } => "complete",
            Self::Fail { .. } => "fail",
            Self::Withdraw { .. } => "withdraw",
            Self::Expire => "expire",
        }
    }

    /// The bot that drove this transition, if it was a bot rather than the clock.
    pub const fn actor(&self) -> Option<AgentId> {
        match self {
            Self::Accept { by }
            | Self::Reject { by, .. }
            | Self::Complete { by, .. }
            | Self::Fail { by, .. }
            | Self::Withdraw { by, .. } => Some(*by),
            Self::Expire => None,
        }
    }
}

/// What the recipient delivered back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffOutcome {
    pub artifacts: Vec<ArtifactId>,
    pub summary: String,
    pub at: Timestamp,
}

/// A transfer of work between bots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    pub id: HandoffId,
    /// The task this handoff is about.
    pub task: TaskId,
    pub from: AgentId,
    pub to: Recipient,
    pub kind: HandoffKind,
    pub state: HandoffState,
    /// The instruction for the recipient.
    pub brief: String,
    /// What the sender needs back before it can resume.
    pub expects: Vec<String>,
    /// Context handed across: prior artifacts the recipient should read.
    pub context: Vec<ArtifactId>,
    /// Whether the sender's task is blocked until this resolves.
    pub blocks_sender: bool,
    /// The bot that actually accepted. Set once `state` leaves `Offered`.
    pub accepted_by: Option<AgentId>,
    /// Set once the handoff completes successfully.
    pub outcome: Option<HandoffOutcome>,
    /// When the handoff expires if unresolved.
    pub deadline: Option<Timestamp>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Handoff {
    /// Build a fresh offer.
    ///
    /// `blocks_sender` defaults to the kind's convention: an assist or a review
    /// stops the sender, a delegation or an escalation does not.
    pub fn offer(
        task: TaskId,
        from: AgentId,
        to: Recipient,
        kind: HandoffKind,
        brief: impl Into<String>,
    ) -> Self {
        let now = Timestamp::now();
        Self {
            id: HandoffId::new(),
            task,
            from,
            to,
            kind,
            state: HandoffState::Offered,
            brief: brief.into(),
            expects: Vec::new(),
            context: Vec::new(),
            blocks_sender: kind.blocks_sender_by_default(),
            accepted_by: None,
            outcome: None,
            deadline: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Whether this offer has outlived its deadline as of `now`.
    pub fn is_expired_at(&self, now: Timestamp) -> bool {
        self.state.is_pending() && self.deadline.is_some_and(|d| now >= d)
    }

    /// Check that `agent` (holding `roles`) may accept, then accept.
    ///
    /// Eligibility is enforced here rather than at the call site so that every
    /// path into acceptance gets the same check.
    pub fn accept_by(
        &mut self,
        agent: AgentId,
        roles: &[String],
        now: Timestamp,
    ) -> Result<(), HandoffError> {
        if !self.to.admits(agent, roles) {
            return Err(HandoffError::NotAddressedTo { agent });
        }
        self.state = self.state.apply(&HandoffTransition::Accept { by: agent })?;
        self.accepted_by = Some(agent);
        self.updated_at = now;
        Ok(())
    }

    /// Apply any transition, keeping the derived fields in step.
    pub fn apply(
        &mut self,
        transition: &HandoffTransition,
        now: Timestamp,
    ) -> Result<(), HandoffError> {
        if let HandoffTransition::Withdraw { by, .. } = transition {
            if *by != self.from {
                return Err(HandoffError::NotTheSender);
            }
        }
        self.state = self.state.apply(transition)?;
        if let HandoffTransition::Accept { by } = transition {
            self.accepted_by = Some(*by);
        }
        if let HandoffTransition::Complete {
            artifacts, summary, ..
        } = transition
        {
            self.outcome = Some(HandoffOutcome {
                artifacts: artifacts.clone(),
                summary: summary.clone(),
                at: now,
            });
        }
        self.updated_at = now;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::HandoffState::*;
    use super::*;

    fn ids() -> (TaskId, AgentId, AgentId) {
        (TaskId::new(), AgentId::new(), AgentId::new())
    }

    #[test]
    fn offer_to_accept_to_complete() {
        let (task, sender, worker) = ids();
        let mut handoff = Handoff::offer(
            task,
            sender,
            Recipient::Agent { agent: worker },
            HandoffKind::Assist,
            "draft the schema",
        );
        assert_eq!(handoff.state, Offered);

        handoff.accept_by(worker, &[], Timestamp::now()).unwrap();
        assert_eq!(handoff.state, Accepted);
        assert_eq!(handoff.accepted_by, Some(worker));

        handoff
            .apply(
                &HandoffTransition::Complete {
                    by: worker,
                    artifacts: vec![],
                    summary: "schema in artifact 1".into(),
                },
                Timestamp::now(),
            )
            .unwrap();
        assert_eq!(handoff.state, Completed);
        assert_eq!(handoff.outcome.unwrap().summary, "schema in artifact 1");
    }

    #[test]
    fn a_bot_not_addressed_cannot_accept() {
        let (task, sender, worker) = ids();
        let intruder = AgentId::new();
        let mut handoff = Handoff::offer(
            task,
            sender,
            Recipient::Agent { agent: worker },
            HandoffKind::Assist,
            "help",
        );
        let err = handoff.accept_by(intruder, &[], Timestamp::now()).unwrap_err();
        assert_eq!(err, HandoffError::NotAddressedTo { agent: intruder });
        assert_eq!(handoff.state, Offered, "a rejected accept must not mutate state");
    }

    #[test]
    fn a_role_addressed_offer_admits_any_holder_of_the_role() {
        let (task, sender, worker) = ids();
        let mut handoff = Handoff::offer(
            task,
            sender,
            Recipient::Role { role: "reviewer".into() },
            HandoffKind::Review,
            "check this",
        );
        let wrong_role = handoff.accept_by(worker, &["writer".into()], Timestamp::now());
        assert!(wrong_role.is_err());
        handoff
            .accept_by(worker, &["writer".into(), "reviewer".into()], Timestamp::now())
            .unwrap();
        assert_eq!(handoff.state, Accepted);
    }

    #[test]
    fn a_human_recipient_admits_no_bot() {
        assert!(!Recipient::Human.admits(AgentId::new(), &["anything".into()]));
    }

    #[test]
    fn only_the_sender_may_withdraw() {
        let (task, sender, worker) = ids();
        let mut handoff = Handoff::offer(
            task,
            sender,
            Recipient::Agent { agent: worker },
            HandoffKind::Delegate,
            "take it",
        );
        let err = handoff
            .apply(
                &HandoffTransition::Withdraw {
                    by: worker,
                    reason: "mine now".into(),
                },
                Timestamp::now(),
            )
            .unwrap_err();
        assert_eq!(err, HandoffError::NotTheSender);
        assert_eq!(handoff.state, Offered);
    }

    #[test]
    fn a_sender_may_withdraw_work_already_in_flight() {
        let (task, sender, worker) = ids();
        let mut handoff = Handoff::offer(
            task,
            sender,
            Recipient::Agent { agent: worker },
            HandoffKind::Assist,
            "help",
        );
        handoff.accept_by(worker, &[], Timestamp::now()).unwrap();
        handoff
            .apply(
                &HandoffTransition::Withdraw {
                    by: sender,
                    reason: "no longer needed".into(),
                },
                Timestamp::now(),
            )
            .unwrap();
        assert_eq!(handoff.state, Withdrawn);
    }

    #[test]
    fn terminal_handoffs_reject_every_transition() {
        let worker = AgentId::new();
        let every = [
            HandoffTransition::Accept { by: worker },
            HandoffTransition::Reject { by: worker, reason: String::new() },
            HandoffTransition::Complete {
                by: worker,
                artifacts: vec![],
                summary: String::new(),
            },
            HandoffTransition::Fail { by: worker, reason: String::new() },
            HandoffTransition::Withdraw { by: worker, reason: String::new() },
            HandoffTransition::Expire,
        ];
        for terminal in [Completed, Rejected, Failed, Withdrawn, Expired] {
            for transition in &every {
                assert!(
                    terminal.apply(transition).is_err(),
                    "{terminal:?} should reject {}",
                    transition.name()
                );
            }
        }
    }

    #[test]
    fn completion_requires_acceptance_first() {
        let worker = AgentId::new();
        assert!(Offered
            .apply(&HandoffTransition::Complete {
                by: worker,
                artifacts: vec![],
                summary: String::new(),
            })
            .is_err());
    }

    #[test]
    fn expiry_is_only_reported_past_the_deadline_and_only_while_pending() {
        let (task, sender, worker) = ids();
        let mut handoff = Handoff::offer(
            task,
            sender,
            Recipient::Agent { agent: worker },
            HandoffKind::Assist,
            "help",
        );
        handoff.deadline = Some(Timestamp(1_000));
        assert!(!handoff.is_expired_at(Timestamp(999)));
        assert!(handoff.is_expired_at(Timestamp(1_000)));

        handoff.state = Completed;
        assert!(
            !handoff.is_expired_at(Timestamp(5_000)),
            "a finished handoff never expires"
        );
    }

    #[test]
    fn kind_conventions_match_the_docs() {
        assert!(HandoffKind::Delegate.transfers_ownership());
        assert!(!HandoffKind::Assist.transfers_ownership());
        assert!(HandoffKind::Assist.blocks_sender_by_default());
        assert!(HandoffKind::Review.blocks_sender_by_default());
        assert!(!HandoffKind::Delegate.blocks_sender_by_default());
    }

    #[test]
    fn a_new_offer_inherits_blocking_from_its_kind() {
        let (task, sender, worker) = ids();
        let to = Recipient::Agent { agent: worker };
        assert!(Handoff::offer(task, sender, to.clone(), HandoffKind::Assist, "x").blocks_sender);
        assert!(!Handoff::offer(task, sender, to, HandoffKind::Delegate, "x").blocks_sender);
    }
}
