//! Wire-format timestamps.
//!
//! Milliseconds since the Unix epoch, as a signed 64-bit integer. Every client
//! language in this repo (Dart, TypeScript, Python) maps that to a native
//! integer with no parsing and no timezone ambiguity, which a formatted string
//! would not.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A point in time, in milliseconds since the Unix epoch (UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// The current wall-clock time.
    ///
    /// Clocks before the epoch are clamped to zero rather than panicking; a
    /// misconfigured device clock should not take the engine down.
    pub fn now() -> Self {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        Self(millis)
    }

    /// Milliseconds since the epoch.
    pub const fn as_millis(self) -> i64 {
        self.0
    }

    /// Milliseconds elapsed from `self` to `later`, saturating at zero.
    pub const fn elapsed_to(self, later: Timestamp) -> i64 {
        let delta = later.0 - self.0;
        if delta < 0 {
            0
        } else {
            delta
        }
    }

    /// This instant advanced by `millis`.
    pub const fn plus_millis(self, millis: i64) -> Self {
        Self(self.0 + millis)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_after_2020() {
        // 2020-01-01T00:00:00Z in millis; guards against a unit mix-up.
        assert!(Timestamp::now().as_millis() > 1_577_836_800_000);
    }

    #[test]
    fn elapsed_saturates_on_backwards_clocks() {
        assert_eq!(Timestamp(500).elapsed_to(Timestamp(100)), 0);
        assert_eq!(Timestamp(100).elapsed_to(Timestamp(500)), 400);
    }

    #[test]
    fn serializes_as_a_bare_integer() {
        assert_eq!(serde_json::to_string(&Timestamp(1234)).unwrap(), "1234");
    }
}
