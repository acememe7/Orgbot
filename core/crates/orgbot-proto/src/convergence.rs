//! Whether review is getting anywhere.
//!
//! A run produced review cycles that each reported one small thing, sent the
//! work back, and found another small thing next time. Twenty individual
//! fences went in where one admitted choke point was needed, and nothing in
//! the system noticed that the same underlying problem had been reported
//! four times in different words.
//!
//! Two questions are asked here, and both need findings to have durable
//! identities rather than being fresh prose every cycle:
//!
//! - **Is the same invariant failing repeatedly?** If so, patching the symptom
//!   again is the wrong move, and the room should say so rather than queue
//!   another remediation.
//! - **Is the unresolved set shrinking?** Reviews that keep finding roughly as
//!   much as last time are not converging, and more cycles will not help.
//!
//! Neither judgement is left to a model's impression of how it is going. Both
//! fall out of comparing successive review cycles.

use serde::{Deserialize, Serialize};

use crate::review::{Finding, Severity};

/// A stable identity for "this problem", across the words used to describe it.
///
/// Derived from what a finding is *about* — its locus and the shape of its
/// summary — rather than the exact sentence, because a reviewer rarely
/// phrases the same objection identically twice and an identity that moved
/// with the wording would make every cycle look like new ground.
pub fn finding_id(finding: &Finding) -> String {
    let locus = finding
        .locus
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let subject = keywords(&finding.summary);
    format!("{locus}\u{1f}{subject}")
}

/// The content words of a summary, normalised and ordered.
///
/// Short and very common words carry no identity — "the missing lock" and "a
/// lock is missing" are the same objection — so they are dropped and what
/// remains is sorted, which makes the identity independent of phrasing order.
///
/// This matches reordering and filler words. It does not match synonyms or
/// morphology: "not fenced" and "unfenced" are different vocabulary and will
/// read as different findings. Recognising those needs a lexicon or a model,
/// and guessing at it would merge objections that are genuinely distinct.
fn keywords(summary: &str) -> String {
    let mut words = content_words(summary);
    words.sort();
    words.dedup();
    if words.is_empty() {
        // Nothing survived the filter — a very short summary, or all filler.
        // Falling through to an empty key would give every such finding the
        // same identity and make unrelated problems look like one recurring
        // one, so the whole summary stands in.
        return summary.trim().to_lowercase();
    }
    words.join(" ")
}

/// The words of a summary that carry meaning.
fn content_words(summary: &str) -> Vec<String> {
    const NOISE: &[&str] = &[
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "to", "of", "in", "on", "for",
        "and", "or", "not", "no", "it", "its", "this", "that", "with", "without", "should",
        "must", "may", "can", "does", "do", "has", "have", "there", "here", "but",
    ];
    summary
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.len() > 2 && !NOISE.contains(word))
        .map(str::to_string)
        .collect()
}

/// How much two findings' vocabularies overlap, from 0 to 1.
fn overlap(a: &Finding, b: &Finding) -> f32 {
    let (mut left, mut right) = (content_words(&a.summary), content_words(&b.summary));
    left.sort();
    left.dedup();
    right.sort();
    right.dedup();
    if left.is_empty() || right.is_empty() {
        return f32::from(u8::from(a.summary.trim().eq_ignore_ascii_case(b.summary.trim())));
    }
    let shared = left.iter().filter(|word| right.contains(word)).count();
    let union = left.len() + right.len() - shared;
    shared as f32 / union as f32
}

/// How much vocabulary two findings must share to be one recurring problem.
///
/// Set below half deliberately: a reviewer restating an objection usually
/// keeps its nouns and changes everything around them, so demanding a majority
/// would miss exactly the recurrence this is looking for.
const SAME_PROBLEM_OVERLAP: f32 = 0.4;

/// Whether two findings are the same problem reported again.
///
/// Requires the same locus. Two unfenced mutations in different files are two
/// problems, however similarly they are described — and treating them as one
/// would hide half the work.
pub fn same_problem(a: &Finding, b: &Finding) -> bool {
    let locus = |f: &Finding| {
        f.locus
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_lowercase()
    };
    locus(a) == locus(b) && overlap(a, b) >= SAME_PROBLEM_OVERLAP
}

/// What successive reviews of one task add up to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Convergence {
    /// Not enough cycles yet to say anything.
    TooEarly,
    /// The unresolved set is shrinking. Keep going.
    Converging {
        /// Blocking findings outstanding after the latest cycle.
        outstanding: usize,
    },
    /// The same invariant has failed repeatedly. Patching the symptom again
    /// is the wrong move; the room should fix the thing underneath.
    Structural {
        /// The finding identity that keeps coming back.
        recurring: String,
        /// How many cycles have reported it.
        cycles: usize,
        /// The most recent wording, for a person reading the recommendation.
        summary: String,
    },
    /// Cycles are not reducing the problem. More of them will not help.
    Stalled {
        /// Blocking findings outstanding after the latest cycle.
        outstanding: usize,
        /// How many cycles have run without progress.
        cycles: usize,
    },
}

