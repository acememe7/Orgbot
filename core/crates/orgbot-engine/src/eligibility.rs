//! One answer to "can this bot claim this task?", for everyone who asks.
//!
//! The board used to answer that question in two places. Advertisement — the
//! per-bot prompt, `list_tasks(claimable)`, the scheduler, the Work UI — asked
//! [`Task::readiness`], which reads the task's stored `dependency` field.
//! `claim_task` asked a different set of gates, one of which walked the raw
//! `depends_on` edges instead. The two agreed almost always, which is what
//! made the disagreement so hard to see.
//!
//! They parted company on a task whose dependency names a task that is not in
//! the room. Walking the edges, that dependency can never be satisfied, so the
//! claim is refused. Reading the stored field, the missing upstream was
//! skipped and the task read as `Satisfied` — so it was advertised as ready,
//! in every prompt, forever, and every claim of it failed. A bot would be told
//! to take work it could not have, discover it could not have it, and be told
//! the same thing again next turn.
//!
//! So there is now one function. [`Engine::evaluate_claim`] decides, and
//! everything else — advertisement, listing, scheduling, counting, and the
//! claim itself — is a caller of it. Advertising something the claim path
//! would refuse is no longer a matter of keeping two implementations in step;
//! there is only one implementation to keep.
//!
//! What this does not remove is time. A prompt is built at the start of a turn
//! and acted on minutes later, and the board moves in between — more so now
//! that bots take turns concurrently. That is not a disagreement between
//! layers, it is a stale read, and the answer to it is different: the claim
//! revalidates at the moment it applies, and a refusal says which of these
//! reasons it was, so a bot that lost a race can tell that apart from a bot
//! that was told something false.

use serde::{Deserialize, Serialize};

use orgbot_proto::graph::Readiness;
use orgbot_proto::time::Timestamp;
use orgbot_proto::{AgentId, TaskId};

/// Why a task is, or is not, claimable.
///
/// These are stable identifiers meant to be matched on — by the room's own
/// code, by tests, and by anything reading telemetry. The prose that goes with
/// one lives in [`Eligibility::detail`]; this is the part that does not change
/// when the wording does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Reason {
    /// Claimable right now.
    ReadyNow,
    /// Waiting on upstream work that exists and has not landed.
    DependencyBlocked,
    /// Waiting on a task that is not in this room, so it can never land.
    ///
    /// Distinct from [`Self::DependencyBlocked`] because the repair is
    /// different: waiting resolves itself, this one never does.
    DependencyMissing,
    /// A governance policy is holding it.
    PolicyHeld,
    /// A security finding is holding it.
    SecurityHold,
    /// A person has to decide something before it can move.
    BlockedOperator,
    /// Blocked on something the room can resolve for itself.
    BlockedInternal,
    /// Created but not yet opened for claiming.
    NotOpenYet,
    /// Someone holds it and their claim is live.
    AlreadyClaimed,
    /// Someone holds it but has stopped reporting progress.
    ///
    /// Recoverable: reconciliation returns it to the pool.
    LeaseExpired,
    /// It is in a state that means someone is working it, and nobody is.
    ///
    /// Recoverable: reconciliation returns it to the pool.
    ZombieInProgress,
    /// Replaced by another task, which carries the work now.
    Superseded,
    /// Abandoned as no longer relevant.
    Retired,
    /// Finished and delivered.
    Delivered,
    /// Terminal for some other reason — failed, cancelled, archived.
    Closed,
    /// This bot does not hold the role the work asks for.
    RoleMismatch,
    /// This bot has failed too often and is not being given work.
    BotFaulted,
    /// This bot is offline or otherwise not taking work.
    BotUnavailable,
    /// No such task in this room.
    UnknownTask,
    /// No such bot in this room.
    UnknownBot,
}

impl Reason {
    /// The stable identifier, as it appears in telemetry and tests.
    pub const fn code(self) -> &'static str {
        match self {
            Self::ReadyNow => "READY_NOW",
            Self::DependencyBlocked => "DEPENDENCY_BLOCKED",
            Self::DependencyMissing => "DEPENDENCY_MISSING",
            Self::PolicyHeld => "POLICY_HELD",
            Self::SecurityHold => "SECURITY_HOLD",
            Self::BlockedOperator => "BLOCKED_OPERATOR",
            Self::BlockedInternal => "BLOCKED_INTERNAL",
            Self::NotOpenYet => "NOT_OPEN_YET",
            Self::AlreadyClaimed => "ALREADY_CLAIMED",
            Self::LeaseExpired => "LEASE_EXPIRED",
            Self::ZombieInProgress => "ZOMBIE_IN_PROGRESS",
            Self::Superseded => "SUPERSEDED",
            Self::Retired => "RETIRED",
            Self::Delivered => "DELIVERED",
            Self::Closed => "CLOSED",
            Self::RoleMismatch => "ROLE_MISMATCH",
            Self::BotFaulted => "BOT_FAULTED",
            Self::BotUnavailable => "BOT_UNAVAILABLE",
            Self::UnknownTask => "UNKNOWN_TASK",
            Self::UnknownBot => "UNKNOWN_BOT",
        }
    }

    /// Whether the room can put this right by itself.
    ///
    /// True only for states that are wrong rather than merely inconvenient: a
    /// task nobody holds that claims to be underway, a claim whose holder has
    /// gone quiet. Waiting on a dependency is not a repair — it is the system
    /// working.
    pub const fn needs_repair(self) -> bool {
        matches!(
            self,
            Self::ZombieInProgress | Self::LeaseExpired | Self::DependencyMissing
        )
    }
}

