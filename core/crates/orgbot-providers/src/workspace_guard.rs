//! The trusted launcher shared by native CLIs and local API workspace tools.
use std::path::{Path, PathBuf};
use std::process::Command;

const PROFILE_MAX_ENTRIES: usize = 50_000;
const PROFILE_MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const PROFILE_MAX_TREE_BYTES: u64 = 1024 * 1024 * 1024;
const OPENCODE_AUTH_MAX_BYTES: u64 = 4 * 1024 * 1024;
const OPENCODE_CONFIG_GITIGNORE: &[u8] =
    b"node_modules\npackage.json\npackage-lock.json\nbun.lock\n.gitignore";
const AGY_RELAY_MOUNT: &str = "/opt/orgbots/room-relay";
const AGY_NATIVE_MAX_ENTRIES: usize = 150_000;
const AGY_NATIVE_MAX_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const AGY_DEFAULT_PROJECT: &[u8] =
    br#"{"id":"default-cli-project","name":"CLI Project","projectResources":{}}"#;

#[derive(Debug, Clone)]
pub struct WorkspaceGuard {
    pub project: PathBuf,
    pub state: PathBuf,
    pub agent: String,
    pub read_only: bool,
    /// A name for the copy a person can read, kept with it so a copy left
    /// behind by a bot that has gone still says whose it was.
    pub label: Option<String>,
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
        orgbot_proto::agent::CliKind::Claude
        | orgbot_proto::agent::CliKind::OpenCode
        | orgbot_proto::agent::CliKind::Antigravity => return Ok(()),
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
    let relay = crate::agent_cli::resolve_mcp_relay_binary()?;
    contents.push_str(&format!(
        "\n{BEGIN}\n[{env_key}.orgbot_room]\ncommand = {}\nargs = [\"room-mcp\", \"--socket\", {}]\n",
        serde_json::to_string(&relay.to_string_lossy()).unwrap(),
        serde_json::to_string(&socket.to_string_lossy()).unwrap(),
    ));
    if cli == orgbot_proto::agent::CliKind::Codex {
        contents.push_str("default_tools_approval_mode = \"approve\"\n");
        // Room tools may run a workspace command for up to 30 minutes plus
        // its setup. From the live room: @mira's profile died with "timed
        // out awaiting tools/call after 300s". The 1 November fix reached
        // only an older config writer; this is the one bots run with.
        contents.push_str("tool_timeout_sec = 2100\n");
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
        orgbot_proto::agent::CliKind::Claude
        | orgbot_proto::agent::CliKind::OpenCode
        | orgbot_proto::agent::CliKind::Antigravity => return Ok(()),
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
    write_private_file(&config, &contents.as_bytes()[..start])
}

/// Remove copied external MCP registrations before any protected CLI launch.
/// Room calls are routed through the OrgBot bridge only when the room enabled it.
pub fn strip_private_agent_mcp_config(
    home: &Path,
    cli: orgbot_proto::agent::CliKind,
) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    // From the benchmark room: the Grok bots tried eighteen times to start an
    // MCP server ("unreal") from the owner's ~/.claude.json, which the private
    // home carries for Claude. No bot needs the owner's own servers, and
    // Claude bots already ignore them (--strict-mcp-config).
    strip_claude_json_mcp(&home.join(".claude.json"))?;
    let profile = match cli {
        orgbot_proto::agent::CliKind::Codex => ".codex",
        orgbot_proto::agent::CliKind::Grok => ".grok",
        orgbot_proto::agent::CliKind::Claude
        | orgbot_proto::agent::CliKind::OpenCode
        | orgbot_proto::agent::CliKind::Antigravity => return Ok(()),
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

/// Remove MCP servers from a private copy of Claude's `~/.claude.json`, at the
/// top level and under each project. Anything unreadable is left as it is:
/// the CLI owns that file and rewrites it.
fn strip_claude_json_mcp(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    match std::fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() || meta.nlink() != 1 => {
            return Err("unsafe private .claude.json".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("inspect private .claude.json: {error}")),
    }
    let Ok(contents) = read_private_file(path) else {
        return Ok(());
    };
    let Ok(mut parsed) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return Ok(());
    };
    let mut removed = false;
    if let Some(top) = parsed.as_object_mut() {
        removed |= top.remove("mcpServers").is_some();
        if let Some(projects) = top.get_mut("projects").and_then(serde_json::Value::as_object_mut) {
            for project in projects.values_mut() {
                if let Some(project) = project.as_object_mut() {
                    removed |= project.remove("mcpServers").is_some();
                }
            }
        }
    }
    if !removed {
        return Ok(());
    }
    let sanitized = serde_json::to_string_pretty(&parsed).map_err(|error| format!("sanitize private .claude.json: {error}"))?;
    write_private_file(path, sanitized.as_bytes()).map_err(|error| format!("sanitize {}: {error}", path.display()))
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

/// Generate isolated, deny-by-default profiles for CLIs whose native auth
/// boundaries have not been approved for exposure to the guarded process.
pub fn configure_new_cli_profile(
    home: &Path,
    mount: &Path,
    cli: orgbot_proto::agent::CliKind,
    _policy: orgbot_proto::agent::AgentPolicy,
    model: &str,
    room_socket: Option<&Path>,
) -> Result<(), String> {
    configure_new_cli_profile_inner(home, mount, cli, model, room_socket, None)
}

fn configure_new_cli_profile_inner(
    home: &Path,
    mount: &Path,
    cli: orgbot_proto::agent::CliKind,
    model: &str,
    room_socket: Option<&Path>,
    opencode_auth_source: Option<&Path>,
) -> Result<(), String> {
    use orgbot_proto::agent::CliKind;

    if !mount.is_absolute() {
        return Err("isolated CLI home mount must be absolute".into());
    }
    ensure_private_path(home)?;
    for directory in [".config", ".local", ".local/share", ".cache"] {
        ensure_private_directory(&home.join(directory))?;
    }

    let relay_command = room_socket
        .map(|socket| {
            let relay = crate::agent_cli::resolve_mcp_relay_binary()?;
            Ok::<_, String>((
                relay.to_string_lossy().into_owned(),
                socket.to_string_lossy().into_owned(),
            ))
        })
        .transpose()?;
    match cli {
        CliKind::Antigravity => {
            if !valid_agy_model_slug(model) {
                return Err("Antigravity model must be a valid explicit model slug".into());
            }
            if let Some(socket) = room_socket {
                if !socket.is_absolute() {
                    return Err("Antigravity room MCP socket must be absolute".into());
                }
                if relay_command.is_none() {
                    return Err("Antigravity room MCP relay is unavailable".into());
                }
            }

            let gemini = home.join(".gemini");
            let cli_config = gemini.join("antigravity-cli");
            let global_config = gemini.join("config");
            let plugins = global_config.join("plugins");
            let projects = global_config.join("projects");
            let settings = cli_config.join("settings.json");
            let mcp_config = global_config.join("mcp_config.json");
            let empty_agents = home.join(".orgbot-empty-agents");

            for path in [
                &gemini,
                &cli_config,
                &global_config,
                &plugins,
                &projects,
                &empty_agents,
                &home.join(".local/state"),
                &home.join(".cache"),
            ] {
                ensure_private_directory(path)?;
            }
            ensure_exact_entries(&gemini, &["antigravity-cli", "config"], "Antigravity home")?;
            ensure_agy_native_cli_data(&cli_config)?;
            ensure_exact_entries(&plugins, &[], "Antigravity plugins")?;
            ensure_allowed_entries(
                &projects,
                &["default-cli-project.json"],
                "Antigravity projects",
            )?;
            ensure_allowed_entries(
                &global_config,
                &["mcp_config.json", "plugins", "projects"],
                "Antigravity global config",
            )?;
            ensure_exact_entries(&empty_agents, &[], "Antigravity empty .agents mask")?;

            let mut deny = vec![
                "command(*)",
                "unsandboxed(*)",
                "read_file(*)",
                "write_file(*)",
                "read_url(*)",
                "execute_url(*)",
            ];
            if room_socket.is_none() {
                deny.push("mcp(*)");
            }
            let allow = if room_socket.is_some() {
                vec!["mcp(orgbot_room/*)"]
            } else {
                Vec::new()
            };
            let settings_value = serde_json::json!({
                "useG1Credits": false,
                "permissions": {
                    "allow": allow,
                    "ask": [],
                    "deny": deny
                }
            });
            let mcp_servers = if let Some((_relay, socket)) = &relay_command {
                serde_json::json!({
                    "orgbot_room": {
                        "command": AGY_RELAY_MOUNT,
                        "args": ["room-mcp", "--socket", socket]
                    }
                })
            } else {
                serde_json::json!({})
            };
            ensure_managed_private_file(
                &projects.join("default-cli-project.json"),
                AGY_DEFAULT_PROJECT,
                "Antigravity default project",
            )?;
            ensure_managed_private_file(
                &settings,
                &serde_json::to_vec(&settings_value)
                    .map_err(|e| format!("encode Antigravity settings: {e}"))?,
                "Antigravity settings",
            )?;
            ensure_managed_private_file(
                &mcp_config,
                &serde_json::to_vec(&serde_json::json!({"mcpServers": mcp_servers}))
                    .map_err(|e| format!("encode Antigravity MCP config: {e}"))?,
                "Antigravity MCP config",
            )?;
            ensure_agy_native_cli_data(&cli_config)?;
            ensure_exact_entries(
                &global_config,
                &["mcp_config.json", "plugins", "projects"],
                "Antigravity global config",
            )?;
            ensure_exact_entries(
                &projects,
                &["default-cli-project.json"],
                "Antigravity projects",
            )
        }
        CliKind::OpenCode => {
            let provider = opencode_provider(model)?;
            if let Some(source) = opencode_auth_source {
                copy_opencode_auth_from(source, home, &provider)?;
            } else {
                copy_opencode_auth(home, &provider)?;
            }
            let opencode_config_dir = home.join(".config/opencode");
            ensure_opencode_config_dir(&opencode_config_dir)?;
            let legacy_config_dir = home.join(".opencode");
            ensure_opencode_config_dir(&legacy_config_dir)?;
            ensure_private_directory(&home.join(".local/state"))?;
            ensure_private_directory(&home.join(".cache"))?;
            ensure_private_directory(&home.join(".orgbot-empty-opencode-system"))?;
            let mut mcp = serde_json::Map::new();
            if let Some((relay, socket)) = &relay_command {
                mcp.insert(
                    "orgbot_room".into(),
                    serde_json::json!({
                        "type": "local",
                        "command": [relay, "room-mcp", "--socket", socket],
                        "enabled": true
                    }),
                );
            }
            let mut permission = serde_json::Map::new();
            permission.insert("*".into(), serde_json::json!("deny"));
            if room_socket.is_some() {
                permission.insert("orgbot_room_*".into(), serde_json::json!("allow"));
            }
            let config = serde_json::json!({
                "$schema": "https://opencode.ai/config.json",
                "mcp": mcp,
                "permission": permission,
                "plugin": []
            });
            write_private_file(
                &home.join("opencode.json"),
                &serde_json::to_vec(&config)
                    .map_err(|e| format!("encode OpenCode profile: {e}"))?,
            )
        }
        _ => {
            Err("isolated profile generation is only available for OpenCode and Antigravity".into())
        }
    }
}

fn valid_agy_model_slug(model: &str) -> bool {
    !model.is_empty()
        && model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn ensure_exact_entries(directory: &Path, expected: &[&str], label: &str) -> Result<(), String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("read {label} directory: {error}"))?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .map_err(|error| format!("read {label} entry: {error}"))
        })
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    let expected = expected
        .iter()
        .map(|entry| (*entry).to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    if entries != expected {
        return Err(format!("{label} contains unmanaged files"));
    }
    Ok(())
}

fn ensure_allowed_entries(directory: &Path, allowed: &[&str], label: &str) -> Result<(), String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("read {label} directory: {error}"))?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .map_err(|error| format!("read {label} entry: {error}"))
        })
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    if entries
        .iter()
        .any(|entry| !allowed.iter().any(|candidate| *candidate == entry))
    {
        return Err(format!("{label} contains unmanaged files"));
    }
    Ok(())
}

