//! What earlier rooms already built, so a new one does not build it again.
//!
//! A room finishes and its board is saved: the tasks, what was delivered, and
//! the artifacts themselves. Then the next room starts with none of it. One
//! room here had nine hundred and fifty-five artifacts and a hundred and
//! eighty-eight reviews, and every one of them was invisible to the room
//! opened after it. Work gets rebuilt, decisions get re-argued, and the same
//! reviewer finding gets written twice by two bots who could not have known.
//!
//! This is deliberately **not** an MCP server, which is where the idea
//! started. A server would run on the connected computer, and the boards live
//! on the device — the phone cannot start a process and the computer cannot
//! see the files. Reaching for the protocol here would mean the device talking
//! to itself through a transport it does not have. These are ordinary room
//! tools reading ordinary files, which is faster, works where the data is, and
//! has one fewer thing that can be down.
//!
//! The index is summaries only. A state file is tens of megabytes and a body
//! is fetched by the bot that decided it wanted it, on the turn it wants it —
//! the same split that makes skills cheap. What a search returns is enough to
//! judge relevance and nothing more.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// One deliverable an earlier room produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriorArtifact {
    /// Its id in the room that made it.
    pub id: String,
    /// What it was called.
    pub name: String,
}

/// One piece of work an earlier room did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriorTask {
    /// What it set out to do.
    pub title: String,
    /// The instruction it was given.
    pub brief: String,
    /// Whether it was actually delivered, or abandoned.
    pub delivered: bool,
    /// What came out of it.
    pub artifacts: Vec<PriorArtifact>,
}

/// An earlier room, distilled to what is worth searching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriorRoom {
    /// Its name.
    pub room: String,
    /// What it was for.
    pub goal: String,
    /// Where its board is saved, for fetching a body later.
    pub path: PathBuf,
    /// The work it did.
    pub tasks: Vec<PriorTask>,
}

/// A match worth showing a bot.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    /// The room it came from.
    pub room: String,
    /// The work.
    pub task: PriorTask,
    /// How closely it matched, from zero to one.
    pub closeness: f32,
}

/// Everything earlier rooms left behind.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Archive {
    rooms: Vec<PriorRoom>,
    /// Shared project memory is not part of the room checkpoint. The server
    /// configures it from the explicit project scope after opening a room.
    #[serde(skip, default)]
    memory: Arc<Mutex<Option<crate::memory::MemoryRepository>>>,
}

/// How many matches a search returns.
///
/// Enough to see that something was done before and by whom; not so many that
/// the answer is itself a wall of text to read.
const MATCH_LIMIT: usize = 6;

impl Archive {
    /// Build an index from saved boards, skipping the room being opened.
    ///
    /// A board that cannot be read is skipped rather than failing: these are
    /// other rooms' files, and one of them being mid-write or from a newer
    /// version of the app is not a reason this room cannot start.
    pub fn load(boards: &[PathBuf], exclude: Option<&str>) -> Self {
        let mut rooms = Vec::new();
        for path in boards {
            match distil(path) {
                Some(room) if Some(room.room.as_str()) != exclude => rooms.push(room),
                _ => {}
            }
        }
        Self {
            rooms,
            ..Self::default()
        }
    }

    /// The rooms held.
    pub fn rooms(&self) -> &[PriorRoom] {
        &self.rooms
    }

