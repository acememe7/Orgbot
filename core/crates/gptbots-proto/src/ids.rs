//! Strongly-typed identifiers.
//!
//! Every id is a UUIDv4 rendered with a short human-readable prefix so that a
//! raw event log stays readable (`task_9f2c…`, `bot_1a4e…`). The prefix is part
//! of the wire format: parsing rejects an id carrying the wrong one, which
//! turns "passed a TaskId where an AgentId was expected" into a decode error
//! instead of a silent lookup miss.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

/// Error returned when a string cannot be decoded as a given id type.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdError {
    #[error("expected id with prefix `{expected}_`, got `{got}`")]
    WrongPrefix { expected: &'static str, got: String },
    #[error("malformed uuid in id `{0}`")]
    BadUuid(String),
}

macro_rules! typed_id {
    ($name:ident, $prefix:literal) => {
        #[doc = concat!("Identifier for a `", $prefix, "` entity.")]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Uuid);

        impl $name {
            /// The wire prefix used by this id type.
            pub const PREFIX: &'static str = $prefix;

            /// Mint a fresh random id.
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wrap an existing UUID.
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// The underlying UUID.
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }

            /// A short form for logs and UI: prefix plus the first 8 hex digits.
            pub fn short(&self) -> String {
                format!("{}_{}", $prefix, &self.0.simple().to_string()[..8])
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}_{}", $prefix, self.0.simple())
            }
        }

        impl FromStr for $name {
            type Err = IdError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let rest = s
                    .strip_prefix(concat!($prefix, "_"))
                    .ok_or_else(|| IdError::WrongPrefix {
                        expected: $prefix,
                        got: s.to_string(),
                    })?;
                Uuid::parse_str(rest)
                    .map(Self)
                    .map_err(|_| IdError::BadUuid(s.to_string()))
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(d)?;
                raw.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

typed_id!(RoomId, "room");
typed_id!(AgentId, "bot");
typed_id!(TaskId, "task");
typed_id!(HandoffId, "ho");
typed_id!(MessageId, "msg");
typed_id!(EventId, "ev");
typed_id!(ArtifactId, "art");
typed_id!(ReviewId, "rev");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_through_string() {
        let id = TaskId::new();
        let parsed: TaskId = id.to_string().parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn rejects_a_foreign_prefix() {
        let agent = AgentId::new().to_string();
        let err = agent.parse::<TaskId>().unwrap_err();
        assert!(matches!(err, IdError::WrongPrefix { expected: "task", .. }));
    }

    #[test]
    fn rejects_a_malformed_uuid() {
        assert_eq!(
            "task_nonsense".parse::<TaskId>().unwrap_err(),
            IdError::BadUuid("task_nonsense".to_string())
        );
    }

    #[test]
    fn serde_uses_the_prefixed_string_form() {
        let id = AgentId::new();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{id}\""));
        assert_eq!(serde_json::from_str::<AgentId>(&json).unwrap(), id);
    }

    #[test]
    fn short_form_is_prefix_plus_eight() {
        let id = RoomId::new();
        let short = id.short();
        assert!(short.starts_with("room_"));
        assert_eq!(short.len(), "room_".len() + 8);
    }
}
