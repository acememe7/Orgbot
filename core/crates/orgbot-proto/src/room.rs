//! Rooms: a group chat, its roster, and the goal it exists to deliver.

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, RoomId};
use crate::time::Timestamp;

/// How a room decides it is finished.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CompletionRule {
    /// Every task on the board has reached a terminal state.
    AllTasksTerminal,
    /// One designated task has been submitted. Used when a coordinator owns a
    /// single top-level deliverable and the rest are subtasks.
    RootTaskSubmitted,
    /// Only a human closes the room.
    Manual,
}

/// Where a room sits in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomPhase {
    /// Roster assembled, no work started.
    Assembling,
    /// Bots are working.
    Running,
    /// Halted by a human; state is preserved.
    Paused,
    /// Every task settled and the completion rule is satisfied.
    Completed,
    /// Stopped early.
    Aborted,
}

impl RoomPhase {
    /// Whether bots should be taking turns.
    pub const fn is_live(self) -> bool {
        matches!(self, Self::Running)
    }

    /// Whether the current execution has ended.
    pub const fn is_finished(self) -> bool {
        matches!(self, Self::Completed | Self::Aborted)
    }
}

/// Safety rails on a whole room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoomLimits {
    /// Total model turns across all bots before the room pauses for a human.
    pub max_total_turns: u32,
    /// Wall-clock ceiling in milliseconds. Zero means no limit.
    pub max_duration_ms: i64,
    /// How long a handoff waits before expiring: an offer for its answer, an
    /// accepted handoff for its result. Time its bot spends in a turn that
    /// began before the offer, or in the turn that took the handoff on, is
    /// not counted.
    pub handoff_timeout_ms: i64,
    /// How many consecutive failed turns fault a bot.
    pub max_consecutive_errors: u32,
    /// How many bots may be taking a turn at the same moment.
    ///
    /// A turn is mostly waiting on a model, so this is what decides how much
    /// of the roster is working rather than sitting idle: at one, a room of
    /// twenty bots has nineteen doing nothing at every instant, and adding
    /// bots cannot make it finish sooner.
    ///
    /// Raising it does not weaken the board. Every command is checked against
    /// the board as it is when it applies, so the worst a race produces is a
    /// bot told the task it wanted has been taken. What it does spend is rate
    /// limit: these are concurrent calls to the model providers.
    ///
    /// Zero or one both mean one at a time.
    pub max_parallel_turns: u32,
    /// How many of those turns may be going to the same backend at once.
    ///
    /// Parallel turns are parallel requests, and bots sharing a provider share
    /// its rate limit, so a room whose roster sits mostly on one account can
    /// turn its concurrency straight into 429s: the turns are spent, the work
    /// is not done, and the room is slower than if it had waited.
    ///
    /// **Off by default**, because that risk is a guess and the cost is not. A
    /// real room of twenty-one bots had fifteen of them on one account; a flat
    /// cap of two would have held the bulk of that roster to two turns at a
    /// time and quietly undone the concurrency it was given — while that room
    /// had been running four at a time all day without being rate limited
    /// once. Guessing a limit tighter than the evidence is how a safety rail
    /// becomes the thing that slows you down.
    ///
    /// So it is the knob to reach for when rate limiting actually shows up,
    /// not a toll charged in advance. Zero means no separate cap —
    /// [`Self::max_parallel_turns`] alone.
    pub max_parallel_per_backend: u32,
}

/// Rooms saved before this was settable still ran one turn at a time.
///
/// They are reopened faster rather than left slow — the whole reason the
/// setting exists — but modestly, because it is a change made without asking.
fn default_parallel_turns() -> u32 {
    3
}

impl Default for RoomLimits {
    fn default() -> Self {
        Self {
            max_total_turns: 500,
            max_duration_ms: 60 * 60 * 1000,
            handoff_timeout_ms: 5 * 60 * 1000,
            max_consecutive_errors: 3,
            max_parallel_turns: default_parallel_turns(),
            max_parallel_per_backend: 0,
        }
    }
}

impl RoomLimits {
    /// How many turns may go to one backend at once, as a usable count.
    pub const fn parallel_per_backend(&self) -> usize {
        if self.max_parallel_per_backend == 0 {
            usize::MAX
        } else {
            self.max_parallel_per_backend as usize
        }
    }

