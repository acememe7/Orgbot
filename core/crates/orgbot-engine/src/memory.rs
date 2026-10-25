//! Durable, project-scoped knowledge shared by rooms.
//!
//! This store is local and deterministic. It never calls a provider. Search
//! uses signed feature vectors with a small, explicit concept map so common
//! paraphrases such as "login" and "authentication" can meet without sharing
//! the same token. It is deliberately separate from a room's checkpoint.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A durable note available to rooms in one project scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Stable identifier within the project.
    pub id: String,
    /// Short human-readable subject.
    pub title: String,
    /// Context to reuse, including decisions and delivered outcomes.
    pub content: String,
    /// Room that originated the note, if it came from completed work.
    pub source_room: Option<String>,
    /// Task that originated the note, if any.
    pub source_task: Option<String>,
    /// Project scope that explicitly shared this note here, if copied.
    #[serde(default)]
    pub shared_from_scope: Option<String>,
    /// Source entry id for a cross-project copy, used to make sharing idempotent.
    #[serde(default)]
    pub shared_from_entry: Option<String>,
    /// Whether the owner has made this entry available to this project's rooms.
    #[serde(default = "default_shared")]
    pub shared: bool,
    /// Unix timestamp in milliseconds.
    pub created_at_ms: u64,
}

fn default_shared() -> bool {
    true
}

/// One semantically ranked project-memory result.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryHit {
    /// The matched note.
    pub entry: MemoryEntry,
    /// Cosine similarity of deterministic local feature vectors.
    pub score: f32,
}

