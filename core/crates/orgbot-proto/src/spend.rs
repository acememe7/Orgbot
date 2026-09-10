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
    /// Tokens read.
    pub input_tokens: u32,
    /// Tokens written.
    pub output_tokens: u32,
    /// Tokens served from the prompt cache.
    pub cache_read_tokens: u32,
    /// Tokens written into the prompt cache.
    pub cache_write_tokens: u32,
}

impl TokenUsage {
    /// Fold another turn's usage into this one.
    ///
    /// Saturating, because a long-running room is exactly where this matters
    /// and an overflow panic in the accounting would take down the room it was
    /// supposed to be reporting on.
    pub fn add(&mut self, other: Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self.cache_read_tokens.saturating_add(other.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(other.cache_write_tokens);
    }

    /// Total tokens billed as input, cache reads included.
    pub const fn total_input(&self) -> u32 {
        self.input_tokens.saturating_add(self.cache_read_tokens)
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
        self.total() == 0
    }
}

/// What one bot, or one backend, has drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spend {
    /// Model turns taken.
    pub turns: u32,
    /// Tokens across those turns.
    pub usage: TokenUsage,
}

impl Spend {
    /// Record one turn.
    pub fn record(&mut self, usage: TokenUsage) {
        self.turns = self.turns.saturating_add(1);
        self.usage.add(usage);
    }

    /// Fold another tally into this one.
    pub fn merge(&mut self, other: Self) {
        self.turns = self.turns.saturating_add(other.turns);
        self.usage.add(other.usage);
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
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_write_tokens: 0,
        }
    }

    #[test]
    fn cache_reads_count_as_input_because_they_are_billed_as_input() {
        let one = usage(100, 50, 900);
        assert_eq!(one.total_input(), 1_000);
        assert_eq!(one.total(), 1_050);
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
