//! Inspection: the gate between "a bot says it is done" and "the work ships".
//!
//! Inspection is a separate step performed by a bot other than the author. The
//! engine enforces both halves of that rule — the author cannot be the
//! inspector, and only a bot with `may_inspect` can record a verdict.

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, ArtifactId, ReviewId, TaskId};
use crate::time::Timestamp;

/// How serious a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Worth mentioning; does not block approval.
    Note,
    /// Should be fixed, but the work is usable.
    Minor,
    /// Blocks approval.
    Major,
    /// Blocks approval and suggests the task was misunderstood.
    Critical,
}

impl Severity {
    /// Whether a finding at this severity prevents approval.
    pub const fn blocks_approval(self) -> bool {
        matches!(self, Self::Major | Self::Critical)
    }
}

/// One specific problem found during inspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    /// What is wrong.
    pub summary: String,
    /// Where: an artifact, a criterion, or free text.
    pub locus: Option<String>,
    /// What the author should do about it.
    pub suggestion: Option<String>,
}

impl Finding {
    /// A blocking finding.
    pub fn major(summary: impl Into<String>) -> Self {
        Self {
            severity: Severity::Major,
            summary: summary.into(),
            locus: None,
            suggestion: None,
        }
    }

    /// A non-blocking observation.
    pub fn note(summary: impl Into<String>) -> Self {
        Self {
            severity: Severity::Note,
            summary: summary.into(),
            locus: None,
            suggestion: None,
        }
    }
}

/// The inspector's decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The work meets the acceptance criteria.
    Approve,
    /// The work needs revision; the task returns to its author.
    RequestChanges,
    /// The work is unsalvageable; the task fails.
    Reject,
}

/// A completed inspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub id: ReviewId,
    pub task: TaskId,
    /// The bot that performed the inspection.
    pub inspector: AgentId,
    /// The task revision that was inspected.
    pub revision: u32,
    pub verdict: Verdict,
    pub findings: Vec<Finding>,
    /// The inspector's prose summary, shown in chat.
    pub summary: String,
    /// Which acceptance criteria were checked and passed.
    pub criteria_met: Vec<String>,
    /// Which acceptance criteria failed.
    pub criteria_failed: Vec<String>,
    /// The artifacts that were inspected.
    pub inspected: Vec<ArtifactId>,
    pub at: Timestamp,
}

impl Review {
    /// Record an inspection, deriving the verdict from the findings.
    ///
    /// The verdict is computed rather than supplied so an inspector cannot
    /// approve work while simultaneously reporting a blocking defect.
    pub fn from_findings(
        task: TaskId,
        inspector: AgentId,
        revision: u32,
        findings: Vec<Finding>,
        summary: impl Into<String>,
    ) -> Self {
        let verdict = if findings.iter().any(|f| f.severity == Severity::Critical) {
            Verdict::Reject
        } else if findings.iter().any(|f| f.severity.blocks_approval()) {
            Verdict::RequestChanges
        } else {
            Verdict::Approve
        };
        Self {
            id: ReviewId::new(),
            task,
            inspector,
            revision,
            verdict,
            findings,
            summary: summary.into(),
            criteria_met: Vec::new(),
            criteria_failed: Vec::new(),
            inspected: Vec::new(),
            at: Timestamp::now(),
        }
    }

    /// The blocking findings, in descending severity.
    pub fn blockers(&self) -> Vec<&Finding> {
        let mut blockers: Vec<&Finding> = self
            .findings
            .iter()
            .filter(|f| f.severity.blocks_approval())
            .collect();
        blockers.sort_by(|a, b| b.severity.cmp(&a.severity));
        blockers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review(findings: Vec<Finding>) -> Review {
        Review::from_findings(TaskId::new(), AgentId::new(), 0, findings, "checked it")
    }

    #[test]
    fn clean_work_is_approved() {
        assert_eq!(review(vec![]).verdict, Verdict::Approve);
    }

    #[test]
    fn notes_alone_do_not_block_approval() {
        let r = review(vec![Finding::note("could be tidier")]);
        assert_eq!(r.verdict, Verdict::Approve);
        assert!(r.blockers().is_empty());
    }

    #[test]
    fn a_major_finding_forces_rework() {
        let r = review(vec![Finding::note("nit"), Finding::major("missing tests")]);
        assert_eq!(r.verdict, Verdict::RequestChanges);
        assert_eq!(r.blockers().len(), 1);
    }

    #[test]
    fn a_critical_finding_rejects_outright() {
        let r = review(vec![Finding {
            severity: Severity::Critical,
            summary: "solved the wrong problem".into(),
            locus: None,
            suggestion: None,
        }]);
        assert_eq!(r.verdict, Verdict::Reject);
    }

    /// The verdict is derived, so "approve" alongside a blocking defect is not
    /// representable.
    #[test]
    fn an_inspector_cannot_approve_over_its_own_blocker() {
        let r = review(vec![Finding::major("data loss on retry")]);
        assert_ne!(r.verdict, Verdict::Approve);
    }

    #[test]
    fn blockers_are_ordered_worst_first() {
        let r = review(vec![
            Finding::major("minor-ish"),
            Finding {
                severity: Severity::Critical,
                summary: "worst".into(),
                locus: None,
                suggestion: None,
            },
        ]);
        assert_eq!(r.blockers()[0].severity, Severity::Critical);
    }

    #[test]
    fn severity_ordering_is_ascending() {
        assert!(Severity::Note < Severity::Minor);
        assert!(Severity::Minor < Severity::Major);
        assert!(Severity::Major < Severity::Critical);
    }
}
