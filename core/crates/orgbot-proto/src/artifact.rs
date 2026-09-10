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

use crate::ids::{AgentId, ArtifactId, RoomId, TaskId};
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
    /// Bytes in *this part*.
    pub bytes: usize,
    /// Digest of *this part*.
    ///
    /// The whole-document digest cannot tell you which piece you are holding.
    /// A reader that asked for part 2 needs something only the transport could
    /// have produced to confirm it got part 2 — text inside the document
    /// saying "PART 2/2" proves nothing, because the author wrote it.
    pub chunk_digest: String,
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
                "{heading}\nBEGIN ({} characters, sha256 {})\n{}\nEND\n{}",
                self.total_chars,
                &self.digest[..12.min(self.digest.len())],
                self.text,
                self.provenance(),
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
        out.push('\n');
        out.push_str(&self.provenance());
        out
    }

    /// What the transport says about this chunk, as opposed to what the
    /// document says about itself.
    ///
    /// A reader verifies against this line, never against prose in the body.
    /// The author controls the body; only the room controls this.
    pub fn provenance(&self) -> String {
        format!(
            "TRANSPORT part_index={} total_parts={} bytes={} chunk_sha256={} \
             artifact_sha256={} is_final_part={} complete={}",
            self.part,
            self.parts,
            self.bytes,
            &self.chunk_digest[..16.min(self.chunk_digest.len())],
            &self.digest[..16.min(self.digest.len())],
            !self.has_more(),
            self.is_whole(),
        )
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
    pub fn excerpt(&self, part: u32, limit: usize) -> Option<Excerpt> {
        let full = self.full_text();
        let chars: Vec<char> = full.chars().collect();
        let limit = limit.max(1);
        let parts = self.parts(limit);
        // Out of range is an error, not the last part.
        //
        // This used to clamp, on the reasoning that a model which miscounts
        // should get the end of the document rather than an empty result. The
        // cost of that turned out to be far higher than the benefit: asking
        // for part 3 of a two-part document returned part 2 labelled "PART
        // 2/2", which is indistinguishable from having read to the end. A
        // reviewer can approve evidence they never saw, and be shown a
        // document that agrees with them.
        if part == 0 || part > parts {
            return None;
        }
        let start = (part as usize - 1) * limit;
        let text: String = chars.iter().skip(start).take(limit).collect();
        let mut hasher = Sha256::new();
        hasher.update(text.as_bytes());

        Some(Excerpt {
            bytes: text.len(),
            chunk_digest: format!("{:x}", hasher.finalize()),
            text,
            part,
            parts,
            total_chars: chars.len(),
            digest: self.digest(),
        })
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
    /// The room this belongs to, so provenance survives being moved or
    /// exported away from the engine that produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<RoomId>,
    /// The artifact this one continues, when a deliverable was written in
    /// several passes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continues: Option<ArtifactId>,
    /// Whether the content carries authority that cannot be re-derived:
    /// cryptographic material, signatures, public keys, schemas, canonical
    /// registries, protocol constants, wire values.
    ///
    /// Marked so nothing tries to reconstruct it from memory. A near-miss on
    /// prose is a bad sentence; a near-miss on a public key is a system that
    /// looks like it works and does not.
    #[serde(default)]
    pub authority_bearing: bool,
    /// What review made of it, once anyone has looked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposition: Option<String>,
    /// The claim this artifact exists to support, if it is evidence.
    ///
    /// Evidence is a thing you attach, not a job someone does. Recording the
    /// claim here means a reviewer can find the proof for an assertion without
    /// a task having been raised to represent it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_for: Option<String>,
}

/// One part of a split artifact, with what is needed to know it is intact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    /// Which part this is, from 1.
    pub index: u32,
    /// How many parts there are.
    pub total: u32,
    /// Characters in this part.
    pub chars: usize,
    /// Digest of this part alone.
    pub digest: String,
}

