//! Artifacts: the deliverables bots produce, hand across, and submit.

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, ArtifactId, TaskId};
use crate::time::Timestamp;

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

impl ArtifactBody {
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
        }
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
