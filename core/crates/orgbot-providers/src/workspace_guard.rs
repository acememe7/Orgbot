//! The trusted launcher shared by native CLIs and local API workspace tools.
use std::path::{Path, PathBuf};
use std::process::Command;

const PROFILE_MAX_ENTRIES: usize = 50_000;
const PROFILE_MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const PROFILE_MAX_TREE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct WorkspaceGuard {
    pub project: PathBuf,
    pub state: PathBuf,
    pub agent: String,
    pub read_only: bool,
    #[cfg(test)]
    pub profile_source: Option<PathBuf>,
}

#[derive(Debug, serde::Deserialize)]
pub struct PreparedWorkspace {
    pub workspace: PathBuf,
    pub profile_root: PathBuf,
    pub private_home: PathBuf,
    pub private_home_mount: PathBuf,
    pub mcp_socket_mount: PathBuf,
    #[serde(default)]
    pub read_only: bool,
}

pub fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let parent = path.parent().ok_or("private profile file has no parent")?;
    ensure_private_directory(parent)?;
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if !meta.is_file() || meta.file_type().is_symlink() || meta.nlink() != 1 {
            return Err(format!(
                "unsafe private CLI profile file: {}",
                path.display()
            ));
        }
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(".orgbot-write-{}-{nonce}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(0x20000)
        .open(&temporary)
        .map_err(|e| format!("create private profile update: {e}"))?;
    let result = file.write_all(bytes).and_then(|_| file.sync_all());
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("write private CLI profile: {error}"));
    }
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("protect private CLI profile: {e}"))?;
    std::fs::rename(&temporary, path).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        format!("install private CLI profile: {e}")
    })
}

fn read_private_file(path: &Path) -> Result<String, String> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut options = std::fs::OpenOptions::new();
    options.read(true).custom_flags(0x20000); // O_NOFOLLOW
    let mut file = options
        .open(path)
        .map_err(|e| format!("open private CLI profile: {e}"))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("inspect private CLI profile: {e}"))?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err("unsafe private CLI profile file".into());
    }
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|e| format!("read private CLI profile: {e}"))?;
    Ok(text)
}

/// Update only the room's generated MCP stanza. Other integrations and all
/// copied auth/session/skill data remain in this room's private profile.
pub fn update_private_agent_config(
    home: &Path,
    cli: orgbot_proto::agent::CliKind,
    mount: &Path,
    socket: &Path,
) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let (profile, env_key) = match cli {
        orgbot_proto::agent::CliKind::Codex => (".codex", "mcp_servers"),
        orgbot_proto::agent::CliKind::Grok => (".grok", "mcp_servers"),
        orgbot_proto::agent::CliKind::Claude => return Ok(()),
    };
    let config = home.join(profile).join("config.toml");
    let mut contents = match std::fs::symlink_metadata(&config) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() || meta.nlink() != 1 => {
            return Err(format!("unsafe private {} config", profile));
        }
        Ok(_) => read_private_file(&config)
            .map_err(|e| format!("read private {} config: {e}", profile))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("inspect private {} config: {error}", profile)),
    };
    if !contents.trim().is_empty() {
        let mut parsed: toml::Value = toml::from_str(&contents)
            .map_err(|error| format!("parse private {profile} config: {error}"))?;
        if let Some(table) = parsed.as_table_mut() {
            table.remove("mcp_servers");
        }
        contents = toml::to_string(&parsed)
            .map_err(|error| format!("sanitize private {profile} config: {error}"))?;
    }
    const BEGIN: &str = "# ORGBOT ROOM BRIDGE BEGIN";
    const END: &str = "# ORGBOT ROOM BRIDGE END";
    if let Some(start) = contents.find(BEGIN) {
        if contents[start + BEGIN.len()..].contains(BEGIN) {
            return Err("duplicate OrgBots room bridge config in private profile".into());
        }
        if let Some(end) = contents[start..].find(END) {
            let end = start + end + END.len();
            if !contents[end..].trim().is_empty() {
                return Err("room bridge config is not at the end of the private profile".into());
            }
            contents.truncate(start);
        } else {
            return Err("incomplete OrgBots room bridge config in private profile".into());
        }
    } else if contents.contains("[mcp_servers.orgbot_room]") {
        return Err("private CLI config already defines mcp_servers.orgbot_room".into());
    }
    let relay = std::env::var_os("ORGBOT_MCP_RELAY_BIN")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .ok_or("cannot locate OrgBots room relay")?;
    contents.push_str(&format!(
        "\n{BEGIN}\n[{env_key}.orgbot_room]\ncommand = {}\nargs = [\"room-mcp\", \"--socket\", {}]\n",
        serde_json::to_string(&relay.to_string_lossy()).unwrap(),
        serde_json::to_string(&socket.to_string_lossy()).unwrap(),
    ));
    if cli == orgbot_proto::agent::CliKind::Codex {
        contents.push_str("default_tools_approval_mode = \"approve\"\n");
    }
    if cli == orgbot_proto::agent::CliKind::Grok {
        contents.push_str("enabled = true\n");
    }
    contents.push_str(&format!("{END}\n"));
    let path = mount.join(profile).join("config.toml");
    write_private_file(&config, contents.as_bytes())
        .map_err(|error| format!("update {} config: {error}", path.display()))
}