/// What is known about a task's claim, from the one place that decides.
///
/// Serialize only: [`Readiness`] carries `&'static str` for its terminal
/// states, so this goes out to telemetry and the UI but is never read back in.
/// That is the right direction anyway — an eligibility result is a reading
/// taken at a moment, not something to store and trust later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Eligibility {
    /// Whether a claim would be accepted at the moment this was evaluated.
    pub eligible: bool,
    /// Why, in a form that can be matched on.
    pub reason: Reason,
    /// Why, in a sentence a bot or a person can read.
    pub detail: String,
    /// The derived readiness, for anything that renders the board.
    pub effective: Readiness,
    /// Upstream work this is waiting on, if any.
    pub blocking_tasks: Vec<TaskId>,
    /// Who holds it, if anyone.
    pub holder: Option<AgentId>,
    /// The state of the holder's claim.
    pub lease: LeaseState,
    /// When the holder's claim lapses.
    pub lease_expires_at: Option<Timestamp>,
    /// How many times this task has changed hands.
    ///
    /// Carried so a reader can tell "the same claim I was told about" from "a
    /// different one that happens to look similar".
    pub ownership_epoch: u64,
    /// Whether reconciliation has something to fix here.
    pub repair_required: bool,
}

impl Eligibility {
    /// The stable reason code.
    pub const fn code(&self) -> &'static str {
        self.reason.code()
    }
}

/// The state of a claim on a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LeaseState {
    /// Nobody holds the task.
    Unheld,
    /// Held, and the holder is reporting progress.
    Active,
    /// Held, but the holder has gone quiet past its expiry.
    Expired,
    /// Held by someone, with no claim recorded at all.
    ///
    /// Rooms saved before claims carried an expiry come back this way. Without
    /// this case such a task is held forever by a bot that may be long gone,
    /// because the sweep that returns abandoned work to the pool only looks at
    /// claims that have an expiry to compare against.
    Missing,
}

impl LeaseState {
    /// Whether this claim stops somebody else taking the task.
    pub const fn holds_the_task(self) -> bool {
        matches!(self, Self::Active)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The codes are a contract with tests, telemetry and the Work UI.
    #[test]
    fn every_reason_has_a_stable_screaming_snake_code() {
        for (reason, code) in [
            (Reason::ReadyNow, "READY_NOW"),
            (Reason::DependencyMissing, "DEPENDENCY_MISSING"),
            (Reason::ZombieInProgress, "ZOMBIE_IN_PROGRESS"),
            (Reason::AlreadyClaimed, "ALREADY_CLAIMED"),
            (Reason::BotFaulted, "BOT_FAULTED"),
        ] {
            assert_eq!(reason.code(), code);
            assert_eq!(
                serde_json::to_value(reason).unwrap(),
                serde_json::Value::String(code.into()),
                "the wire form and the code must not drift apart"
            );
        }
    }

    /// Only genuinely wrong states are repairable. Waiting is not a fault.
    #[test]
    fn waiting_on_work_is_not_something_to_repair() {
        assert!(!Reason::DependencyBlocked.needs_repair());
        assert!(!Reason::AlreadyClaimed.needs_repair());
        assert!(!Reason::BlockedOperator.needs_repair());
        assert!(Reason::ZombieInProgress.needs_repair());
        assert!(Reason::LeaseExpired.needs_repair());
        assert!(Reason::DependencyMissing.needs_repair());
    }

    /// Only a live claim keeps others out; the rest are reclaimable.
    #[test]
    fn only_a_live_claim_holds_a_task() {
        assert!(LeaseState::Active.holds_the_task());
        for state in [LeaseState::Unheld, LeaseState::Expired, LeaseState::Missing] {
            assert!(
                !state.holds_the_task(),
                "{state:?} must not keep a task out of the pool"
            );
        }
    }
}
