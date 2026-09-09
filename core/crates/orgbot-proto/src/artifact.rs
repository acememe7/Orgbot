//! Artifacts: the deliverables bots produce, hand across, and submit.
//!
//! One rule governs everything here: **an artifact is never silently cut**.
//!
//! A reviewer who reads eight thousand characters of a twenty-thousand
//! character architecture document, sees no sign that anything is missing, and
//! approves it has approved something nobody read. That is worse than a
//! failure, because it looks like success. So a long artifact is handed over
//! in explicitly numbered parts, each one saying which part it is, how many
//! there are, and how to get the rest.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::{AgentId, ArtifactId, TaskId};
use crate::time::Timestamp;

/// How much of an artifact fits in one handover before it must be split.
///
/// Chosen to sit well inside a model's working context while still being
/// large enough that most deliverables arrive whole.
pub const PART_CHARS: usize = 8_000;

/// The payload of an artifact.
///
/// Small results are inlined so a handoff carries its own context with no
/// second fetch; anything large is stored out of band and referenced by path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArtifactBody {
    /// Inline UTF-8 text: prose, code, logs.
    Text { text: String },
    /// Inline structured data.
    Json { value: serde_json::Value },
    /// A file in the room's workspace, relative to the workspace root.
    File { path: String, bytes: u64 },
    /// A unified diff against the workspace.
    Patch { diff: String },
    /// An external reference.
    Url { url: String },
}

/// One numbered piece of an artifact, with everything needed to know it is
/// intact and what comes next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Excerpt {
    /// The content of this part.
    pub text: String,
    /// Which part this is, from 1.
    pub part: u32,
    /// How many parts there are in total.
    pub parts: u32,
    /// Characters in the whole artifact, so a reader can tell at a glance
    /// whether they are holding all of it.
    pub total_chars: usize,
    /// Digest of the whole artifact, so two readers can confirm they read the
    /// same thing.
    pub digest: String,
}

impl Excerpt {
    /// Whether this is the entire artifact.
    pub const fn is_whole(&self) -> bool {
        self.parts == 1
    }

    /// Whether anything follows this part.
    pub const fn has_more(&self) -> bool {
        self.part < self.parts
    }

    /// The artifact as a reader should see it, with explicit boundaries.
    ///
    /// The markers are not decoration. Without them a model cannot tell a
    /// document that ended from one that stopped, and the whole point of
    /// numbering the parts is lost.
    pub fn render(&self, heading: &str) -> String {
        if self.is_whole() {
            return format!(
                "{heading}\nBEGIN ({} characters, sha256 {})\n{}\nEND",
                self.total_chars,
                &self.digest[..12.min(self.digest.len())],
                self.text
            );
        }
        let mut out = format!(
            "{heading}\nBEGIN PART {}/{} ({} characters in total, sha256 {})\n{}\nEND PART {}/{}",
            self.part,
            self.parts,
            self.total_chars,
            &self.digest[..12.min(self.digest.len())],
            self.text,
            self.part,
            self.parts,
        );
        if self.has_more() {
            out.push_str(&format!(
                "\n\nThis is not the whole document. Call read_artifact again \
                 with part={} for the next section.",
                self.part + 1
            ));
        } else {
            out.push_str("\n\nThat was the last part.");
        }
        out
    }
}

impl ArtifactBody {
    /// The artifact's full text, for hashing and splitting.
    ///
    /// A file or URL body has no inline content, so its reference stands in:
    /// there is nothing to truncate, and nothing to mislead a reader about.
    pub fn full_text(&self) -> String {
        match self {
            Self::Text { text } => text.clone(),
            Self::Json { value } => value.to_string(),
            Self::File { path, bytes } => format!("{path} ({bytes} bytes)"),
            Self::Patch { diff } => diff.clone(),
            Self::Url { url } => url.clone(),
        }
    }

    /// A stable digest of the content, for checking that two readers saw the
    /// same thing and that nothing was lost between them.
    pub fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.full_text().as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// How many parts this artifact takes to hand over.
    pub fn parts(&self, limit: usize) -> u32 {
        let chars = self.full_text().chars().count();
        if chars == 0 {
            return 1;
        }
        chars.div_ceil(limit.max(1)) as u32
    }

    /// One numbered part of the artifact.
    ///
    /// Out-of-range parts clamp to the last rather than returning nothing: a
    /// model that miscounts should get the end of the document and a clear
    /// statement that it is the end, not an empty result it might read as
    /// "there was nothing there".
    pub fn excerpt(&self, part: u32, limit: usize) -> Excerpt {
        let full = self.full_text();
        let chars: Vec<char> = full.chars().collect();
        let limit = limit.max(1);
        let parts = self.parts(limit);
        let part = part.clamp(1, parts);
        let start = (part as usize - 1) * limit;
        let text: String = chars.iter().skip(start).take(limit).collect();

        Excerpt {
            text,
            part,
            parts,
            total_chars: chars.len(),
            digest: self.digest(),
        }
    }