fn ensure_agy_native_cli_data(root: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;

    let settings = root.join("settings.json");
    let mut pending = vec![root.to_path_buf()];
    let mut entries = 0usize;
    let mut bytes = 0u64;
    while let Some(directory) = pending.pop() {
        let directory_meta = std::fs::symlink_metadata(&directory)
            .map_err(|error| format!("inspect Antigravity CLI data directory: {error}"))?;
        if !directory_meta.is_dir()
            || directory_meta.file_type().is_symlink()
            || directory_meta.uid() != current_uid()
            || directory_meta.mode() & 0o077 != 0
        {
            return Err("Antigravity CLI data contains an unsafe directory".into());
        }
        for entry in std::fs::read_dir(&directory)
            .map_err(|error| format!("read Antigravity CLI data: {error}"))?
        {
            let path = entry
                .map_err(|error| format!("read Antigravity CLI data entry: {error}"))?
                .path();
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|error| format!("inspect Antigravity CLI data entry: {error}"))?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                if metadata.uid() != current_uid() || metadata.mode() & 0o077 != 0 {
                    return Err("Antigravity CLI data contains an unsafe directory".into());
                }
                entries += 1;
                pending.push(path);
            } else if metadata.is_file() && !metadata.file_type().is_symlink() {
                if metadata.uid() != current_uid()
                    || metadata.mode() & 0o077 != 0
                    || metadata.nlink() != 1
                {
                    return Err("Antigravity CLI data contains an unsafe file".into());
                }
                if path != settings {
                    entries += 1;
                    bytes = bytes
                        .checked_add(metadata.len())
                        .ok_or("Antigravity CLI data size overflow")?;
                }
            } else if path == root.join("cli.log") && metadata.file_type().is_symlink() {
                let target = ensure_agy_cli_log_link(root, &path, &metadata)?;
                entries += 1;
                bytes = bytes
                    .checked_add(target.len())
                    .ok_or("Antigravity CLI data size overflow")?;
            } else {
                return Err("Antigravity CLI data contains an unsafe entry".into());
            }
            if entries > AGY_NATIVE_MAX_ENTRIES || bytes > AGY_NATIVE_MAX_BYTES {
                return Err("Antigravity CLI data exceeds its safety limits".into());
            }
        }
    }
    Ok(())
}

fn ensure_agy_cli_log_link(
    root: &Path,
    link: &Path,
    link_metadata: &std::fs::Metadata,
) -> Result<std::fs::Metadata, String> {
    use std::os::unix::fs::MetadataExt;

    if link_metadata.uid() != current_uid() || link_metadata.nlink() != 1 {
        return Err("Antigravity CLI data contains an unsafe entry".into());
    }
    let relative = std::fs::read_link(link)
        .map_err(|error| format!("inspect Antigravity CLI log link: {error}"))?;
    let name = relative
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("Antigravity CLI log link has an invalid target")?;
    let target_name_valid = name.len() == 23
        && name.starts_with("cli-")
        && name.as_bytes()[4..12].iter().all(u8::is_ascii_digit)
        && name.as_bytes()[12] == b'_'
        && name.as_bytes()[13..19].iter().all(u8::is_ascii_digit)
        && name.ends_with(".log");
    if relative != Path::new("log").join(name) || !target_name_valid {
        return Err("Antigravity CLI data contains an unsafe entry".into());
    }
    let log_dir = root.join("log");
    let directory = std::fs::symlink_metadata(&log_dir)
        .map_err(|error| format!("inspect Antigravity CLI log directory: {error}"))?;
    if !directory.is_dir()
        || directory.file_type().is_symlink()
        || directory.uid() != current_uid()
        || directory.mode() & 0o077 != 0
    {
        return Err("Antigravity CLI data contains an unsafe directory".into());
    }
    let target = log_dir.join(name);
    let target_metadata = std::fs::symlink_metadata(&target)
        .map_err(|error| format!("inspect Antigravity CLI log target: {error}"))?;
    if !target_metadata.is_file()
        || target_metadata.file_type().is_symlink()
        || target_metadata.uid() != current_uid()
        || target_metadata.mode() & 0o077 != 0
        || target_metadata.nlink() != 1
    {
        return Err("Antigravity CLI data contains an unsafe log target".into());
    }
    Ok(target_metadata)
}

fn ensure_managed_private_file(path: &Path, bytes: &[u8], label: &str) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    match std::fs::symlink_metadata(path) {
        Ok(metadata)
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.uid() != current_uid()
                || metadata.nlink() != 1
                || metadata.mode() & 0o077 != 0 =>
        {
            Err(format!("unsafe {label} file"))
        }
        // From the 13 November audit: a managed file that no longer matched
        // (the room's tools were switched on or off, or the room socket moved)
        // stopped the bot from ever starting again. A file of ours that is
        // out of date is replaced; one that is not ours is still refused.
        Ok(_) => {
            if std::fs::read(path).map_err(|error| format!("read {label}: {error}"))? != bytes {
                write_private_file(path, bytes)?;
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_private_file(path, bytes)
        }
        Err(error) => Err(format!("inspect {label}: {error}")),
    }
}

fn opencode_provider(model: &str) -> Result<String, String> {
    let (provider, model_name) = model
        .split_once('/')
        .ok_or("OpenCode model must include a provider prefix")?;
    if provider.is_empty()
        || model_name.is_empty()
        || !provider
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("OpenCode model has an invalid provider prefix".into());
    }
    Ok(provider.to_owned())
}

fn ensure_opencode_config_dir(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    ensure_private_directory(path)?;
    let entries = std::fs::read_dir(path)
        .map_err(|e| format!("read private OpenCode config directory: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("read private OpenCode config entry: {e}"))?;
    match entries.as_slice() {
        [] => write_private_file(&path.join(".gitignore"), OPENCODE_CONFIG_GITIGNORE),
        [entry] if entry.file_name() == ".gitignore" => {
            let marker = entry.path();
            let meta = std::fs::symlink_metadata(&marker)
                .map_err(|e| format!("inspect OpenCode config marker: {e}"))?;
            if !meta.is_file()
                || meta.file_type().is_symlink()
                || meta.uid() != current_uid()
                || meta.nlink() != 1
                || meta.mode() & 0o077 != 0
                || std::fs::read(&marker)
                    .map_err(|e| format!("read OpenCode config marker: {e}"))?
                    != OPENCODE_CONFIG_GITIGNORE
            {
                return Err("private OpenCode config directory has an unsafe marker".into());
            }
            Ok(())
        }
        _ => Err("private OpenCode config directory contains unmanaged files".into()),
    }
}

fn clear_private_opencode_auth(destination: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    match std::fs::symlink_metadata(destination) {
        Ok(meta)
            if !meta.is_file()
                || meta.file_type().is_symlink()
                || meta.uid() != current_uid()
                || meta.nlink() != 1 =>
        {
            Err("unsafe private OpenCode auth file".into())
        }
        Ok(_) => std::fs::remove_file(destination)
            .map_err(|e| format!("remove stale private OpenCode auth: {e}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("inspect private OpenCode auth file: {error}")),
    }
}

fn copy_opencode_auth(home: &Path, provider: &str) -> Result<(), String> {
    let source_home = std::env::var_os("HOME").ok_or("HOME is unavailable")?;
    let source_data = match std::env::var_os("XDG_DATA_HOME") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(source_home).join(".local/share"),
    };
    if !source_data.is_absolute() {
        return Err("OpenCode XDG data directory must be absolute".into());
    }
    copy_opencode_auth_from(&source_data.join("opencode/auth.json"), home, provider)
}

fn copy_opencode_auth_from(source: &Path, home: &Path, provider: &str) -> Result<(), String> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let destination = home.join(".local/share/opencode/auth.json");
    ensure_private_directory(&home.join(".local"))?;
    ensure_private_directory(&home.join(".local/share"))?;
    ensure_private_directory(&home.join(".local/share/opencode"))?;
    match std::fs::symlink_metadata(&destination) {
        Ok(meta)
            if !meta.is_file()
                || meta.file_type().is_symlink()
                || meta.uid() != current_uid()
                || meta.nlink() != 1
                || meta.mode() & 0o077 != 0 =>
        {
            return Err("unsafe private OpenCode auth file".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("inspect private OpenCode auth file: {error}")),
    }
    let parent = source
        .parent()
        .ok_or("OpenCode auth file has no parent directory")?;
    if !source.is_absolute()
        || parent
            .canonicalize()
            .map_err(|e| format!("verify OpenCode auth directory: {e}"))?
            != parent
    {
        return Err("OpenCode auth file path contains symlink or relative components".into());
    }
    let meta = match std::fs::symlink_metadata(source) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if std::fs::symlink_metadata(&destination).is_ok() {
                std::fs::remove_file(&destination)
                    .map_err(|e| format!("remove stale private OpenCode auth: {e}"))?;
            }
            return Err(
                "OpenCode auth.json was not found; sign in to OpenCode on this host first".into(),
            );
        }
        Err(error) => return Err(format!("inspect OpenCode auth file: {error}")),
    };
    if !meta.is_file()
        || meta.file_type().is_symlink()
        || meta.uid() != current_uid()
        || meta.nlink() != 1
        || meta.len() > OPENCODE_AUTH_MAX_BYTES
    {
        return Err("OpenCode auth file is unsafe or exceeds the 4 MiB limit".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).custom_flags(0x20000); // O_NOFOLLOW
    let input = options
        .open(source)
        .map_err(|e| format!("open OpenCode auth file: {e}"))?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    input
        .take(OPENCODE_AUTH_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read OpenCode auth file: {e}"))?;
    if bytes.len() as u64 != meta.len() || bytes.len() as u64 > OPENCODE_AUTH_MAX_BYTES {
        return Err("OpenCode auth file changed or exceeds the 4 MiB limit".into());
    }
    let auth: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(auth) => auth,
        Err(_) => {
            clear_private_opencode_auth(&destination)?;
            return Err("OpenCode auth.json is not valid JSON".into());
        }
    };
    let providers = auth
        .as_object()
        .ok_or("OpenCode auth.json has an unsupported format")?;
    let Some(selected) = providers.get(provider) else {
        clear_private_opencode_auth(&destination)?;
        return Err(format!(
            "OpenCode auth.json has no credential for provider {provider}"
        ));
    };
    let selected = selected
        .as_object()
        .ok_or("OpenCode provider credential has an unsupported format")?;
    let kind = selected
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or("OpenCode provider credential has no supported type")?;
    if kind == "oauth" {
        clear_private_opencode_auth(&destination)?;
        return Err("OpenCode OAuth credential sharing is not verified; use the CLI directly. OrgBots will not change authentication or billing.".into());
    }
    if kind != "api" {
        clear_private_opencode_auth(&destination)?;
        return Err(format!(
            "OpenCode provider credential type {kind} is not allowed in the protected workspace"
        ));
    }
    let Some(key) = selected
        .get("key")
        .and_then(serde_json::Value::as_str)
        .filter(|key| !key.trim().is_empty())
    else {
        clear_private_opencode_auth(&destination)?;
        return Err(format!(
            "OpenCode API credential for provider {provider} has no non-empty key"
        ));
    };
    let scoped = serde_json::json!({provider: {"type": "api", "key": key}});
    let scoped_bytes = serde_json::to_vec(&scoped)
        .map_err(|e| format!("encode selected OpenCode credential: {e}"))?;
    write_private_file(&destination, &scoped_bytes)?;
    let copied = std::fs::symlink_metadata(&destination)
        .map_err(|e| format!("inspect copied OpenCode auth file: {e}"))?;
    if !copied.is_file()
        || copied.file_type().is_symlink()
        || copied.uid() != current_uid()
        || copied.nlink() != 1
        || copied.mode() & 0o077 != 0
        || copied.len() != scoped_bytes.len() as u64
    {
        return Err("copied OpenCode auth file failed private-profile validation".into());
    }
    Ok(())
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
        // Which project's copy, for a bot that has one for an earlier project
        // folder too. From the 13 November audit: a second manifest made
        // every command fail.
        if matches!(operation, "run" | "status" | "apply" | "set-aside") {
            command.arg("--project").arg(&self.project);
        }
        Ok(command)
    }

    pub fn command_for_agent_cli(
        &self,
        operation: &str,
        cli: orgbot_proto::agent::CliKind,
        mcp: bool,
    ) -> Result<Command, String> {
        let mut command = self.command(operation)?;
        if cli == orgbot_proto::agent::CliKind::Antigravity && mcp {
            command
                .arg("--mcp-relay")
                .arg(crate::agent_cli::resolve_mcp_relay_binary()?);
        }
        Ok(command)
    }

    pub fn prepare(&self) -> Result<PreparedWorkspace, String> {
        let mut command = self.command("prepare")?;
        command.arg("--project").arg(&self.project);
        if self.read_only {
            command.arg("--read-only");
        }
        if let Some(label) = &self.label {
            command.arg("--label").arg(label);
        }
        let output = output_within(&mut command, HELPER_TIME_LIMIT)
            .map_err(|e| format!("workspace setup failed: {e}"))?;
        if !output.status.success() {
            return Err(helper_failure(&output));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("invalid workspace setup result: {e}"))
    }

    pub fn apply(&self) -> Result<serde_json::Value, String> {
        self.apply_inner(false)
    }

    /// Merge a bot's own changes, setting aside any file that conflicts with
    /// a newer version in the project: the bot's version is kept in its
    /// private home and its copy takes the project's, so the rest merges and
    /// the bot can keep working. The result's `note` says what was set aside.
    pub fn apply_setting_aside_conflicts(&self) -> Result<serde_json::Value, String> {
        self.apply_inner(true)
    }

    fn apply_inner(&self, set_aside: bool) -> Result<serde_json::Value, String> {
        let mut command = self.command("apply")?;
        if set_aside {
            command.arg("--set-aside-conflicts");
        }
        let output = output_within(&mut command, HELPER_TIME_LIMIT)
            .map_err(|e| format!("workspace integration failed: {e}"))?;
        if !output.status.success() {
            return Err(helper_failure(&output));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("invalid workspace integration result: {e}"))
    }

    /// Put the changes a bot made while it held no task aside, out of its
    /// copy, so they never merge under a task it takes later. The copy takes
    /// the project's files back; the bot's versions go to a leftovers folder
    /// the owner can open and bots cannot.
    ///
    /// From the 13 November audit: held changes merged at the end of the
    /// first later turn in which the bot held any task, under that task.
    pub fn set_aside_changes(&self) -> Result<serde_json::Value, String> {
        let mut command = self.command("set-aside")?;
        let output = output_within(&mut command, HELPER_TIME_LIMIT)
            .map_err(|e| format!("setting changes aside failed: {e}"))?;
        if !output.status.success() {
            return Err(helper_failure(&output));
        }
        serde_json::from_slice(&output.stdout).map_err(|e| format!("invalid set-aside result: {e}"))
    }
}

/// Why a helper failed: its `{"ok": false, "error"}` on stdout first, then
/// the end of what it wrote to stderr. From the 13 November audit: stderr
/// came first, and it holds progress lines, so a failed merge reported
/// `{"progress": ...}` instead of the error.
fn helper_failure(output: &std::process::Output) -> String {
    let reported = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .ok()
        .and_then(|value| value.get("error").and_then(serde_json::Value::as_str).map(str::to_owned));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let notes: Vec<&str> = stderr
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with("{\"progress\""))
        .collect();
    let tail = notes[notes.len().saturating_sub(5)..].join("\n");
    let tail: String = tail.chars().rev().take(2048).collect::<Vec<_>>().into_iter().rev().collect();
    match (reported, tail.is_empty()) {
        (Some(error), true) => error,
        (Some(error), false) => format!("{error}\n{tail}"),
        (None, false) => tail,
        (None, true) => String::from_utf8_lossy(&output.stdout).chars().take(2048).collect(),
    }
}