/// Errors encountered while loading or changing project memory.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    /// Scope is missing or invalid.
    #[error("project memory requires a non-empty scope of at most 256 characters")]
    InvalidScope,
    /// The store could not be read or written.
    #[error("project memory storage failed: {0}")]
    Io(#[from] std::io::Error),
    /// The store did not contain valid JSON.
    #[error("project memory file is invalid: {0}")]
    Json(#[from] serde_json::Error),
    /// A memory entry exceeded a supported bound.
    #[error("memory title must be 1–160 characters and content at most 12,000 characters")]
    EntryTooLarge,
    /// Content appears to contain a credential value.
    #[error("project memory cannot store credential-like values")]
    SensitiveMaterial,
    /// There is no matching note in this project.
    #[error("memory entry was not found in this project")]
    NotFound,
    /// The owner permanently deleted the completed-work memory for this task.
    #[error("completed-work memory was deleted by the owner and will not be recreated")]
    DeletedSource,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct MemoryFile {
    version: u32,
    scope_hash: String,
    entries: BTreeMap<String, MemoryEntry>,
    #[serde(default)]
    deleted_sources: BTreeSet<String>,
}

/// A view of one project's durable local memory.
#[derive(Debug, Clone)]
pub struct MemoryRepository {
    path: PathBuf,
    scope_hash: String,
}

const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 2_000;
const MAX_RESULTS: usize = 10;
const SEARCH_FLOOR: f32 = 0.14;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl MemoryRepository {
    /// Open a store using a caller-supplied root and explicit project scope.
    ///
    /// The scope is hashed before it becomes a filename and is never inferred
    /// from a room name or the process working directory.
    pub fn open(root: impl AsRef<Path>, project_scope: &str) -> Result<Self, MemoryError> {
        let repo = Self::for_scope(root.as_ref(), project_scope)?;
        // Validate existing data at configuration time, rather than failing
        // unexpectedly on the first model tool call.
        let _lock = repo.lock()?;
        let _ = repo.load_unlocked()?;
        Ok(repo)
    }

    fn for_scope(root: &Path, project_scope: &str) -> Result<Self, MemoryError> {
        let scope = project_scope.trim();
        if scope.is_empty() || scope.len() > 256 || scope.chars().any(char::is_control) {
            return Err(MemoryError::InvalidScope);
        }
        let scope_hash = hex(&Sha256::digest(scope.as_bytes()));
        fs::create_dir_all(root)?;
        Ok(Self {
            path: root.join(format!("{scope_hash}.json")),
            scope_hash,
        })
    }

    /// List every note in this project for owner controls, oldest first.
    pub fn list(&self) -> Result<Vec<MemoryEntry>, MemoryError> {
        let _lock = self.lock()?;
        Ok(self.load_unlocked()?.entries.into_values().collect())
    }

    /// Search shared notes by deterministic semantic feature-vector cosine.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryHit>, MemoryError> {
        let _lock = self.lock()?;
        let query_vector = semantic_vector(query);
        if query_vector.values().all(|weight| *weight == 0.0) {
            return Ok(Vec::new());
        }
        let file = self.load_unlocked()?;
        let mut hits: Vec<MemoryHit> = file
            .entries
            .into_values()
            .filter(|entry| entry.shared)
            .filter_map(|entry| {
                let subject = format!("{} {}", entry.title, entry.content);
                let score = cosine(&query_vector, &semantic_vector(&subject));
                (score >= SEARCH_FLOOR).then_some(MemoryHit { entry, score })
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then(a.entry.id.cmp(&b.entry.id))
        });
        hits.truncate(limit.min(MAX_RESULTS));
        Ok(hits)
    }

    /// Add a durable note. An identical source task updates its existing note.
    pub fn add(
        &self,
        title: &str,
        content: &str,
        source_room: Option<&str>,
        source_task: Option<&str>,
    ) -> Result<MemoryEntry, MemoryError> {
        let _lock = self.lock()?;
        let title = title.trim();
        let content = content.trim();
        if title.is_empty() || title.chars().count() > 160 || content.chars().count() > 12_000 {
            return Err(MemoryError::EntryTooLarge);
        }
        if contains_credential_value(title) || contains_credential_value(content) {
            return Err(MemoryError::SensitiveMaterial);
        }
        let mut file = self.load_unlocked()?;
        let key = source_key(source_room, source_task);
        if key
            .as_ref()
            .is_some_and(|key| file.deleted_sources.contains(key))
        {
            return Err(MemoryError::DeletedSource);
        }
        let existing = source_task.and_then(|task| {
            file.entries.values().find(|entry| {
                entry.shared_from_scope.is_none()
                    && entry.source_task.as_deref() == Some(task)
                    && entry.source_room.as_deref() == source_room
            })
        });
        let id = existing.map(|entry| entry.id.clone()).unwrap_or_else(|| {
            entry_id(&self.scope_hash, source_room, source_task, title, now_ms())
        });
        let shared = existing.map_or(true, |entry| entry.shared);
        let entry = MemoryEntry {
            id: id.clone(),
            title: title.to_string(),
            content: content.to_string(),
            source_room: source_room.map(str::to_owned),
            source_task: source_task.map(str::to_owned),
            shared_from_scope: None,
            shared_from_entry: None,
            shared,
            created_at_ms: existing.map_or_else(now_ms, |entry| entry.created_at_ms),
        };
        if !file.entries.contains_key(&id) && file.entries.len() >= MAX_ENTRIES {
            return Err(MemoryError::EntryTooLarge);
        }
        file.entries.insert(id, entry.clone());
        self.save_unlocked(&file)?;
        Ok(entry)
    }

    /// Change project visibility for a note without deleting its contents.
    pub fn set_shared(&self, id: &str, shared: bool) -> Result<MemoryEntry, MemoryError> {
        let _lock = self.lock()?;
        let mut file = self.load_unlocked()?;
        let entry = file.entries.get_mut(id).ok_or(MemoryError::NotFound)?;
        entry.shared = shared;
        let updated = entry.clone();
        self.save_unlocked(&file)?;
        Ok(updated)
    }

    /// Permanently remove one note from this project store.
    pub fn delete(&self, id: &str) -> Result<(), MemoryError> {
        let _lock = self.lock()?;
        let mut file = self.load_unlocked()?;
        let entry = file.entries.remove(id).ok_or(MemoryError::NotFound)?;
        if entry.shared_from_scope.is_none() {
            if let Some(key) =
                source_key(entry.source_room.as_deref(), entry.source_task.as_deref())
            {
                file.deleted_sources.insert(key);
            }
        }
        self.save_unlocked(&file)
    }

    /// Copy a note into a different project only when explicitly requested.
    pub fn share_to_project(
        &self,
        id: &str,
        target_root: impl AsRef<Path>,
        target_scope: &str,
    ) -> Result<MemoryEntry, MemoryError> {
        let target = Self::for_scope(target_root.as_ref(), target_scope)?;
        if target.scope_hash == self.scope_hash {
            return Err(MemoryError::InvalidScope);
        }
        let (_first_lock, _second_lock) = if self.path < target.path {
            (self.lock()?, target.lock()?)
        } else {
            (target.lock()?, self.lock()?)
        };
        let source = self
            .load_unlocked()?
            .entries
            .remove(id)
            .ok_or(MemoryError::NotFound)?;
        let mut target_file = target.load_unlocked()?;
        if let Some(existing) = target_file.entries.values().find(|entry| {
            entry.shared_from_scope.as_deref() == Some(self.scope_hash.as_str())
                && entry.shared_from_entry.as_deref() == Some(id)
        }) {
            return Ok(existing.clone());
        }
        if target_file.entries.len() >= MAX_ENTRIES {
            return Err(MemoryError::EntryTooLarge);
        }
        let copy_id = entry_id(
            &target.scope_hash,
            source.source_room.as_deref(),
            source.source_task.as_deref(),
            &source.title,
            now_ms(),
        );
        let copy = MemoryEntry {
            id: copy_id.clone(),
            shared_from_scope: Some(self.scope_hash.clone()),
            shared_from_entry: Some(id.to_string()),
            shared: true,
            ..source
        };
        target_file.entries.insert(copy_id, copy.clone());
        target.save_unlocked(&target_file)?;
        Ok(copy)
    }

    fn lock(&self) -> Result<File, MemoryError> {
        let lock_path = self.path.with_extension("lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let file = options.open(lock_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if file.metadata()?.permissions().mode() & 0o077 != 0 {
                file.set_permissions(fs::Permissions::from_mode(0o600))?;
            }
        }
        fs2::FileExt::lock_exclusive(&file)?;
        Ok(file)
    }

    fn load_unlocked(&self) -> Result<MemoryFile, MemoryError> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let mut input = match options.open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(MemoryFile {
                    version: 1,
                    scope_hash: self.scope_hash.clone(),
                    entries: BTreeMap::new(),
                    deleted_sources: BTreeSet::new(),
                });
            }
            Err(error) => return Err(error.into()),
        };
        let metadata = input.metadata()?;
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            return Err(MemoryError::EntryTooLarge);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                input.set_permissions(fs::Permissions::from_mode(0o600))?;
            }
        }
        let mut raw = Vec::with_capacity(metadata.len() as usize);
        input.read_to_end(&mut raw)?;
        let mut file: MemoryFile = serde_json::from_slice(&raw)?;
        if file.version != 1
            || file.scope_hash != self.scope_hash
            || file.entries.len() > MAX_ENTRIES
        {
            return Err(MemoryError::InvalidScope);
        }
        file.entries.retain(|id, entry| id == &entry.id);
        Ok(file)
    }

    fn save_unlocked(&self, file: &MemoryFile) -> Result<(), MemoryError> {
        let raw = serde_json::to_vec_pretty(file)?;
        if raw.len() as u64 > MAX_FILE_BYTES {
            return Err(MemoryError::EntryTooLarge);
        }
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = self
            .path
            .with_extension(format!("{}-{sequence}.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let mut output = options.open(&temp)?;
        let result = (|| -> Result<(), std::io::Error> {
            output.write_all(&raw)?;
            output.sync_all()?;
            drop(output);
            fs::rename(&temp, &self.path)?;
            if let Some(parent) = self.path.parent() {
                File::open(parent)?.sync_all()?;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result?;
        Ok(())
    }
}

/// Turn completed task metadata into a reusable concise note without an LLM.
pub fn completed_work_content(
    brief: &str,
    decisions: &[String],
    artifact_names: &[String],
) -> String {
    let mut parts = vec![format!("Outcome: {}", brief.trim())];
    if !decisions.is_empty() {
        parts.push(format!("Decisions: {}", decisions.join("; ")));
    }
    if !artifact_names.is_empty() {
        parts.push(format!("Artifacts: {}", artifact_names.join(", ")));
    }
    parts.join("\n")
}

fn entry_id(
    scope_hash: &str,
    room: Option<&str>,
    task: Option<&str>,
    title: &str,
    salt: u64,
) -> String {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let material = format!(
        "{scope_hash}\0{}\0{}\0{title}\0{salt}\0{sequence}",
        room.unwrap_or(""),
        task.unwrap_or("")
    );
    hex(&Sha256::digest(material.as_bytes()))[..32].to_string()
}

fn source_key(room: Option<&str>, task: Option<&str>) -> Option<String> {
    let (room, task) = (room?, task?);
    Some(hex(&Sha256::digest(format!("{room}\0{task}").as_bytes())))
}

fn contains_credential_value(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("-----begin private key-----") || lower.contains("bearer ") {
        return true;
    }
    for marker in ["api_key", "api key", "password", "secret", "token"] {
        let mut rest = lower.as_str();
        while let Some(index) = rest.find(marker) {
            rest = &rest[index + marker.len()..];
            let value = rest.trim_start();
            let Some(value) = value.strip_prefix('=').or_else(|| value.strip_prefix(':')) else {
                continue;
            };
            let value = value.trim_start().trim_matches(|c| c == '"' || c == '\'');
            let value = value
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_matches(|c| c == ',' || c == ';' || c == '"' || c == '\'');
            if value.chars().count() >= 8
                && !["<redacted>", "placeholder", "your_key_here"].contains(&value)
            {
                return true;
            }
        }
    }
    false
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

/// A compact, stable semantic feature map. Synonyms share a concept feature;
/// the original token also contributes to keep unrelated meanings separated.
fn semantic_vector(text: &str) -> BTreeMap<u32, f32> {
    let mut vector = BTreeMap::new();
    for token in orgbot_proto::text::content_words(text) {
        let token = normalize(&token);
        if token.len() < 2 {
            continue;
        }
        add_feature(&mut vector, &format!("w:{token}"), 1.0);
        if let Some(concept) = concept(&token) {
            add_feature(&mut vector, &format!("c:{concept}"), 1.4);
        }
    }
    vector
}

fn normalize(token: &str) -> String {
    let t = token.to_ascii_lowercase().replace('-', "");
    match t.strip_suffix('s') {
        Some(stem) if stem.len() > 3 => stem.to_owned(),
        _ => t,
    }
}

fn concept(word: &str) -> Option<&'static str> {
    const GROUPS: &[(&str, &[&str])] = &[
        (
            "authentication",
            &[
                "login",
                "signin",
                "auth",
                "authentication",
                "authenticate",
                "oauth",
                "identity",
            ],
        ),
        (
            "failure",
            &[
                "bug", "defect", "error", "failure", "broken", "crash", "fault",
            ],
        ),
        (
            "performance",
            &[
                "slow",
                "latency",
                "delay",
                "performance",
                "speed",
                "timeout",
            ],
        ),
        (
            "persistence",
            &[
                "save",
                "persist",
                "store",
                "durable",
                "checkpoint",
                "recover",
                "restore",
            ],
        ),
        (
            "access-control",
            &[
                "permission",
                "authorization",
                "access",
                "policy",
                "privilege",
                "role",
            ],
        ),
        (
            "attachment",
            &["attachment", "upload", "file", "document", "image", "media"],
        ),
        (
            "retrieval",
            &["search", "find", "lookup", "retrieve", "query", "index"],
        ),
        (
            "message",
            &[
                "message",
                "chat",
                "reply",
                "thread",
                "conversation",
                "transcript",
            ],
        ),
        (
            "summary",
            &[
                "summary",
                "summarize",
                "compact",
                "compaction",
                "condense",
                "history",
            ],
        ),
    ];
    GROUPS
        .iter()
        .find_map(|(name, words)| words.contains(&word).then_some(*name))
}

fn add_feature(vector: &mut BTreeMap<u32, f32>, feature: &str, weight: f32) {
    let digest = Sha256::digest(feature.as_bytes());
    let index = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    let sign = if digest[4] & 1 == 0 { 1.0 } else { -1.0 };
    *vector.entry(index).or_default() += weight * sign;
}

fn cosine(a: &BTreeMap<u32, f32>, b: &BTreeMap<u32, f32>) -> f32 {
    let dot: f32 = a
        .iter()
        .filter_map(|(key, value)| b.get(key).map(|other| value * other))
        .sum();
    let norm = |v: &BTreeMap<u32, f32>| v.values().map(|value| value * value).sum::<f32>().sqrt();
    let denominator = norm(a) * norm(b);
    if denominator == 0.0 {
        0.0
    } else {
        dot / denominator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("orgbot-memory-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn project_scope_isolation_persistence_and_deletion() {
        let root = temp_root();
        let one = MemoryRepository::open(&root, "project-one").unwrap();
        let two = MemoryRepository::open(&root, "project-two").unwrap();
        let entry = one
            .add(
                "Login recovery",
                "Persist the OAuth session after app restart",
                Some("room-a"),
                Some("task-a"),
            )
            .unwrap();
        assert_eq!(one.list().unwrap().len(), 1);
        assert!(two
            .search("authentication after restart", 5)
            .unwrap()
            .is_empty());
        drop(one);
        let reopened = MemoryRepository::open(&root, "project-one").unwrap();
        assert_eq!(reopened.list().unwrap()[0].id, entry.id);
        reopened.delete(&entry.id).unwrap();
        assert!(reopened.list().unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn paraphrases_retrieve_by_shared_concept_not_only_shared_words() {
        let root = temp_root();
        let repo = MemoryRepository::open(&root, "project").unwrap();
        repo.add(
            "Persist credentials across restart",
            "Recover saved authentication after an app crash",
            None,
            None,
        )
        .unwrap();
        let hits = repo.search("restore login after reboot", 5).unwrap();
        assert_eq!(
            hits.len(),
            1,
            "login/authentication and restore/recover should associate"
        );
        assert!(repo
            .search("quarterly marketing calendar", 5)
            .unwrap()
            .is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn automatic_completed_work_capture_is_deterministic_and_updates_same_task() {
        let root = temp_root();
        let repo = MemoryRepository::open(&root, "project").unwrap();
        let content = completed_work_content(
            "Resume OAuth sign-in reliably",
            &["Keep session IDs stable".into()],
            &["continuity.md".into()],
        );
        let first = repo
            .add("OAuth continuity", &content, Some("room"), Some("task"))
            .unwrap();
        let updated = repo
            .add(
                "OAuth continuity",
                "Updated result",
                Some("room"),
                Some("task"),
            )
            .unwrap();
        assert_eq!(first.id, updated.id);
        assert_eq!(repo.list().unwrap().len(), 1);
        assert!(updated.content.contains("Updated result"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn hidden_and_deleted_automatic_notes_stay_hidden_after_recapture() {
        let root = temp_root();
        let repo = MemoryRepository::open(&root, "project").unwrap();
        let note = repo
            .add(
                "Outcome",
                "Resume the OAuth session",
                Some("room"),
                Some("task"),
            )
            .unwrap();
        repo.set_shared(&note.id, false).unwrap();
        let recaptured = repo
            .add(
                "Outcome",
                "Updated completion result",
                Some("room"),
                Some("task"),
            )
            .unwrap();
        assert!(
            !recaptured.shared,
            "recapture must respect the owner's hidden state"
        );
        repo.set_shared(&note.id, true).unwrap();
        repo.delete(&note.id).unwrap();
        assert!(matches!(
            repo.add(
                "Outcome",
                "Completion replayed after restore",
                Some("room"),
                Some("task")
            ),
            Err(MemoryError::DeletedSource)
        ));
        assert!(repo.list().unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_room_writes_do_not_lose_updates() {
        let root = temp_root();
        let repo = MemoryRepository::open(&root, "project").unwrap();
        let executable = std::env::current_exe().unwrap();
        let children: Vec<_> = (0..6)
            .map(|index| {
                std::process::Command::new(&executable)
                    .arg("--exact")
                    .arg("memory::tests::memory_write_helper_process")
                    .env("ORGBOT_MEMORY_TEST_ROOT", &root)
                    .env("ORGBOT_MEMORY_TEST_INDEX", index.to_string())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .unwrap()
            })
            .collect();
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        assert_eq!(repo.list().unwrap().len(), 6);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn memory_write_helper_process() {
        let (Ok(root), Ok(index)) = (
            std::env::var("ORGBOT_MEMORY_TEST_ROOT"),
            std::env::var("ORGBOT_MEMORY_TEST_INDEX"),
        ) else {
            return;
        };
        let repo = MemoryRepository::open(root, "project").unwrap();
        repo.add(
            &format!("Decision {index}"),
            &format!("Keep project decision {index}"),
            None,
            None,
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_store_is_rejected() {
        use std::os::unix::fs::symlink;

        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let outside = root.join("outside.json");
        fs::write(&outside, "{}").unwrap();
        let scope_hash = hex(&Sha256::digest(b"project"));
        symlink(&outside, root.join(format!("{scope_hash}.json"))).unwrap();
        assert!(MemoryRepository::open(&root, "project").is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sharing_is_an_explicit_copy_to_the_target_scope() {
        let root = temp_root();
        let source = MemoryRepository::open(&root, "source").unwrap();
        let target = MemoryRepository::open(&root, "target").unwrap();
        let note = source
            .add("Decision", "Use a local cache", None, None)
            .unwrap();
        let shared = source.share_to_project(&note.id, &root, "target").unwrap();
        assert_eq!(
            source.share_to_project(&note.id, &root, "target").unwrap(),
            shared,
            "repeating the same explicit share must not create duplicates"
        );
        assert_eq!(target.list().unwrap(), vec![shared]);
        assert_eq!(source.list().unwrap().len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn owner_can_revoke_visibility_without_losing_the_note() {
        let root = temp_root();
        let repo = MemoryRepository::open(&root, "project").unwrap();
        let note = repo
            .add("Decision", "Persist the workspace checkpoint", None, None)
            .unwrap();
        repo.set_shared(&note.id, false).unwrap();
        assert_eq!(repo.list().unwrap().len(), 1, "owner can still manage it");
        assert!(repo.search("restore checkpoint", 5).unwrap().is_empty());
        repo.set_shared(&note.id, true).unwrap();
        assert_eq!(repo.search("restore checkpoint", 5).unwrap().len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn credential_like_values_are_rejected_from_project_memory() {
        let root = temp_root();
        let repo = MemoryRepository::open(&root, "project").unwrap();
        assert!(matches!(
            repo.add("Config", "api_key=sk-1234567890abcdef", None, None),
            Err(MemoryError::SensitiveMaterial)
        ));
        assert!(matches!(
            repo.add("Config", "Bearer abcdefghijklmnop", None, None),
            Err(MemoryError::SensitiveMaterial)
        ));
        let _ = fs::remove_dir_all(root);
    }
}
