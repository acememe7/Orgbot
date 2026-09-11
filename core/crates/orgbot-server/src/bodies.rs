//! Keeping deliverables out of the board file.
//!
//! A board has a hard ceiling: past sixty-four megabytes nothing can open it,
//! and a room that cannot open its board has lost everything in it. The event
//! log and the transcript are bounded now, but artifacts are not, and they
//! must not be — they are the work. A room that deletes its own deliverables
//! to stay under a limit has solved the wrong problem.
//!
//! So the bodies move out and references stay behind. Nine hundred artifacts
//! are five megabytes of board today; as files beside it they are five
//! megabytes that the board never has to parse, and the ceiling stops being
//! something the room can reach by working.
//!
//! This is deliberately a **save-file transformation and nothing else**. It
//! runs on the JSON on its way to disk and on the way back, so the engine, the
//! tools and every test see exactly what they always saw: an artifact with its
//! body in it. Teaching the engine about stored bodies would have meant giving
//! a pure, filesystem-free component a filesystem, and every layer above it a
//! new case to get wrong.
//!
//! Two rules the ordering depends on. A body is written before the board that
//! references it, so a board never points at a file that is not there yet. And
//! a body is only taken out of the board once its file is written, so a failed
//! write costs a larger board rather than a lost deliverable.

use std::path::Path;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Bodies smaller than this stay in the board.
///
/// A file costs an inode, a read and a syscall; below a couple of kilobytes
/// that is worse than the bytes it saves. The size is dominated by the few
/// large documents anyway — nine hundred artifacts averaging five kilobytes,
/// with the weight in the long ones.
const SPILL_OVER_BYTES: usize = 2_048;

/// Which field of a body holds its text, by kind.
fn text_field(kind: &str) -> Option<&'static str> {
    match kind {
        "text" => Some("text"),
        "patch" => Some("diff"),
        "json" => Some("value"),
        // A file or url body is a reference already; there is nothing to move.
        _ => None,
    }
}

/// Move large artifact bodies out of `board` into `dir`.
///
/// Returns how many were moved. An artifact whose body cannot be written stays
/// in the board: a bigger file is a problem, and a deliverable that exists in
/// neither place is a catastrophe.
pub fn spill(board: &mut Value, dir: &Path) -> std::io::Result<usize> {
    let Some(engine) = board.get_mut("engine") else {
        return Ok(0);
    };
    std::fs::create_dir_all(dir)?;
    let mut moved: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    if let Some(artifacts) = engine.get_mut("artifacts").and_then(Value::as_object_mut) {
        for (id, artifact) in artifacts.iter_mut() {
            let Some(body) = artifact.get_mut("body") else {
                continue;
            };
            if spill_one(id, body, dir) {
                moved.insert(id.clone());
            }
        }
    }

    // The same deliverable is held twice: in the artifact map, and again
    // inside the `artifact_added` event that announced it. Spilling only the
    // first leaves the whole document in the log — which is how the first
    // version of this saved almost nothing and looked like it worked.
    if let Some(log) = engine.get_mut("log").and_then(Value::as_array_mut) {
        for entry in log.iter_mut() {
            let Some(artifact) = entry
                .get_mut("event")
                .and_then(|event| event.get_mut("artifact"))
            else {
                continue;
            };
            let Some(id) = artifact.get("id").and_then(Value::as_str).map(str::to_string) else {
                continue;
            };
            let Some(body) = artifact.get_mut("body") else {
                continue;
            };
            if spill_one(&id, body, dir) {
                moved.insert(id);
            }
        }
    }
    Ok(moved.len())
}

