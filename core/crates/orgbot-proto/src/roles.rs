//! Roles as capabilities, not as names.
//!
//! A task that names `required_role = "reviewer"` can only ever be done by a
//! bot carrying that exact label. A real run stalled on this: work sat
//! role-locked while the one bot holding the role was busy, and other bots
//! that could plainly have done it were not eligible. The coordinator's only
//! way out was to create a duplicate task without the lock, which left two
//! records for one piece of work.
//!
//! So a role declares *what it can do*, and may name other roles that can
//! stand in for it. Eligibility then asks a better question than "is this bot
//! labelled correctly?" — it asks "can this bot do the thing?".
//!
//! Fallback is deliberately one-directional and explicit. A reviewer standing
//! in for an architect is a decision someone made when they wrote the room;
//! it is not inferred from overlapping capabilities, because two roles sharing
//! a capability does not make them interchangeable.

use serde::{Deserialize, Serialize};

/// What a role is able to do, and who may stand in for it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RoleSpec {
    /// The role's name, as tasks refer to it.
    pub name: String,
    /// What holders of this role can do — `review`, `write`, `test`, `deploy`.
    /// A task may require a capability instead of a named role.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Roles that may take this role's work when no holder is available.
    ///
    /// Named explicitly rather than derived from capabilities: two roles
    /// sharing a capability does not make them interchangeable, and guessing
    /// that they are is how work ends up with the wrong bot.
    #[serde(default)]
    pub fallbacks: Vec<String>,
}

impl RoleSpec {
    /// A role with a name and nothing else declared.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            capabilities: Vec::new(),
            fallbacks: Vec::new(),
        }
    }

    /// Builder: what holders of this role can do.
    pub fn able_to<I, S>(mut self, capabilities: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.capabilities = capabilities.into_iter().map(Into::into).collect();
        self
    }

    /// Builder: roles that may stand in for this one.
    pub fn falling_back_to<I, S>(mut self, roles: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.fallbacks = roles.into_iter().map(Into::into).collect();
        self
    }
}

/// How closely a bot matches what a task asked for.
///
/// Ordered worst to best so the scheduler can prefer an exact holder and only
/// reach for a stand-in when it must — a fallback should be a relief valve,
/// not the normal path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Eligibility {
    /// Cannot do this work.
    No,
    /// Can stand in for the role that was asked for.
    Fallback,
    /// Holds a capability the work needs, without holding the named role.
    Capable,
    /// Holds the role that was asked for.
    Exact,
}

impl Eligibility {
    /// Whether this bot may take the work at all.
    pub const fn is_eligible(self) -> bool {
        !matches!(self, Self::No)
    }

    /// Whether the bot is standing in rather than doing its own job.
    ///
    /// Worth recording on the task: a room where most work is done by
    /// stand-ins is a room whose roster does not match its work.
    pub const fn is_stand_in(self) -> bool {
        matches!(self, Self::Fallback | Self::Capable)
    }
}

/// The room's roles, and the questions the scheduler asks of them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleTable {
    /// Every role the room declares. A role a bot holds but the room does not
    /// declare still works as a plain name; it simply has no capabilities and
    /// no stand-ins.
    #[serde(default)]
    pub roles: Vec<RoleSpec>,
}

impl RoleTable {
    /// A table from declared roles.
    pub fn new(roles: Vec<RoleSpec>) -> Self {
        Self { roles }
    }

    /// Look up one role.
    pub fn get(&self, name: &str) -> Option<&RoleSpec> {
        self.roles.iter().find(|role| role.name == name)
    }