impl Convergence {
    /// Whether review should stop and something else should happen.
    ///
    /// Both of these are internal decisions: the room escalates to
    /// architecture-level work itself rather than asking a person whether to
    /// keep going round.
    pub const fn needs_escalation(&self) -> bool {
        matches!(self, Self::Structural { .. } | Self::Stalled { .. })
    }

    /// What the room should do about it, in words a bot can act on.
    pub fn recommendation(&self) -> Option<String> {
        match self {
            Self::TooEarly | Self::Converging { .. } => None,
            Self::Structural {
                cycles, summary, ..
            } => Some(format!(
                "`{summary}` has now failed review {cycles} times. Stop patching \
                 the symptom: raise one remediation that fixes the invariant \
                 underneath, and supersede the per-case fixes."
            )),
            Self::Stalled { outstanding, cycles } => Some(format!(
                "{cycles} review cycles have not reduced the {outstanding} \
                 blocking findings. Escalate to an architecture-level \
                 remediation rather than another round."
            )),
        }
    }
}

/// How many times one invariant may fail before it is called structural.
///
/// Twice is a coincidence worth watching; three times is a pattern. Set here
/// so the threshold is one decision rather than a number buried in a loop.
pub const STRUCTURAL_THRESHOLD: usize = 3;

/// How many non-improving cycles count as stalled.
pub const STALL_THRESHOLD: usize = 3;

/// Judge a task's review history, oldest cycle first.
///
/// Takes the blocking findings of each cycle rather than whole reviews, so it
/// can be used on a stored history or on findings assembled in memory.
pub fn assess(cycles: &[Vec<Finding>]) -> Convergence {
    if cycles.len() < 2 {
        return Convergence::TooEarly;
    }

    // A finding that keeps coming back, cycle after cycle.
    let mut seen: Vec<(Finding, usize, String)> = Vec::new();
    for cycle in cycles {
        let mut counted_this_cycle: Vec<Finding> = Vec::new();
        for finding in cycle.iter().filter(|f| f.severity.blocks_approval()) {
            // Once per cycle: a reviewer listing the same problem twice in one
            // pass has not shown it recurring.
            if counted_this_cycle.iter().any(|prior| same_problem(prior, finding)) {
                continue;
            }
            counted_this_cycle.push(finding.clone());
            match seen
                .iter_mut()
                .find(|(known, _, _)| same_problem(known, finding))
            {
                Some((_, count, summary)) => {
                    *count += 1;
                    summary.clone_from(&finding.summary);
                }
                None => seen.push((finding.clone(), 1, finding.summary.clone())),
            }
        }
    }

    if let Some((exemplar, count, summary)) = seen
        .iter()
        .filter(|(_, count, _)| *count >= STRUCTURAL_THRESHOLD)
        .max_by_key(|(_, count, _)| *count)
    {
        return Convergence::Structural {
            recurring: finding_id(exemplar),
            cycles: *count,
            summary: summary.clone(),
        };
    }

    let blocking: Vec<usize> = cycles
        .iter()
        .map(|cycle| {
            cycle
                .iter()
                .filter(|f| f.severity.blocks_approval())
                .count()
        })
        .collect();
    let outstanding = *blocking.last().unwrap_or(&0);

    // Shrinking against the cycle before it is progress, however slow.
    let improving = blocking
        .windows(2)
        .any(|pair| pair[1] < pair[0]);
    if improving || outstanding == 0 {
        return Convergence::Converging { outstanding };
    }
    if cycles.len() >= STALL_THRESHOLD {
        return Convergence::Stalled {
            outstanding,
            cycles: cycles.len(),
        };
    }
    Convergence::Converging { outstanding }
}

/// Whether a review looks like it reported only part of what it found.
///
/// A reviewer that returns one small thing per cycle turns a single pass into
/// five, each costing a full turn from two bots. This cannot know what was
/// missed, so it flags only the shape that is nearly always a partial report:
/// a single blocking finding on work that has already been rejected before.
pub fn looks_partial(findings: &[Finding], prior_cycles: usize) -> bool {
    let blocking = findings
        .iter()
        .filter(|f| f.severity.blocks_approval())
        .count();
    blocking == 1 && prior_cycles >= 1
}