    /// Whether anything was found.
    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }

    /// Attach a local memory store for an explicitly supplied project scope.
    ///
    /// This is called by the local server after room configuration is checked.
    /// An empty project scope is rejected instead of widening visibility.
    pub fn configure_memory(
        &self,
        root: impl AsRef<Path>,
        project_scope: &str,
    ) -> Result<(), crate::memory::MemoryError> {
        let repository = crate::memory::MemoryRepository::open(root, project_scope)?;
        let mut slot = self
            .memory
            .lock()
            .map_err(|_| std::io::Error::other("memory lock poisoned"))?;
        *slot = Some(repository);
        Ok(())
    }

    /// Whether this room has an explicitly configured project memory store.
    pub fn has_memory(&self) -> bool {
        self.memory.lock().ok().is_some_and(|slot| slot.is_some())
    }

    /// Clone the configured project repository handle without doing memory
    /// I/O. Callers can move it to a blocking worker after releasing the
    /// engine lock; no scope is inferred when memory is unconfigured.
    pub fn memory_repository(
        &self,
    ) -> Result<Option<crate::memory::MemoryRepository>, crate::memory::MemoryError> {
        let slot = self
            .memory
            .lock()
            .map_err(|_| std::io::Error::other("memory lock poisoned"))?;
        Ok(slot.clone())
    }

    /// Search shared notes in the configured project. No configured scope is
    /// treated as an empty result rather than a global fallback.
    pub fn search_memory(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::memory::MemoryHit>, crate::memory::MemoryError> {
        let slot = self
            .memory
            .lock()
            .map_err(|_| std::io::Error::other("memory lock poisoned"))?;
        match slot.as_ref() {
            Some(repository) => repository.search(query, limit),
            None => Ok(Vec::new()),
        }
    }

    /// Save a note to the active project, optionally tying it to completed
    /// room work. Automatic extraction supplies the task metadata directly.
    pub fn remember(
        &self,
        title: &str,
        content: &str,
        source_room: Option<&str>,
        source_task: Option<&str>,
    ) -> Result<crate::memory::MemoryEntry, crate::memory::MemoryError> {
        let slot = self
            .memory
            .lock()
            .map_err(|_| std::io::Error::other("memory lock poisoned"))?;
        let repository = slot
            .as_ref()
            .ok_or(crate::memory::MemoryError::InvalidScope)?;
        repository.add(title, content, source_room, source_task)
    }

    /// Return entries for the owner-facing project memory view.
    pub fn memory_entries(
        &self,
    ) -> Result<Vec<crate::memory::MemoryEntry>, crate::memory::MemoryError> {
        let slot = self
            .memory
            .lock()
            .map_err(|_| std::io::Error::other("memory lock poisoned"))?;
        match slot.as_ref() {
            Some(repository) => repository.list(),
            None => Ok(Vec::new()),
        }
    }

    /// Change visibility or delete a note in the current project. These
    /// methods are for owner-controlled server operations, not model tools.
    pub fn set_memory_shared(
        &self,
        id: &str,
        shared: bool,
    ) -> Result<crate::memory::MemoryEntry, crate::memory::MemoryError> {
        let slot = self
            .memory
            .lock()
            .map_err(|_| std::io::Error::other("memory lock poisoned"))?;
        slot.as_ref()
            .ok_or(crate::memory::MemoryError::InvalidScope)?
            .set_shared(id, shared)
    }

    /// Permanently delete a note from the current project.
    pub fn delete_memory(&self, id: &str) -> Result<(), crate::memory::MemoryError> {
        let slot = self
            .memory
            .lock()
            .map_err(|_| std::io::Error::other("memory lock poisoned"))?;
        slot.as_ref()
            .ok_or(crate::memory::MemoryError::InvalidScope)?
            .delete(id)
    }

    /// Copy one note to another explicitly named project scope.
    pub fn share_memory_to_project(
        &self,
        id: &str,
        root: impl AsRef<Path>,
        target_scope: &str,
    ) -> Result<crate::memory::MemoryEntry, crate::memory::MemoryError> {
        let slot = self
            .memory
            .lock()
            .map_err(|_| std::io::Error::other("memory lock poisoned"))?;
        slot.as_ref()
            .ok_or(crate::memory::MemoryError::InvalidScope)?
            .share_to_project(id, root, target_scope)
    }

    /// Extract a completed task into the current project's shared knowledge
    /// store without a model call.
    pub fn capture_completed_work(
        &self,
        room: &str,
        task: &str,
        title: &str,
        brief: &str,
        decisions: &[String],
        artifact_names: &[String],
    ) -> Result<crate::memory::MemoryEntry, crate::memory::MemoryError> {
        let content = crate::memory::completed_work_content(brief, decisions, artifact_names);
        self.remember(title, &content, Some(room), Some(task))
    }

    /// Work resembling `query`, closest first.
    ///
    /// Matched on the title and brief together, because a title alone is often
    /// a code — `N1-C0` says nothing — and the brief is where the subject
    /// actually appears.
    pub fn search(&self, query: &str) -> Vec<Match> {
        let mut asked: Vec<String> = orgbot_proto::text::content_words(query)
            .iter()
            .map(|word| singular(word))
            .collect();
        asked.sort();
        asked.dedup();

        let mut found: Vec<Match> = Vec::new();
        for room in &self.rooms {
            for task in &room.tasks {
                let subject = format!("{} {}", task.title, task.brief);
                let closeness = covers(&asked, &subject);
                if closeness < RELEVANCE_FLOOR {
                    continue;
                }
                found.push(Match {
                    room: room.room.clone(),
                    task: task.clone(),
                    closeness,
                });
            }
        }
        found.sort_by(|a, b| {
            b.closeness
                .partial_cmp(&a.closeness)
                .unwrap_or(std::cmp::Ordering::Equal)
                // Something delivered is worth more than something abandoned.
                .then(b.task.delivered.cmp(&a.task.delivered))
        });
        found.truncate(MATCH_LIMIT);
        found
    }

    /// The full text of one earlier artifact.
    ///
    /// Re-reads that room's board, because bodies are the part deliberately
    /// left out of the index.
    pub fn artifact(&self, id: &str) -> Option<(String, String)> {
        for room in &self.rooms {
            if !room
                .tasks
                .iter()
                .any(|task| task.artifacts.iter().any(|a| a.id == id))
            {
                continue;
            }
            let raw = std::fs::read_to_string(&room.path).ok()?;
            let saved: serde_json::Value = serde_json::from_str(&raw).ok()?;
            let artifacts = saved
                .get("engine")
                .unwrap_or(&saved)
                .get("artifacts")?
                .as_object()?;
            let artifact = artifacts.get(id)?;
            let name = artifact
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("untitled")
                .to_string();
            let body = artifact
                .get("body")
                .and_then(|body| {
                    body.get("text")
                        .or_else(|| body.get("diff"))
                        .or_else(|| body.get("url"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                        .or_else(|| body.get("value").map(|v| v.to_string()))
                })
                .unwrap_or_default();
            return Some((name, body));
        }
        None
    }
}

/// How much of `query` the `subject` actually covers, from zero to one.
///
/// Not Jaccard, which is what this first used and got wrong in a way only real
/// data showed. Jaccard divides by the union, so a long brief is penalised for
/// being long: a task titled "multi-part artifacts cannot be read past part 1"
/// with a detailed brief scored 0.03 against a query that named it almost
/// exactly, and unrelated work scored the same. Every search came back full of
/// noise that happened to share a word.
///
/// The question a bot is asking is "is this about what I asked", not "are
/// these two texts alike". So: what fraction of the query's own words appear
/// in the subject. A long brief can only help.
fn covers(query_words: &[String], subject: &str) -> f32 {
    if query_words.is_empty() {
        return 0.0;
    }
    let mut subject_words: Vec<String> = orgbot_proto::text::content_words(subject)
        .iter()
        .map(|word| singular(word))
        .collect();
    subject_words.sort();
    subject_words.dedup();
    let shared = query_words
        .iter()
        .filter(|word| subject_words.binary_search(word).is_ok())
        .count();
    shared as f32 / query_words.len() as f32
}

/// Fold a plural onto its singular, so `artifact` finds `artifacts`.
///
/// The crudest possible stemming, and worth exactly this much: a bot asking
/// about "artifacts" and work titled "artifact" were missing each other
/// entirely, which is the single most common way an English search fails.
/// Anything cleverer would need a dictionary to earn its keep, and would start
/// matching words that only look alike.
fn singular(word: &str) -> String {
    match word.strip_suffix('s') {
        Some(stem) if stem.len() > 3 && !stem.ends_with('s') => stem.to_string(),
        _ => word.to_string(),
    }
}

/// How much of a question a piece of work must answer to be worth offering.
///
/// A third of the asked-about words. Lower and a bot spends a read discovering
/// a false lead, which is the same tax that makes an over-eager notification
/// worse than none; higher and genuinely related work phrased differently is
/// never found.
const RELEVANCE_FLOOR: f32 = 0.34;

/// Read one saved board down to what is worth keeping.
fn distil(path: &Path) -> Option<PriorRoom> {
    let raw = std::fs::read_to_string(path).ok()?;
    let saved: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let engine = saved.get("engine").unwrap_or(&saved);
    let room = engine.get("room")?;
    let spec = room.get("spec")?;

    // Artifacts grouped by the task that produced them, so a match can say
    // what came out of it without a second lookup.
    let mut by_task: BTreeMap<String, Vec<PriorArtifact>> = BTreeMap::new();
    if let Some(artifacts) = engine.get("artifacts").and_then(|a| a.as_object()) {
        for (id, artifact) in artifacts {
            let Some(task) = artifact.get("task").and_then(serde_json::Value::as_str) else {
                continue;
            };
            by_task
                .entry(task.to_string())
                .or_default()
                .push(PriorArtifact {
                    id: id.clone(),
                    name: artifact
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("untitled")
                        .to_string(),
                });
        }
    }

    let mut tasks = Vec::new();
    if let Some(board) = engine.get("tasks").and_then(|t| t.as_object()) {
        for (id, task) in board {
            let state = task
                .get("state")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            // Drafts and work nobody finished are not worth offering as prior
            // art; what is worth offering is what was actually produced.
            let delivered = state == "submitted";
            let artifacts = by_task.get(id).cloned().unwrap_or_default();
            if !delivered && artifacts.is_empty() {
                continue;
            }
            tasks.push(PriorTask {
                title: task
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                brief: task
                    .get("brief")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                delivered,
                artifacts,
            });
        }
    }

    Some(PriorRoom {
        room: spec
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("a room")
            .to_string(),
        goal: spec
            .get("goal")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
        path: path.to_path_buf(),
        tasks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(name: &str, tasks: serde_json::Value, artifacts: serde_json::Value) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "orgbot-archive-{}-{}.json",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let saved = serde_json::json!({
            "engine": {
                "room": { "spec": { "name": name, "goal": "ship things" } },
                "tasks": tasks,
                "artifacts": artifacts,
            }
        });
        std::fs::write(&path, serde_json::to_string(&saved).unwrap()).unwrap();
        path
    }

    #[test]
    fn work_from_an_earlier_room_can_be_found_by_what_it_was_about() {
        let path = board(
            "Release",
            serde_json::json!({
                "task_1": {
                    "title": "OAuth device code sign-in",
                    "brief": "add a device code flow for the desktop client",
                    "state": "submitted"
                }
            }),
            serde_json::json!({
                "art_1": { "task": "task_1", "name": "device_auth.md",
                           "body": {"kind": "text", "text": "poll the token endpoint"} }
            }),
        );
        let archive = Archive::load(std::slice::from_ref(&path), None);

        let hits = archive.search("implement device code oauth sign in");
        assert_eq!(hits.len(), 1, "the earlier work should surface");
        assert_eq!(hits[0].room, "Release");
        assert!(hits[0].task.delivered);
        assert_eq!(hits[0].task.artifacts[0].name, "device_auth.md");

        // And the deliverable itself is fetchable, but only when asked for.
        let (name, body) = archive.artifact("art_1").expect("the artifact is readable");
        assert_eq!(name, "device_auth.md");
        assert!(body.contains("poll the token endpoint"));
        let _ = std::fs::remove_file(path);
    }

    /// A detailed brief must not bury the work it describes.
    ///
    /// This is the bug real data found. Scoring by Jaccard divides by the
    /// union, so the more a task explains itself the worse it matches: a task
    /// named almost exactly by the query scored 0.03, the same as work sharing
    /// one incidental word, and every search came back full of noise. Asking
    /// "how much of what I asked does this cover" is the question a bot
    /// actually has, and a long brief can only help answer it.
    #[test]
    fn a_long_brief_helps_a_match_rather_than_diluting_it() {
        let path = board(
            "Release",
            serde_json::json!({
                "wordy": {
                    "title": "multi-part artifacts cannot be read past part 1",
                    "brief": "The transport clamps an out-of-range part to the last \
                              one. Reviewers approve evidence they never saw. This \
                              brief goes on at some length about the boundary \
                              conditions, the probes run, and the regression shape \
                              that would catch it, none of which should count \
                              against the match.",
                    "state": "submitted"
                },
                "incidental": {
                    "title": "part numbering for release notes",
                    "brief": "cosmetic",
                    "state": "submitted"
                }
            }),
            serde_json::json!({}),
        );
        let archive = Archive::load(std::slice::from_ref(&path), None);
        let hits = archive.search("multi-part artifact pagination read defect");

        assert!(
            !hits.is_empty(),
            "the work named by the query must be found"
        );
        assert_eq!(
            hits[0].task.title,
            "multi-part artifacts cannot be read past part 1"
        );
        assert!(
            hits[0].closeness > 0.5,
            "a near-exact match should score like one, got {:.2}",
            hits[0].closeness
        );
        let _ = std::fs::remove_file(path);
    }

    /// A false lead costs a read to dismiss, so the floor has to hold.
    #[test]
    fn work_sharing_one_incidental_word_is_below_the_floor() {
        let path = board(
            "Release",
            serde_json::json!({
                "t": {"title": "rotate the signing secret",
                      "brief": "quarterly rotation of the release signing key",
                      "state": "submitted"}
            }),
            serde_json::json!({}),
        );
        let archive = Archive::load(std::slice::from_ref(&path), None);
        assert!(
            archive
                .search("write the quarterly investor update")
                .is_empty(),
            "sharing `quarterly` is not being about the same thing"
        );
        let _ = std::fs::remove_file(path);
    }

    /// The room being opened must not be offered its own work as prior art.
    #[test]
    fn the_current_room_is_not_its_own_prior_art() {
        let path = board(
            "Leo",
            serde_json::json!({"t": {"title": "a thing", "brief": "do it", "state": "submitted"}}),
            serde_json::json!({}),
        );
        let archive = Archive::load(std::slice::from_ref(&path), Some("Leo"));
        assert!(archive.is_empty());
        let _ = std::fs::remove_file(path);
    }

    /// Unrelated work is not offered; a false lead costs a read to dismiss.
    #[test]
    fn work_about_something_else_does_not_surface() {
        let path = board(
            "Release",
            serde_json::json!({
                "t": {"title": "rotate database credentials",
                      "brief": "quarterly secret rotation", "state": "submitted"}
            }),
            serde_json::json!({}),
        );
        let archive = Archive::load(std::slice::from_ref(&path), None);
        assert!(archive
            .search("write the marketing launch email")
            .is_empty());
        let _ = std::fs::remove_file(path);
    }

    /// A board that cannot be read must not stop a room opening.
    #[test]
    fn an_unreadable_board_is_skipped_rather_than_fatal() {
        let bad = std::env::temp_dir().join("orgbot-archive-not-json.json");
        std::fs::write(&bad, "this is not json").unwrap();
        let archive = Archive::load(&[bad.clone(), PathBuf::from("/nowhere/at/all.json")], None);
        assert!(archive.is_empty());
        let _ = std::fs::remove_file(bad);
    }

    /// Something delivered outranks something merely attempted.
    #[test]
    fn delivered_work_is_offered_before_abandoned_work() {
        let path = board(
            "Release",
            serde_json::json!({
                "t1": {"title": "export to csv", "brief": "csv export", "state": "failed"},
                "t2": {"title": "export to csv", "brief": "csv export", "state": "submitted"}
            }),
            serde_json::json!({
                "a1": {"task": "t1", "name": "attempt.md", "body": {"kind":"text","text":"x"}}
            }),
        );
        let archive = Archive::load(std::slice::from_ref(&path), None);
        let hits = archive.search("csv export");
        assert_eq!(hits.len(), 2);
        assert!(hits[0].task.delivered, "the delivered one comes first");
        let _ = std::fs::remove_file(path);
    }
}
