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
                if !relay_command.is_some() {
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
                || metadata.mode() & 0o077 != 0
                || metadata.len() != bytes.len() as u64 =>
        {
            return Err(format!("unsafe {label} file"));
        }
        Ok(_) => {
            if std::fs::read(path).map_err(|error| format!("read {label}: {error}"))? != bytes {
                return Err(format!("{label} differs from the managed profile"));
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
                    if credential_files(name).contains(file) {
                        // Sign-ins follow the owner: a newer real file (after
                        // signing in again) replaces the bot's copy.
                        refresh_credential(&from.join(file), &to.join(file), &mut budget)?;
                    } else {
                        copy_file_if_missing(&from.join(file), &to.join(file), &mut budget)?;
                    }
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

/// After a turn, hand a sign-in the bot refreshed back to the owner's file,
/// so the owner and the other bots continue from the newest token rather than
/// one the refresh has just invalidated.
///
/// Only when the bot's copy is newer: if the owner signed in meanwhile, their
/// file wins and reaches the bot on its next turn. Only a real sign-in that
/// keeps every field the owner's file has is written, and the owner's previous
/// file is kept beside it as `<name>.orgbot-previous`.
///
/// `owner_home` must be the home the private copy was made from. It is passed
/// in rather than read from `HOME`, so nothing but the real owner's profile is
/// ever written; a test run once replaced a real sign-in with a fixture here.
pub fn write_back_credentials_to(private_home: &Path, owner_home: &Path) -> Result<(), String> {
    let source_home = owner_home;
    for cli_dir in [".claude", ".codex", ".grok"] {
        for file in credential_files(cli_dir) {
            let copy = private_home.join(cli_dir).join(file);
            let source = source_home.join(cli_dir).join(file);
            let (Some(copy_time), Some(source_time)) =
                (regular_file_mtime(&copy), regular_file_mtime(&source))
            else {
                // A signed-out owner stays signed out.
                continue;
            };
            if copy_time <= source_time || std::fs::read(&copy).ok() == std::fs::read(&source).ok() {
                continue;
            }
            let Some(refreshed) = holds_sign_in(&copy) else {
                continue;
            };
            if let Some(current) = holds_sign_in(&source) {
                if !current.keys().all(|key| refreshed.contains_key(key)) {
                    // Not the same kind of sign-in; leave the owner's alone.
                    continue;
                }
            }
            let previous = source.with_file_name(format!("{file}.orgbot-previous"));
            replace_private_file(&source, &previous)?;
            replace_private_file(&copy, &source)?;
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
        let home = private_home_from(&profiles, &owner, None).unwrap();
        let bot_file = home.join(".claude/.credentials.json");
        assert_eq!(read(&bot_file), sign_in("t1"));

        // The owner signs in again: the bot's next turn uses the new sign-in.
        set(&owner_file, &sign_in("t2"), 400);
        private_home_from(&profiles, &owner, None).unwrap();
        assert_eq!(read(&bot_file), sign_in("t2"));

        // The bot's CLI refreshes its copy: it goes back to the owner, the
        // owner's previous file is kept, and the older owner file does not
        // overwrite the bot's on the next turn.
        set(&bot_file, &sign_in("t3"), 300);
        write_back_credentials_to(&home, &owner).unwrap();
        assert_eq!(read(&owner_file), sign_in("t3"));
        assert_eq!(read(&owner.join(".claude/.credentials.json.orgbot-previous")), sign_in("t2"));
        private_home_from(&profiles, &owner, None).unwrap();
        assert_eq!(read(&bot_file), sign_in("t3"));

        // Anything that is not a real sign-in is never written to the owner:
        // not a placeholder, and not a different kind of file.
        set(&bot_file, "modified-private-auth", 200);
        write_back_credentials_to(&home, &owner).unwrap();
        assert_eq!(read(&owner_file), sign_in("t3"));
        set(&bot_file, r#"{"somethingElse":true}"#, 100);
        write_back_credentials_to(&home, &owner).unwrap();
        assert_eq!(read(&owner_file), sign_in("t3"));

        // And a damaged owner file is not spread to the bot.
        set(&owner_file, "garbage", 50);
        private_home_from(&profiles, &owner, None).unwrap();
        assert_eq!(read(&bot_file), r#"{"somethingElse":true}"#);

        // A signed-out owner is not signed back in by a bot.
        std::fs::remove_file(&owner_file).unwrap();
        set(&bot_file, &sign_in("t4"), 0);
        write_back_credentials_to(&home, &owner).unwrap();
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
        assert!(configure_new_cli_profile_inner(
            &agy_home,
            mount,
            CliKind::Antigravity,
            "gemini-3.8-flash-medium",
            Some(Path::new("/run/user/1000/orgbot-room-mcp.sock")),
            None,
        )
        .unwrap_err()
        .contains("Antigravity settings"));
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