/// The severities of a security review, which are three different things.
///
/// Collapsing them is how a room ends up asking a person to accept a risk when
/// what it actually needed was to go and gather more evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityDisposition {
    /// The claim is unproven. Work for the room: gather the evidence.
    InsufficientEvidence,
    /// A real finding, to be fixed. Still the room's work.
    SecurityFinding,
    /// Something a person has to weigh and accept. The only one that
    /// legitimately reaches the operator.
    MaterialIncident,
}

impl SecurityDisposition {
    /// Whether this genuinely requires a person.
    pub const fn needs_operator(self) -> bool {
        matches!(self, Self::MaterialIncident)
    }

    /// Whether the room should answer this by producing evidence.
    pub const fn wants_evidence(self) -> bool {
        matches!(self, Self::InsufficientEvidence)
    }

    /// Stable lowercase name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InsufficientEvidence => "insufficient_evidence",
            Self::SecurityFinding => "security_finding",
            Self::MaterialIncident => "material_incident",
        }
    }

    /// Map a finding's severity onto a disposition, conservatively.
    ///
    /// Only a critical finding is treated as material. Anything less is the
    /// room's own work, which is the whole point: missing proof should
    /// generate evidence gathering, not an escalation.
    pub const fn of(severity: Severity) -> Self {
        match severity {
            Severity::Critical => Self::MaterialIncident,
            Severity::Major => Self::SecurityFinding,
            Severity::Note | Severity::Minor => Self::InsufficientEvidence,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn major(summary: &str, locus: Option<&str>) -> Finding {
        Finding {
            severity: Severity::Major,
            summary: summary.into(),
            locus: locus.map(str::to_string),
            suggestion: None,
        }
    }

    fn note(summary: &str) -> Finding {
        Finding {
            severity: Severity::Note,
            summary: summary.into(),
            locus: None,
            suggestion: None,
        }
    }

    /// A reviewer rarely phrases the same objection twice the same way.
    #[test]
    fn the_same_objection_keeps_one_identity_across_rewordings() {
        let first = major("the mutation is unfenced", Some("handler.rs"));
        let reordered = major("unfenced mutation", Some("handler.rs"));
        let padded = major("this mutation is not unfenced here", Some("handler.rs"));

        assert_eq!(finding_id(&first), finding_id(&reordered));
        assert_eq!(finding_id(&first), finding_id(&padded));
    }

    /// Restating an objection with different words around the same nouns is
    /// still the same problem.
    #[test]
    fn a_restated_objection_is_recognised_as_the_same_problem() {
        assert!(same_problem(
            &major("stale async mutation", Some("reducer")),
            &major("mutation stale in the async path", Some("reducer")),
        ));
    }

    /// Synonyms are beyond a keyword match, and pretending otherwise would
    /// merge objections that are genuinely distinct.
    #[test]
    fn different_vocabulary_reads_as_a_different_problem() {
        assert!(!same_problem(
            &major("the lock is absent", Some("x")),
            &major("no mutex guards it", Some("x")),
        ));
    }

    /// Two similar problems in different files are two problems.
    #[test]
    fn the_same_words_in_another_place_is_another_problem() {
        assert!(!same_problem(
            &major("stale async mutation", Some("a.rs")),
            &major("stale async mutation", Some("b.rs")),
        ));
    }

    /// A one-word summary must not collapse into every other one-word summary.
    #[test]
    fn very_short_summaries_keep_distinct_identities() {
        assert_ne!(finding_id(&major("a", None)), finding_id(&major("b", None)));
        assert!(!same_problem(&major("a", None), &major("b", None)));
    }

    #[test]
    fn different_places_are_different_findings() {
        assert_ne!(
            finding_id(&major("the mutation is unfenced", Some("a.rs"))),
            finding_id(&major("the mutation is unfenced", Some("b.rs")))
        );
    }

    /// The exact pattern from the run: the same invariant, patched over and
    /// over in different places, never fixed underneath.
    #[test]
    fn one_invariant_failing_repeatedly_is_called_structural() {
        let cycles = vec![
            vec![major("stale async mutation", Some("reducer"))],
            vec![major("async mutation is stale", Some("reducer"))],
            vec![major("mutation stale in async path", Some("reducer"))],
        ];

        let verdict = assess(&cycles);
        match &verdict {
            Convergence::Structural { cycles, .. } => assert_eq!(*cycles, 3),
            other => panic!("expected structural, got {other:?}"),
        }
        assert!(verdict.needs_escalation());
        let advice = verdict.recommendation().unwrap();
        assert!(advice.contains("Stop patching the symptom"), "{advice}");
    }

    #[test]
    fn a_shrinking_set_is_converging() {
        // Shrinking with turnover. Note that a set which only ever loses
        // members leaves its survivor present in every cycle — which is the
        // structural signal, not this one. Real progress replaces problems as
        // well as removing them.
        let cycles = vec![
            vec![
                major("the parser drops trailing commas", Some("parse.rs")),
                major("the writer emits tabs", Some("write.rs")),
                major("the index is off by one", Some("index.rs")),
            ],
            vec![
                major("the writer emits tabs", Some("write.rs")),
                major("the index is off by one", Some("index.rs")),
            ],
            vec![major("the cache never evicts", Some("cache.rs"))],
        ];
        assert_eq!(
            assess(&cycles),
            Convergence::Converging { outstanding: 1 }
        );
        assert!(!assess(&cycles).needs_escalation());
    }

    /// One invariant that keeps failing is structural even while other work
    /// is being fixed around it — that is precisely the case worth catching.
    #[test]
    fn a_persistent_invariant_is_structural_even_amid_progress() {
        let stubborn = major("stale async mutation", Some("reducer"));
        let cycles = vec![
            vec![
                stubborn.clone(),
                major("the writer emits tabs", Some("write.rs")),
            ],
            vec![stubborn.clone(), major("bad index", Some("index.rs"))],
            vec![stubborn],
        ];
        assert!(matches!(
            assess(&cycles),
            Convergence::Structural { .. }
        ));
    }

    /// Cycles that keep finding just as much are not making progress, and more
    /// of them will not help.
    #[test]
    fn a_set_that_never_shrinks_is_stalled() {
        // Fresh problems every cycle, never fewer of them.
        let cycles = vec![
            vec![
                major("the parser drops commas", Some("1")),
                major("the writer emits tabs", Some("2")),
            ],
            vec![
                major("the index is off by one", Some("3")),
                major("the cache never evicts", Some("4")),
            ],
            vec![
                major("the socket leaks", Some("5")),
                major("the timer drifts", Some("6")),
            ],
        ];
        let verdict = assess(&cycles);
        assert!(
            matches!(verdict, Convergence::Stalled { outstanding: 2, .. }),
            "got {verdict:?}"
        );
        assert!(verdict.recommendation().unwrap().contains("architecture"));
    }

    #[test]
    fn one_cycle_is_not_enough_to_judge() {
        assert_eq!(assess(&[vec![major("a", None)]]), Convergence::TooEarly);
        assert_eq!(assess(&[]), Convergence::TooEarly);
    }

    #[test]
    fn a_clean_review_converges_at_zero() {
        let cycles = vec![vec![major("the index is off by one", Some("x"))], vec![]];
        assert_eq!(
            assess(&cycles),
            Convergence::Converging { outstanding: 0 }
        );
    }

    /// Notes and minor findings do not block, so they do not count toward
    /// either judgement.
    #[test]
    fn non_blocking_findings_are_ignored() {
        let cycles = vec![
            vec![note("spacing is inconsistent"), note("a stray import")],
            vec![note("spacing is inconsistent"), note("a stray import")],
            vec![note("spacing is inconsistent")],
        ];
        assert_eq!(
            assess(&cycles),
            Convergence::Converging { outstanding: 0 }
        );
    }

    /// A reviewer listing the same thing twice in one pass has not shown it
    /// recurring.
    #[test]
    fn a_repeat_within_one_cycle_is_counted_once() {
        let repeated = vec![
            major("stale mutation", Some("x")),
            major("mutation stale", Some("x")),
        ];
        let cycles = vec![repeated.clone(), repeated];
        assert!(
            !matches!(assess(&cycles), Convergence::Structural { .. }),
            "two cycles cannot reach the threshold of three"
        );
    }

    #[test]
    fn one_blocking_finding_after_a_rejection_looks_partial() {
        let one = [major("the index is off by one", Some("x"))];
        assert!(looks_partial(&one, 1));
        assert!(!looks_partial(&one, 0), "a first review may find one thing");
        assert!(!looks_partial(
            &[
                major("the index is off by one", Some("x")),
                major("the cache never evicts", Some("y")),
            ],
            2
        ));
    }

    /// Missing proof is the room's work, not a question for a person.
    #[test]
    fn only_a_critical_security_finding_reaches_the_operator() {
        assert!(!SecurityDisposition::of(Severity::Minor).needs_operator());
        assert!(SecurityDisposition::of(Severity::Minor).wants_evidence());
        assert!(!SecurityDisposition::of(Severity::Major).needs_operator());
        assert!(SecurityDisposition::of(Severity::Critical).needs_operator());
    }

    #[test]
    fn dispositions_round_trip() {
        for disposition in [
            SecurityDisposition::InsufficientEvidence,
            SecurityDisposition::SecurityFinding,
            SecurityDisposition::MaterialIncident,
        ] {
            let json = serde_json::to_string(&disposition).unwrap();
            assert_eq!(
                serde_json::from_str::<SecurityDisposition>(&json).unwrap(),
                disposition
            );
        }
    }
}
