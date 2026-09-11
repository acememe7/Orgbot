//! Playbooks a bot can reach for, without carrying them all the time.
//!
//! A room full of capable models still does poor work if every bot works out
//! how to approach a problem from first principles on every task. The fix is
//! not a longer system prompt — that is paid for on every request, by every
//! bot, forever, whether the advice applies or not.
//!
//! So a skill is split. Its **description** is one line and is always in view,
//! because a bot cannot ask for something it does not know exists. Its
//! **body** is however long it needs to be and is fetched only by the bot that
//! decided it applied, on the turn it applies. Twenty skills cost a few
//! hundred tokens to advertise and nothing at all until one is used.
//!
//! That split is the whole point. A room that had been re-sending four million
//! tokens of stale board snapshots did not need more context; it needed the
//! context it carries to be the context it is using.

use serde::{Deserialize, Serialize};

/// One playbook: how to approach a kind of work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    /// What a bot asks for it by.
    pub name: String,
    /// One line, always in view, saying when to reach for this.
    pub description: String,
    /// The instructions themselves, fetched only when chosen.
    pub body: String,
}

/// The longest a description may be where it is advertised.
///
/// A skill file's own description is written for a human reading the file and
/// often runs to a paragraph. Twenty of those in every system prompt is a tax
/// on every request for advice most of them will not take. One line is enough
/// to decide whether to look.
pub const INDEX_DESCRIPTION_CHARS: usize = 180;

impl Skill {
    /// Parse a `SKILL.md`: YAML-ish front matter, then the instructions.
    ///
    /// Deliberately tolerant. These files are written by hand and by other
    /// tools, and a skill that fails to load is worse than one that loads with
    /// a slightly wrong description — the first is invisible, and nobody finds
    /// out it was meant to be there.
    pub fn parse(source: &str, fallback_name: &str) -> Option<Self> {
        let trimmed = source.trim_start();
        let (front, body) = match trimmed.strip_prefix("---") {
            Some(rest) => match rest.split_once("\n---") {
                Some((front, body)) => (front, body.trim_start_matches(['-', '\n', '\r'])),
                None => ("", trimmed),
            },
            None => ("", trimmed),
        };

        let name = field(front, "name").unwrap_or_else(|| fallback_name.to_string());
        let description = field(front, "description").unwrap_or_default();
        let body = body.trim().to_string();
        if body.is_empty() && description.is_empty() {
            return None;
        }
        Some(Self {
            name,
            description,
            body,
        })
    }

    /// The one line that goes in the index.
    pub fn summary(&self) -> String {
        let flat = self
            .description
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if flat.chars().count() <= INDEX_DESCRIPTION_CHARS {
            return flat;
        }
        let cut: String = flat.chars().take(INDEX_DESCRIPTION_CHARS).collect();
        // Break on a word so the line does not end mid-thought.
        match cut.rsplit_once(' ') {
            Some((head, _)) => format!("{head}…"),
            None => format!("{cut}…"),
        }
    }
}

/// Read one field out of front matter, folding a `>` block into a line.
fn field(front: &str, key: &str) -> Option<String> {
    let mut lines = front.lines();
    let start = lines.by_ref().find(|line| {
        line.trim_start()
            .strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with(':'))
    })?;
    let inline = start.split_once(':').map(|(_, rest)| rest.trim()).unwrap_or("");
    if !inline.is_empty() && inline != ">" && inline != "|" && inline != ">-" && inline != "|-" {
        return Some(inline.trim_matches(['"', '\'']).to_string());
    }
    // A folded block: every following indented line belongs to it.
    let mut folded = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with(' ') && !line.starts_with('\t') {
            break;
        }
        folded.push(line.trim());
    }
    (!folded.is_empty()).then(|| folded.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PONYTAIL: &str = r#"---
name: ponytail
description: >
  Forces the laziest solution that actually works, simplest, shortest, most
  minimal. Use on ANY coding task.
---

# Ponytail

Question whether the task needs to exist at all.
"#;

    #[test]
    fn a_skill_file_parses_into_a_name_a_line_and_a_body() {
        let skill = Skill::parse(PONYTAIL, "ignored").expect("it parses");
        assert_eq!(skill.name, "ponytail");
        assert!(skill.description.starts_with("Forces the laziest"));
        assert!(skill.body.contains("Question whether the task"));
        assert!(
            !skill.body.contains("description:"),
            "front matter must not leak into the instructions"
        );
    }

    /// The index is paid for on every request, so it is one line.
    #[test]
    fn the_index_line_is_short_enough_to_carry_twenty_of() {
        let long = format!(
            "---\nname: x\ndescription: {}\n---\nbody",
            "word ".repeat(300)
        );
        let skill = Skill::parse(&long, "x").unwrap();
        assert!(skill.description.len() > INDEX_DESCRIPTION_CHARS);
        assert!(skill.summary().chars().count() <= INDEX_DESCRIPTION_CHARS + 1);
        assert!(skill.summary().ends_with('…'), "and says it was cut");
    }

    /// A hand-written file with no front matter is still a skill.
    #[test]
    fn a_file_without_front_matter_still_loads_under_its_own_name() {
        let skill = Skill::parse("Just do the simple thing.", "hermes-deploy").unwrap();
        assert_eq!(skill.name, "hermes-deploy");
        assert_eq!(skill.body, "Just do the simple thing.");
    }

    /// Loading nothing is not a skill.
    #[test]
    fn an_empty_file_is_not_a_skill() {
        assert!(Skill::parse("", "x").is_none());
        assert!(Skill::parse("---\n---\n", "x").is_none());
    }

    #[test]
    fn a_quoted_single_line_description_keeps_its_text() {
        let skill = Skill::parse("---\nname: n\ndescription: \"Do a thing\"\n---\nbody", "x")
            .unwrap();
        assert_eq!(skill.description, "Do a thing");
        assert_eq!(skill.summary(), "Do a thing");
    }
}
