//! What a room has actually consumed.
//!
//! Turns were counted from the beginning, because a turn is what the budget
//! was written in. Tokens were not counted at all: every turn reported its
//! usage, the report was folded into the turn's own total, and then thrown
//! away. So the room could say it had taken 347 turns and nothing whatever
//! about what that cost.
//!
//! That gap is not academic. Concurrency was raised from one turn at a time to
//! four — a four-fold increase in the rate a room draws on an account — and
//! the only evidence available for whether that was safe was the absence of
//! rate-limit errors during a single day. A weekly allowance does not announce
//! itself that way. It depletes silently and then refuses, and by the time the
//! refusals start the week is spent. Reasoning from "no errors yet" was
//! reasoning from a signal that could not have shown the problem, and the
//! answer to that is not a better guess. It is to count.
//!
//! Tokens are not the same thing as an allowance — providers meter in their
//! own units and none of this knows the exchange rate. What it gives is the
//! shape: which bots draw the most, which backend carries the room, and how
//! fast that is moving. Enough to see a limit coming rather than discovering
//! it.

use serde::{Deserialize, Serialize};

/// Token accounting for one turn, or summed over many.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Whether the provider supplied usage. Reported zero remains distinct from missing.
    #[serde(default = "legacy_reported")]
    pub reported: bool,
    /// Whether all provider calls represented by this turn reported usage.
    #[serde(default = "legacy_reported")]
    pub complete: bool,
    /// Provider calls represented by this aggregate. Zero is the fold identity.
    #[serde(default)]
    pub calls: u32,
    /// Tokens read.
    pub input_tokens: u32,
    /// Tokens written.
    pub output_tokens: u32,
    /// Tokens served from the prompt cache.
    pub cache_read_tokens: u32,
    /// Tokens written into the prompt cache.
    pub cache_write_tokens: u32,
    /// Estimated API charge, known only when this was an API call and rates exist.
    #[serde(default)]
    pub actual_api_cost_microusd: Option<u64>,
    /// Cost at configured API rates, including for subscription-backed calls.
    #[serde(default)]
    pub api_equivalent_cost_microusd: Option<u64>,
}

fn legacy_reported() -> bool {
    true
}

impl TokenUsage {
    pub fn missing() -> Self {
        Self {
            calls: 1,
            ..Self::default()
        }
    }

    /// Fold another turn's usage into this one.
    ///
    /// Saturating at u32::MAX (4,294,967,295), because a long-running room is
    /// exactly where this matters and an overflow panic in the accounting
    /// would take down the room it was supposed to be reporting on. The digest
    /// UI renders that maximum as a lower bound so saturation is visible.
    pub fn add(&mut self, other: Self) {
        if other.is_identity() {
            return;
        }
        if self.is_identity() {
            *self = other;
            return;
        }
        self.calls = self.calls.saturating_add(other.calls);
        self.complete &= other.complete;
        self.reported |= other.reported;
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(other.cache_write_tokens);
        self.actual_api_cost_microusd = add_optional(
            self.actual_api_cost_microusd,
            other.actual_api_cost_microusd,
        );
        self.api_equivalent_cost_microusd = add_optional(
            self.api_equivalent_cost_microusd,
            other.api_equivalent_cost_microusd,
        );
    }

    /// Total tokens billed as input, cache reads included.
    pub const fn total_input(&self) -> u32 {
        self.input_tokens
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_write_tokens)
    }

    /// Every token that crossed the wire, in and out.
    pub const fn total(&self) -> u32 {
        self.total_input().saturating_add(self.output_tokens)
    }

    /// Whether anything was recorded at all.
    ///
    /// Not every backend reports usage, and a zero here means "not counted"
    /// rather than "free" — worth keeping distinguishable so a room does not
    /// present an unmetered provider as a cheap one.
    pub const fn is_empty(&self) -> bool {
        !self.reported
    }

    /// Whether this value is only the fold identity. A legacy serialized
    /// usage record has `calls == 0`, but may still carry token totals.
    const fn is_identity(&self) -> bool {
        !self.reported
            && !self.complete
            && self.calls == 0
            && self.input_tokens == 0
            && self.output_tokens == 0
            && self.cache_read_tokens == 0
            && self.cache_write_tokens == 0
            && self.actual_api_cost_microusd.is_none()
            && self.api_equivalent_cost_microusd.is_none()
    }
}