/// How long one prepare or merge may take, waiting for the project lock
/// included. From the audit after speed test 1: the helper ran with no limit
/// while holding the lock every bot's merge needs, so one that hung would
/// have stopped the whole room's work from landing.
const HELPER_TIME_LIMIT: std::time::Duration = std::time::Duration::from_secs(20 * 60);

/// Run a helper and collect its output, stopping it once `limit` passes.
fn output_within(
    command: &mut Command,
    limit: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(stream) = stdout.as_mut() {
            let _ = stream.read_to_end(&mut buffer);
        }
        buffer
    });
    let err = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(stream) = stderr.as_mut() {
            let _ = stream.read_to_end(&mut buffer);
        }
        buffer
    });
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "the workspace helper ran over {} minutes and was stopped, so other bots' work can land",
                    limit.as_secs() / 60
                ),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    Ok(std::process::Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

/// Seed selected credentials, config, plugins, skills and resume data into a
/// room's writable private home. Symlinks and special files are ignored;
/// copied files are independent inodes and total work is bounded.
pub fn private_home(
    profile_root: &Path,
    expected_home: &Path,
    resume_id: Option<&str>,
    cli_dir: &str,
) -> Result<PathBuf, String> {
    let original = std::env::var_os("HOME").ok_or("HOME is unavailable")?;
    let expected = profile_root.join("home");
    if expected != expected_home {
        return Err("workspace helper returned an unexpected CLI home".into());
    }
    private_home_from(profile_root, Path::new(&original), resume_id, cli_dir)
}

/// Create an empty private HOME for CLIs that must not inherit any other
/// provider's credentials, extensions, or settings.
pub fn private_home_for_new_cli(home: &Path) -> Result<PathBuf, String> {
    let profile_root = home
        .parent()
        .ok_or("private CLI home has no profile root")?;
    ensure_private_path(profile_root)?;
    ensure_private_directory(profile_root)?;
    ensure_private_directory(home)?;
    Ok(home.to_path_buf())
}

fn ensure_private_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("private CLI home path must be absolute".into());
    }
    let mut current = PathBuf::from("/");
    for component in path.components() {
        match component {
            std::path::Component::RootDir => continue,
            std::path::Component::Normal(part) => current.push(part),
            _ => return Err("private CLI home contains an unsafe path component".into()),
        }
        let metadata = std::fs::symlink_metadata(&current)
            .map_err(|e| format!("inspect private CLI home path: {e}"))?;
        if metadata.file_type().is_symlink() {
            return Err("private CLI home path contains a symlink".into());
        }
    }
    Ok(())
}

fn private_home_from(
    profile_root: &Path,
    source_home: &Path,
    resume_id: Option<&str>,
    cli_dir: &str,
) -> Result<PathBuf, String> {
    let home = profile_root.join("home");
    ensure_private_directory(&home)?;
    let mut budget = CopyBudget {
        entries: 0,
        bytes: 0,
    };
    let mut seeded = SeedStamp::load(profile_root);
    // Only the bot's own CLI. From the 13 November audit: every bot was
    // seeded with all three CLIs' sign-ins and configs, so a Grok bot could
    // read the owner's Claude and Codex sign-ins.
    for name in [".codex", ".claude", ".grok"].into_iter().filter(|name| *name == cli_dir) {
        let mut from = source_home.join(name);
        let to = home.join(name);
        // From the 13 November audit: an owner whose folder is a link (stow,
        // chezmoi) had nothing seeded, and their bots were told to sign in
        // again. A link to a folder of the owner's own is followed, once.
        if std::fs::symlink_metadata(&from).is_ok_and(|meta| meta.file_type().is_symlink()) {
            use std::os::unix::fs::MetadataExt;
            match std::fs::canonicalize(&from) {
                Ok(target)
                    if std::fs::symlink_metadata(&target)
                        .is_ok_and(|meta| meta.is_dir() && meta.uid() == current_uid()) =>
                {
                    from = target;
                }
                _ => continue,
            }
        }
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
                    if credential_files(name).contains(file) {
                        // Sign-ins follow the owner: a newer real file (after
                        // signing in again) replaces the bot's copy.
                        refresh_credential(&from.join(file), &to.join(file), &mut budget)?;
                    } else {
                        copy_file_following_owner(&from.join(file), &to.join(file), &mut budget)?;
                    }
                }
                for directory in [
                    "skills", "plugins", "rules", "prompts", "commands", "agents", "hooks",
                ] {
                    let src = from.join(directory);
                    let Ok(meta) = std::fs::symlink_metadata(&src) else {
                        continue;
                    };
                    if meta.is_dir() && !meta.file_type().is_symlink() {
                        let dst = to.join(directory);
                        let key = format!("{name}/{directory}");
                        if seeded.tree_current(&key, &meta, &dst) {
                            continue;
                        }
                        ensure_private_directory(&dst)?;
                        copy_tree_bounded(&src, &dst, &mut budget)?;
                        seeded.walked(key, &meta);
                    }
                }
                if let Some(id) = resume_id
                    .filter(|id| !id.is_empty() && id.len() <= 256)
                    .filter(|id| !seeded.session_checked(name, id))
                {
                    let session_dirs: &[&str] = match name {
                        ".claude" => &["projects", "sessions"],
                        _ => &["sessions", "history", "projects"],
                    };
                    // Looking is not copying: the search has its own limit, and
                    // reaching it means "not found". From the 13 November
                    // audit: walking an owner's 50,000 history files counted
                    // against the copy budget, and every resumed turn failed.
                    let mut looked = 0;
                    for directory in session_dirs {
                        let src = from.join(directory);
                        if std::fs::symlink_metadata(&src)
                            .is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
                        {
                            copy_matching_session(&src, &to.join(directory), id, &mut budget, &mut looked, 0)?;
                        }
                    }
                    seeded.checked_session(name, id);
                }
            }
        }
    }
    if cli_dir == ".claude" {
        copy_file_if_missing(
            &source_home.join(".claude.json"),
            &home.join(".claude.json"),
            &mut budget,
        )?;
    }
    seeded.save();
    Ok(home)
}

/// How often a bot's copy of the owner's skills, plugins and the like is
/// checked for new files when the folder itself looks unchanged: files added
/// deep inside a folder don't change the folder's own time.
const PROFILE_TREE_RECHECK_SECS: u64 = 10 * 60;

/// What a bot's private home was last seeded from, so a turn walks the
/// owner's folders only when they may have changed. From the audit after
/// speed test 1: every CLI turn walked about 12,000 entries of the owner's
/// .claude, .codex and .grok folders, and every session folder, to copy
/// nothing new.
struct SeedStamp {
    path: PathBuf,
    data: serde_json::Value,
    changed: bool,
}

impl SeedStamp {
    fn load(profile_root: &Path) -> Self {
        let path = profile_root.join("home-seeded.json");
        let data = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .filter(serde_json::Value::is_object)
            .unwrap_or_else(|| serde_json::json!({}));
        Self { path, data, changed: false }
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0)
    }

    fn stamp_of(meta: &std::fs::Metadata) -> String {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}:{}:{}", meta.dev(), meta.ino(), meta.mtime(), meta.mtime_nsec())
    }

    /// The bot's copy of this folder exists and was filled from the folder
    /// as it is now, recently enough.
    fn tree_current(&self, key: &str, source: &std::fs::Metadata, copy: &Path) -> bool {
        let Some(entry) = self.data.get("trees").and_then(|trees| trees.get(key)) else {
            return false;
        };
        let same = entry.get("stamp").and_then(serde_json::Value::as_str) == Some(Self::stamp_of(source).as_str());
        let recent = entry
            .get("walked_at")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|at| Self::now().saturating_sub(at) < PROFILE_TREE_RECHECK_SECS);
        same && recent
            && std::fs::symlink_metadata(copy).is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
    }

    fn walked(&mut self, key: String, source: &std::fs::Metadata) {
        let entry = serde_json::json!({ "stamp": Self::stamp_of(source), "walked_at": Self::now() });
        if !self.data.get("trees").is_some_and(serde_json::Value::is_object) {
            self.data["trees"] = serde_json::json!({});
        }
        self.data["trees"][key] = entry;
        self.changed = true;
    }

    /// The owner's folders were already searched for this session: a bot's
    /// session lives in its own home from its first turn on.
    fn session_checked(&self, cli_dir: &str, id: &str) -> bool {
        let key = format!("{cli_dir}/{id}");
        self.data
            .get("sessions")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|ids| ids.iter().any(|known| known.as_str() == Some(key.as_str())))
    }

    fn checked_session(&mut self, cli_dir: &str, id: &str) {
        let key = serde_json::Value::String(format!("{cli_dir}/{id}"));
        let mut ids = self
            .data
            .get("sessions")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        if ids.contains(&key) {
            return;
        }
        ids.push(key);
        let excess = ids.len().saturating_sub(64);
        ids.drain(..excess);
        self.data["sessions"] = serde_json::Value::Array(ids);
        self.changed = true;
    }

    fn save(&self) {
        if self.changed {
            // Only a hint: losing it means one more walk.
            let _ = write_private_file(&self.path, self.data.to_string().as_bytes());
        }
    }
}