/// Move one body out, reporting whether it went.
fn spill_one(id: &str, body: &mut Value, dir: &Path) -> bool {
    {
        let Some(body) = body.as_object_mut() else {
            return false;
        };
        if body.contains_key("stored") {
            return false;
        }
        let Some(kind) = body.get("kind").and_then(Value::as_str) else {
            return false;
        };
        let Some(field) = text_field(kind) else {
            return false;
        };
        let Some(value) = body.get(field) else {
            return false;
        };
        // A json body is arbitrary JSON; everything else is already a string.
        let text = match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        if text.len() < SPILL_OVER_BYTES {
            return false;
        }

        let file = dir.join(format!("{id}.body"));
        // Written whole under a temporary name and renamed, so a board can
        // never find a half-written deliverable behind a reference.
        let temp = dir.join(format!(".{id}.tmp"));
        if std::fs::write(&temp, text.as_bytes()).is_err() {
            return false;
        }
        if std::fs::rename(&temp, &file).is_err() {
            let _ = std::fs::remove_file(&temp);
            return false;
        }

        let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
        body.remove(field);
        body.insert("stored".into(), json!(true));
        body.insert("bytes".into(), json!(text.len()));
        body.insert("sha256".into(), json!(digest));
    }
    true
}

/// Put the bodies back, so everything above sees an ordinary board.
///
/// A body that cannot be read, or that does not match the digest recorded when
/// it was written, is replaced by a line saying so. Silently handing back an
/// empty artifact would let a reviewer approve a document nobody could read —
/// the same failure as a paginated artifact that stopped early and said
/// nothing, which is worth not repeating.
pub fn restore(board: &mut Value, dir: &Path) -> Vec<String> {
    let mut damaged: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let Some(engine) = board.get_mut("engine") else {
        return Vec::new();
    };

    if let Some(artifacts) = engine.get_mut("artifacts").and_then(Value::as_object_mut) {
        for (id, artifact) in artifacts.iter_mut() {
            let Some(body) = artifact.get_mut("body") else {
                continue;
            };
            if !restore_one(id, body, dir) {
                damaged.insert(id.clone());
            }
        }
    }
    if let Some(log) = engine.get_mut("log").and_then(Value::as_array_mut) {
        for entry in log.iter_mut() {
            let Some(artifact) = entry
                .get_mut("event")
                .and_then(|event| event.get_mut("artifact"))
            else {
                continue;
            };
            let Some(id) = artifact.get("id").and_then(Value::as_str).map(str::to_string) else {
                continue;
            };
            let Some(body) = artifact.get_mut("body") else {
                continue;
            };
            if !restore_one(&id, body, dir) {
                damaged.insert(id);
            }
        }
    }
    damaged.into_iter().collect()
}

/// Put one body back. False when what came back is not what was saved.
fn restore_one(id: &str, body: &mut Value, dir: &Path) -> bool {
    let Some(map) = body.as_object_mut() else {
        return true;
    };
    if map.get("stored").and_then(Value::as_bool) != Some(true) {
        return true;
    }
    let kind = map
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("text")
        .to_string();
    let Some(field) = text_field(&kind) else {
        return true;
    };
    let expected = map.get("sha256").and_then(Value::as_str).map(str::to_string);

    let mut intact = true;
    let text = match std::fs::read_to_string(dir.join(format!("{id}.body"))) {
        Ok(text) => {
            let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
            if expected.is_some_and(|expected| expected != digest) {
                intact = false;
                "[this deliverable could not be read back intact: the stored file does \
                 not match the digest recorded when it was saved. Do not treat its \
                 contents as known.]"
                    .to_string()
            } else {
                text
            }
        }
        Err(_) => {
            intact = false;
            "[this deliverable is missing: its stored file could not be read. Do not \
             treat its contents as known.]"
                .to_string()
        }
    };

    map.remove("stored");
    map.remove("bytes");
    map.remove("sha256");
    // A json body has to go back as JSON, not as a string of it.
    let restored = if kind == "json" && intact {
        serde_json::from_str(&text).unwrap_or(Value::String(text))
    } else {
        Value::String(text)
    };
    map.insert(field.into(), restored);
    intact
}