fn add_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
        (None, Some(b)) => Some(b),
        (Some(a), None) => Some(a),
        (None, None) => None,
    }
}

/// What one bot, or one backend, has drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spend {
    /// Model turns taken.
    pub turns: u32,
    /// Tokens across those turns.
    pub usage: TokenUsage,
    /// Number of turns whose provider supplied usage.
    #[serde(default)]
    pub reported_turns: u32,
    /// Number of turns whose usage was absent.
    #[serde(default)]
    pub unreported_turns: u32,
    /// Number of turns with a known actual API estimate.
    #[serde(default)]
    pub actual_cost_turns: u32,
    /// Number of turns with a known API-equivalent estimate.
    #[serde(default)]
    pub equivalent_cost_turns: u32,
}

impl Spend {
    /// Record one turn.
    pub fn record(&mut self, usage: TokenUsage) {
        let was_empty = self.turns == 0;
        self.turns = self.turns.saturating_add(1);
        if usage.reported {
            self.reported_turns = self.reported_turns.saturating_add(1);
            if !usage.complete {
                self.unreported_turns = self.unreported_turns.saturating_add(1);
            }
            if usage.complete {
                if usage.actual_api_cost_microusd.is_some() {
                    self.actual_cost_turns = self.actual_cost_turns.saturating_add(1);
                }
            }
            if usage.complete {
                if usage.api_equivalent_cost_microusd.is_some() {
                    self.equivalent_cost_turns = self.equivalent_cost_turns.saturating_add(1);
                }
            }
        } else {
            self.unreported_turns = self.unreported_turns.saturating_add(1);
        }
        let mut contribution = usage;
        if !usage.complete {
            contribution.actual_api_cost_microusd = None;
            contribution.api_equivalent_cost_microusd = None;
        }
        self.usage.add(contribution);
        self.usage.complete = if was_empty {
            usage.complete
        } else {
            self.usage.complete && usage.complete
        };
    }

    /// Fold another tally into this one.
    pub fn merge(&mut self, other: Self) {
        let was_empty = self.turns == 0;
        self.turns = self.turns.saturating_add(other.turns);
        self.reported_turns = self.reported_turns.saturating_add(other.reported_turns);
        self.unreported_turns = self.unreported_turns.saturating_add(other.unreported_turns);
        self.actual_cost_turns = self
            .actual_cost_turns
            .saturating_add(other.actual_cost_turns);
        self.equivalent_cost_turns = self
            .equivalent_cost_turns
            .saturating_add(other.equivalent_cost_turns);
        self.usage.input_tokens = self
            .usage
            .input_tokens
            .saturating_add(other.usage.input_tokens);
        self.usage.output_tokens = self
            .usage
            .output_tokens
            .saturating_add(other.usage.output_tokens);
        self.usage.cache_read_tokens = self
            .usage
            .cache_read_tokens
            .saturating_add(other.usage.cache_read_tokens);
        self.usage.cache_write_tokens = self
            .usage
            .cache_write_tokens
            .saturating_add(other.usage.cache_write_tokens);
        self.usage.reported |= other.usage.reported;
        self.usage.complete = if was_empty {
            other.usage.complete
        } else {
            self.usage.complete && other.usage.complete
        };
        self.usage.calls = self.usage.calls.saturating_add(other.usage.calls);
        self.usage.actual_api_cost_microusd = add_optional(
            self.usage.actual_api_cost_microusd,
            other.usage.actual_api_cost_microusd,
        );
        self.usage.api_equivalent_cost_microusd = add_optional(
            self.usage.api_equivalent_cost_microusd,
            other.usage.api_equivalent_cost_microusd,
        );
    }