#[cfg(test)]
pub fn private_home_from_test(
    profile_root: &Path,
    source_home: &Path,
    resume_id: Option<&str>,
    cli_dir: &str,
) -> Result<PathBuf, String> {
    private_home_from(profile_root, source_home, resume_id, cli_dir)
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
            copy_file_following_owner(&src, &dst, budget)?;
        }
    }
    Ok(())
}

/// How many entries of the owner's CLI history a session search looks at.
const SESSION_SEARCH_LIMIT: usize = 500_000;

fn copy_matching_session(
    from: &Path,
    to: &Path,
    session_id: &str,
    budget: &mut CopyBudget,
    looked: &mut usize,
    depth: usize,
) -> Result<(), String> {
    if depth > 20 {
        return Ok(());
    }
    for entry in std::fs::read_dir(from).map_err(|e| format!("read CLI session profile: {e}"))? {
        let entry = entry.map_err(|e| format!("read CLI session entry: {e}"))?;
        *looked += 1;
        if *looked > SESSION_SEARCH_LIMIT {
            return Ok(());
        }
        let src = entry.path();
        let meta = std::fs::symlink_metadata(&src)
            .map_err(|e| format!("inspect CLI session entry: {e}"))?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let dst = to.join(entry.file_name());
        if meta.is_dir() {
            copy_matching_session(&src, &dst, session_id, budget, looked, depth + 1)?;
        } else if meta.is_file() && entry.file_name().to_string_lossy().contains(session_id) {
            // Codex keeps sessions by date, folders deep; each is made.
            if let Some(parent) = dst.parent() {
                let mut missing = Vec::new();
                let mut current = parent;
                while std::fs::symlink_metadata(current).is_err() {
                    missing.push(current.to_path_buf());
                    let Some(up) = current.parent() else { break };
                    current = up;
                }
                for folder in missing.iter().rev() {
                    ensure_private_directory(folder)?;
                }
                ensure_private_directory(parent)?;
            }
            copy_file_if_missing(&src, &dst, budget)?;
        }
    }
    Ok(())
}

/// The sign-in files each agent CLI keeps in its config folder.
///
/// These hold refresh tokens that are replaced each time they are used, so a
/// copy that never syncs goes stale: a bot keeps an expired sign-in after the
/// owner signs in again, and bots refreshing their own copies invalidate each
/// other's and the owner's. They are kept in step with the real file instead.
fn credential_files(cli_dir: &str) -> &'static [&'static str] {
    match cli_dir {
        ".claude" => &[".credentials.json", "credentials.json"],
        ".codex" => &["auth.json"],
        ".grok" => &["auth.json", "credentials.json", ".credentials.json"],
        _ => &[],
    }
}

/// A regular, non-symlinked file's modification time, if it is one.
fn regular_file_mtime(path: &Path) -> Option<std::time::SystemTime> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    (meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= PROFILE_MAX_FILE_BYTES)
        .then(|| meta.modified().ok())
        .flatten()
}

/// Replace `to` with `from` atomically, private to the owner.
fn replace_private_file(from: &Path, to: &Path) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let bytes = std::fs::read(from).map_err(|e| format!("read CLI sign-in: {e}"))?;
    let parent = to.parent().ok_or("CLI sign-in has no folder")?;
    let temp = parent.join(format!(
        ".orgbot-signin-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(0x20000) // O_NOFOLLOW
            .open(&temp)?;
        file.write_all(&bytes)?;
        // Keep the original's time, so "newer" compares sign-ins rather than
        // when a copy happened to be made.
        if let Ok(modified) = std::fs::metadata(from).and_then(|m| m.modified()) {
            file.set_modified(modified)?;
        }
        file.sync_all()?;
        std::fs::rename(&temp, to)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(|e| format!("update CLI sign-in: {e}"))
}

/// Whether a file holds a sign-in: a non-empty JSON object, not a placeholder
/// or a half-written file. Nothing else is ever copied in either direction.
fn holds_sign_in(path: &Path) -> Option<serde_json::Map<String, serde_json::Value>> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice::<serde_json::Value>(&bytes).ok()? {
        serde_json::Value::Object(map) if !map.is_empty() => Some(map),
        _ => None,
    }
}

/// Bring a bot's copy of a sign-in up to date with the owner's.
fn refresh_credential(source: &Path, copy: &Path, budget: &mut CopyBudget) -> Result<(), String> {
    // A bot with no copy yet gets the owner's file as it is, as it always has;
    // an existing copy that is unsafe (a symlink) is refused here.
    let had_copy = std::fs::symlink_metadata(copy).is_ok();
    copy_file_if_missing(source, copy, budget)?;
    if !had_copy {
        // Keep the original's time, so later comparisons are between sign-ins.
        if let (Ok(modified), Ok(file)) = (
            std::fs::metadata(source).and_then(|m| m.modified()),
            std::fs::File::options().write(true).open(copy),
        ) {
            let _ = file.set_modified(modified);
        }
        return Ok(());
    }
    let Some(source_time) = regular_file_mtime(source) else {
        return Ok(());
    };
    if holds_sign_in(source).is_none() {
        // A damaged owner file does not replace a bot's copy.
        return Ok(());
    }
    let newer = regular_file_mtime(copy).is_none_or(|copy_time| source_time > copy_time);
    if !newer || std::fs::read(source).ok() == std::fs::read(copy).ok() {
        return Ok(());
    }
    replace_private_file(source, copy)
}

/// The longest a refreshed sign-in may say it is good for. A file claiming
/// years was written by hand, not by a refresh.
const SIGN_IN_MAX_LIFETIME_MS: i64 = 30 * 24 * 3_600_000;
/// How many of the owner's earlier sign-ins are kept.
const SIGN_IN_BACKUPS_KEPT: usize = 5;

/// Where the owner's earlier sign-ins are kept: OrgBots' own data folder,
/// which the sandbox hides from every bot. From the 13 November audit: the
/// backup sat beside the sign-in as `<name>.orgbot-previous`, readable from
/// every sandbox, and a second hand-back overwrote it.
fn sign_in_backup_dir(owner_home: &Path, cli_dir: &str) -> PathBuf {
    owner_home
        .join(".local/share/orgbot/sign-in-backups")
        .join(cli_dir.trim_start_matches('.'))
}