    /// How many bots may act at once, as a usable count.
    ///
    /// Zero is treated as one rather than as "no limit": unlike a budget, an
    /// unbounded fan-out is never what someone means, and it would put the
    /// whole roster on the providers at once.
    pub const fn parallel_turns(&self) -> usize {
        if self.max_parallel_turns == 0 {
            1
        } else {
            self.max_parallel_turns as usize
        }
    }
}

/// What a room is for, supplied when it is created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomSpec {
    /// A short name for the room.
    pub name: String,
    /// The outcome the room exists to produce, in prose. Every bot sees this.
    pub goal: String,
    /// Bots to seat, in the order they should be introduced.
    pub roster: Vec<AgentId>,
    pub completion: CompletionRule,
    pub limits: RoomLimits,
    /// What bots may use of the owner's, and how much of it needs asking.
    #[serde(default)]
    pub access: RoomAccess,
}

/// How much of what bots ask to use the owner approves by hand.
///
/// Requests are rated [`Risk`]; each mode approves some ratings on its own and
/// brings the rest to the owner. Questions about the work itself (`ask_owner`)
/// always reach the owner, in every mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    /// Every request is asked.
    #[default]
    Approval,
    /// Regular requests are approved on their own; risky and critical ones are
    /// asked.
    HalfApproval,
    /// Everything is approved on its own except critical requests.
    Auto,
    /// Every request is approved on its own. Bots still write only inside their
    /// own copy of the project.
    FullWrite,
}

impl AccessMode {
    /// Whether a request rated `risk` is approved without asking.
    pub const fn approves(self, risk: Risk) -> bool {
        match self {
            Self::Approval => false,
            Self::HalfApproval => matches!(risk, Risk::Regular),
            Self::Auto => !matches!(risk, Risk::Critical),
            Self::FullWrite => true,
        }
    }

    /// The name the owner sees for this mode.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Approval => "Approval",
            Self::HalfApproval => "Half approval",
            Self::Auto => "Auto",
            Self::FullWrite => "Full write",
        }
    }
}

/// How much harm a request could do if it went wrong.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Reading, building, testing, or changes that are easy to undo.
    Regular,
    /// Changes to live services or shared accounts.
    #[default]
    Risky,
    /// Deleting data, spending money, publishing, or changing credentials.
    Critical,
}

impl Risk {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Regular => "regular",
            Self::Risky => "risky",
            Self::Critical => "critical",
        }
    }
}

/// One of the owner's sign-ins they are willing to lend a bot for a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LendItem {
    /// The short name bots ask for, such as `wrangler`.
    pub name: String,
    /// What the owner calls it, such as "Cloudflare (Wrangler)".
    #[serde(default)]
    pub label: String,
    /// Folders or files under the owner's home that hold the sign-in, such as
    /// `.config/.wrangler`. Hidden from every bot unless lent.
    pub paths: Vec<String>,
    /// How much harm using it could do.
    #[serde(default)]
    pub risk: Risk,
}

/// What a room's bots may use of the owner's, and how much needs asking.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoomAccess {
    pub mode: AccessMode,
    /// Sign-ins the owner lends, each only for a task it is approved for.
    pub lend: Vec<LendItem>,
}

impl RoomAccess {
    pub fn item(&self, name: &str) -> Option<&LendItem> {
        self.lend.iter().find(|item| item.name == name)
    }

    /// Every path the owner lends, lent or not: hidden unless granted.
    pub fn all_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self
            .lend
            .iter()
            .flat_map(|item| item.paths.iter().cloned())
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }
}