    /// Average tokens per turn, or zero when nothing has been counted.
    ///
    /// The number that says whether a room is spending on thinking or on
    /// re-reading the same context every turn.
    pub const fn per_turn(&self) -> u32 {
        match self.usage.total().checked_div(self.turns) {
            Some(average) => average,
            None => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u32, output: u32, cache_read: u32) -> TokenUsage {
        TokenUsage {
            reported: true,
            complete: true,
            calls: 1,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_write_tokens: 0,
            ..TokenUsage::default()
        }
    }

    #[test]
    fn cache_reads_count_as_input_because_they_are_billed_as_input() {
        let one = usage(100, 50, 900);
        assert_eq!(one.total_input(), 1_000);
        assert_eq!(one.total(), 1_050);
    }

    #[test]
    fn missing_usage_and_reported_zero_remain_distinct() {
        assert!(TokenUsage::missing().is_empty());
        let zero = TokenUsage {
            reported: true,
            complete: true,
            calls: 1,
            ..TokenUsage::default()
        };
        assert!(!zero.is_empty());
    }

    #[test]
    fn a_later_report_does_not_hide_an_earlier_missing_call() {
        let mut aggregate = TokenUsage::default();
        aggregate.add(TokenUsage::missing());
        aggregate.add(usage(10, 2, 3));
        assert!(aggregate.reported);
        assert!(!aggregate.complete);
        assert_eq!(aggregate.calls, 2);
        assert_eq!(aggregate.total(), 15);
    }

    #[test]
    fn legacy_serialized_usage_tokens_survive_a_new_usage_fold() {
        let legacy: TokenUsage = serde_json::from_value(serde_json::json!({
            "input_tokens": 17,
            "output_tokens": 4,
            "cache_read_tokens": 6,
            "cache_write_tokens": 2
        }))
        .unwrap();
        assert_eq!(legacy.calls, 0);

        let mut aggregate = TokenUsage::default();
        aggregate.add(legacy);
        aggregate.add(usage(3, 1, 0));
        assert_eq!(aggregate.input_tokens, 20);
        assert_eq!(aggregate.output_tokens, 5);
        assert_eq!(aggregate.cache_read_tokens, 6);
        assert_eq!(aggregate.cache_write_tokens, 2);
        assert_eq!(aggregate.calls, 1);
    }

    #[test]
    fn a_missing_turn_taints_spend_completeness_in_either_order() {
        let mut complete = usage(12, 5, 2);
        complete.actual_api_cost_microusd = Some(9);
        let missing = TokenUsage::missing();

        for sequence in [[complete, missing], [missing, complete]] {
            let mut spend = Spend::default();
            for usage in sequence {
                spend.record(usage);
            }
            assert_eq!(spend.turns, 2);
            assert_eq!(spend.usage.calls, 2);
            assert!(spend.usage.reported);
            assert!(!spend.usage.complete);
            assert_eq!(spend.usage.total(), complete.total());
            assert_eq!(spend.reported_turns, 1);
            assert_eq!(spend.unreported_turns, 1);
            assert_eq!(spend.actual_cost_turns, 1);
            assert_eq!(spend.usage.actual_api_cost_microusd, Some(9));
        }
    }

    #[test]
    fn turns_and_tokens_accumulate_together() {
        let mut spend = Spend::default();
        spend.record(usage(100, 50, 0));
        spend.record(usage(300, 50, 0));
        assert_eq!(spend.turns, 2);
        assert_eq!(spend.usage.total(), 500);
        assert_eq!(spend.per_turn(), 250);
    }

    /// A long room must not panic in the part that reports on it.
    #[test]
    fn accounting_saturates_rather_than_overflowing() {
        let mut spend = Spend {
            turns: u32::MAX,
            usage: usage(u32::MAX, u32::MAX, u32::MAX),
            ..Spend::default()
        };
        spend.record(usage(1, 1, 1));
        assert_eq!(spend.turns, u32::MAX);
        assert_eq!(spend.usage.total(), u32::MAX);
    }

    /// Zero means unmetered, not free.
    #[test]
    fn nothing_counted_is_distinguishable_from_nothing_spent() {
        assert!(TokenUsage::default().is_empty());
        assert!(!usage(1, 0, 0).is_empty());
        assert_eq!(Spend::default().per_turn(), 0);
    }
}