pub fn clear_private_agent_config(
    home: &Path,
    cli: orgbot_proto::agent::CliKind,
) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let profile = match cli {
        orgbot_proto::agent::CliKind::Codex => ".codex",
        orgbot_proto::agent::CliKind::Grok => ".grok",
        orgbot_proto::agent::CliKind::Claude => return Ok(()),
    };
    let config = home.join(profile).join("config.toml");
    let meta = match std::fs::symlink_metadata(&config) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("inspect private {profile} config: {error}")),
    };
    if !meta.is_file() || meta.file_type().is_symlink() || meta.nlink() != 1 {
        return Err(format!("unsafe private {profile} config"));
    }
    let contents =
        read_private_file(&config).map_err(|e| format!("read private {profile} config: {e}"))?;
    let begin = "# ORGBOT ROOM BRIDGE BEGIN";
    let end = "# ORGBOT ROOM BRIDGE END";
    let Some(start) = contents.find(begin) else {
        return Ok(());
    };
    if contents[start + begin.len()..].contains(begin) {
        return Err("duplicate OrgBots room bridge config in private profile".into());
    }
    let end_at = contents[start..]
        .find(end)
        .ok_or("incomplete OrgBots room bridge config in private profile")?
        + start
        + end.len();
    if !contents[end_at..].trim().is_empty() {
        return Err("room bridge config is not at the end of the private profile".into());
    }
    write_private_file(&config, contents[..start].as_bytes())
}

/// Remove copied external MCP registrations before any protected CLI launch.
/// Room calls are routed through the OrgBot bridge only when the room enabled it.
pub fn strip_private_agent_mcp_config(
    home: &Path,
    cli: orgbot_proto::agent::CliKind,
) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let profile = match cli {
        orgbot_proto::agent::CliKind::Codex => ".codex",
        orgbot_proto::agent::CliKind::Grok => ".grok",
        orgbot_proto::agent::CliKind::Claude => return Ok(()),
    };
    let config = home.join(profile).join("config.toml");
    let contents = match std::fs::symlink_metadata(&config) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() || meta.nlink() != 1 => {
            return Err(format!("unsafe private {} config", profile));
        }
        Ok(_) => read_private_file(&config)
            .map_err(|error| format!("read private {} config: {error}", profile))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("inspect private {} config: {error}", profile)),
    };
    let mut parsed: toml::Value = toml::from_str(&contents)
        .map_err(|error| format!("parse private {profile} config: {error}"))?;
    if let Some(table) = parsed.as_table_mut() {
        table.remove("mcp_servers");
    }
    let sanitized = toml::to_string(&parsed)
        .map_err(|error| format!("sanitize private {profile} config: {error}"))?;
    write_private_file(&config, sanitized.as_bytes())
        .map_err(|error| format!("sanitize {} config: {error}", config.display()))
}

