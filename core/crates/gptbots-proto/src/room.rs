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

    /// Whether the room has finished for good.
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
    /// How long an unanswered handoff waits before expiring.
    pub handoff_timeout_ms: i64,
    /// How many consecutive failed turns fault a bot.
    pub max_consecutive_errors: u32,
}

impl Default for RoomLimits {
    fn default() -> Self {
        Self {
            max_total_turns: 500,
            max_duration_ms: 60 * 60 * 1000,
            handoff_timeout_ms: 5 * 60 * 1000,
            max_consecutive_errors: 3,
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
        self.turns_used >= self.spec.limits.max_total_turns
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
mod tests {
    use super::*;

    fn room() -> Room {
        Room::new(RoomSpec {
            name: "launch".into(),
            goal: "ship the release notes".into(),
            roster: vec![],
            completion: CompletionRule::AllTasksTerminal,
            limits: RoomLimits::default(),
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