    /// A short preview for chat transcripts and list views.
    ///
    /// Truncation is on a character boundary, so this is safe for any UTF-8
    /// content including multi-byte text.
    pub fn preview(&self, max_chars: usize) -> String {
        let full = match self {
            Self::Text { text } => text.clone(),
            Self::Json { value } => value.to_string(),
            Self::File { path, bytes } => format!("{path} ({bytes} bytes)"),
            Self::Patch { diff } => diff.clone(),
            Self::Url { url } => url.clone(),
        };
        if full.chars().count() <= max_chars {
            return full;
        }
        let head: String = full.chars().take(max_chars).collect();
        format!("{head}…")
    }

    /// Whether the content lives in this struct rather than on disk.
    pub const fn is_inline(&self) -> bool {
        matches!(self, Self::Text { .. } | Self::Json { .. } | Self::Patch { .. })
    }

    /// A stable name for the body kind.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Text { .. } => "text",
            Self::Json { .. } => "json",
            Self::File { .. } => "file",
            Self::Patch { .. } => "patch",
            Self::Url { .. } => "url",
        }
    }
}

/// A deliverable produced by a bot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: ArtifactId,
    pub task: TaskId,
    pub author: AgentId,
    /// A short label, e.g. `schema.sql` or `market summary`.
    pub name: String,
    pub body: ArtifactBody,
    /// Which revision of the task produced this. Lets an inspector compare a
    /// resubmission against what it rejected last time.
    pub revision: u32,
    pub created_at: Timestamp,
    /// The artifact this one corrects, if any. The old one stays for audit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<ArtifactId>,
    /// The artifact that replaced this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<ArtifactId>,
    /// Frozen once formal review begins, so the thing being inspected cannot
    /// change under the inspector.
    #[serde(default)]
    pub sealed: bool,
}

impl Artifact {
    /// A new artifact at revision zero.
    pub fn new(task: TaskId, author: AgentId, name: impl Into<String>, body: ArtifactBody) -> Self {
        Self {
            id: ArtifactId::new(),
            task,
            author,
            name: name.into(),
            body,
            revision: 0,
            created_at: Timestamp::now(),
            supersedes: None,
            superseded_by: None,
            sealed: false,
        }
    }

    /// The content's digest, for provenance and completeness checks.
    pub fn digest(&self) -> String {
        self.body.digest()
    }