pub fn remove_private_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    match std::fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() || meta.nlink() != 1 => {
            Err(format!("unsafe private CLI file: {}", path.display()))
        }
        Ok(_) => std::fs::remove_file(path).map_err(|e| format!("remove private CLI file: {e}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("inspect private CLI file: {error}")),
    }
}

pub fn helper() -> Result<PathBuf, String> {
    let installed = PathBuf::from("/usr/lib/orgbot/orgbot-workspace.py");
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../app/packaging/orgbot-workspace.py");
    // A source build must test its own helper instead of an older installed one.
    let development = std::env::current_exe()
        .ok()
        .is_some_and(|path| path.components().any(|part| part.as_os_str() == "target"));
    let candidates = if development {
        [source, installed]
    } else {
        [installed, source]
    };
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            "OrgBots workspace protection is missing; reinstall the Linux package".into()
        })
}

impl WorkspaceGuard {
    pub fn empty_project(state: &Path, agent: &str) -> Result<PathBuf, String> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let parent = state
            .parent()
            .ok_or("private room state has no parent directory")?;
        let canonical_parent = parent
            .canonicalize()
            .map_err(|e| format!("verify private room state: {e}"))?;
        let root = canonical_parent.join("cli-empty-projects");
        match std::fs::symlink_metadata(&root) {
            Ok(meta)
                if !meta.is_dir()
                    || meta.file_type().is_symlink()
                    || meta.uid() != current_uid() =>
            {
                return Err("unsafe private empty-workspace root".into())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&root)
                    .map_err(|e| format!("create private empty-workspace root: {e}"))?
            }
            Err(error) => return Err(format!("inspect private empty-workspace root: {error}")),
        }
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("protect private empty-workspace root: {e}"))?;
        let project = root.join(agent);
        match std::fs::symlink_metadata(&project) {
            Ok(meta)
                if !meta.is_dir()
                    || meta.file_type().is_symlink()
                    || meta.uid() != current_uid() =>
            {
                return Err("unsafe private empty workspace".into())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&project)
                    .map_err(|e| format!("create private empty workspace: {e}"))?
            }
            Err(error) => return Err(format!("inspect private empty workspace: {error}")),
        }
        std::fs::set_permissions(&project, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("protect private empty workspace: {e}"))?;
        Ok(project)
    }

    pub fn command(&self, operation: &str) -> Result<Command, String> {
        let mut command = Command::new("/usr/bin/python3");
        command
            .arg(helper()?)
            .arg(operation)
            .arg("--state")
            .arg(&self.state)
            .arg("--agent")
            .arg(&self.agent);
        Ok(command)
    }

    pub fn prepare(&self) -> Result<PreparedWorkspace, String> {
        let mut command = self.command("prepare")?;
        command.arg("--project").arg(&self.project);
        if self.read_only {
            command.arg("--read-only");
        }
        let output = command
            .output()
            .map_err(|e| format!("workspace setup failed: {e}"))?;
        if !output.status.success() {
            let text = if output.stderr.is_empty() {
                &output.stdout
            } else {
                &output.stderr
            };
            return Err(String::from_utf8_lossy(text).chars().take(2048).collect());
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("invalid workspace setup result: {e}"))
    }

    pub fn apply(&self) -> Result<serde_json::Value, String> {
        let mut command = self.command("apply")?;
        let output = command
            .output()
            .map_err(|e| format!("workspace integration failed: {e}"))?;
        if !output.status.success() {
            let text = if output.stderr.is_empty() {
                &output.stdout
            } else {
                &output.stderr
            };
            return Err(String::from_utf8_lossy(text).chars().take(4096).collect());
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("invalid workspace integration result: {e}"))
    }
}

/// Seed selected credentials, config, plugins, skills and resume data into a
/// room's writable private home. Symlinks and special files are ignored;
/// copied files are independent inodes and total work is bounded.
pub fn private_home(
    profile_root: &Path,
    expected_home: &Path,
    resume_id: Option<&str>,
) -> Result<PathBuf, String> {
    let original = std::env::var_os("HOME").ok_or("HOME is unavailable")?;
    let expected = profile_root.join("home");
    if expected != expected_home {
        return Err("workspace helper returned an unexpected CLI home".into());
    }
    private_home_from(profile_root, Path::new(&original), resume_id)
}