    /// Every capability the given roles confer.
    pub fn capabilities_of<'a>(
        &self,
        held: impl IntoIterator<Item = &'a String>,
    ) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for role in held {
            if let Some(spec) = self.get(role) {
                for capability in &spec.capabilities {
                    if !out.contains(capability) {
                        out.push(capability.clone());
                    }
                }
            }
        }
        out
    }

    /// How well a bot holding `held` matches work requiring `required`.
    ///
    /// `required` may name a role or a capability. Work with no requirement is
    /// open to anyone, which is what it meant before roles had capabilities.
    pub fn eligibility(&self, held: &[String], required: Option<&str>) -> Eligibility {
        let Some(required) = required.filter(|r| !r.is_empty()) else {
            return Eligibility::Exact;
        };
        if held.iter().any(|role| role == required) {
            return Eligibility::Exact;
        }
        // A capability the work names directly.
        if self
            .capabilities_of(held.iter())
            .iter()
            .any(|capability| capability == required)
        {
            return Eligibility::Capable;
        }
        // A role explicitly allowed to stand in for the one asked for.
        if let Some(spec) = self.get(required) {
            if spec.fallbacks.iter().any(|stand_in| held.contains(stand_in)) {
                return Eligibility::Fallback;
            }
        }
        Eligibility::No
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> RoleTable {
        RoleTable::new(vec![
            RoleSpec::new("reviewer")
                .able_to(["review", "test"])
                .falling_back_to(["architect"]),
            RoleSpec::new("architect").able_to(["review", "design"]),
            RoleSpec::new("writer").able_to(["write"]),
        ])
    }

    fn held(roles: &[&str]) -> Vec<String> {
        roles.iter().map(|r| (*r).to_string()).collect()
    }

    #[test]
    fn holding_the_role_is_an_exact_match() {
        assert_eq!(
            table().eligibility(&held(&["reviewer"]), Some("reviewer")),
            Eligibility::Exact
        );
    }

    /// The case that stalled a real room: the only reviewer was busy, and work
    /// sat locked while a bot that could plainly have reviewed it waited.
    #[test]
    fn a_declared_stand_in_can_take_the_work() {
        let eligibility = table().eligibility(&held(&["architect"]), Some("reviewer"));
        assert_eq!(eligibility, Eligibility::Fallback);
        assert!(eligibility.is_eligible());
        assert!(eligibility.is_stand_in());
    }

    /// Fallback is one-directional: a reviewer standing in for an architect is
    /// a separate decision, and was not made here.
    #[test]
    fn fallback_does_not_run_both_ways() {
        assert_eq!(
            table().eligibility(&held(&["reviewer"]), Some("architect")),
            Eligibility::No
        );
    }

    #[test]
    fn work_can_ask_for_a_capability_instead_of_a_role() {
        assert_eq!(
            table().eligibility(&held(&["architect"]), Some("design")),
            Eligibility::Capable
        );
        assert_eq!(
            table().eligibility(&held(&["writer"]), Some("design")),
            Eligibility::No
        );
    }

    /// Sharing a capability does not make two roles interchangeable.
    #[test]
    fn a_shared_capability_is_not_a_licence_to_stand_in() {
        // Both can `review`, but nothing says a writer may act as a reviewer.
        let table = RoleTable::new(vec![
            RoleSpec::new("reviewer").able_to(["review"]),
            RoleSpec::new("writer").able_to(["review"]),
        ]);
        assert_eq!(
            table.eligibility(&held(&["writer"]), Some("reviewer")),
            Eligibility::No
        );
    }

    #[test]
    fn an_exact_holder_outranks_a_stand_in() {
        assert!(Eligibility::Exact > Eligibility::Fallback);
        assert!(Eligibility::Capable > Eligibility::Fallback);
        assert!(Eligibility::Fallback > Eligibility::No);
        assert!(!Eligibility::No.is_eligible());
        assert!(!Eligibility::Exact.is_stand_in());
    }

    #[test]
    fn work_with_no_requirement_is_open_to_anyone() {
        assert_eq!(
            table().eligibility(&held(&[]), None),
            Eligibility::Exact
        );
        assert_eq!(
            table().eligibility(&held(&[]), Some("")),
            Eligibility::Exact
        );
    }

    /// A room that declares no roles behaves exactly as it did before this
    /// existed: names match or they do not.
    #[test]
    fn an_undeclared_role_still_matches_by_name() {
        let empty = RoleTable::default();
        assert_eq!(
            empty.eligibility(&held(&["reviewer"]), Some("reviewer")),
            Eligibility::Exact
        );
        assert_eq!(
            empty.eligibility(&held(&["writer"]), Some("reviewer")),
            Eligibility::No
        );
    }

    #[test]
    fn capabilities_are_collected_without_duplicates() {
        let capabilities = table().capabilities_of(held(&["reviewer", "architect"]).iter());
        assert_eq!(capabilities.iter().filter(|c| *c == "review").count(), 1);
        assert!(capabilities.contains(&"design".to_string()));
        assert!(capabilities.contains(&"test".to_string()));
    }

    #[test]
    fn a_table_round_trips_through_json() {
        let table = table();
        let json = serde_json::to_string(&table).unwrap();
        assert_eq!(serde_json::from_str::<RoleTable>(&json).unwrap(), table);
    }
}