/// A live group chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Room {
    pub id: RoomId,
    pub spec: RoomSpec,
    pub phase: RoomPhase,
    /// Model turns consumed so far, against `limits.max_total_turns`.
    pub turns_used: u32,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Room {
    /// Open a new room in the assembling phase.
    pub fn new(spec: RoomSpec) -> Self {
        let now = Timestamp::now();
        Self {
            id: RoomId::new(),
            spec,
            phase: RoomPhase::Assembling,
            turns_used: 0,
            created_at: now,
            updated_at: now,
        }
    }

    /// Whether the room has spent its turn budget.
    pub const fn is_out_of_turns(&self) -> bool {
        // Zero means no ceiling, matching `max_duration_ms`. Without this the
        // two limits disagree: a room asking for no turn limit would be out of
        // budget before it took its first turn.
        let limit = self.spec.limits.max_total_turns;
        limit > 0 && self.turns_used >= limit
    }

    /// Whether the room has run past its wall-clock ceiling as of `now`.
    pub fn is_out_of_time(&self, now: Timestamp) -> bool {
        let limit = self.spec.limits.max_duration_ms;
        limit > 0 && self.created_at.elapsed_to(now) >= limit
    }

    /// Whether a bot may take a turn right now.
    pub fn may_take_turn(&self, now: Timestamp) -> bool {
        self.phase.is_live() && !self.is_out_of_turns() && !self.is_out_of_time(now)
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    fn room(max_turns: u32) -> Room {
        let mut room = Room::new(RoomSpec {
            name: "budget".into(),
            goal: "spend carefully".into(),
            roster: vec![],
            completion: CompletionRule::AllTasksTerminal,
            limits: RoomLimits {
                max_total_turns: max_turns,
                ..Default::default()
            },
            access: Default::default(),
        });
        room.phase = RoomPhase::Running;
        room
    }

    #[test]
    fn a_budget_stops_the_room_when_it_is_spent() {
        let mut room = room(3);
        assert!(!room.is_out_of_turns());
        room.turns_used = 3;
        assert!(room.is_out_of_turns());
    }

    /// Zero means no ceiling, as it does for the time limit. Read the other
    /// way it means "out of budget before the first turn", which is how a
    /// room would refuse to do anything at all.
    #[test]
    fn no_turn_limit_means_no_limit_not_no_turns() {
        let mut room = room(0);
        assert!(!room.is_out_of_turns());
        room.turns_used = 100_000;
        assert!(!room.is_out_of_turns());
        assert!(room.may_take_turn(Timestamp::now()));
    }

    /// The two limits should agree about what zero means.
    #[test]
    fn the_two_ceilings_read_zero_the_same_way() {
        let room = room(0);
        assert!(!room.is_out_of_turns());
        assert!(!room.is_out_of_time(Timestamp::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room() -> Room {
        Room::new(RoomSpec {
            name: "launch".into(),
            goal: "ship the release notes".into(),
            roster: vec![],
            completion: CompletionRule::AllTasksTerminal,
            limits: RoomLimits::default(),
            access: Default::default(),
        })
    }

    #[test]
    fn a_new_room_is_assembling_and_not_yet_live() {
        let room = room();
        assert_eq!(room.phase, RoomPhase::Assembling);
        assert!(!room.may_take_turn(Timestamp::now()));
    }

    #[test]
    fn a_running_room_permits_turns() {
        let mut room = room();
        room.phase = RoomPhase::Running;
        assert!(room.may_take_turn(Timestamp::now()));
    }

    #[test]
    fn a_paused_room_stops_turns_without_finishing() {
        let mut room = room();
        room.phase = RoomPhase::Paused;
        assert!(!room.may_take_turn(Timestamp::now()));
        assert!(!room.phase.is_finished());
    }

    #[test]
    fn the_turn_budget_stops_a_running_room() {
        let mut room = room();
        room.phase = RoomPhase::Running;
        room.turns_used = room.spec.limits.max_total_turns;
        assert!(room.is_out_of_turns());
        assert!(!room.may_take_turn(Timestamp::now()));
    }

    #[test]
    fn the_wall_clock_budget_stops_a_running_room() {
        let mut room = room();
        room.phase = RoomPhase::Running;
        let past_deadline = room.created_at.plus_millis(room.spec.limits.max_duration_ms);
        assert!(room.is_out_of_time(past_deadline));
        assert!(!room.may_take_turn(past_deadline));
    }

    #[test]
    fn a_zero_duration_limit_means_unlimited() {
        let mut room = room();
        room.phase = RoomPhase::Running;
        room.spec.limits.max_duration_ms = 0;
        let far_future = room.created_at.plus_millis(999_999_999);
        assert!(!room.is_out_of_time(far_future));
        assert!(room.may_take_turn(far_future));
    }

    #[test]
    fn finished_phases_are_reported_as_finished() {
        assert!(RoomPhase::Completed.is_finished());
        assert!(RoomPhase::Aborted.is_finished());
        assert!(!RoomPhase::Paused.is_finished());
        assert!(!RoomPhase::Running.is_finished());
    }
}