    /// Whether this artifact still stands as the current version.
    pub const fn is_current(&self) -> bool {
        self.superseded_by.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_truncates_long_text() {
        let body = ArtifactBody::Text {
            text: "a".repeat(100),
        };
        let preview = body.preview(10);
        assert_eq!(preview.chars().count(), 11, "10 chars plus the ellipsis");
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn preview_leaves_short_text_alone() {
        let body = ArtifactBody::Text { text: "short".into() };
        assert_eq!(body.preview(10), "short");
    }

    /// Truncating by bytes would panic here; truncating by chars must not.
    #[test]
    fn preview_does_not_split_multibyte_characters() {
        let body = ArtifactBody::Text {
            text: "日本語のテキストです".into(),
        };
        assert_eq!(body.preview(3), "日本語…");
    }

    #[test]
    fn file_previews_show_path_and_size() {
        let body = ArtifactBody::File {
            path: "out/report.md".into(),
            bytes: 4096,
        };
        assert_eq!(body.preview(64), "out/report.md (4096 bytes)");
        assert!(!body.is_inline());
    }

    #[test]
    fn inline_bodies_are_reported_as_inline() {
        assert!(ArtifactBody::Text { text: String::new() }.is_inline());
        assert!(ArtifactBody::Patch { diff: String::new() }.is_inline());
        assert!(!ArtifactBody::Url { url: String::new() }.is_inline());
    }

    #[test]
    fn artifacts_survive_a_json_round_trip() {
        let artifact = Artifact::new(
            TaskId::new(),
            AgentId::new(),
            "schema.sql",
            ArtifactBody::Json {
                value: serde_json::json!({"tables": ["users"]}),
            },
        );
        let json = serde_json::to_string(&artifact).unwrap();
        assert_eq!(serde_json::from_str::<Artifact>(&json).unwrap(), artifact);
    }
}

#[cfg(test)]
mod completeness_tests {
    use super::*;

    fn long(chars: usize) -> ArtifactBody {
        ArtifactBody::Text {
            text: "x".repeat(chars),
        }
    }

    /// The failure this exists to prevent: a reviewer reading part of a
    /// document, seeing no sign of that, and approving it.
    #[test]
    fn a_long_artifact_never_arrives_looking_whole() {
        let body = long(20_000);
        let first = body.excerpt(1, PART_CHARS);

        assert!(!first.is_whole());
        assert_eq!(first.parts, 3);
        assert_eq!(first.total_chars, 20_000);

        let rendered = first.render("architecture.md:");
        assert!(rendered.contains("BEGIN PART 1/3"));
        assert!(rendered.contains("END PART 1/3"));
        assert!(
            rendered.contains("not the whole document"),
            "the reader must be told, got:\n{rendered}"
        );
        assert!(rendered.contains("part=2"), "and told how to continue");
    }

    #[test]
    fn a_short_artifact_says_it_is_complete() {
        let body = ArtifactBody::Text {
            text: "the whole thing".into(),
        };
        let only = body.excerpt(1, PART_CHARS);

        assert!(only.is_whole());
        assert!(!only.has_more());
        let rendered = only.render("notes.md:");
        assert!(rendered.contains("BEGIN ("));
        assert!(rendered.trim_end().ends_with("END"));
        assert!(!rendered.contains("PART"));
    }

    /// Reading every part in order reconstructs the document exactly.
    #[test]
    fn the_parts_reassemble_into_the_original() {
        let text: String = (0..25_000).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
        let body = ArtifactBody::Text { text: text.clone() };

        let parts = body.parts(PART_CHARS);
        let mut rebuilt = String::new();
        for part in 1..=parts {
            rebuilt.push_str(&body.excerpt(part, PART_CHARS).text);
        }
        assert_eq!(rebuilt, text, "nothing lost and nothing repeated");
    }

    #[test]
    fn the_last_part_says_it_is_the_last() {
        let body = long(20_000);
        let last = body.excerpt(3, PART_CHARS);
        assert!(!last.has_more());
        assert!(last.render("x:").contains("That was the last part"));
    }

    /// A model that miscounts should get the end of the document and be told
    /// so, not an empty result it might read as "there was nothing there".
    #[test]
    fn asking_past_the_end_returns_the_last_part() {
        let body = long(20_000);
        let past = body.excerpt(99, PART_CHARS);
        assert_eq!(past.part, 3);
        assert!(!past.text.is_empty());
        assert!(!past.has_more());
    }

    #[test]
    fn asking_for_part_zero_returns_the_first() {
        assert_eq!(long(20_000).excerpt(0, PART_CHARS).part, 1);
    }

    /// Splitting must not cut a character in half.
    #[test]
    fn multibyte_text_splits_on_character_boundaries() {
        let text = "日本語のテキストです".repeat(500);
        let body = ArtifactBody::Text { text: text.clone() };

        let parts = body.parts(100);
        let mut rebuilt = String::new();
        for part in 1..=parts {
            rebuilt.push_str(&body.excerpt(part, 100).text);
        }
        assert_eq!(rebuilt, text);
    }

    #[test]
    fn an_empty_artifact_is_one_whole_part() {
        let body = ArtifactBody::Text { text: String::new() };
        let only = body.excerpt(1, PART_CHARS);
        assert!(only.is_whole());
        assert_eq!(only.total_chars, 0);
    }

    /// Two readers can confirm they read the same document.
    #[test]
    fn the_digest_covers_the_content_and_travels_with_every_part() {
        let body = long(20_000);
        let digest = body.digest();
        assert_eq!(digest.len(), 64, "sha256 hex");

        for part in 1..=3 {
            assert_eq!(body.excerpt(part, PART_CHARS).digest, digest);
        }

        let different = ArtifactBody::Text {
            text: "y".repeat(20_000),
        };
        assert_ne!(different.digest(), digest);
    }

    #[test]
    fn a_corrected_artifact_supersedes_the_old_one() {
        let mut original = Artifact::new(
            TaskId::new(),
            AgentId::new(),
            "schema.sql",
            ArtifactBody::Text { text: "v1".into() },
        );
        let mut fixed = Artifact::new(
            original.task,
            original.author,
            "schema.sql",
            ArtifactBody::Text { text: "v2".into() },
        );

        fixed.supersedes = Some(original.id);
        original.superseded_by = Some(fixed.id);

        assert!(!original.is_current());
        assert!(fixed.is_current());
        // The old one is still there to read.
        assert_ne!(original.digest(), fixed.digest());
    }

    /// A room written before any of this loads unchanged.
    #[test]
    fn a_legacy_artifact_decodes_with_defaults() {
        let legacy = serde_json::json!({
            "id": ArtifactId::new(),
            "task": TaskId::new(),
            "author": AgentId::new(),
            "name": "old.md",
            "body": {"kind": "text", "text": "from before"},
            "revision": 0,
            "created_at": Timestamp::now(),
        });
        let artifact: Artifact = serde_json::from_value(legacy).unwrap();
        assert!(artifact.is_current());
        assert!(!artifact.sealed);
        assert_eq!(artifact.supersedes, None);
    }
}