fn private_home_from(
    profile_root: &Path,
    source_home: &Path,
    resume_id: Option<&str>,
) -> Result<PathBuf, String> {
    let home = profile_root.join("home");
    ensure_private_directory(&home)?;
    let mut budget = CopyBudget {
        entries: 0,
        bytes: 0,
    };
    for name in [".codex", ".claude", ".grok"] {
        let from = source_home.join(name);
        let to = home.join(name);
        if let Ok(metadata) = std::fs::symlink_metadata(&from) {
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                ensure_private_directory(&to)?;
                let files: &[&str] = match name {
                    ".codex" => &["config.toml", "auth.json", "version.json"],
                    ".claude" => &[
                        "settings.json",
                        ".credentials.json",
                        "credentials.json",
                        "config.json",
                    ],
                    _ => &[
                        "config.toml",
                        "config.json",
                        "settings.json",
                        "auth.json",
                        "credentials.json",
                        ".credentials.json",
                    ],
                };
                for file in files {
                    copy_file_if_missing(&from.join(file), &to.join(file), &mut budget)?;
                }
                for directory in [
                    "skills", "plugins", "rules", "prompts", "commands", "agents", "hooks",
                ] {
                    let src = from.join(directory);
                    if std::fs::symlink_metadata(&src)
                        .is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
                    {
                        let dst = to.join(directory);
                        ensure_private_directory(&dst)?;
                        copy_tree_bounded(&src, &dst, &mut budget)?;
                    }
                }
                if let Some(id) = resume_id.filter(|id| !id.is_empty() && id.len() <= 256) {
                    let session_dirs: &[&str] = match name {
                        ".claude" => &["projects", "sessions"],
                        _ => &["sessions", "history", "projects"],
                    };
                    for directory in session_dirs {
                        let src = from.join(directory);
                        if std::fs::symlink_metadata(&src)
                            .is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
                        {
                            copy_matching_session(&src, &to.join(directory), id, &mut budget, 0)?;
                        }
                    }
                }
            }
        }
    }
    copy_file_if_missing(
        &source_home.join(".claude.json"),
        &home.join(".claude.json"),
        &mut budget,
    )?;
    Ok(home)
}

#[cfg(test)]
pub fn private_home_from_test(
    profile_root: &Path,
    source_home: &Path,
    resume_id: Option<&str>,
) -> Result<PathBuf, String> {
    private_home_from(profile_root, source_home, resume_id)
}

#[derive(Default)]
struct CopyBudget {
    entries: usize,
    bytes: u64,
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    match std::fs::symlink_metadata(path) {
        Ok(meta)
            if !meta.is_dir() || meta.file_type().is_symlink() || meta.uid() != current_uid() =>
        {
            return Err(format!("unsafe CLI profile directory: {}", path.display()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(path)
                .map_err(|e| format!("create private profile directory: {e}"))?;
        }
        Err(error) => return Err(format!("inspect CLI profile directory: {error}")),
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("protect CLI profile directory: {e}"))
}

// Kept local to avoid adding a libc dependency just to inspect ownership.
#[cfg(unix)]
fn current_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self")
        .map(|meta| meta.uid())
        .unwrap_or(u32::MAX)
}

fn copy_tree_bounded(from: &Path, to: &Path, budget: &mut CopyBudget) -> Result<(), String> {
    let entries = std::fs::read_dir(from).map_err(|e| format!("read CLI profile: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read CLI profile entry: {e}"))?;
        budget.entries += 1;
        if budget.entries > PROFILE_MAX_ENTRIES {
            return Err("CLI profile exceeds the 50,000-entry limit".into());
        }
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let meta = std::fs::symlink_metadata(&src)
            .map_err(|e| format!("inspect CLI profile entry: {e}"))?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            match std::fs::symlink_metadata(&dst) {
                Ok(target) if target.is_dir() && !target.file_type().is_symlink() => {}
                Ok(_) => {
                    return Err(format!(
                        "unsafe private CLI profile entry: {}",
                        dst.display()
                    ))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::create_dir(&dst)
                        .map_err(|e| format!("create private CLI profile folder: {e}"))?;
                }
                Err(error) => return Err(format!("inspect private CLI profile folder: {error}")),
            }
            ensure_private_directory(&dst)?;
            copy_tree_bounded(&src, &dst, budget)?;
        } else if meta.is_file() {
            copy_file_if_missing(&src, &dst, budget)?;
        }
    }
    Ok(())
}