/// Keep the owner's current sign-in before it is replaced, with the last few.
fn keep_previous_sign_in(owner_home: &Path, cli_dir: &str, file: &str, current: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let folder = sign_in_backup_dir(owner_home, cli_dir);
    std::fs::create_dir_all(&folder).map_err(|e| format!("keep the previous sign-in: {e}"))?;
    for dir in [folder.parent().unwrap_or(&folder), folder.as_path()] {
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    write_sign_in(current, None, &folder.join(format!("{file}.{stamp}")))?;
    let mut kept: Vec<PathBuf> = std::fs::read_dir(&folder)
        .map_err(|e| format!("list the kept sign-ins: {e}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.strip_prefix(file).is_some_and(|rest| rest.starts_with('.')))
        })
        .collect();
    kept.sort();
    let excess = kept.len().saturating_sub(SIGN_IN_BACKUPS_KEPT);
    for old in kept.into_iter().take(excess) {
        let _ = std::fs::remove_file(old);
    }
    Ok(())
}

/// Write a sign-in from bytes already read and checked, atomically and
/// private to the owner, never through a link.
fn write_sign_in(bytes: &[u8], modified: Option<std::time::SystemTime>, to: &Path) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let parent = to.parent().ok_or("CLI sign-in has no folder")?;
    let temp = parent.join(format!(
        ".orgbot-signin-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(0x20000) // O_NOFOLLOW
            .open(&temp)?;
        file.write_all(bytes)?;
        if let Some(modified) = modified {
            file.set_modified(modified)?;
        }
        file.sync_all()?;
        std::fs::rename(&temp, to)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(|e| format!("update CLI sign-in: {e}"))
}

/// A sign-in read once: its bytes, its JSON object and its time. The file is
/// opened without following a link and checked through the open handle, and
/// the same bytes are what gets written. From the 13 November audit: a bot
/// could swap its file for a link between the check and the copy.
struct ReadSignIn {
    bytes: Vec<u8>,
    map: serde_json::Map<String, serde_json::Value>,
    modified: std::time::SystemTime,
}

fn read_sign_in(path: &Path) -> Option<ReadSignIn> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(0x20000 | 0x800) // O_NOFOLLOW | O_NONBLOCK
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > OPENCODE_AUTH_MAX_BYTES {
        return None;
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.read_to_end(&mut bytes).ok()?;
    match serde_json::from_slice::<serde_json::Value>(&bytes).ok()? {
        serde_json::Value::Object(map) if !map.is_empty() => Some(ReadSignIn { bytes, map, modified: meta.modified().ok()? }),
        _ => None,
    }
}

/// Whether `fresh` keeps every field of `owner`, with the same kind of value,
/// a few levels down: the same kind of sign-in, not just any JSON object.
fn same_shape(owner: &serde_json::Value, fresh: &serde_json::Value, depth: usize) -> bool {
    use serde_json::Value;
    match (owner, fresh) {
        (Value::Object(owner), Value::Object(fresh)) => owner.iter().all(|(key, value)| {
            fresh.get(key).is_some_and(|other| depth == 0 || same_shape(value, other, depth - 1))
        }),
        (Value::Null, _) | (_, Value::Null) => true,
        (Value::String(_), Value::String(_))
        | (Value::Number(_), Value::Number(_))
        | (Value::Bool(_), Value::Bool(_))
        | (Value::Array(_), Value::Array(_)) => true,
        _ => false,
    }
}

/// A claim from a JWT's payload, read without checking its signature: only
/// to tell one account from another, never to trust it.
fn jwt_claim(token: Option<&serde_json::Value>, claim: &str) -> Option<serde_json::Value> {
    use base64::Engine;
    let payload = token?.as_str()?.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice::<serde_json::Value>(&bytes).ok()?.get(claim).cloned()
}

/// Whether the fresh sign-in is for the same account as the owner's, as far
/// as the file can say.
fn same_account(cli_dir: &str, owner: &serde_json::Value, fresh: &serde_json::Value) -> bool {
    let differs = |a: Option<&serde_json::Value>, b: Option<&serde_json::Value>| matches!((a, b), (Some(a), Some(b)) if !a.is_null() && a != b);
    match cli_dir {
        ".codex" => {
            let tokens = |v: &serde_json::Value| v.get("tokens").cloned().unwrap_or_default();
            let (o, f) = (tokens(owner), tokens(fresh));
            !differs(o.get("account_id"), f.get("account_id"))
                && !differs(
                    jwt_claim(o.get("id_token"), "email").as_ref(),
                    jwt_claim(f.get("id_token"), "email").as_ref(),
                )
        }
        ".claude" => {
            let oauth = |v: &serde_json::Value| v.get("claudeAiOauth").cloned().unwrap_or_default();
            let (o, f) = (oauth(owner), oauth(fresh));
            !differs(o.get("scopes"), f.get("scopes")) && !differs(o.get("subscriptionType"), f.get("subscriptionType"))
        }
        _ => true,
    }
}

/// When the CLI's own token in a sign-in runs out, in milliseconds. From the
/// 13 November audit: the latest expiry anywhere in the file counted, so an
/// MCP token good for a month hid every real refresh.
fn own_expiry(cli_dir: &str, value: &serde_json::Value) -> Option<i64> {
    let millis = |n: i64| if n < 100_000_000_000 { n * 1000 } else { n };
    let number = |v: Option<&serde_json::Value>| v.and_then(serde_json::Value::as_i64).map(millis);
    match cli_dir {
        ".claude" => number(value.get("claudeAiOauth").and_then(|o| o.get("expiresAt"))),
        ".codex" => jwt_claim(value.get("tokens").and_then(|t| t.get("access_token")), "exp")
            .and_then(|exp| exp.as_i64())
            .map(millis),
        _ => {
            // The object that holds a token holds its expiry.
            let expiry_of = |object: &serde_json::Map<String, serde_json::Value>| {
                ["expiresAt", "expires_at", "expiry"].into_iter().find_map(|key| number(object.get(key)))
            };
            let map = value.as_object()?;
            expiry_of(map).or_else(|| {
                map.values().filter_map(serde_json::Value::as_object).find_map(|child| {
                    let holds_token = ["access_token", "accessToken", "token"].iter().any(|key| child.contains_key(*key));
                    holds_token.then(|| expiry_of(child)).flatten()
                })
            })
        }
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// Whether a bot's sign-in may replace the owner's: the same kind of sign-in
/// for the same account, holding a token good for longer than the owner's,
/// past now and not absurdly far out. Without expiries on either side, and
/// only at the end of a turn, a newer file counts.
fn may_hand_back(cli_dir: &str, owner: &ReadSignIn, fresh: &ReadSignIn, end_of_turn: bool) -> bool {
    let (owner_value, fresh_value) = (serde_json::Value::Object(owner.map.clone()), serde_json::Value::Object(fresh.map.clone()));
    if owner.bytes == fresh.bytes || !same_shape(&owner_value, &fresh_value, 3) || !same_account(cli_dir, &owner_value, &fresh_value) {
        return false;
    }
    let now = now_millis();
    match (own_expiry(cli_dir, &fresh_value), own_expiry(cli_dir, &owner_value)) {
        (Some(fresh), owners) => {
            fresh > now && fresh <= now + SIGN_IN_MAX_LIFETIME_MS && owners.is_none_or(|owners| fresh > owners)
        }
        (None, None) => end_of_turn && fresh.modified > owner.modified,
        (None, Some(_)) => false,
    }
}

/// Copy the bot's sign-in over the owner's, keeping the owner's first.
fn hand_back(owner_home: &Path, cli_dir: &str, file: &str, owner: &ReadSignIn, fresh: &ReadSignIn) -> Result<(), String> {
    keep_previous_sign_in(owner_home, cli_dir, file, &owner.bytes)?;
    write_sign_in(&fresh.bytes, Some(fresh.modified), &owner_home.join(cli_dir).join(file))
}

/// After a turn, hand a sign-in the bot refreshed back to the owner's file,
/// so the owner and the other bots continue from the newest token rather than
/// one the refresh has just invalidated. Only the bot's own CLI's sign-in,
/// and only under [`may_hand_back`]; a signed-out owner stays signed out.
///
/// `owner_home` must be the home the private copy was made from. It is passed
/// in rather than read from `HOME`, so nothing but the real owner's profile is
/// ever written; a test run once replaced a real sign-in with a fixture here.
pub fn write_back_credentials_to(private_home: &Path, owner_home: &Path, cli_dir: &str) -> Result<(), String> {
    for file in credential_files(cli_dir) {
        let Some(owner) = read_sign_in(&owner_home.join(cli_dir).join(file)) else {
            continue;
        };
        let Some(fresh) = read_sign_in(&private_home.join(cli_dir).join(file)) else {
            continue;
        };
        if may_hand_back(cli_dir, &owner, &fresh, true) {
            hand_back(owner_home, cli_dir, file, &owner, &fresh)?;
        }
    }
    Ok(())
}

/// After a turn the CLI ended signed out, replace the bot's copy of its
/// sign-in with the owner's, whatever the files' times say: the bot's copy
/// was refused, and a failed refresh may have rewritten it newer. Returns
/// whether the owner held a different, real sign-in to take.
pub fn take_owner_sign_in(private_home: &Path, owner_home: &Path, cli_dir: &str) -> Result<bool, String> {
    let mut took = false;
    for file in credential_files(cli_dir) {
        let source = owner_home.join(cli_dir).join(file);
        let copy = private_home.join(cli_dir).join(file);
        if regular_file_mtime(&source).is_none() || holds_sign_in(&source).is_none() {
            continue;
        }
        if std::fs::read(&source).ok() == std::fs::read(&copy).ok() {
            continue;
        }
        replace_private_file(&source, &copy)?;
        took = true;
    }
    Ok(took)
}

/// While a bot's CLI is still running, hand a sign-in it has just refreshed
/// back to the owner's file, returning whether it did.
///
/// From speed test 1: Claude's refresh code works once, so @opus refreshing
/// at the start of its turn left the owner's file, and every other Claude
/// bot's copy of it, refused until @opus's turn ended. @quinn faulted 74
/// seconds in. Only a sign-in good for longer than the owner's, and for
/// longer than now, goes back: a refresh that failed is never written over a
/// working one.
pub fn hand_back_fresh_sign_in(private_home: &Path, owner_home: &Path, cli_dir: &str) -> Result<bool, String> {
    let mut handed = false;
    for file in credential_files(cli_dir) {
        let (Some(owner), Some(fresh)) = (
            read_sign_in(&owner_home.join(cli_dir).join(file)),
            read_sign_in(&private_home.join(cli_dir).join(file)),
        ) else {
            continue;
        };
        if may_hand_back(cli_dir, &owner, &fresh, false) {
            hand_back(owner_home, cli_dir, file, &owner, &fresh)?;
            handed = true;
        }
    }
    Ok(handed)
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
    if source_meta.len() > PROFILE_MAX_FILE_BYTES {
        return Err("CLI profile file exceeds the 512 MiB size limit".into());
    }
    if budget.bytes.saturating_add(source_meta.len()) > PROFILE_MAX_TREE_BYTES {
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
    let written = (|| -> Result<u64, String> {
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
        Ok(copied)
    })();
    // From the 13 November audit: a copy cut short was kept, and being
    // there, was never copied again.
    let copied = match written {
        Ok(copied) => copied,
        Err(error) => {
            drop(writer);
            let _ = std::fs::remove_file(to);
            return Err(error);
        }
    };
    budget.bytes += copied;
    Ok(())
}

/// Keep a bot's copy of one of the owner's settings or tree files current.
///
/// From the 13 November audit: seeding only ever added files, so an owner's
/// fixed hook or changed settings never reached a bot that already had a
/// copy. The owner's file replaces the copy when it changed after the copy
/// was made (copies carry the owner's time), so a change the bot made to its
/// own copy since stays until the owner changes theirs. Written beside the
/// copy and renamed into place, so a cut-short copy never replaces a good one.
fn copy_file_following_owner(from: &Path, to: &Path, budget: &mut CopyBudget) -> Result<(), String> {
    let source = match std::fs::symlink_metadata(from) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => meta,
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("inspect CLI profile file: {error}")),
    };
    match std::fs::symlink_metadata(to) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("inspect private CLI profile file: {error}")),
        Ok(existing) if !existing.is_file() || existing.file_type().is_symlink() => {
            return Err(format!("unsafe private CLI profile file: {}", to.display()));
        }
        Ok(existing) => {
            let changed_since = match (source.modified(), existing.modified()) {
                (Ok(owner), Ok(copy)) => owner > copy,
                _ => false,
            };
            if !changed_since {
                return Ok(());
            }
        }
    }
    let parent = to.parent().ok_or("private CLI profile file has no folder")?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = parent.join(format!(".orgbot-seed-{}-{nonce}", std::process::id()));
    copy_file_if_missing(from, &temp, budget)?;
    let placed = (|| -> std::io::Result<()> {
        if let Ok(modified) = source.modified() {
            std::fs::OpenOptions::new().write(true).open(&temp)?.set_modified(modified)?;
        }
        std::fs::rename(&temp, to)
    })();
    if let Err(error) = placed {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("update private CLI profile file: {error}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The owner's kept sign-ins, oldest first.
    fn kept_sign_ins(owner: &Path, cli_dir: &str) -> Vec<String> {
        let folder = sign_in_backup_dir(owner, cli_dir);
        let mut kept: Vec<PathBuf> = std::fs::read_dir(&folder)
            .map(|entries| entries.filter_map(Result::ok).map(|entry| entry.path()).collect())
            .unwrap_or_default();
        kept.sort();
        kept.iter().map(|path| std::fs::read_to_string(path).unwrap()).collect()
    }

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

    /// From the audit after speed test 1: every CLI turn walked the owner's
    /// skills, plugins and session folders to copy nothing new.
    #[test]
    fn a_bots_home_is_reseeded_only_when_the_owners_folders_change() {
        let root = temp();
        let owner = root.join("owner");
        let profiles = root.join("profiles");
        std::fs::create_dir_all(owner.join(".claude/skills/deep")).unwrap();
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(owner.join(".claude/skills/deep/one.md"), "one").unwrap();
        let home = private_home_from(&profiles, &owner, None, ".claude").unwrap();
        assert!(home.join(".claude/skills/deep/one.md").exists());

        // A file added deep inside leaves the folder's own time alone: the
        // next turn doesn't walk it again.
        std::fs::write(owner.join(".claude/skills/deep/two.md"), "two").unwrap();
        private_home_from(&profiles, &owner, None, ".claude").unwrap();
        assert!(!home.join(".claude/skills/deep/two.md").exists());

        // A new entry at the top changes the folder: it is walked again.
        std::fs::write(owner.join(".claude/skills/three.md"), "three").unwrap();
        private_home_from(&profiles, &owner, None, ".claude").unwrap();
        assert!(home.join(".claude/skills/deep/two.md").exists());
        assert!(home.join(".claude/skills/three.md").exists());

        // And after the recheck interval, deep additions arrive too.
        std::fs::write(owner.join(".claude/skills/deep/four.md"), "four").unwrap();
        let stamp_path = profiles.join("home-seeded.json");
        let mut stamp: serde_json::Value = serde_json::from_slice(&std::fs::read(&stamp_path).unwrap()).unwrap();
        stamp["trees"][".claude/skills"]["walked_at"] = serde_json::json!(0);
        std::fs::write(&stamp_path, stamp.to_string()).unwrap();
        private_home_from(&profiles, &owner, None, ".claude").unwrap();
        assert!(home.join(".claude/skills/deep/four.md").exists());

        // A session is looked for in the owner's folders once.
        std::fs::create_dir_all(owner.join(".codex/sessions")).unwrap();
        private_home_from(&profiles, &owner, Some("session-789"), ".codex").unwrap();
        std::fs::write(owner.join(".codex/sessions/rollout-session-789.jsonl"), "late").unwrap();
        private_home_from(&profiles, &owner, Some("session-789"), ".codex").unwrap();
        assert!(!home.join(".codex/sessions/rollout-session-789.jsonl").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// From the 13 November audit: searching a large CLI history for the
    /// session to resume used up the copy budget, and the turn failed.
    #[test]
    fn a_session_search_never_fails_a_turn_for_its_size() {
        let root = temp();
        let (from, to) = (root.join("owner/sessions"), root.join("home/sessions"));
        std::fs::create_dir_all(from.join("2026/11")).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(from.join("2026/11/rollout-abc.jsonl"), "session").unwrap();
        let mut budget = CopyBudget { entries: PROFILE_MAX_ENTRIES, bytes: 0 };
        let mut looked = 0;
        copy_matching_session(&from, &to, "abc", &mut budget, &mut looked, 0).unwrap();
        assert!(to.join("2026/11/rollout-abc.jsonl").exists(), "found though the copy budget was used up");
        let mut looked = SESSION_SEARCH_LIMIT;
        std::fs::remove_dir_all(&to).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        copy_matching_session(&from, &to, "abc", &mut budget, &mut looked, 0).unwrap();
        assert!(!to.join("2026/11/rollout-abc.jsonl").exists(), "a search that runs out is \"not found\"");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_helper_that_runs_too_long_is_stopped() {
        let started = std::time::Instant::now();
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        let error = output_within(&mut command, std::time::Duration::from_millis(300)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        let mut command = Command::new("/bin/echo");
        command.arg("done");
        let output = output_within(&mut command, std::time::Duration::from_secs(10)).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"done\n");
    }

    /// From speed test 1: one Claude bot's refresh left the owner's sign-in,
    /// and every other bot's copy of it, refused until that bot's turn ended.
    #[test]
    fn a_refreshed_sign_in_goes_back_mid_turn_only_when_it_is_good_for_longer() {
        let root = std::env::temp_dir().join(format!(
            "orgbot-signin-handback-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let (owner, home) = (root.join("owner"), root.join("home"));
        for dir in [&owner, &home] {
            std::fs::create_dir_all(dir.join(".claude")).unwrap();
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
        let sign_in = |token: &str, expires: i64| {
            format!(r#"{{"claudeAiOauth":{{"accessToken":"{token}","refreshToken":"r-{token}","expiresAt":{expires}}}}}"#)
        };
        let (owner_file, bot_file) = (owner.join(".claude/.credentials.json"), home.join(".claude/.credentials.json"));
        let read = |path: &Path| std::fs::read_to_string(path).unwrap();
        std::fs::write(&owner_file, sign_in("old", now - 60_000)).unwrap();

        // A refresh that failed leaves nothing good for longer: kept back.
        std::fs::write(&bot_file, sign_in("refused", now - 30_000)).unwrap();
        assert!(!hand_back_fresh_sign_in(&home, &owner, ".claude").unwrap());
        assert_eq!(read(&owner_file), sign_in("old", now - 60_000));

        // A refresh that worked goes back at once, the owner's kept beside it.
        std::fs::write(&bot_file, sign_in("fresh", now + 8 * 3_600_000)).unwrap();
        assert!(hand_back_fresh_sign_in(&home, &owner, ".claude").unwrap());
        assert_eq!(read(&owner_file), sign_in("fresh", now + 8 * 3_600_000));
        assert_eq!(kept_sign_ins(&owner, ".claude"), vec![sign_in("old", now - 60_000)]);
        assert!(!hand_back_fresh_sign_in(&home, &owner, ".claude").unwrap(), "handed back once");

        // The owner signs in again, good for longer: the bot's never replaces it.
        std::fs::write(&owner_file, sign_in("login", now + 9 * 3_600_000)).unwrap();
        assert!(!hand_back_fresh_sign_in(&home, &owner, ".claude").unwrap());
        assert_eq!(read(&owner_file), sign_in("login", now + 9 * 3_600_000));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// From the 13 November audit: a bot could hand the owner a sign-in for
    /// another account, a link to a file of its choosing, or one claiming
    /// years; an MCP token good for a month hid every real refresh; one
    /// backup sat beside the sign-in; and every bot got every CLI's sign-in.
    #[test]
    fn only_a_real_refresh_of_the_owners_own_sign_in_goes_back() {
        use base64::Engine;
        let root = temp();
        let (owner, home) = (root.join("owner"), root.join("profiles/home"));
        for dir in [&owner, &home] {
            std::fs::create_dir_all(dir.join(".codex")).unwrap();
            std::fs::create_dir_all(dir.join(".claude")).unwrap();
        }
        let now = now_millis();
        let jwt = |claims: serde_json::Value| {
            let part = |v: &serde_json::Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
            format!("{}.{}.sig", part(&serde_json::json!({"alg": "none"})), part(&claims))
        };
        let codex = |account: &str, email: &str, exp_secs: i64| {
            serde_json::json!({
                "tokens": {
                    "account_id": account,
                    "id_token": jwt(serde_json::json!({"email": email})),
                    "access_token": jwt(serde_json::json!({"exp": exp_secs})),
                    "refresh_token": "r"
                }
            })
            .to_string()
        };
        let (owner_codex, bot_codex) = (owner.join(".codex/auth.json"), home.join(".codex/auth.json"));
        let read = |path: &Path| std::fs::read_to_string(path).unwrap();
        let hour = 3_600;
        std::fs::write(&owner_codex, codex("acct-1", "owner@example.com", now / 1000 + hour)).unwrap();

        // Another account, by id or by email: kept back.
        std::fs::write(&bot_codex, codex("acct-2", "owner@example.com", now / 1000 + 8 * hour)).unwrap();
        assert!(!hand_back_fresh_sign_in(&home, &owner, ".codex").unwrap());
        std::fs::write(&bot_codex, codex("acct-1", "bot@example.com", now / 1000 + 8 * hour)).unwrap();
        assert!(!hand_back_fresh_sign_in(&home, &owner, ".codex").unwrap());
        // Claiming more than a month: kept back.
        std::fs::write(&bot_codex, codex("acct-1", "owner@example.com", now / 1000 + 400 * 24 * hour)).unwrap();
        assert!(!hand_back_fresh_sign_in(&home, &owner, ".codex").unwrap());
        write_back_credentials_to(&home, &owner, ".codex").unwrap();
        assert_eq!(read(&owner_codex), codex("acct-1", "owner@example.com", now / 1000 + hour));
        // A link to another file is never followed.
        let elsewhere = root.join("elsewhere.json");
        std::fs::write(&elsewhere, codex("acct-1", "owner@example.com", now / 1000 + 8 * hour)).unwrap();
        std::fs::remove_file(&bot_codex).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &bot_codex).unwrap();
        assert!(!hand_back_fresh_sign_in(&home, &owner, ".codex").unwrap());
        // A real refresh of the same account goes back.
        std::fs::remove_file(&bot_codex).unwrap();
        std::fs::write(&bot_codex, codex("acct-1", "owner@example.com", now / 1000 + 8 * hour)).unwrap();
        assert!(hand_back_fresh_sign_in(&home, &owner, ".codex").unwrap());
        assert_eq!(read(&owner_codex), read(&bot_codex));
        // The other CLIs' folders are never handed back from a Codex bot.
        let claude = |token: &str, expires: i64, mcp: i64| {
            serde_json::json!({
                "claudeAiOauth": {"accessToken": token, "expiresAt": expires},
                "mcpOAuth": {"server": {"accessToken": "m", "expiresAt": mcp}}
            })
            .to_string()
        };
        let (owner_claude, bot_claude) = (owner.join(".claude/.credentials.json"), home.join(".claude/.credentials.json"));
        std::fs::write(&owner_claude, claude("old", now + 60_000, now + 20 * 24 * 3_600_000)).unwrap();
        std::fs::write(&bot_claude, claude("fresh", now + 8 * 3_600_000, now + 20 * 24 * 3_600_000)).unwrap();
        assert!(!hand_back_fresh_sign_in(&home, &owner, ".codex").unwrap());
        assert!(read(&owner_claude).contains("\"old\""));
        // A long-lived MCP token doesn't hide Claude's own refresh.
        assert!(hand_back_fresh_sign_in(&home, &owner, ".claude").unwrap());
        assert!(read(&owner_claude).contains("\"fresh\""));

        // Only the last few of the owner's sign-ins are kept, out of the
        // CLI's folder.
        for round in 0..7 {
            std::fs::write(&bot_claude, claude(&format!("r{round}"), now + (9 + round) * 3_600_000, 0)).unwrap();
            assert!(hand_back_fresh_sign_in(&home, &owner, ".claude").unwrap());
        }
        let kept = kept_sign_ins(&owner, ".claude");
        assert_eq!(kept.len(), SIGN_IN_BACKUPS_KEPT);
        assert!(kept.last().unwrap().contains("\"r5\""));
        assert!(std::fs::read_dir(owner.join(".claude")).unwrap().all(|entry| {
            !entry.unwrap().file_name().to_string_lossy().contains("orgbot-previous")
        }));

        // A Codex bot's home holds Codex's folder only.
        let profiles_two = root.join("profiles-two");
        std::fs::create_dir_all(&profiles_two).unwrap();
        let seeded = private_home_from(&profiles_two, &owner, None, ".codex").unwrap();
        assert!(seeded.join(".codex/auth.json").exists());
        assert!(!seeded.join(".claude/.credentials.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bot_sign_ins_stay_in_step_with_the_owners() {
        use std::time::{Duration, SystemTime};
        let root = std::env::temp_dir().join(format!(
            "orgbot-signin-sync-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let owner = root.join("owner");
        let profiles = root.join("profiles");
        std::fs::create_dir_all(owner.join(".claude")).unwrap();
        std::fs::create_dir_all(&profiles).unwrap();
        let owner_file = owner.join(".claude/.credentials.json");
        let sign_in = |token: &str| format!(r#"{{"claudeAiOauth":{{"accessToken":"{token}"}}}}"#);
        let set = |path: &Path, text: &str, age_secs: u64| {
            std::fs::write(path, text).unwrap();
            let file = std::fs::File::options().write(true).open(path).unwrap();
            file.set_modified(SystemTime::now() - Duration::from_secs(age_secs))
                .unwrap();
        };
        let read = |path: &Path| std::fs::read_to_string(path).unwrap();

        // First turn: the bot gets the owner's sign-in.
        set(&owner_file, &sign_in("t1"), 500);
        let home = private_home_from(&profiles, &owner, None, ".claude").unwrap();
        let bot_file = home.join(".claude/.credentials.json");
        assert_eq!(read(&bot_file), sign_in("t1"));

        // The owner signs in again: the bot's next turn uses the new sign-in.
        set(&owner_file, &sign_in("t2"), 400);
        private_home_from(&profiles, &owner, None, ".claude").unwrap();
        assert_eq!(read(&bot_file), sign_in("t2"));

        // The bot's CLI refreshes its copy: it goes back to the owner, the
        // owner's previous file is kept, and the older owner file does not
        // overwrite the bot's on the next turn.
        set(&bot_file, &sign_in("t3"), 300);
        write_back_credentials_to(&home, &owner, ".claude").unwrap();
        assert_eq!(read(&owner_file), sign_in("t3"));
        assert_eq!(kept_sign_ins(&owner, ".claude"), vec![sign_in("t2")]);
        private_home_from(&profiles, &owner, None, ".claude").unwrap();
        assert_eq!(read(&bot_file), sign_in("t3"));

        // Anything that is not a real sign-in is never written to the owner:
        // not a placeholder, and not a different kind of file.
        set(&bot_file, "modified-private-auth", 200);
        write_back_credentials_to(&home, &owner, ".claude").unwrap();
        assert_eq!(read(&owner_file), sign_in("t3"));
        set(&bot_file, r#"{"somethingElse":true}"#, 100);
        write_back_credentials_to(&home, &owner, ".claude").unwrap();
        assert_eq!(read(&owner_file), sign_in("t3"));

        // And a damaged owner file is not spread to the bot.
        set(&owner_file, "garbage", 50);
        private_home_from(&profiles, &owner, None, ".claude").unwrap();
        assert_eq!(read(&bot_file), r#"{"somethingElse":true}"#);

        // A signed-out owner is not signed back in by a bot.
        std::fs::remove_file(&owner_file).unwrap();
        set(&bot_file, &sign_in("t4"), 0);
        write_back_credentials_to(&home, &owner, ".claude").unwrap();
        assert!(!owner_file.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_cli_private_home_does_not_seed_other_provider_profiles() {
        use std::os::unix::fs::symlink;

        let root = temp();
        let source = root.join("source");
        let profile_root = root.join("profile");
        let home = profile_root.join("home-opencode");
        std::fs::create_dir_all(source.join(".codex")).unwrap();
        std::fs::write(source.join(".codex/auth.json"), "other-provider-secret").unwrap();
        std::fs::create_dir_all(&profile_root).unwrap();
        private_home_for_new_cli(&home).unwrap();
        assert!(!home.join(".codex/auth.json").exists());
        assert!(!home.join(".claude").exists());
        assert!(!home.join(".grok").exists());
        let linked_root = root.join("linked-profile");
        symlink(&profile_root, &linked_root).unwrap();
        assert!(private_home_for_new_cli(&linked_root.join("home-opencode")).is_err());
        std::fs::remove_dir_all(root).unwrap();
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

        let private = private_home_from(&profiles, &source, Some("resume-id-123"), ".codex").unwrap();
        let session = private.join(".codex/sessions/rollout-resume-id-123.jsonl");
        assert_eq!(std::fs::read_to_string(&session).unwrap(), "resume-id-123");
        std::fs::write(&session, "private-resume-state").unwrap();
        assert_eq!(
            std::fs::read_to_string(source.join(".codex/sessions/rollout-resume-id-123.jsonl"))
                .unwrap(),
            "resume-id-123"
        );
        private_home_from(&profiles, &source, Some("claude-session-456"), ".claude").unwrap();
        assert_eq!(
            std::fs::read_to_string(private.join(".claude/projects/claude-session-456.jsonl"))
                .unwrap(),
            "claude-session-456"
        );

        let private_auth = private.join(".codex/auth.json");
        std::fs::remove_file(&private_auth).unwrap();
        symlink(source.join(".codex/auth.json"), &private_auth).unwrap();
        assert!(private_home_from(&profiles, &source, Some("resume-id-123"), ".codex").is_err());
        assert!(write_private_file(&private_auth, b"attack").is_err());
        assert_eq!(
            std::fs::read_to_string(source.join(".codex/auth.json")).unwrap(),
            "original-auth"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// From the 13 November audit: seeding only added files, so an owner's
    /// fixed hook never reached a bot that already had a copy.
    #[cfg(unix)]
    #[test]
    fn a_changed_owner_setting_reaches_the_bot_and_a_bot_edit_since_is_kept() {
        let root = temp();
        std::fs::create_dir_all(&root).unwrap();
        let owner = root.join("hook.sh");
        let copy = root.join("hook-copy.sh");
        let at = |secs: u64| std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        let stamp = |path: &Path, secs: u64| {
            std::fs::OpenOptions::new().write(true).open(path).unwrap().set_modified(at(secs)).unwrap();
        };
        std::fs::write(&owner, "echo broken").unwrap();
        stamp(&owner, 1_000);
        let mut budget = CopyBudget::default();
        copy_file_following_owner(&owner, &copy, &mut budget).unwrap();
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), "echo broken");

        std::fs::write(&owner, "echo fixed").unwrap();
        stamp(&owner, 2_000);
        copy_file_following_owner(&owner, &copy, &mut budget).unwrap();
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), "echo fixed", "the owner's fix reaches the bot");

        std::fs::write(&copy, "echo the bot's own change").unwrap();
        stamp(&copy, 3_000);
        copy_file_following_owner(&owner, &copy, &mut budget).unwrap();
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), "echo the bot's own change");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2, "no temp file is left");
        std::fs::remove_dir_all(root).unwrap();
    }

    /// An owner whose CLI folder is a link (stow, chezmoi) still has it seeded.
    #[cfg(unix)]
    #[test]
    fn an_owners_linked_cli_folder_is_followed() {
        let root = temp();
        let owner = root.join("owner");
        let profiles = root.join("profiles");
        std::fs::create_dir_all(owner.join("dotfiles/claude")).unwrap();
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(owner.join("dotfiles/claude/settings.json"), "{\"theme\":\"dark\"}").unwrap();
        std::os::unix::fs::symlink(owner.join("dotfiles/claude"), owner.join(".claude")).unwrap();
        let home = private_home_from(&profiles, &owner, None, ".claude").unwrap();
        assert_eq!(std::fs::read_to_string(home.join(".claude/settings.json")).unwrap(), "{\"theme\":\"dark\"}");
        assert!(!std::fs::symlink_metadata(home.join(".claude")).unwrap().file_type().is_symlink());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn profile_copy_allows_large_plugin_file_but_keeps_file_and_tree_limits() {
        let root = temp();
        let source = root.join("source");
        let copied = root.join("copied");
        std::fs::create_dir_all(&source).unwrap();
        let large_source = source.join("codex-plugin-runtime");
        std::fs::File::create(&large_source)
            .unwrap()
            .set_len(256 * 1024 * 1024 + 1)
            .unwrap();
        let mut budget = CopyBudget::default();
        copy_file_if_missing(&large_source, &copied, &mut budget).unwrap();
        assert_eq!(
            std::fs::metadata(&copied).unwrap().len(),
            256 * 1024 * 1024 + 1
        );
        assert_eq!(budget.bytes, 256 * 1024 * 1024 + 1);

        let oversized = source.join("oversized");
        std::fs::File::create(&oversized)
            .unwrap()
            .set_len(PROFILE_MAX_FILE_BYTES + 1)
            .unwrap();
        assert_eq!(
            copy_file_if_missing(&oversized, &root.join("oversized-copy"), &mut budget)
                .unwrap_err(),
            "CLI profile file exceeds the 512 MiB size limit"
        );

        let small = source.join("small");
        std::fs::write(&small, b"x").unwrap();
        budget.bytes = PROFILE_MAX_TREE_BYTES;
        assert_eq!(
            copy_file_if_missing(&small, &root.join("tree-overflow"), &mut budget).unwrap_err(),
            "CLI profile exceeds the 1 GiB size limit"
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
        let relay = crate::agent_cli::resolve_mcp_relay_binary().unwrap();
        assert!(first.contains(&format!(
            "command = {}",
            serde_json::to_string(&relay.to_string_lossy()).unwrap()
        )));
        assert!(first.contains("/run/user/1000/orgbot-room-mcp.sock"));
        assert!(first.contains("model = \"gpt-6.1-sol\""));
        assert!(!first.contains("outside-server"));
        // The config bots actually run with gives room tools the workspace
        // time limit, not Codex's 300-second default.
        assert!(first.contains("tool_timeout_sec = 2100"), "{first}");
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

    /// From the benchmark room: Grok bots tried eighteen times to start an MCP
    /// server from the owner's ~/.claude.json, copied into the private home.
    #[cfg(unix)]
    #[test]
    fn the_owners_own_mcp_servers_never_reach_a_bot() {
        use orgbot_proto::agent::CliKind;
        let root = temp();
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let claude = serde_json::json!({
            "numStartups": 3,
            "mcpServers": {"unreal": {"type": "http", "url": "http://127.0.0.1:8765"}},
            "projects": {"/work": {"allowedTools": [], "mcpServers": {"local": {"command": "x"}}}}
        });
        for cli in [CliKind::Grok, CliKind::Claude, CliKind::Codex, CliKind::OpenCode] {
            write_private_file(&home.join(".claude.json"), claude.to_string().as_bytes()).unwrap();
            strip_private_agent_mcp_config(&home, cli).unwrap();
            let after: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap();
            assert!(after.get("mcpServers").is_none(), "{cli:?}: {after}");
            assert!(after["projects"]["/work"].get("mcpServers").is_none(), "{cli:?}");
            assert_eq!(after["numStartups"], 3, "everything else is kept");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn new_cli_profiles_contain_only_private_room_mcp_and_deny_direct_tools() {
        use orgbot_proto::agent::CliKind;
        use std::os::unix::fs::PermissionsExt;

        let root = temp();
        let home = root.join("home");
        let mount = Path::new("/run/user/1000/orgbot-home");
        std::fs::create_dir_all(&home).unwrap();
        let auth_source = root.join("host-data/opencode/auth.json");
        std::fs::create_dir_all(auth_source.parent().unwrap()).unwrap();
        std::fs::write(
            &auth_source,
            serde_json::to_vec(&serde_json::json!({
                "openai": {"type": "api", "key": "test-only-auth-sentinel", "refresh": "drop-this", "expires": 123},
                "anthropic": {"type": "api", "key": "must-not-copy"}
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::set_permissions(&auth_source, std::fs::Permissions::from_mode(0o640)).unwrap();

        configure_new_cli_profile_inner(
            &home,
            mount,
            CliKind::OpenCode,
            "openai/gpt-6.1-sol",
            Some(Path::new("/run/user/1000/orgbot-room-mcp.sock")),
            Some(&auth_source),
        )
        .unwrap();
        let opencode: serde_json::Value =
            serde_json::from_slice(&std::fs::read(home.join("opencode.json")).unwrap()).unwrap();
        assert_eq!(opencode["permission"]["*"], "deny");
        assert_eq!(opencode["permission"]["orgbot_room_*"], "allow");
        assert_eq!(opencode["mcp"]["orgbot_room"]["command"][1], "room-mcp");
        assert_eq!(
            opencode["mcp"]["orgbot_room"]["command"][3],
            "/run/user/1000/orgbot-room-mcp.sock"
        );
        assert_eq!(opencode["plugin"].as_array().unwrap().len(), 0);
        assert_eq!(
            std::fs::read(home.join(".config/opencode/.gitignore")).unwrap(),
            OPENCODE_CONFIG_GITIGNORE
        );
        assert_eq!(
            std::fs::read(home.join(".opencode/.gitignore")).unwrap(),
            OPENCODE_CONFIG_GITIGNORE
        );
        for path in [".local/share", ".local/state", ".cache"] {
            assert!(home.join(path).is_dir());
        }
        let private_auth = home.join(".local/share/opencode/auth.json");
        let copied_auth: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&private_auth).unwrap()).unwrap();
        assert_eq!(copied_auth.as_object().unwrap().len(), 1);
        assert_eq!(copied_auth["openai"]["key"], "test-only-auth-sentinel");
        assert_eq!(copied_auth["openai"].as_object().unwrap().len(), 2);
        assert!(copied_auth["openai"].get("refresh").is_none());
        assert!(copied_auth.get("anthropic").is_none());
        assert_eq!(
            std::fs::metadata(&private_auth)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        std::fs::write(
            &private_auth,
            serde_json::to_vec(&serde_json::json!({
                "openai": {"type": "api", "key": "private-modification"}
            }))
            .unwrap(),
        )
        .unwrap();
        configure_new_cli_profile_inner(
            &home,
            mount,
            CliKind::OpenCode,
            "openai/gpt-6.1-sol",
            Some(Path::new("/run/user/1000/orgbot-room-mcp.sock")),
            Some(&auth_source),
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&private_auth).unwrap())
                .unwrap()["openai"]["key"],
            "test-only-auth-sentinel"
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&auth_source).unwrap())
                .unwrap()["anthropic"]["key"],
            "must-not-copy"
        );
        std::fs::write(
            &auth_source,
            serde_json::to_vec(&serde_json::json!({
                "openai": {"type": "oauth", "access": "test-only-oauth"}
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(configure_new_cli_profile_inner(
            &home,
            mount,
            CliKind::OpenCode,
            "openai/gpt-6.1-sol",
            None,
            Some(&auth_source),
        )
        .unwrap_err()
        .contains("OAuth credential sharing is not verified"));
        assert!(
            !private_auth.exists(),
            "unsupported OAuth must clear the prior API clone"
        );
        std::fs::write(
            &auth_source,
            serde_json::to_vec(&serde_json::json!({
                "openai": {"type": "wellknown", "key": "must-not-copy"}
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(configure_new_cli_profile_inner(
            &home,
            mount,
            CliKind::OpenCode,
            "openai/gpt-6.1-sol",
            None,
            Some(&auth_source),
        )
        .unwrap_err()
        .contains("type wellknown is not allowed"));
        std::fs::write(
            &auth_source,
            serde_json::to_vec(&serde_json::json!({
                "openai": {"type": "api", "key": "refreshed-test-only-auth"},
                "anthropic": {"type": "api", "key": "must-not-copy"}
            }))
            .unwrap(),
        )
        .unwrap();
        configure_new_cli_profile_inner(
            &home,
            mount,
            CliKind::OpenCode,
            "openai/gpt-6.1-sol",
            Some(Path::new("/run/user/1000/orgbot-room-mcp.sock")),
            Some(&auth_source),
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&private_auth).unwrap())
                .unwrap()["openai"]["key"],
            "refreshed-test-only-auth"
        );
        std::fs::remove_file(&auth_source).unwrap();
        assert!(configure_new_cli_profile_inner(
            &home,
            mount,
            CliKind::OpenCode,
            "openai/gpt-6.1-sol",
            None,
            Some(&auth_source),
        )
        .is_err());
        assert!(!private_auth.exists(), "stale auth copy must be removed");

        let agy_home = root.join("home-agy");
        std::fs::create_dir(&agy_home).unwrap();
        configure_new_cli_profile_inner(
            &agy_home,
            mount,
            CliKind::Antigravity,
            "gemini-3.8-flash-medium",
            Some(Path::new("/run/user/1000/orgbot-room-mcp.sock")),
            Some(&auth_source),
        )
        .unwrap();
        let agy_settings: serde_json::Value = serde_json::from_slice(
            &std::fs::read(agy_home.join(".gemini/antigravity-cli/settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(agy_settings["useG1Credits"], false);
        assert_eq!(
            agy_settings["permissions"]["allow"][0],
            "mcp(orgbot_room/*)"
        );
        assert_eq!(agy_settings["permissions"]["ask"], serde_json::json!([]));
        assert_eq!(
            agy_settings["permissions"]["deny"],
            serde_json::json!([
                "command(*)",
                "unsandboxed(*)",
                "read_file(*)",
                "write_file(*)",
                "read_url(*)",
                "execute_url(*)"
            ])
        );
        let agy_mcp: serde_json::Value = serde_json::from_slice(
            &std::fs::read(agy_home.join(".gemini/config/mcp_config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            agy_mcp["mcpServers"]["orgbot_room"]["command"],
            AGY_RELAY_MOUNT
        );
        assert_eq!(
            agy_mcp["mcpServers"]["orgbot_room"]["args"],
            serde_json::json!([
                "room-mcp",
                "--socket",
                "/run/user/1000/orgbot-room-mcp.sock"
            ])
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &std::fs::read(agy_home.join(".gemini/config/projects/default-cli-project.json"))
                    .unwrap()
            )
            .unwrap(),
            serde_json::json!({
                "id": "default-cli-project", "name": "CLI Project", "projectResources": {}
            })
        );
        assert!(!agy_home.join(".local/share/opencode/auth.json").exists());
        std::fs::write(
            agy_home.join(".gemini/antigravity-cli/settings.json"),
            br#"{"permissions":{"allow":["command(*)"]}}"#,
        )
        .unwrap();
        // A changed settings file is put back to the managed one rather than
        // stopping the bot for good; what it had granted itself is gone.
        configure_new_cli_profile_inner(
            &agy_home,
            mount,
            CliKind::Antigravity,
            "gemini-3.8-flash-medium",
            Some(Path::new("/run/user/1000/orgbot-room-mcp.sock")),
            None,
        )
        .unwrap();
        let restored: serde_json::Value = serde_json::from_slice(
            &std::fs::read(agy_home.join(".gemini/antigravity-cli/settings.json")).unwrap(),
        )
        .unwrap();
        assert!(!restored["permissions"]["allow"].as_array().unwrap().iter().any(|rule| rule == "command(*)"));
        assert!(restored["permissions"]["deny"].as_array().unwrap().iter().any(|rule| rule == "command(*)"));
        // A link in its place is still refused.
        let settings = agy_home.join(".gemini/antigravity-cli/settings.json");
        std::fs::remove_file(&settings).unwrap();
        std::os::unix::fs::symlink("/etc/hostname", &settings).unwrap();
        let refused = configure_new_cli_profile_inner(
            &agy_home,
            mount,
            CliKind::Antigravity,
            "gemini-3.8-flash-medium",
            Some(Path::new("/run/user/1000/orgbot-room-mcp.sock")),
            None,
        )
        .unwrap_err();
        assert!(refused.contains("unsafe"), "{refused}");
        std::fs::remove_file(&settings).unwrap();
        strip_private_agent_mcp_config(&home, CliKind::OpenCode).unwrap();
        clear_private_agent_config(&home, CliKind::Antigravity).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn antigravity_launcher_receives_only_the_fixed_room_relay_argument() {
        use orgbot_proto::agent::CliKind;

        let root = temp();
        std::fs::create_dir_all(&root).unwrap();
        let guard = WorkspaceGuard {
            project: root.join("project"),
            state: root.join("state"),
            agent: "agent-a".into(),
            read_only: true,
            label: None,
            #[cfg(test)]
            profile_source: None,
        };
        let relay = crate::agent_cli::resolve_mcp_relay_binary().unwrap();
        let args = |cli, mcp| {
            guard
                .command_for_agent_cli("run", cli, mcp)
                .unwrap()
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };

        let agy = args(CliKind::Antigravity, true);
        let relay_arg = agy.iter().position(|arg| arg == "--mcp-relay").unwrap();
        assert_eq!(agy[relay_arg + 1], relay.to_string_lossy());
        assert_eq!(agy.iter().filter(|arg| *arg == "--mcp-relay").count(), 1);
        assert!(!args(CliKind::Antigravity, false)
            .iter()
            .any(|arg| arg == "--mcp-relay"));
        assert!(!args(CliKind::OpenCode, true)
            .iter()
            .any(|arg| arg == "--mcp-relay"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn antigravity_native_conversation_files_survive_profile_reconfiguration() {
        use orgbot_proto::agent::CliKind;
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = temp();
        std::fs::create_dir_all(&root).unwrap();
        let home = root.join("home-agy");
        std::fs::create_dir(&home).unwrap();
        let mount = Path::new("/run/user/1000/orgbot-home");
        configure_new_cli_profile_inner(
            &home,
            mount,
            CliKind::Antigravity,
            "gemini-3.8-flash-medium",
            None,
            None,
        )
        .unwrap();

        let native_dir = home.join(".gemini/antigravity-cli/conversations");
        std::fs::create_dir(&native_dir).unwrap();
        std::fs::set_permissions(&native_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let conversation = native_dir.join("conversation.json");
        std::fs::write(&conversation, br#"{"conversationId":"synthetic"}"#).unwrap();
        std::fs::set_permissions(&conversation, std::fs::Permissions::from_mode(0o600)).unwrap();

        configure_new_cli_profile_inner(
            &home,
            mount,
            CliKind::Antigravity,
            "gemini-3.8-flash-medium",
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(&conversation).unwrap(),
            br#"{"conversationId":"synthetic"}"#
        );

        let external = root.join("outside.json");
        std::fs::write(&external, b"not private").unwrap();
        symlink(&external, native_dir.join("unsafe-link.json")).unwrap();
        assert!(configure_new_cli_profile_inner(
            &home,
            mount,
            CliKind::Antigravity,
            "gemini-3.8-flash-medium",
            None,
            None,
        )
        .unwrap_err()
        .contains("unsafe entry"));
        assert_eq!(std::fs::read(&external).unwrap(), b"not private");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn antigravity_cli_log_symlink_accepts_only_owned_private_log_target() {
        use orgbot_proto::agent::CliKind;
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = temp();
        std::fs::create_dir_all(&root).unwrap();
        let home = root.join("home-agy");
        std::fs::create_dir(&home).unwrap();
        configure_new_cli_profile_inner(
            &home,
            Path::new("/run/user/1000/orgbot-home"),
            CliKind::Antigravity,
            "gemini-3.8-flash-medium",
            None,
            None,
        )
        .unwrap();

        let cli_config = home.join(".gemini/antigravity-cli");
        let log_dir = cli_config.join("log");
        std::fs::create_dir(&log_dir).unwrap();
        std::fs::set_permissions(&log_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let target = log_dir.join("cli-20261027_142923.log");
        std::fs::write(&target, b"synthetic cli log\n").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        symlink("log/cli-20261027_142923.log", cli_config.join("cli.log")).unwrap();

        configure_new_cli_profile_inner(
            &home,
            Path::new("/run/user/1000/orgbot-home"),
            CliKind::Antigravity,
            "gemini-3.8-flash-medium",
            None,
            None,
        )
        .unwrap();

        std::fs::remove_file(cli_config.join("cli.log")).unwrap();
        let outside = root.join("outside.log");
        std::fs::write(&outside, b"outside synthetic file\n").unwrap();
        symlink(&outside, cli_config.join("cli.log")).unwrap();
        assert!(configure_new_cli_profile_inner(
            &home,
            Path::new("/run/user/1000/orgbot-home"),
            CliKind::Antigravity,
            "gemini-3.8-flash-medium",
            None,
            None,
        )
        .unwrap_err()
        .contains("unsafe entry"));
        assert_eq!(
            std::fs::read(&outside).unwrap(),
            b"outside synthetic file\n"
        );

        let reject = || {
            configure_new_cli_profile_inner(
                &home,
                Path::new("/run/user/1000/orgbot-home"),
                CliKind::Antigravity,
                "gemini-3.8-flash-medium",
                None,
                None,
            )
            .is_err()
        };
        std::fs::remove_file(cli_config.join("cli.log")).unwrap();
        symlink("log/cli-20261027_142924.log", cli_config.join("cli.log")).unwrap();
        assert!(reject(), "dangling log target must be rejected");

        std::fs::remove_file(cli_config.join("cli.log")).unwrap();
        symlink("../outside.log", cli_config.join("cli.log")).unwrap();
        assert!(reject(), "traversal target must be rejected");

        std::fs::remove_file(cli_config.join("cli.log")).unwrap();
        std::fs::remove_file(target).unwrap();
        let external_log = root.join("external-log");
        std::fs::create_dir(&external_log).unwrap();
        std::fs::set_permissions(&external_log, std::fs::Permissions::from_mode(0o700)).unwrap();
        let external_target = external_log.join("cli-20261027_142923.log");
        std::fs::write(&external_target, b"external target\n").unwrap();
        std::fs::set_permissions(&external_target, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::remove_dir(&log_dir).unwrap();
        symlink(&external_log, &log_dir).unwrap();
        symlink("log/cli-20261027_142923.log", cli_config.join("cli.log")).unwrap();
        assert!(reject(), "symlinked log parent must be rejected");

        std::fs::remove_file(cli_config.join("cli.log")).unwrap();
        std::fs::remove_file(&log_dir).unwrap();
        std::fs::create_dir(&log_dir).unwrap();
        std::fs::set_permissions(&log_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let target = log_dir.join("cli-20261027_142923.log");
        std::fs::write(&target, b"synthetic cli log\n").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        let hard_link = root.join("hard-link.log");
        std::fs::hard_link(&target, &hard_link).unwrap();
        symlink("log/cli-20261027_142923.log", cli_config.join("cli.log")).unwrap();
        assert!(reject(), "hard-linked log target must be rejected");

        std::fs::remove_file(cli_config.join("cli.log")).unwrap();
        std::fs::remove_file(hard_link).unwrap();
        symlink("log/not-cli.log", cli_config.join("cli.log")).unwrap();
        assert!(reject(), "malformed log basename must be rejected");

        std::fs::remove_file(cli_config.join("cli.log")).unwrap();
        std::fs::remove_file(&target).unwrap();
        symlink(&outside, &target).unwrap();
        symlink("log/cli-20261027_142923.log", cli_config.join("cli.log")).unwrap();
        assert!(reject(), "symlinked log target must be rejected");

        std::fs::remove_file(cli_config.join("cli.log")).unwrap();
        std::fs::remove_file(&target).unwrap();
        std::fs::write(&target, b"synthetic cli log\n").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        symlink("log/cli-20261027_142923.log", cli_config.join("cli.log")).unwrap();
        assert!(reject(), "non-private log target must be rejected");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn opencode_api_auth_requires_a_nonempty_string_key() {
        use orgbot_proto::agent::CliKind;

        let root = temp();
        let source = root.join("auth.json");
        let invalid_credentials = [
            serde_json::json!({"type": "api"}),
            serde_json::json!({"type": "api", "key": null}),
            serde_json::json!({"type": "api", "key": ""}),
            serde_json::json!({"type": "api", "key": "   "}),
        ];
        for (index, credential) in invalid_credentials.into_iter().enumerate() {
            let home = root.join(format!("home-{index}"));
            std::fs::create_dir_all(&home).unwrap();
            std::fs::write(
                &source,
                serde_json::to_vec(&serde_json::json!({"openai": credential})).unwrap(),
            )
            .unwrap();
            assert!(configure_new_cli_profile_inner(
                &home,
                Path::new("/run/user/1000/orgbot-home"),
                CliKind::OpenCode,
                "openai/gpt-6.1-sol",
                None,
                Some(&source),
            )
            .unwrap_err()
            .contains("no non-empty key"));
            assert!(!home.join(".local/share/opencode/auth.json").exists());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn new_cli_profile_without_room_socket_keeps_only_scoped_opencode_auth() {
        use orgbot_proto::agent::CliKind;

        let root = temp();
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let auth_source = root.join("host-data/opencode/auth.json");
        std::fs::create_dir_all(auth_source.parent().unwrap()).unwrap();
        std::fs::write(
            &auth_source,
            serde_json::to_vec(&serde_json::json!({
                "openai": {"type": "api", "key": "test-only-auth-sentinel"}
            }))
            .unwrap(),
        )
        .unwrap();
        configure_new_cli_profile_inner(
            &home,
            Path::new("/run/user/1000/orgbot-home"),
            CliKind::OpenCode,
            "openai/gpt-6.1-sol",
            None,
            Some(&auth_source),
        )
        .unwrap();
        let agy_home = root.join("home-agy");
        std::fs::create_dir(&agy_home).unwrap();
        configure_new_cli_profile_inner(
            &agy_home,
            Path::new("/run/user/1000/orgbot-home"),
            CliKind::Antigravity,
            "gemini-3.8-flash-medium",
            None,
            Some(&auth_source),
        )
        .unwrap();
        let agy_settings: serde_json::Value = serde_json::from_slice(
            &std::fs::read(agy_home.join(".gemini/antigravity-cli/settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(agy_settings["permissions"]["allow"], serde_json::json!([]));
        assert_eq!(agy_settings["permissions"]["deny"][6], "mcp(*)");
        let agy_mcp: serde_json::Value = serde_json::from_slice(
            &std::fs::read(agy_home.join(".gemini/config/mcp_config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(agy_mcp["mcpServers"], serde_json::json!({}));
        let opencode: serde_json::Value =
            serde_json::from_slice(&std::fs::read(home.join("opencode.json")).unwrap()).unwrap();
        assert!(opencode["mcp"].as_object().unwrap().is_empty());
        assert_eq!(opencode["permission"]["*"], "deny");
        assert!(opencode["permission"].get("orgbot_room_*").is_none());
        assert!(!home.join(".gemini").exists());
        let auth: serde_json::Value = serde_json::from_slice(
            &std::fs::read(home.join(".local/share/opencode/auth.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(auth["openai"]["key"], "test-only-auth-sentinel");
        std::fs::remove_dir_all(root).unwrap();
    }
}