/// The reading order and integrity of a split artifact.
///
/// The point is not the structure but the promise: if you have every part
/// listed here and each one's digest matches, you have the whole document. A
/// reader with no manifest can only hope it reached the end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The artifact this describes.
    pub artifact: ArtifactId,
    /// Its name.
    pub name: String,
    /// Parts in reading order.
    pub parts: Vec<Part>,
    /// Characters in the whole document.
    pub total_chars: usize,
    /// Digest of the whole document, so the reassembly can be checked.
    pub digest: String,
    /// Whether every part is present and accounted for.
    pub complete: bool,
}

impl Manifest {
    /// The manifest as a reader should see it.
    pub fn render(&self) -> String {
        let mut out = format!(
            "MANIFEST {} ({})\n{} parts, {} characters, sha256 {}\n",
            self.name,
            self.artifact,
            self.parts.len(),
            self.total_chars,
            &self.digest[..12.min(self.digest.len())],
        );
        for part in &self.parts {
            out.push_str(&format!(
                "  part {}/{}  {} chars  sha256 {}\n",
                part.index,
                part.total,
                part.chars,
                &part.digest[..12.min(part.digest.len())],
            ));
        }
        out.push_str(if self.complete {
            "COMPLETE — reading every part in order gives the whole document."
        } else {
            "INCOMPLETE — parts are missing. Do not treat this as the document."
        });
        out
    }
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
            room: None,
            continues: None,
            authority_bearing: false,
            disposition: None,
            evidence_for: None,
        }
    }

    /// The content's digest, for provenance and completeness checks.
    pub fn digest(&self) -> String {
        self.body.digest()
    }

    /// Whether this artifact is proof of something rather than a deliverable.
    pub const fn is_evidence(&self) -> bool {
        self.evidence_for.is_some()
    }

    /// Whether this artifact still stands as the current version.
    pub const fn is_current(&self) -> bool {
        self.superseded_by.is_none()
    }

    /// The reading order and integrity of this artifact, split at `limit`.
    pub fn manifest(&self, limit: usize) -> Manifest {
        let total = self.body.parts(limit);
        let parts = (1..=total)
            .map(|index| {
                let excerpt = self
                    .body
                    .excerpt(index, limit)
                    .expect("indices run 1..=total, which are in range by construction");
                Part {
                    index,
                    total,
                    chars: excerpt.text.chars().count(),
                    digest: excerpt.chunk_digest,
                }
            })
            .collect::<Vec<Part>>();
        let total_chars = parts.iter().map(|part| part.chars).sum();
        Manifest {
            artifact: self.id,
            name: self.name.clone(),
            parts,
            // Every part was produced from the content in hand, so this
            // manifest is complete by construction. A manifest read back from
            // storage with a part missing is the case this flag exists for.
            complete: total_chars == self.body.full_text().chars().count(),
            total_chars,
            digest: self.digest(),
        }
    }

    /// Whether handing this over will take more than one part.
    ///
    /// Asked before writing rather than discovered on reading, so a bot can
    /// decide to split a deliverable itself rather than have it split under it.
    pub fn needs_splitting(&self, limit: usize) -> bool {
        self.body.parts(limit) > 1
    }

    /// What to ask the originating author for when this is lost or truncated.
    ///
    /// Addressed to the bot that wrote it, because it is the one that has the
    /// content. Asking a person for something the room produced is the wrong
    /// question, and asking any other bot invites a plausible reconstruction
    /// of something that has to be exact.
    pub fn recovery_request(&self, limit: usize) -> String {
        let manifest = self.manifest(limit);
        let exactness = if self.authority_bearing {
            "\n\nThis artifact carries authority that cannot be re-derived. Do \
             not reconstruct it from memory or infer any part of it: re-emit \
             the exact bytes, or say plainly that you no longer have them."
        } else {
            ""
        };
        format!(
            "Re-emit `{}` ({}), which you authored.\n\nUse this format:\n\
             BEGIN\npart 1\npart 2\n...\nEND\n\n\
             In reading order, {} part(s), {} characters in total. The whole \
             document should hash to sha256 {}.{exactness}",
            self.name,
            self.id,
            manifest.parts.len(),
            manifest.total_chars,
            &manifest.digest[..12.min(manifest.digest.len())],
        )
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
        let first = body.excerpt(1, PART_CHARS).expect("part 1 exists");

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
        let only = body.excerpt(1, PART_CHARS).expect("the only part");

        assert!(only.is_whole());
        assert!(!only.has_more());
        let rendered = only.render("notes.md:");
        assert!(rendered.contains("BEGIN ("));
        assert!(rendered.contains("\nEND\n"), "the body still closes: {rendered}");
        assert!(
            !rendered.contains("BEGIN PART"),
            "a whole document is not announced as a part: {rendered}"
        );
        // A single-part read still carries transport identity, so a reader
        // never has to decide whether to trust the metadata based on how long
        // the document happened to be.
        assert!(rendered.contains("TRANSPORT part_index=1 total_parts=1"));
        assert!(rendered.contains("is_final_part=true complete=true"));
    }

    /// Reading every part in order reconstructs the document exactly.
    #[test]
    fn the_parts_reassemble_into_the_original() {
        let text: String = (0..25_000).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
        let body = ArtifactBody::Text { text: text.clone() };

        let parts = body.parts(PART_CHARS);
        let mut rebuilt = String::new();
        for part in 1..=parts {
            rebuilt.push_str(&body.excerpt(part, PART_CHARS).expect("in range").text);
        }
        assert_eq!(rebuilt, text, "nothing lost and nothing repeated");
    }

    #[test]
    fn the_last_part_says_it_is_the_last() {
        let body = long(20_000);
        let last = body.excerpt(3, PART_CHARS).expect("the last part");
        assert!(!last.has_more());
        assert!(last.render("x:").contains("That was the last part"));
    }

    /// Asking past the end is an error, not the last part.
    ///
    /// This test used to assert the opposite, on the reasoning that a model
    /// which miscounts is better served the end of the document than an empty
    /// result. That reasoning is wrong in the one case that matters: part 3 of
    /// a three-part document and part 99 of it come back identical, so a
    /// reader cannot tell "I have reached the end" from "I asked for something
    /// that does not exist". A reviewer checking they had the whole document
    /// by reading until the parts stopped changing would have been told yes.
    #[test]
    fn asking_past_the_end_is_refused_rather_than_answered_with_the_last_part() {
        let body = long(20_000);
        assert_eq!(body.parts(PART_CHARS), 3);
        assert!(body.excerpt(4, PART_CHARS).is_none());
        assert!(body.excerpt(99, PART_CHARS).is_none());
        assert!(body.excerpt(u32::MAX, PART_CHARS).is_none());
        assert!(body.excerpt(0, PART_CHARS).is_none(), "nor part zero");
        assert!(body.excerpt(3, PART_CHARS).is_some(), "but the last is fine");
    }

    /// Every part can be told apart from every other by something the author
    /// did not write.
    #[test]
    fn each_part_carries_its_own_digest_and_size() {
        let text: String = (0..20_000).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
        let body = ArtifactBody::Text { text };
        let one = body.excerpt(1, PART_CHARS).unwrap();
        let two = body.excerpt(2, PART_CHARS).unwrap();

        assert_ne!(
            one.chunk_digest, two.chunk_digest,
            "two different parts must not hash the same"
        );
        assert_eq!(one.digest, two.digest, "but the document digest is shared");
        assert_eq!(one.bytes, one.text.len());

        let rendered = two.render("x:");
        assert!(rendered.contains("TRANSPORT part_index=2 total_parts=3"), "{rendered}");
        assert!(rendered.contains("is_final_part=false"));
        assert!(rendered.contains(&two.chunk_digest[..16]));
    }

    #[test]
    fn part_zero_is_not_a_part() {
        assert!(long(20_000).excerpt(0, PART_CHARS).is_none());
    }

    /// Splitting must not cut a character in half.
    #[test]
    fn multibyte_text_splits_on_character_boundaries() {
        let text = "日本語のテキストです".repeat(500);
        let body = ArtifactBody::Text { text: text.clone() };

        let parts = body.parts(100);
        let mut rebuilt = String::new();
        for part in 1..=parts {
            rebuilt.push_str(&body.excerpt(part, 100).expect("in range").text);
        }
        assert_eq!(rebuilt, text);
    }

    #[test]
    fn an_empty_artifact_is_one_whole_part() {
        let body = ArtifactBody::Text { text: String::new() };
        let only = body.excerpt(1, PART_CHARS).expect("the only part");
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
            assert_eq!(body.excerpt(part, PART_CHARS).unwrap().digest, digest);
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

#[cfg(test)]
mod recovery_tests {
    use super::*;

    fn artifact(text: &str) -> Artifact {
        Artifact::new(
            TaskId::new(),
            AgentId::new(),
            "schema.json",
            ArtifactBody::Text { text: text.into() },
        )
    }

    /// The manifest's promise: every part listed, each digest matching, means
    /// you have the whole document.
    #[test]
    fn a_manifest_lists_every_part_in_reading_order() {
        // Varied content, so each part genuinely differs. A document of one
        // repeated character has identical parts, and identical digests for
        // them is correct rather than a fault.
        let varied: String = (0..20_000)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let manifest = artifact(&varied).manifest(PART_CHARS);

        assert_eq!(manifest.parts.len(), 3);
        assert!(manifest.complete);
        assert_eq!(manifest.total_chars, 20_000);
        for (position, part) in manifest.parts.iter().enumerate() {
            assert_eq!(part.index as usize, position + 1);
            assert_eq!(part.total, 3);
            assert_eq!(part.digest.len(), 64);
        }
        // The parts hash differently from one another and from the whole.
        assert_ne!(manifest.parts[0].digest, manifest.parts[1].digest);
        assert_ne!(manifest.parts[0].digest, manifest.digest);
    }

    #[test]
    fn a_manifest_says_plainly_whether_it_is_complete() {
        let rendered = artifact(&"x".repeat(20_000)).manifest(PART_CHARS).render();
        assert!(rendered.contains("MANIFEST"));
        assert!(rendered.contains("part 1/3"));
        assert!(rendered.contains("COMPLETE"));
        assert!(!rendered.contains("INCOMPLETE"));
    }

    /// Asked before writing, so a bot can split a deliverable itself rather
    /// than have it split under it.
    #[test]
    fn splitting_is_predicted_not_discovered() {
        assert!(!artifact("short").needs_splitting(PART_CHARS));
        assert!(artifact(&"x".repeat(20_000)).needs_splitting(PART_CHARS));
    }

    /// The room produced it; the room should be asked for it back.
    #[test]
    fn a_recovery_request_is_addressed_to_the_author_with_a_format() {
        let request = artifact(&"x".repeat(20_000)).recovery_request(PART_CHARS);

        assert!(request.contains("which you authored"));
        assert!(request.contains("BEGIN"));
        assert!(request.contains("END"));
        assert!(request.contains("3 part(s)"));
        assert!(request.contains("20000 characters"));
        assert!(request.contains("sha256"), "so the re-emission can be checked");
    }

    /// A near-miss on prose is a bad sentence. A near-miss on a public key is
    /// a system that looks like it works and does not.
    #[test]
    fn authority_bearing_content_forbids_reconstruction_explicitly() {
        let mut key = artifact("-----BEGIN PUBLIC KEY-----");
        key.authority_bearing = true;

        let request = key.recovery_request(PART_CHARS);
        assert!(request.contains("cannot be re-derived"));
        assert!(request.contains("Do not reconstruct it from memory"));
        assert!(request.contains("say plainly that you no longer have them"));
    }

    #[test]
    fn ordinary_content_is_not_lectured_about_exactness() {
        let request = artifact("some prose").recovery_request(PART_CHARS);
        assert!(!request.contains("cannot be re-derived"));
    }

    #[test]
    fn provenance_survives_a_round_trip() {
        let mut record = artifact("content");
        record.room = Some(RoomId::new());
        record.authority_bearing = true;
        record.continues = Some(ArtifactId::new());
        record.disposition = Some("approved".into());

        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(serde_json::from_str::<Artifact>(&json).unwrap(), record);
    }
}