/// Where a board's bodies live, given where the board is.
pub fn directory_for(board: &Path) -> std::path::PathBuf {
    board
        .parent()
        .unwrap_or(Path::new("."))
        .join("artifact_bodies")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board_with(body: Value) -> Value {
        json!({
            "version": 1,
            "engine": { "artifacts": { "art_1": { "name": "notes.md", "body": body } } }
        })
    }

    fn dir() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "orgbot-bodies-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn a_large_body_leaves_the_board_and_comes_back_unchanged() {
        let text = "x".repeat(10_000);
        let mut board = board_with(json!({"kind": "text", "text": text}));
        let dir = dir();

        assert_eq!(spill(&mut board, &dir).unwrap(), 1);
        let saved = serde_json::to_string(&board).unwrap();
        assert!(
            saved.len() < 500,
            "the board must no longer carry the document: {} bytes",
            saved.len()
        );
        assert!(dir.join("art_1.body").is_file());

        assert!(restore(&mut board, &dir).is_empty());
        assert_eq!(
            board["engine"]["artifacts"]["art_1"]["body"]["text"],
            json!(text),
            "and it comes back exactly as it went"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A short note is not worth a file of its own.
    #[test]
    fn a_small_body_stays_where_it_is() {
        let mut board = board_with(json!({"kind": "text", "text": "done"}));
        let dir = dir();
        assert_eq!(spill(&mut board, &dir).unwrap(), 0);
        assert_eq!(board["engine"]["artifacts"]["art_1"]["body"]["text"], "done");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A json body must come back as json, not as a string of it.
    #[test]
    fn structured_content_keeps_its_shape() {
        let value = json!({"rows": (0..400).map(|i| json!({"n": i})).collect::<Vec<_>>()});
        let mut board = board_with(json!({"kind": "json", "value": value}));
        let dir = dir();

        assert_eq!(spill(&mut board, &dir).unwrap(), 1);
        restore(&mut board, &dir);
        assert_eq!(
            board["engine"]["artifacts"]["art_1"]["body"]["value"],
            value
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A missing deliverable must say so, not read as an empty one.
    #[test]
    fn a_body_that_cannot_be_read_is_reported_not_silently_empty() {
        let mut board = board_with(json!({"kind": "text", "text": "y".repeat(5_000)}));
        let dir = dir();
        spill(&mut board, &dir).unwrap();
        std::fs::remove_file(dir.join("art_1.body")).unwrap();

        let damaged = restore(&mut board, &dir);
        assert_eq!(damaged, vec!["art_1".to_string()]);
        let text = board["engine"]["artifacts"]["art_1"]["body"]["text"]
            .as_str()
            .unwrap();
        assert!(
            text.contains("missing"),
            "a reader must not be handed an empty document as if it were the work"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A file that changed under us is not the deliverable that was saved.
    #[test]
    fn a_body_that_does_not_match_its_digest_is_refused() {
        let mut board = board_with(json!({"kind": "text", "text": "z".repeat(5_000)}));
        let dir = dir();
        spill(&mut board, &dir).unwrap();
        std::fs::write(dir.join("art_1.body"), "something else entirely").unwrap();

        let damaged = restore(&mut board, &dir);
        assert_eq!(damaged, vec!["art_1".to_string()]);
        assert!(
            board["engine"]["artifacts"]["art_1"]["body"]["text"]
                .as_str()
                .unwrap()
                .contains("does not match"),
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Saving twice must not spill a body that is already outside.
    #[test]
    fn spilling_an_already_spilled_board_does_nothing() {
        let mut board = board_with(json!({"kind": "text", "text": "w".repeat(5_000)}));
        let dir = dir();
        assert_eq!(spill(&mut board, &dir).unwrap(), 1);
        assert_eq!(spill(&mut board, &dir).unwrap(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A file or url body is a reference already.
    #[test]
    fn a_reference_body_is_left_alone() {
        let mut board = board_with(json!({"kind": "file", "path": "a/b.txt", "bytes": 99}));
        let dir = dir();
        assert_eq!(spill(&mut board, &dir).unwrap(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
