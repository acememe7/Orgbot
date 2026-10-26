//! Durable, one-use consumption of owner approval for external MCP calls.

use std::path::Path;

use orgbot_proto::DecisionId;

/// Stable JSON spelling: object key order cannot create or evade approvals.
pub(crate) fn canonical_json(value: &serde_json::Value) -> String {
    fn normalize(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(object) => {
                let mut entries: Vec<_> = object.iter().collect();
                entries.sort_by(|left, right| left.0.cmp(right.0));
                let mut sorted = serde_json::Map::new();
                for (key, value) in entries {
                    sorted.insert(key.clone(), normalize(value));
                }
                serde_json::Value::Object(sorted)
            }
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(normalize).collect())
            }
            other => other.clone(),
        }
    }
    serde_json::to_string(&normalize(value)).unwrap_or_default()
}

/// Bind a decision to this canonical state directory and room instance.
pub(crate) async fn canonical_state_identity(
    state_dir: std::path::PathBuf,
    engine: crate::SharedEngine,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || canonical_state_identity_sync(&state_dir, &engine))
        .await
        .map_err(|error| format!("MCP approval identity worker failed: {error}"))?
}

fn canonical_state_identity_sync(
    state_dir: &Path,
    engine: &crate::SharedEngine,
) -> Result<String, String> {
    let canonical = std::fs::canonicalize(state_dir)
        .map_err(|error| format!("MCP approval state path is unavailable: {error}"))?;
    let metadata = std::fs::symlink_metadata(state_dir)
        .map_err(|error| format!("inspect MCP approval state path: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("MCP approval state path is not a safe directory".into());
    }
    let room_id = engine.with(|room| room.room().id.to_string());
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("{}\0{room_id}", canonical.display()).as_bytes());
    Ok(hex_digest(&digest))
}

pub(crate) fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 15) as usize] as char);
    }
    output
}

/// Atomically persist that a decision ID has been used; create-new prevents replay.
pub(crate) async fn consume_approval(
    state_dir: std::path::PathBuf,
    decision: DecisionId,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || consume_approval_sync(&state_dir, decision))
        .await
        .map_err(|error| format!("MCP approval ledger worker failed: {error}"))?
}

/// Whether this exact decision has a durable one-use record.
pub(crate) async fn approval_consumed(
    state_dir: std::path::PathBuf,
    decision: DecisionId,
) -> Result<bool, String> {
    tokio::task::spawn_blocking(move || approval_consumed_sync(&state_dir, decision))
        .await
        .map_err(|error| format!("MCP approval ledger worker failed: {error}"))?
}

fn approval_consumed_sync(state_dir: &Path, decision: DecisionId) -> Result<bool, String> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let canonical = std::fs::canonicalize(state_dir)
        .map_err(|error| format!("MCP approval state path is unavailable: {error}"))?;
    let path = canonical.join("mcp-approvals").join(decision.to_string());
    let parent = path.parent().expect("ledger parent");
    let parent_metadata = match std::fs::symlink_metadata(parent) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("inspect MCP approval ledger: {error}")),
    };
    if !parent_metadata.is_dir()
        || parent_metadata.file_type().is_symlink()
        || parent_metadata.uid()
            != std::fs::metadata(state_dir)
                .map_err(|error| format!("inspect MCP approval state path: {error}"))?
                .uid()
        || parent_metadata.permissions().mode() & 0o077 != 0
    {
        return Err("unsafe MCP approval ledger directory".into());
    }
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("inspect MCP approval use: {error}")),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.nlink() != 1
        || metadata.uid() != parent_metadata.uid()
    {
        return Err("unsafe MCP approval use record".into());
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| format!("read MCP approval use record: {error}"))?;
    let mut marker = String::new();
    file.read_to_string(&mut marker)
        .map_err(|error| format!("read MCP approval use record: {error}"))?;
    if marker != "consumed\n" {
        return Err("invalid MCP approval use record".into());
    }
    Ok(true)
}

fn consume_approval_sync(state_dir: &Path, decision: DecisionId) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let canonical = std::fs::canonicalize(state_dir)
        .map_err(|error| format!("MCP approval state path is unavailable: {error}"))?;
    let approvals = canonical.join("mcp-approvals");
    match std::fs::create_dir(&approvals) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("create MCP approval ledger: {error}")),
    }
    let metadata = std::fs::symlink_metadata(&approvals)
        .map_err(|error| format!("inspect MCP approval ledger: {error}"))?;
    let root_metadata = std::fs::metadata(&canonical)
        .map_err(|error| format!("inspect MCP approval state path: {error}"))?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != root_metadata.uid()
    {
        return Err("unsafe MCP approval ledger directory".into());
    }
    std::fs::set_permissions(&approvals, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("protect MCP approval ledger: {error}"))?;
    let path = approvals.join(decision.to_string());
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                "This owner approval was already used; request a new approval.".to_string()
            } else {
                format!("consume MCP owner approval: {error}")
            }
        })?;
    file.write_all(b"consumed\n")
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("persist MCP owner approval use: {error}"))?;
    std::fs::File::open(&approvals)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("sync MCP approval ledger: {error}"))
}

#[cfg(test)]
pub(crate) fn test_consume_approval_sync(
    state_dir: &Path,
    decision: DecisionId,
) -> Result<(), String> {
    consume_approval_sync(state_dir, decision)
}