fn copy_matching_session(
    from: &Path,
    to: &Path,
    session_id: &str,
    budget: &mut CopyBudget,
    depth: usize,
) -> Result<(), String> {
    if depth > 20 {
        return Err("CLI session profile nesting exceeds the limit".into());
    }
    for entry in std::fs::read_dir(from).map_err(|e| format!("read CLI session profile: {e}"))? {
        let entry = entry.map_err(|e| format!("read CLI session entry: {e}"))?;
        budget.entries += 1;
        if budget.entries > PROFILE_MAX_ENTRIES {
            return Err("CLI profile exceeds the 50,000-entry limit".into());
        }
        let src = entry.path();
        let meta = std::fs::symlink_metadata(&src)
            .map_err(|e| format!("inspect CLI session entry: {e}"))?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let dst = to.join(entry.file_name());
        if meta.is_dir() {
            copy_matching_session(&src, &dst, session_id, budget, depth + 1)?;
        } else if meta.is_file() && entry.file_name().to_string_lossy().contains(session_id) {
            if let Some(parent) = dst.parent() {
                ensure_private_directory(parent)?;
            }
            copy_file_if_missing(&src, &dst, budget)?;
        }
    }
    Ok(())
}

fn copy_file_if_missing(from: &Path, to: &Path, budget: &mut CopyBudget) -> Result<(), String> {
    use std::io::{Read, Write};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let source_meta = match std::fs::symlink_metadata(from) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("inspect CLI profile file: {error}")),
    };
    if !source_meta.is_file() || source_meta.file_type().is_symlink() {
        return Ok(());
    }
    if source_meta.len() > PROFILE_MAX_FILE_BYTES
        || budget.bytes.saturating_add(source_meta.len()) > PROFILE_MAX_TREE_BYTES
    {
        return Err("CLI profile exceeds the 1 GiB size limit".into());
    }
    let mut source_options = std::fs::OpenOptions::new();
    source_options.read(true).custom_flags(0x20000); // O_NOFOLLOW
    let input = source_options
        .open(from)
        .map_err(|e| format!("open CLI profile file: {e}"))?;
    let mut options = std::fs::OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(0x20000); // O_NOFOLLOW
    let output = match options.open(to) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = std::fs::symlink_metadata(to)
                .map_err(|e| format!("inspect private CLI profile file: {e}"))?;
            if !existing.is_file() || existing.file_type().is_symlink() {
                return Err(format!("unsafe private CLI profile file: {}", to.display()));
            }
            return Ok(());
        }
        Err(error) => return Err(format!("create private CLI profile file: {error}")),
    };
    let mut reader = input.take(PROFILE_MAX_FILE_BYTES + 1);
    let mut writer = output;
    let copied = std::io::copy(&mut reader, &mut writer)
        .map_err(|e| format!("copy CLI profile file: {e}"))?;
    if copied != source_meta.len() {
        return Err(format!(
            "CLI profile file changed or exceeded limit: {}",
            from.display()
        ));
    }
    writer
        .flush()
        .map_err(|e| format!("flush CLI profile file: {e}"))?;
    writer
        .set_permissions(std::fs::Permissions::from_mode(
            source_meta.permissions().mode() & 0o700,
        ))
        .map_err(|e| format!("protect private CLI profile file: {e}"))?;
    budget.bytes += copied;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        std::env::temp_dir().join(format!(
            "orgbot-cli-profile-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ))
    }

    #[cfg(unix)]
    #[test]
    fn profile_copy_preserves_session_files_privately_and_refuses_second_turn_symlink() {
        use std::os::unix::fs::symlink;
        let root = temp();
        let source = root.join("original-home");
        let profiles = root.join("state/profiles");
        let home = profiles.join("home");
        std::fs::create_dir_all(source.join(".codex/sessions")).unwrap();
        std::fs::create_dir_all(source.join(".claude/projects")).unwrap();
        std::fs::write(source.join(".codex/auth.json"), "original-auth").unwrap();
        std::fs::write(
            source.join(".codex/sessions/rollout-resume-id-123.jsonl"),
            "resume-id-123",
        )
        .unwrap();
        std::fs::File::create(source.join(".codex/sessions/unrelated-large-session.jsonl"))
            .unwrap()
            .set_len(PROFILE_MAX_FILE_BYTES + 1)
            .unwrap();
        std::fs::write(
            source.join(".claude/projects/claude-session-456.jsonl"),
            "claude-session-456",
        )
        .unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(home.join(".grok")).unwrap();

        let private = private_home_from(&profiles, &source, Some("resume-id-123")).unwrap();
        let session = private.join(".codex/sessions/rollout-resume-id-123.jsonl");
        assert_eq!(std::fs::read_to_string(&session).unwrap(), "resume-id-123");
        std::fs::write(&session, "private-resume-state").unwrap();
        assert_eq!(
            std::fs::read_to_string(source.join(".codex/sessions/rollout-resume-id-123.jsonl"))
                .unwrap(),
            "resume-id-123"
        );
        private_home_from(&profiles, &source, Some("claude-session-456")).unwrap();
        assert_eq!(
            std::fs::read_to_string(private.join(".claude/projects/claude-session-456.jsonl"))
                .unwrap(),
            "claude-session-456"
        );

        let private_auth = private.join(".codex/auth.json");
        std::fs::remove_file(&private_auth).unwrap();
        symlink(source.join(".codex/auth.json"), &private_auth).unwrap();
        assert!(private_home_from(&profiles, &source, Some("resume-id-123")).is_err());
        assert!(write_private_file(&private_auth, b"attack").is_err());
        assert_eq!(
            std::fs::read_to_string(source.join(".codex/auth.json")).unwrap(),
            "original-auth"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn room_bridge_uses_inner_socket_and_replaces_only_generated_stanza() {
        let root = temp();
        let home = root.join("home");
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(
            home.join(".codex/config.toml"),
            "model = \"gpt-6.1-sol\"\n[mcp_servers.secret]\ncommand = \"outside-server\"\n",
        )
        .unwrap();
        update_private_agent_config(
            &home,
            orgbot_proto::agent::CliKind::Codex,
            Path::new("/run/user/1000/orgbot-home"),
            Path::new("/run/user/1000/orgbot-room-mcp.sock"),
        )
        .unwrap();
        let first = std::fs::read_to_string(home.join(".codex/config.toml")).unwrap();
        assert!(first.contains("/run/user/1000/orgbot-room-mcp.sock"));
        assert!(first.contains("model = \"gpt-6.1-sol\""));
        assert!(!first.contains("outside-server"));
        update_private_agent_config(
            &home,
            orgbot_proto::agent::CliKind::Codex,
            Path::new("/run/user/1000/orgbot-home"),
            Path::new("/run/user/1000/orgbot-room-mcp.sock"),
        )
        .unwrap();
        let second = std::fs::read_to_string(home.join(".codex/config.toml")).unwrap();
        assert_eq!(second.matches("[mcp_servers.orgbot_room]").count(), 1);
        assert!(!second.contains("/tmp/room.sock"));
        clear_private_agent_config(&home, orgbot_proto::agent::CliKind::Codex).unwrap();
        let clean = std::fs::read_to_string(home.join(".codex/config.toml")).unwrap();
        assert!(!clean.contains("orgbot_room"));
        assert!(clean.contains("model = \"gpt-6.1-sol\""));
        std::fs::remove_dir_all(root).unwrap();
    }
}
