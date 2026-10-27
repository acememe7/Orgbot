//! Backends that drive the machine's installed agent CLIs.
//!
//! An [AgentCliProvider] runs one model turn by spawning a real agent CLI —
//! [CliKind::Codex] via `codex exec`, [CliKind::Claude] via `claude -p`, or
//! [CliKind::Grok] via `grok -p` — in the bot's project folder. The CLI is the
//! agent: it carries the machine's sign-in, memories, skills, MCP servers, and
//! model configuration, and it runs its own tool loop inside the configured
//! [AgentPolicy]. Nothing is reimplemented here.
//!
//! Sessions make the bot's memory last. The first turn starts a fresh CLI
//! session and records its id; later turns resume that session. When a room has
//! a private state path, the session id is also recorded per bot and restored
//! only while provider, model, workdir, policy, and MCP mode still match.
//!
//! The room board can be served as MCP tools. Text is what crosses the boundary;
//! the rich event stream an agent produces
//!   (file edits, shell runs, tool results) is summarised into the agent's final
//!   message. Full action telemetry can stream later over the same pipe.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The literal room server name, so `concat!` can build rules from it.
macro_rules! room_server_name {
    () => {
        "orgbot_room"
    };
}

use async_trait::async_trait;
use orgbot_proto::agent::{AgentPolicy, CliKind};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::timeout;

async fn read_stderr_tail(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
    limit: usize,
) -> Vec<u8> {
    use tokio::io::AsyncReadExt;
    let mut tail = Vec::with_capacity(limit.min(8192));
    let mut chunk = [0u8; 8192];
    loop {
        let count = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        if count >= limit {
            tail.clear();
            tail.extend_from_slice(&chunk[count - limit..count]);
        } else {
            let overflow = tail.len().saturating_add(count).saturating_sub(limit);
            if overflow > 0 {
                tail.drain(..overflow);
            }
            tail.extend_from_slice(&chunk[..count]);
        }
    }
    tail
}

#[cfg(target_os = "linux")]
fn replace_room_socket(config: &mut Value, inner_socket: &Path) -> std::result::Result<(), String> {
    let args = config
        .pointer_mut("/mcpServers/orgbot_room/args")
        .and_then(Value::as_array_mut)
        .ok_or("room MCP config is missing its server arguments")?;
    let index = args
        .iter()
        .position(|value| value.as_str() == Some("--socket"))
        .ok_or("room MCP config is missing its socket argument")?;
    let socket = args
        .get_mut(index + 1)
        .ok_or("room MCP config has no socket path")?;
    *socket = Value::String(inner_socket.to_string_lossy().into_owned());
    Ok(())
}

use crate::error::{ProviderError, Result};
use crate::provider::{emit, Provider};
use crate::types::{
    CompletionRequest, CompletionResponse, ContentBlock, Role, StopReason, StreamEvent, Turn, Usage,
};

/// How long one agent episode may run before it is killed.
///
/// Agents do real work — long tool chains, tests, renders — so this is
/// generous. Override with the `ORGBOT_CLI_TURN_TIMEOUT_SECS` variable.
const DEFAULT_TURN_TIMEOUT_SECS: u64 = 600;

/// Extra places to look for a CLI beyond PATH: this machine installs its
/// global CLIs under the user's home.
const HOME_BIN_DIRS: [&str; 3] = [".local/bin", ".npm-global/bin", ".grok/bin"];

/// A backend that runs one of the installed agent CLIs as a session.
#[derive(Debug)]
pub struct AgentCliProvider {
    /// Which CLI this drives.
    cli: CliKind,
    /// The resolved binary path.
    bin: String,
    /// Model id, or empty to use the CLI's default.
    model: String,
    /// Project folder to run in; empty inherits the process directory.
    workdir: PathBuf,
    /// How much of the project the agent may touch.
    policy: AgentPolicy,
    /// The CLI session id, established on the first turn and resumed after.
    session: Mutex<Option<String>>,
    /// Last standing system brief applied to this native session.
    session_system_brief: Mutex<Option<String>>,
    /// The socket handed in on the last request, for diagnostics.
    last_socket: Mutex<Option<String>>,
    /// Whether the room's board is served to the agent as MCP tools.
    mcp: bool,
    /// Turn ceiling.
    timeout: Duration,
    /// Optional owner-only record that lets one bot resume its native session
    /// after the room process is rebuilt.
    session_store: Mutex<Option<SessionStore>>,
    /// Last cumulative usage reported by a Codex session.
    codex_usage: Mutex<Option<Usage>>,
    #[cfg(target_os = "linux")]
    guard: Option<crate::workspace_guard::WorkspaceGuard>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SessionStore {
    path: PathBuf,
    cli: CliKind,
    model: String,
    workdir: String,
    policy: AgentPolicy,
    mcp: bool,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct PersistedSession {
    cli: CliKind,
    model: String,
    workdir: String,
    policy: AgentPolicy,
    mcp: bool,
    session_id: String,
    /// Missing in older records; causes one brief refresh on resume.
    #[serde(default)]
    system_brief: Option<String>,
    /// Codex reports session-cumulative usage; absent in records written before accounting.
    #[serde(default)]
    codex_usage: Option<Usage>,
}

impl AgentCliProvider {
    /// Build a provider for `cli`, failing when the binary cannot be found.
    ///
    /// `workdir` is the project folder the agent works in; an empty string
    /// inherits the current process directory.
    pub fn new(
        cli: CliKind,
        model: impl Into<String>,
        workdir: impl Into<String>,
        policy: AgentPolicy,
        mcp: bool,
    ) -> Result<Self> {
        let bin = locate(cli).ok_or_else(|| {
            ProviderError::Unsupported(format!(
                "{} not found on PATH (looked in $PATH, ~/.local/bin, ~/.npm-global/bin, \
                 ~/.grok/bin). Install it and sign in before seating a bot on it.",
                cli.binary()
            ))
        })?;
        let model = model.into();
        let workdir = workdir.into();
        let timeout = std::env::var("ORGBOT_CLI_TURN_TIMEOUT_SECS")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(DEFAULT_TURN_TIMEOUT_SECS));
        Ok(Self {
            cli,
            bin,
            model,
            workdir: if workdir.is_empty() {
                PathBuf::new()
            } else {
                PathBuf::from(workdir)
            },
            policy,
            mcp,
            session: Mutex::new(None),
            session_system_brief: Mutex::new(None),
            last_socket: Mutex::new(None),
            timeout,
            session_store: Mutex::new(None),
            codex_usage: Mutex::new(None),
            #[cfg(target_os = "linux")]
            guard: None,
        })
    }

    /// Persist this bot's native session in the private room-state directory.
    /// Existing state is restored only when provider, model, and canonical
    /// working directory still match this bot's current configuration.
    pub fn with_session_store(self, path: PathBuf) -> Self {
        let workdir = canonical_workdir(&self.workdir);
        let store = SessionStore {
            path,
            cli: self.cli,
            model: self.model.clone(),
            workdir,
            policy: self.policy,
            mcp: self.mcp,
        };
        if let Some(persisted) = read_session_record(&store) {
            *self.session.lock().unwrap() = Some(persisted.session_id);
            *self.session_system_brief.lock().unwrap() = persisted.system_brief;
            *self.codex_usage.lock().unwrap() = persisted.codex_usage;
        }
        *self.session_store.lock().unwrap() = Some(store);
        self
    }

    #[cfg(target_os = "linux")]
    pub fn with_workspace_guard(self, state: PathBuf, agent: String) -> Self {
        let project = self.workdir.clone();
        self.with_workspace_guard_project(state, agent, project)
    }

    #[cfg(target_os = "linux")]
    pub fn with_workspace_guard_project(
        mut self,
        state: PathBuf,
        agent: String,
        project: PathBuf,
    ) -> Self {
        let canonical_project = canonical_workdir(&project);
        self.guard = Some(crate::workspace_guard::WorkspaceGuard {
            project,
            state,
            agent,
            read_only: self.policy == AgentPolicy::ReadOnly,
            #[cfg(test)]
            profile_source: None,
        });
        let stored_session = { self.session_store.lock().unwrap().clone() };
        if let Some(mut store) = stored_session {
            store.workdir = canonical_project;
            *self.session.lock().unwrap() = None;
            *self.session_system_brief.lock().unwrap() = None;
            *self.codex_usage.lock().unwrap() = None;
            if let Some(persisted) = read_session_record(&store) {
                *self.session.lock().unwrap() = Some(persisted.session_id);
                *self.session_system_brief.lock().unwrap() = persisted.system_brief;
                *self.codex_usage.lock().unwrap() = persisted.codex_usage;
            }
            *self.session_store.lock().unwrap() = Some(store);
        }
        self
    }

    /// Whether `cli` is installed somewhere the adapter looks.
    pub fn available(cli: CliKind) -> bool {
        locate(cli).is_some()
    }

    /// Whether the room board is served to the agent as MCP tools.
    pub fn serves_room_tools(&self) -> bool {
        self.mcp
    }

    /// Whether a room MCP socket was supplied on the last request, for logs.
    fn mcp_socket_debug(&self) -> Option<String> {
        self.last_socket.lock().unwrap().clone()
    }

    /// The session id this provider is currently resuming, if any.
    pub fn session_id(&self) -> Option<String> {
        self.session.lock().unwrap().clone()
    }
}

/// Find a CLI binary on PATH and in this machine's usual home locations.
pub fn locate(cli: CliKind) -> Option<String> {
    let name = cli.binary();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME") {
        for sub in HOME_BIN_DIRS {
            let candidate = PathBuf::from(&home).join(sub);
            if !dirs.contains(&candidate) {
                dirs.push(candidate);
            }
        }
    }
    for dir in dirs {
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

fn canonical_workdir(workdir: &Path) -> String {
    let path = if workdir.as_os_str().is_empty() {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        workdir.to_path_buf()
    };
    std::fs::canonicalize(&path)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn read_session_record(store: &SessionStore) -> Option<PersistedSession> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::symlink_metadata(&store.path).ok()?;
    if !metadata.file_type().is_file() || metadata.permissions().mode() & 0o077 != 0 {
        return None;
    }
    let state: PersistedSession = serde_json::from_slice(&std::fs::read(&store.path).ok()?).ok()?;
    (state.cli == store.cli
        && state.model == store.model
        && state.workdir == store.workdir
        && state.policy == store.policy
        && state.mcp == store.mcp)
        .then_some(state)
}

#[cfg(test)]
fn read_session(store: &SessionStore) -> Option<String> {
    read_session_record(store).map(|state| state.session_id)
}

fn write_session(
    store: &SessionStore,
    session_id: &str,
    system_brief: Option<&str>,
    codex_usage: Option<Usage>,
) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let parent = store
        .path
        .parent()
        .ok_or_else(|| std::io::Error::other("session store has no parent directory"))?;
    std::fs::create_dir_all(parent)?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    let state = PersistedSession {
        cli: store.cli,
        model: store.model.clone(),
        workdir: store.workdir.clone(),
        policy: store.policy,
        mcp: store.mcp,
        session_id: session_id.to_owned(),
        system_brief: system_brief.map(str::to_owned),
        codex_usage,
    };
    let bytes = serde_json::to_vec(&state)
        .map_err(|error| std::io::Error::other(format!("session serialization failed: {error}")))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let temporary = parent.join(format!(
        ".orgbot-session-{}-{nonce}.tmp",
        std::process::id()
    ));
    let result = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&temporary, &store.path)
        });
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
        return result;
    }
    std::fs::set_permissions(&store.path, std::fs::Permissions::from_mode(0o600))
}

fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn remove_app_credentials(
    command: &mut Command,
    inherited_names: impl IntoIterator<Item = std::ffi::OsString>,
) {
    command
        .env_remove("ORGBOT_ROOM_TOKEN")
        .env_remove("DEEPSEEK_API_KEY");
    for name in inherited_names {
        if is_orgbot_api_key(&name) {
            command.env_remove(name);
        }
    }
}

fn is_orgbot_api_key(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    (name.starts_with("ORGBOT_") && name.ends_with("_API_KEY")) || name.starts_with("ORGBOT_MCP_")
}

/// The sandbox/permission flag this CLI expects for a policy.
fn policy_flags(cli: CliKind, policy: AgentPolicy) -> (&'static str, &'static str) {
    let value = match (cli, policy) {
        (CliKind::Codex, AgentPolicy::ReadOnly) => "read-only",
        (CliKind::Codex, AgentPolicy::WorkspaceWrite) => "workspace-write",
        (CliKind::Codex, AgentPolicy::FullAccess) => "danger-full-access",
        // Claude's CLI accepts `dontAsk`, which denies actions needing approval;
        // Grok retains its existing default permission mode.
        (CliKind::Claude, AgentPolicy::ReadOnly) => "dontAsk",
        (CliKind::Grok, AgentPolicy::ReadOnly) => "default",
        (CliKind::Claude | CliKind::Grok, AgentPolicy::WorkspaceWrite) => "acceptEdits",
        (CliKind::Claude | CliKind::Grok, AgentPolicy::FullAccess) => "bypassPermissions",
    };
    let flag = match cli {
        CliKind::Codex => "-s",
        CliKind::Claude | CliKind::Grok => "--permission-mode",
    };
    (flag, value)
}

/// One episode's worth of accumulated agent output.
#[derive(Debug, Default)]
struct Episode {
    /// The final prose the agent produced.
    text: String,
    /// The session id to resume next time, once known.
    session_id: Option<String>,
    /// Token accounting, when the CLI reports it.
    usage: Usage,
    /// A structured provider failure reported on stdout, preferred over
    /// incidental diagnostics from stderr.
    structured_error: Option<String>,
}

#[async_trait]
impl Provider for AgentCliProvider {
    fn name(&self) -> &str {
        self.cli.label()
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> Result<CompletionResponse> {
        *self.last_socket.lock().unwrap() = request.mcp_socket.clone();
        let resumed = self.session.lock().unwrap().clone();
        let situation = latest_instruction(&request.messages);
        let previous_brief = self.session_system_brief.lock().unwrap().clone();
        let prompt = compose_prompt(
            &request.system,
            situation.as_deref(),
            resumed.is_some(),
            previous_brief.as_deref(),
        );

        let mut episode = Episode::default();
        let (claude_config, codex_home) = if self.mcp {
            match self.cli {
                CliKind::Claude => {
                    let path = request
                        .mcp_socket
                        .as_deref()
                        .map(write_claude_mcp_config)
                        .transpose()?;
                    (path, None)
                }
                CliKind::Codex => {
                    let path = request
                        .mcp_socket
                        .as_deref()
                        .map(write_codex_home)
                        .transpose()?;
                    (None, path)
                }
                CliKind::Grok => {
                    let path = request
                        .mcp_socket
                        .as_deref()
                        .map(write_grok_home)
                        .transpose()?;
                    (None, path)
                }
            }
        } else {
            (None, None)
        };
        let mut result = self
            .run_episode(
                &prompt,
                resumed.as_deref(),
                &sink,
                &mut episode,
                claude_config.as_deref(),
                codex_home.as_deref(),
            )
            .await;
        // A saved session the CLI no longer has (its history was pruned, or
        // it lived in a home that is gone) must not halt the bot: forget it
        // and take this turn as a fresh session, once.
        if resumed.is_some()
            && result
                .as_ref()
                .err()
                .is_some_and(|error| is_missing_session(&error.to_string()))
        {
            tracing::warn!(
                cli = self.cli.binary(),
                "saved session is gone; starting a fresh one"
            );
            self.forget_session();
            let fresh_prompt = compose_prompt(&request.system, situation.as_deref(), false, None);
            episode = Episode::default();
            result = self
                .run_episode(
                    &fresh_prompt,
                    None,
                    &sink,
                    &mut episode,
                    claude_config.as_deref(),
                    codex_home.as_deref(),
                )
                .await;
        }
        if let Some(path) = &claude_config {
            let _ = std::fs::remove_file(path);
        }
        if let Some(dir) = &codex_home {
            let _ = std::fs::remove_dir_all(dir);
        }
        result?;

        if self.cli == CliKind::Codex && episode.usage.reported {
            let mut cumulative = self.codex_usage.lock().unwrap();
            let reported = episode.usage;
            episode.usage = codex_usage_delta(
                reported,
                *cumulative,
                resumed.as_deref(),
                episode.session_id.as_deref().or(resumed.as_deref()),
            );
            if reported.complete {
                *cumulative = Some(reported);
            }
        }

        // Remember the session for the next turn. A resumed session reports
        // its id again on init; either way the freshest id wins.
        if let Some(id) = episode.session_id.clone() {
            *self.session.lock().unwrap() = Some(id);
        }
        if self.session.lock().unwrap().is_some() {
            *self.session_system_brief.lock().unwrap() = Some(request.system.clone());
            self.persist_session_brief(&request.system)?;
        }

        let _ = emit(
            &sink,
            StreamEvent::Done {
                stop_reason: StopReason::EndTurn,
            },
        )
        .await;

        let model = if request.model.is_empty() {
            self.cli.label().to_string()
        } else {
            request.model.clone()
        };
        Ok(CompletionResponse {
            model,
            content: if episode.text.trim().is_empty() {
                vec![]
            } else {
                vec![ContentBlock::text(episode.text)]
            },
            stop_reason: StopReason::EndTurn,
            usage: episode.usage,
        })
    }
}

impl AgentCliProvider {
    /// Spawn the CLI for one turn, stream its prose, and reap the process.
    async fn run_episode(
        &self,
        prompt: &str,
        resumed: Option<&str>,
        sink: &Option<mpsc::Sender<StreamEvent>>,
        episode: &mut Episode,
        mcp_config: Option<&Path>,
        codex_home: Option<&Path>,
    ) -> Result<()> {
        #[cfg(target_os = "linux")]
        if self.guard.is_none() && !cfg!(test) {
            return Err(ProviderError::Unsupported(
                "Linux native CLI requires a protected room workspace".into(),
            ));
        }
        let mut cmd = Command::new(&self.bin);
        #[cfg(target_os = "linux")]
        let prepared = self
            .guard
            .as_ref()
            .map(|guard| guard.prepare())
            .transpose()
            .map_err(ProviderError::Unsupported)?;
        #[cfg(target_os = "linux")]
        let private_home = prepared
            .as_ref()
            .map(|prepared| {
                #[cfg(test)]
                if let Some(source) = self
                    .guard
                    .as_ref()
                    .and_then(|guard| guard.profile_source.as_deref())
                {
                    return crate::workspace_guard::private_home_from_test(
                        &prepared.profile_root,
                        source,
                        resumed,
                    );
                }
                crate::workspace_guard::private_home(
                    &prepared.profile_root,
                    &prepared.private_home,
                    resumed,
                )
            })
            .transpose()
            .map_err(ProviderError::Unsupported)?;
        #[cfg(target_os = "linux")]
        let private_mcp_config = if let (Some(home), Some(prepared)) = (&private_home, &prepared) {
            if self.cli != CliKind::Claude {
                None
            } else {
                let target = home.join("orgbot-room-mcp.json");
                let mut value: Value = if let Some(config) = mcp_config {
                    serde_json::from_slice(
                        &std::fs::read(config)
                            .map_err(|e| ProviderError::Unsupported(e.to_string()))?,
                    )
                    .map_err(|e| {
                        ProviderError::Unsupported(format!("invalid room MCP config: {e}"))
                    })?
                } else {
                    serde_json::json!({"mcpServers": {}})
                };
                if mcp_config.is_some() {
                    replace_room_socket(&mut value, &prepared.mcp_socket_mount)
                        .map_err(ProviderError::Unsupported)?;
                }
                crate::workspace_guard::write_private_file(
                    &target,
                    &serde_json::to_vec(&value)
                        .map_err(|e| ProviderError::Unsupported(e.to_string()))?,
                )
                .map_err(ProviderError::Unsupported)?;
                Some(prepared.private_home_mount.join("orgbot-room-mcp.json"))
            }
        } else {
            None
        };
        #[cfg(target_os = "linux")]
        let mcp_config = private_mcp_config.as_deref().or(mcp_config);
        #[cfg(target_os = "linux")]
        if let Some(home) = private_home.as_deref() {
            crate::workspace_guard::strip_private_agent_mcp_config(home, self.cli)
                .map_err(ProviderError::Unsupported)?;
        }
        #[cfg(target_os = "linux")]
        if let (Some(home), Some(prepared), Some(_overlay)) = (&private_home, &prepared, codex_home)
        {
            crate::workspace_guard::update_private_agent_config(
                home,
                self.cli,
                &prepared.private_home_mount,
                &prepared.mcp_socket_mount,
            )
            .map_err(ProviderError::Unsupported)?;
        }
        build_args(
            &mut cmd,
            self.cli,
            prompt,
            resumed,
            &self.model,
            self.policy,
            mcp_config,
            self.mcp,
        );
        remove_app_credentials(&mut cmd, std::env::vars_os().map(|(name, _)| name));
        if !self.workdir.as_os_str().is_empty() {
            cmd.current_dir(&self.workdir);
        }
        cmd.env("NO_COLOR", "1");
        tracing::debug!(
            cli = self.cli.binary(),
            overlay = ?codex_home,
            mcp = self.mcp,
            socket = ?self.mcp_socket_debug(),
            "spawning agent CLI"
        );
        if let Some(home) = codex_home {
            // Both CLIs discover MCP servers through their config home; the
            // variable that points at it differs.
            cmd.env(
                match self.cli {
                    CliKind::Grok => "GROK_HOME",
                    _ => "CODEX_HOME",
                },
                home,
            );
        }
        #[cfg(target_os = "linux")]
        if let (Some(guard), Some(prepared)) = (&self.guard, &prepared) {
            let original = cmd.as_std();
            let mut protected =
                Command::from(guard.command("run").map_err(ProviderError::Unsupported)?);
            if guard.read_only || prepared.read_only {
                protected.arg("--read-only");
            }
            protected
                .arg("--private-home")
                .arg(private_home.as_ref().expect("prepared private home"));
            if let Some(socket) = self.mcp_socket_debug() {
                protected.arg("--mcp-socket").arg(socket);
            }
            protected
                .arg("--")
                .arg(original.get_program())
                .args(original.get_args());
            for (name, value) in original.get_envs() {
                if let Some(value) = value {
                    protected.env(name, value);
                } else {
                    protected.env_remove(name);
                }
            }
            protected.current_dir(&prepared.workspace);
            match self.cli {
                CliKind::Codex => {
                    protected.env("CODEX_HOME", prepared.private_home_mount.join(".codex"))
                }
                CliKind::Grok => {
                    protected.env("GROK_HOME", prepared.private_home_mount.join(".grok"))
                }
                CliKind::Claude => protected.env(
                    "CLAUDE_CONFIG_DIR",
                    prepared.private_home_mount.join(".claude"),
                ),
            };
            protected.env("HOME", &prepared.private_home_mount);
            cmd = protected;
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|error| {
            ProviderError::Unsupported(format!("could not start {}: {}", self.cli.binary(), error))
        })?;
        let stdout = child.stdout.take().expect("stdout piped");
        let mut stderr = child.stderr.take().expect("stderr piped");

        // Drain stderr concurrently so the pipe can never fill and wedge the
        // agent; its tail is surfaced only on failure.
        let stderr_task =
            tokio::spawn(async move { read_stderr_tail(&mut stderr, 64 * 1024).await });

        let provider = self;
        let run = async {
            let mut reader = BufReader::new(stdout).lines();
            loop {
                let line = reader
                    .next_line()
                    .await
                    .map_err(|error| ProviderError::Process {
                        provider: provider.cli.label().to_string(),
                        detail: format!("reading output: {error}"),
                    })?;
                let Some(line) = line else { break };
                let previous_session = episode.session_id.clone();
                ingest_line(provider.cli, &line, sink, episode);
                if previous_session != episode.session_id {
                    if let Some(session_id) = episode.session_id.as_deref() {
                        let mut active = provider.session.lock().unwrap();
                        if active.as_deref() != Some(session_id) {
                            *provider.codex_usage.lock().unwrap() = None;
                        }
                        *active = Some(session_id.to_string());
                    }
                    if let (Some(store), Some(session_id)) = (
                        provider.session_store.lock().unwrap().clone(),
                        episode.session_id.as_deref(),
                    ) {
                        let applied_brief = provider.session_system_brief.lock().unwrap().clone();
                        let codex_usage = *provider.codex_usage.lock().unwrap();
                        write_session(&store, session_id, applied_brief.as_deref(), codex_usage)
                            .map_err(|error| ProviderError::Process {
                                provider: provider.cli.label().to_string(),
                                detail: format!("could not persist native session state: {error}"),
                            })?;
                    }
                }
            }
            Ok::<(), ProviderError>(())
        };

        let outcome = timeout(self.timeout, async {
            let parsed = run.await;
            let stderr_bytes = stderr_task.await.unwrap_or_default();
            let status = child.wait().await;
            (parsed, stderr_bytes, status)
        })
        .await;

        #[cfg(target_os = "linux")]
        let cleanup_private_profile = || -> Result<()> {
            if self.guard.is_some() {
                if let Some(home) = private_home.as_deref() {
                    match self.cli {
                        CliKind::Claude if mcp_config.is_some() => {
                            crate::workspace_guard::remove_private_file(
                                &home.join("orgbot-room-mcp.json"),
                            )
                            .map_err(ProviderError::Unsupported)?;
                        }
                        CliKind::Codex | CliKind::Grok if codex_home.is_some() => {
                            crate::workspace_guard::clear_private_agent_config(home, self.cli)
                                .map_err(ProviderError::Unsupported)?;
                        }
                        _ => {}
                    }
                }
            }
            Ok(())
        };
        #[cfg(target_os = "linux")]
        if outcome.is_ok() {
            cleanup_private_profile()?;
        }

        match outcome {
            Ok((parsed, stderr_bytes, status)) => {
                parsed?;
                let exit = status.map_err(|error| ProviderError::Process {
                    provider: self.cli.label().to_string(),
                    detail: format!("waiting for exit: {error}"),
                })?;
                if let Some(message) = episode.structured_error.as_deref() {
                    return Err(classify_cli_error(self.cli, message));
                }
                if !exit.success() {
                    let stderr = String::from_utf8_lossy(&stderr_bytes);
                    let tail: Vec<&str> = stderr.lines().rev().take(6).collect();
                    let tail = tail.iter().rev().cloned().collect::<Vec<_>>().join("\n");
                    let tail = if tail.is_empty() {
                        "no error output".to_string()
                    } else {
                        tail
                    };
                    return Err(ProviderError::Process {
                        provider: self.cli.label().to_string(),
                        detail: format!("exited with {exit}: {tail}"),
                    });
                }
                #[cfg(target_os = "linux")]
                if let Some(guard) = &self.guard {
                    if !guard.read_only
                        && !prepared
                            .as_ref()
                            .is_some_and(|workspace| workspace.read_only)
                    {
                        let integration = guard.apply().map_err(ProviderError::Unsupported)?;
                        if integration
                            .get("deleted")
                            .and_then(Value::as_array)
                            .is_some_and(|paths| !paths.is_empty())
                        {
                            episode.text.push_str("\nFile removals await owner approval in Workspace changes; originals are preserved.");
                        }
                    }
                }
                Ok(())
            }
            Err(_elapsed) => {
                // Kill and reap so the child cannot linger.
                let _ = child.start_kill();
                let _ = child.wait().await;
                #[cfg(target_os = "linux")]
                cleanup_private_profile()?;
                Err(ProviderError::Process {
                    provider: self.cli.label().to_string(),
                    detail: format!("timed out after {}s", self.timeout.as_secs()),
                })
            }
        }
    }

    /// Drop the remembered native session, in memory and on disk.
    fn forget_session(&self) {
        *self.session.lock().unwrap() = None;
        *self.session_system_brief.lock().unwrap() = None;
        *self.codex_usage.lock().unwrap() = None;
        if let Some(store) = self.session_store.lock().unwrap().as_ref() {
            let _ = std::fs::remove_file(&store.path);
        }
    }

    fn persist_session_brief(&self, system_brief: &str) -> Result<()> {
        let store = self.session_store.lock().unwrap().clone();
        let session_id = self.session.lock().unwrap().clone();
        let (Some(store), Some(session_id)) = (store, session_id) else {
            return Ok(());
        };
        write_session(
            &store,
            &session_id,
            Some(system_brief),
            *self.codex_usage.lock().unwrap(),
        )
        .map_err(|error| ProviderError::Process {
            provider: self.cli.label().to_string(),
            detail: format!("could not persist native session brief: {error}"),
        })
    }
}

fn compose_prompt(
    system_brief: &str,
    situation: Option<&str>,
    resumed: bool,
    previous_brief: Option<&str>,
) -> String {
    let Some(situation) = situation else {
        return if resumed && previous_brief == Some(system_brief) {
            "Continue from the current room state.".to_string()
        } else {
            system_brief.to_string()
        };
    };
    if !resumed {
        return format!("{system_brief}\n\n{situation}");
    }
    if previous_brief != Some(system_brief) {
        format!("Updated standing role and operating brief:\n{system_brief}\n\n{situation}")
    } else {
        situation.to_string()
    }
}

/// Read one JSONL line of agent output into the episode.
fn ingest_line(
    cli: CliKind,
    line: &str,
    sink: &Option<mpsc::Sender<StreamEvent>>,
    episode: &mut Episode,
) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return; // A banner or warning on stdout; nothing to do.
    };
    match cli {
        CliKind::Codex => ingest_codex_line(&value, sink, episode),
        CliKind::Claude | CliKind::Grok => ingest_messages_line(&value, sink, episode),
    }
}

/// Codex --json events: thread.started gives the session id, item.completed
/// carries finished messages, and turn.completed reports usage.
fn ingest_codex_line(
    value: &Value,
    sink: &Option<mpsc::Sender<StreamEvent>>,
    episode: &mut Episode,
) {
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return;
    };
    match kind {
        "thread.started" => {
            if let Some(id) = value.get("thread_id").and_then(Value::as_str) {
                episode.session_id = Some(id.to_string());
            }
        }
        "turn.completed" => {
            if let Some(usage) = value.get("usage") {
                episode.usage = usage_from_json(usage);
            }
        }
        "turn.failed" => {
            if let Some(message) = value
                .get("error")
                .and_then(|error| error.as_str().or_else(|| error.get("message")?.as_str()))
                .or_else(|| value.get("message").and_then(Value::as_str))
            {
                episode.structured_error = bounded_error_message(message);
            }
        }
        "item.started" => {
            let Some(item) = value.get("item") else {
                return;
            };
            let Some(item_type) = item.get("type").and_then(Value::as_str) else {
                return;
            };
            let Some(label) = codex_activity_label(item_type) else {
                return;
            };
            let id = item.get("id").and_then(Value::as_str).unwrap_or(item_type);
            send_activity(
                sink,
                StreamEvent::ToolUseStart {
                    id: id.chars().take(96).collect(),
                    name: label.into(),
                },
            );
        }
        "item.completed" => {
            let Some(item) = value.get("item") else {
                return;
            };
            let Some(item_type) = item.get("type").and_then(Value::as_str) else {
                return;
            };
            if codex_activity_label(item_type).is_some() {
                let id = item.get("id").and_then(Value::as_str).unwrap_or(item_type);
                let failed = item
                    .get("exit_code")
                    .and_then(Value::as_i64)
                    .is_some_and(|code| code != 0)
                    || item
                        .get("status")
                        .and_then(Value::as_str)
                        .is_some_and(|status| matches!(status, "failed" | "error" | "cancelled"));
                send_activity(
                    sink,
                    StreamEvent::ToolUseEnd {
                        id: id.chars().take(96).collect(),
                        succeeded: !failed,
                    },
                );
            }
            // The agent's finished message carries its answer. A question the
            // agent needs answered is carried the same way and must reach the
            // room as text too.
            if matches!(item_type, "agent_message" | "ask_user_question") {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        episode.text.push_str(text);
                        episode.text.push('\n');
                    }
                }
            }
        }
        _ => {}
    }
}

/// Claude (stream-json) and Grok (streaming-messages-json) both speak the
/// Anthropic wire shape as NDJSON: an init line names the session, and
/// assistant lines carry whole messages whose content blocks hold the text.
fn ingest_messages_line(
    value: &Value,
    sink: &Option<mpsc::Sender<StreamEvent>>,
    episode: &mut Episode,
) {
    // The init event names the session on both fresh and resumed runs.
    if let Some(id) = value.get("session_id").and_then(Value::as_str) {
        episode.session_id = Some(id.to_string());
    }
    if value.get("type").and_then(Value::as_str) == Some("result") {
        if value.get("is_error").and_then(Value::as_bool) == Some(true) {
            let message = value.get("result").and_then(Value::as_str).or_else(|| {
                value
                    .get("error")
                    .and_then(|error| error.as_str().or_else(|| error.get("message")?.as_str()))
            });
            if let Some(message) = message {
                episode.structured_error = bounded_error_message(message);
            }
        }
        if let Some(usage) = value.get("usage") {
            episode.usage = usage_from_json(usage);
        }
        return;
    }
    let line_type = value.get("type").and_then(Value::as_str);
    if line_type == Some("stream_event") {
        if let Some(event) = value.get("event") {
            if event.get("type").and_then(Value::as_str) == Some("content_block_start") {
                let block = event.get("content_block");
                if block
                    .and_then(|block| block.get("type"))
                    .and_then(Value::as_str)
                    == Some("tool_use")
                {
                    let id = block
                        .and_then(|block| block.get("id"))
                        .and_then(Value::as_str);
                    let name = block
                        .and_then(|block| block.get("name"))
                        .and_then(Value::as_str);
                    if let (Some(id), Some(name)) = (id, name) {
                        send_activity(
                            sink,
                            StreamEvent::ToolUseStart {
                                id: id.chars().take(96).collect(),
                                name: safe_tool_label(name),
                            },
                        );
                    }
                }
            }
        }
        return;
    }
    if !matches!(line_type, Some("assistant" | "user")) {
        return;
    }
    let Some(message) = value.get("message") else {
        return;
    };
    let Some(blocks) = message.get("content").and_then(Value::as_array) else {
        return;
    };
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") if line_type == Some("assistant") => {
                if let (Some(id), Some(name)) = (
                    block.get("id").and_then(Value::as_str),
                    block.get("name").and_then(Value::as_str),
                ) {
                    send_activity(
                        sink,
                        StreamEvent::ToolUseStart {
                            id: id.chars().take(96).collect(),
                            name: safe_tool_label(name),
                        },
                    );
                }
            }
            Some("tool_result") if line_type == Some("user") => {
                if let Some(id) = block.get("tool_use_id").and_then(Value::as_str) {
                    send_activity(
                        sink,
                        StreamEvent::ToolUseEnd {
                            id: id.chars().take(96).collect(),
                            succeeded: block.get("is_error").and_then(Value::as_bool) != Some(true),
                        },
                    );
                }
            }
            Some("text") if line_type == Some("assistant") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        episode.text.push_str(text);
                        episode.text.push('\n');
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(usage) = message.get("usage") {
        episode.usage = usage_from_json(usage);
    }
}

fn codex_activity_label(item_type: &str) -> Option<&'static str> {
    match item_type {
        "command_execution" => Some("Run command"),
        "file_change" | "apply_patch" => Some("Edit files"),
        "mcp_tool_call" => Some("Use a connected tool"),
        "web_search_call" | "web_search" => Some("Search the web"),
        "web_fetch" => Some("Read a web page"),
        "image_generation_call" => Some("Generate an image"),
        _ => None,
    }
}

fn safe_tool_label(name: &str) -> String {
    let label = name
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | ':' | ' ')
        })
        .take(48)
        .collect::<String>();
    if label.trim().is_empty() {
        "Use a tool".into()
    } else {
        label
    }
}

fn send_activity(sink: &Option<mpsc::Sender<StreamEvent>>, event: StreamEvent) {
    if let Some(sink) = sink {
        let _ = sink.try_send(event);
    }
}

fn bounded_error_message(message: &str) -> Option<String> {
    let message = message.trim();
    (!message.is_empty()).then(|| message.chars().take(2_000).collect())
}

fn classify_cli_error(cli: CliKind, message: &str) -> ProviderError {
    let normalized = message.to_ascii_lowercase();
    let rate_limited = [
        "rate limit",
        "rate-limit",
        "rate limited",
        "rate-limited",
        "too many requests",
        "quota",
        "weekly limit",
        "usage limit",
    ]
    .iter()
    .any(|phrase| normalized.contains(phrase));
    if rate_limited {
        ProviderError::RateLimited {
            provider: cli.label().to_string(),
            message: message.to_string(),
            retry_after_secs: None,
        }
    } else {
        ProviderError::Process {
            provider: cli.label().to_string(),
            detail: format!("CLI reported an error: {message}"),
        }
    }
}

/// Token accounting from either vocabulary. Every field is optional so an
/// event that omits usage degrades to zeros instead of failing the turn.
fn usage_from_json(value: &Value) -> Usage {
    let num = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0) as u32;
    let openai_cached = num("cached_input_tokens");
    let cache_read = openai_cached.max(num("cache_read_input_tokens"));
    let input = num("input_tokens");
    Usage {
        reported: [
            "input_tokens",
            "output_tokens",
            "cached_input_tokens",
            "cache_read_input_tokens",
            "cache_write_input_tokens",
            "cache_creation_input_tokens",
        ]
        .iter()
        .any(|key| value.get(*key).and_then(Value::as_u64).is_some()),
        complete: value.get("input_tokens").and_then(Value::as_u64).is_some()
            && value.get("output_tokens").and_then(Value::as_u64).is_some(),
        calls: 1,
        // CLI usage follows the OpenAI shape when it reports cached input.
        input_tokens: input.saturating_sub(openai_cached),
        output_tokens: num("output_tokens"),
        cache_read_tokens: cache_read,
        cache_write_tokens: num("cache_write_input_tokens").max(num("cache_creation_input_tokens")),
        ..Usage::default()
    }
}

fn usage_delta(current: Usage, previous: Option<Usage>) -> Usage {
    if !current.reported {
        return current;
    }
    if !current.complete {
        return Usage::missing();
    }
    let Some(previous) = previous else {
        return current;
    };
    let counters_rolled_back = current.input_tokens < previous.input_tokens
        || current.output_tokens < previous.output_tokens
        || current.cache_read_tokens < previous.cache_read_tokens
        || current.cache_write_tokens < previous.cache_write_tokens;
    if !previous.complete || counters_rolled_back {
        return Usage::missing();
    }
    Usage {
        complete: current.complete && previous.complete,
        calls: current.calls,
        input_tokens: current.input_tokens.saturating_sub(previous.input_tokens),
        output_tokens: current.output_tokens.saturating_sub(previous.output_tokens),
        cache_read_tokens: current
            .cache_read_tokens
            .saturating_sub(previous.cache_read_tokens),
        cache_write_tokens: current
            .cache_write_tokens
            .saturating_sub(previous.cache_write_tokens),
        ..current
    }
}

fn codex_usage_delta(
    current: Usage,
    previous: Option<Usage>,
    resumed_session: Option<&str>,
    current_session: Option<&str>,
) -> Usage {
    if !current.complete {
        return Usage::missing();
    }
    if previous.is_none() && resumed_session.is_some() && current_session == resumed_session {
        return Usage::missing();
    }
    usage_delta(current, previous)
}

/// Write the --mcp-config file that points an agent CLI at the room bridge.
///
/// The CLI spawns the server named in the config as a child process; that
/// process is this program's own `room-mcp` relay, which shuttles bytes
/// between the CLI's stdio and the room's unix socket. The running binary's
/// own path is used so the child is always the same orgbot install.
/// The binary the agent CLI will spawn as its MCP relay child.
///
/// Normally this is the running orgbot install; tests point it at the built
/// `orgbot` binary via ORGBOT_MCP_RELAY_BIN, since the test runner itself
/// has no `room-mcp` subcommand.
fn relay_binary() -> Result<std::path::PathBuf> {
    resolve_mcp_relay_binary().map_err(ProviderError::Unsupported)
}

pub(crate) fn resolve_mcp_relay_binary() -> std::result::Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("ORGBOT_MCP_RELAY_BIN") {
        return Ok(PathBuf::from(path));
    }
    let running = std::env::current_exe()
        .map_err(|error| format!("cannot locate own binary for MCP relay: {error}; set ORGBOT_MCP_RELAY_BIN or restart OrgBots"))?;
    resolve_running_relay_binary(running)
}

fn resolve_running_relay_binary(running: PathBuf) -> std::result::Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    if !running.exists() {
        // Linux marks an unlinked executable with this suffix after atomic replacement.
        if let Some(name) = running.file_name().and_then(|name| name.to_str()) {
            if let Some(original) = name.strip_suffix(" (deleted)") {
                let candidate = running.with_file_name(original);
                use std::os::unix::fs::PermissionsExt;
                let executable = std::fs::metadata(&candidate).is_ok_and(|metadata| {
                    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                });
                if executable {
                    return Ok(candidate);
                }
                return Err(format!(
                    "the running OrgBots binary was replaced or removed and {} is not executable; restart OrgBots or set ORGBOT_MCP_RELAY_BIN to a trusted room-mcp binary",
                    candidate.display()
                ));
            }
        }
    }
    Ok(running)
}

/// A per-run overlay of the user's codex home.
///
/// codex only loads MCP servers from its config file and ignores per-run
/// overrides, so this builds a temporary `CODEX_HOME`: every entry of the
/// real `~/.codex` is symlinked (sessions, skills, memories and the rest all
/// stay exactly where they are) except `config.toml`, which is copied and
/// extended with the room's MCP server. The child codex therefore keeps the
/// user's entire setup and gains the board.
/// Build a Grok config home that carries the room's MCP server.
///
/// Grok's MCP is config-managed rather than a per-run flag, and it honours
/// `GROK_HOME` — so the same overlay trick as codex works: symlink the real
/// home so the sign-in, sessions and settings all still apply, and add one
/// server entry pointing at the relay. Without this a Grok bot can talk but
/// can never touch the board, which is exactly a room where nothing happens.
fn write_grok_home(socket: &str) -> Result<std::path::PathBuf> {
    write_agent_home(socket, ".grok", "grok", |config, relay, socket| {
        config.push_str(concat!("\n[mcp_servers.", room_server_name!(), "]\n"));
        config.push_str(&format!(
            "command = {}\n",
            serde_json::to_string(&relay.to_string_lossy()).unwrap()
        ));
        config.push_str(&format!(
            "args = [\"room-mcp\", \"--socket\", \"{socket}\" ]\n"
        ));
        // Grok keeps servers switched off unless told otherwise.
        config.push_str("enabled = true\n");
    })
}

fn write_codex_home(socket: &str) -> Result<std::path::PathBuf> {
    write_agent_home(socket, ".codex", "codex", append_codex_room_server)
}

fn append_codex_room_server(config: &mut String, relay: &Path, socket: &str) {
    config.push_str(concat!("\n[mcp_servers.", room_server_name!(), "]\n"));
    config.push_str(&format!(
        "command = {}\n",
        serde_json::to_string(&relay.to_string_lossy()).unwrap()
    ));
    config.push_str(&format!(
        "args = [\"room-mcp\", \"--socket\", \"{socket}\" ]\n"
    ));
    // Codex normally asks the headless user to approve MCP calls. Keep the
    // approval exception scoped to this local room bridge; global approval
    // remains `never` in the invocation arguments.
    config.push_str("default_tools_approval_mode = \"approve\"\n");
}

// This grants auto-approval only to OrgBot's guarded local board bridge. Any
// other inherited Codex MCP integrations retain their own CLI configuration;
// the provider does not claim to block arbitrary remote mutations through them.

/// Build a throwaway config home for a CLI that manages MCP through config.
///
/// The real home is symlinked entry by entry so the CLI keeps its sign-in,
/// sessions and settings; only `config.toml` is replaced, with the room's MCP
/// server appended to whatever was already there.
fn write_agent_home(
    socket: &str,
    dot_dir: &str,
    label: &str,
    add_server: impl Fn(&mut String, &std::path::Path, &str),
) -> Result<std::path::PathBuf> {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| {
            ProviderError::Unsupported(format!("HOME is not set; cannot build a {label} overlay"))
        })?
        .join(dot_dir);
    write_agent_home_at(socket, &home, label, add_server)
}

fn write_agent_home_at(
    socket: &str,
    home: &Path,
    label: &str,
    add_server: impl Fn(&mut String, &Path, &str),
) -> Result<std::path::PathBuf> {
    use std::os::unix::fs::symlink;

    let relay = relay_binary()?;
    if !home.is_dir() {
        return Err(ProviderError::Unsupported(format!(
            "{} is not a directory; {label} is probably not set up",
            home.display()
        )));
    }

    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let overlay =
        std::env::temp_dir().join(format!("orgbot-{label}-{}-{nonce}", std::process::id()));
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(&overlay).map_err(|error| {
        ProviderError::Unsupported(format!("cannot create {label} overlay: {error}"))
    })?;
    if let Err(error) = std::fs::set_permissions(&overlay, std::fs::Permissions::from_mode(0o700)) {
        let _ = std::fs::remove_dir_all(&overlay);
        return Err(ProviderError::Unsupported(format!(
            "cannot protect {label} overlay: {error}"
        )));
    }

    let mut config = String::new();
    for entry in std::fs::read_dir(&home).map_err(|error| {
        ProviderError::Unsupported(format!(
            "cannot read {label} home {}: {error}",
            home.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            ProviderError::Unsupported(format!("cannot read {label} home entry: {error}"))
        })?;
        let name = entry.file_name();
        if name == "config.toml" {
            config = std::fs::read_to_string(entry.path()).unwrap_or_default();
            continue;
        }
        let _ = symlink(entry.path(), overlay.join(&name));
    }
    add_server(&mut config, &relay, socket);
    let config_path = overlay.join("config.toml");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&config_path)
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(config.as_bytes())
        })
        .map_err(|error| {
            let _ = std::fs::remove_dir_all(&overlay);
            ProviderError::Unsupported(format!("cannot write {label} overlay config: {error}"))
        })?;
    Ok(overlay)
}
fn write_claude_mcp_config(socket: &str) -> Result<std::path::PathBuf> {
    let exe = relay_binary()?;
    let config = serde_json::json!({
        "mcpServers": {
            (ROOM_SERVER_NAME): {
                "command": exe.to_string_lossy(),
                "args": ["room-mcp", "--socket", socket],
            }
        }
    });
    use std::os::unix::fs::OpenOptionsExt;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let path = std::env::temp_dir().join(format!("orgbot-mcp-{}-{nonce}.json", std::process::id()));
    let bytes = serde_json::to_vec(&config).map_err(|error| {
        ProviderError::Unsupported(format!("MCP config serialisation failed: {error}"))
    })?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(&bytes)
        })
        .map_err(|error| {
            ProviderError::Unsupported(format!(
                "cannot write MCP config {}: {error}",
                path.display()
            ))
        })?;
    Ok(path)
}

/// Whether a CLI failure means the session it was asked to resume is gone.
fn is_missing_session(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "no rollout found",      // codex
        "no conversation found", // claude
        "session not found",
        "thread not found",
        "could not find session",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

/// The newest instruction for the agent: the last user turn carrying prose.
///
/// The engine pushes a fresh "what changed" situation each turn; anything
/// older lives in the CLI's own session memory and must not be replayed.
fn latest_instruction(messages: &[Turn]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|turn| turn.role == Role::User)
        .map(|turn| {
            let mut text = turn.text();
            for block in &turn.content {
                if let ContentBlock::Image {
                    source: crate::types::ImageSource::File { path },
                    ..
                } = block
                {
                    text.push_str(&format!(
                        "\nAttached image file (use your image-reading tool): {}",
                        path.display()
                    ));
                }
            }
            text
        })
        .filter(|text| !text.trim().is_empty())
}

/// Lay out the CLI invocation for one turn.
fn build_args(
    cmd: &mut Command,
    cli: CliKind,
    prompt: &str,
    resumed: Option<&str>,
    model: &str,
    policy: AgentPolicy,
    mcp_config: Option<&Path>,
    mcp: bool,
) {
    let (policy_flag, policy_value) = policy_flags(cli, policy);
    if let Some(path) = mcp_config {
        // Only Claude Code accepts a per-run MCP config today; the other
        // CLIs ignore it (their attach story is config-managed).
        if cli == CliKind::Claude {
            cmd.arg("--strict-mcp-config").arg("--mcp-config").arg(path);
        }
    }
    match (cli, resumed) {
        (CliKind::Codex, None) => {
            cmd.arg("exec")
                .arg("--json")
                .arg("--skip-git-repo-check")
                .arg(prompt);
            cmd.arg(policy_flag)
                .arg(policy_value)
                .arg("-c")
                .arg("approval_policy=\"never\"");
        }
        (CliKind::Codex, Some(_session)) => {
            // `codex exec resume` does not take a sandbox flag (the resumed
            // session keeps the one it started with), and its trailing prompt
            // is appended after all options below.
            cmd.arg("exec")
                .arg("resume")
                .arg("--json")
                .arg("--skip-git-repo-check")
                .arg("-c")
                .arg("approval_policy=\"never\"");
        }
        (CliKind::Claude, None) => {
            cmd.arg("-p")
                .arg(prompt)
                .arg("--output-format")
                .arg("stream-json")
                .arg("--verbose")
                .arg("--permission-mode")
                .arg(policy_value);
        }
        (CliKind::Claude, Some(session)) => {
            cmd.arg("-p")
                .arg(prompt)
                .arg("--output-format")
                .arg("stream-json")
                .arg("--verbose")
                .arg("--permission-mode")
                .arg(policy_value)
                .arg("--resume")
                .arg(session);
        }
        (CliKind::Grok, None) => {
            cmd.arg("-p")
                .arg(prompt)
                .arg("--output-format")
                .arg("streaming-messages-json")
                .arg("--permission-mode")
                .arg(policy_value);
        }
        (CliKind::Grok, Some(session)) => {
            cmd.arg("-p")
                .arg(prompt)
                .arg("--output-format")
                .arg("streaming-messages-json")
                .arg("--permission-mode")
                .arg(policy_value)
                .arg("-r")
                .arg(session);
        }
    }
    if !model.is_empty() {
        // Claude Code spells this out; codex and grok take the short form.
        // Sending `-m` to claude is "unknown option '-m'" and kills the turn.
        match cli {
            CliKind::Claude => cmd.arg("--model").arg(model),
            CliKind::Codex | CliKind::Grok => cmd.arg("-m").arg(model),
        };
    }
    if cli == CliKind::Codex && model == "gpt-6.1-sol" {
        cmd.arg("-c").arg("model_reasoning_effort=\"xhigh\"");
    }

    // Let the room's own tools through the permission gate.
    //
    // Under a normal permission mode an MCP tool call needs approval, and a
    // headless session has nobody to approve it — the agent sees the tools,
    // calls one, and is refused, which shows up in the transcript as "room
    // tooling is permission-blocked". The room board is this bot's job, so it
    // is allowed explicitly rather than by loosening the sandbox generally.
    if mcp {
        match cli {
            CliKind::Claude => {
                // Both the bare server name and an explicit wildcard: the
                // CLIs differ on which form matches a whole server, and an
                // unmatched rule is simply ignored.
                cmd.arg("--allowedTools")
                    .arg(ROOM_TOOL_PATTERN)
                    .arg(ROOM_TOOL_WILDCARD);
            }
            CliKind::Grok => {
                cmd.arg("--allow")
                    .arg(ROOM_TOOL_PATTERN)
                    .arg("--allow")
                    .arg(ROOM_TOOL_WILDCARD);
            }
            // Codex's local room-server approval is set only in its overlay.
            CliKind::Codex => {}
        }
    }
    if cli == CliKind::Codex {
        if let Some(session) = resumed {
            // A resumed prompt can begin with `-`; stop option parsing before
            // passing the session id and prompt as positional arguments.
            cmd.arg("--").arg(session).arg(prompt);
        }
    }
}

/// The name every CLI registers the room's MCP server under.
///
/// The permission rules below are spelled from this name, so the two cannot
/// drift apart: a server registered as `orgbot-room` while the rule allows
/// `mcp__orgbot_room__*` leaves every room tool permission-blocked.
const ROOM_SERVER_NAME: &str = room_server_name!();

/// Matches every tool the room's MCP server exposes.
///
/// Both Claude Code and Grok namespace MCP tools as `mcp__<server>__<tool>`
/// in their permission rules; naming the server allows all of its tools.
const ROOM_TOOL_PATTERN: &str = concat!("mcp__", room_server_name!());

/// The same set, spelled as a wildcard for CLIs that want one.
const ROOM_TOOL_WILDCARD: &str = concat!("mcp__", room_server_name!(), "__*");

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn deleted_current_executable_resolves_only_to_an_executable_at_its_original_path() {
        use std::os::unix::fs::PermissionsExt;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("orgbot-relay-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let original = root.join("orgbot");
        std::fs::write(&original, b"binary replacement fixture").unwrap();
        std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert_eq!(
            resolve_running_relay_binary(root.join("orgbot (deleted)")).unwrap(),
            original
        );
        let missing = root.join("missing (deleted)");
        let error = resolve_running_relay_binary(missing).unwrap_err();
        assert!(error.contains("ORGBOT_MCP_RELAY_BIN"), "{error}");
        let existing_deleted_suffix = root.join("literal (deleted)");
        std::fs::write(&existing_deleted_suffix, b"still the current executable").unwrap();
        std::fs::set_permissions(
            &existing_deleted_suffix,
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        assert_eq!(
            resolve_running_relay_binary(existing_deleted_suffix.clone()).unwrap(),
            existing_deleted_suffix
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A full codex run as captured from a live probe.
    const CODEX_RUN: &str = r#"{"type":"thread.started","thread_id":"01a067db-ea53-7973-b9ae-4b63f2af5026"}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"probe-ok"}}
{"type":"turn.completed","usage":{"input_tokens":24416,"cached_input_tokens":11264,"cache_write_input_tokens":0,"output_tokens":6,"reasoning_output_tokens":0}}
"#;

    /// A claude stream-json init plus assistant message (real shape).
    const CLAUDE_RUN: &str = r#"{"type":"system","subtype":"init","cwd":"/tmp","session_id":"95465654-0fdf-4e1a-ab39-4f30c5575580","tools":["Read"],"model":"claude-sonnet-5"}
{"type":"assistant","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-5","content":[{"type":"text","text":"probe-ok"}],"usage":{"input_tokens":15,"output_tokens":7,"cache_read_input_tokens":3}}}
"#;

    /// A grok streaming-messages-json init plus assistant message (real shape).
    const GROK_RUN: &str = r#"{"type":"system","subtype":"init","session_id":"01a067dc-92a1-7513-89d6-df5767f0ceda","apiKeySource":"oauth","model":"grok-4.6","cwd":"/tmp"}
{"type":"assistant","message":{"id":"msg_0","type":"message","role":"assistant","model":"grok-4.6","content":[{"type":"thinking","thinking":"inner"},{"type":"text","text":"probe-ok"}]}}
"#;

    const CLAUDE_QUOTA_FAILURE: &str = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"partial output"}],"usage":{"input_tokens":19,"output_tokens":4}}}
{"type":"result","subtype":"success","is_error":true,"result":"You've hit your weekly limit · resets 11pm (Africa/Lagos)"}
"#;

    const CODEX_RATE_LIMIT_FAILURE: &str = r#"{"type":"turn.failed","error":{"message":"Rate limit exceeded. Try again later."}}
"#;

    #[tokio::test]
    async fn stderr_capture_retains_a_bounded_tail() {
        use tokio::io::AsyncWriteExt;
        let (mut writer, mut reader) = tokio::io::duplex(8192);
        let write = tokio::spawn(async move {
            let block = vec![b'x'; 8192];
            for _ in 0..40 {
                writer.write_all(&block).await.unwrap();
            }
            writer.write_all(b"tail-marker").await.unwrap();
        });
        let tail = read_stderr_tail(&mut reader, 4096).await;
        write.await.unwrap();
        assert_eq!(tail.len(), 4096);
        assert!(tail.ends_with(b"tail-marker"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn claude_room_config_replaces_the_host_socket_with_the_private_mount() {
        let mut config = serde_json::json!({"mcpServers":{"orgbot_room":{"args":["room-mcp","--socket","/run/user/1000/host.sock"]}}});
        replace_room_socket(
            &mut config,
            Path::new("/run/user/1000/orgbot-room-mcp.sock"),
        )
        .unwrap();
        assert_eq!(
            config["mcpServers"]["orgbot_room"]["args"][2],
            "/run/user/1000/orgbot-room-mcp.sock"
        );
        assert!(
            replace_room_socket(&mut serde_json::json!({}), Path::new("/safe/socket")).is_err()
        );
    }

    fn fold(cli: CliKind, run: &str) -> Episode {
        let mut episode = Episode::default();
        for line in run.lines() {
            ingest_line(cli, line, &None, &mut episode);
        }
        episode
    }

    #[cfg(unix)]
    fn test_directory(name: &str) -> PathBuf {
        let mut nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        loop {
            let path = std::env::temp_dir().join(format!(
                "{name}-{}-{nonce}",
                std::process::id()
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return path,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    nonce = nonce.wrapping_add(1);
                }
                Err(error) => panic!("could not create test directory {}: {error}", path.display()),
            }
        }
    }

    #[cfg(unix)]
    fn fake_claude_script(
        binary: &Path,
        counter: &Path,
        prompts: &Path,
        args_log: &Path,
        fail_on_call: Option<usize>,
    ) {
        use std::os::unix::fs::PermissionsExt;

        let mut script = r##"#!/bin/sh
count=0
if [ -f 'COUNTER_PATH' ]; then read count < 'COUNTER_PATH'; fi
count=$((count + 1))
printf '%s\n' "$count" > 'COUNTER_PATH'
printf '%s\n' '---PROMPT---' >> 'PROMPT_PATH'
printf '%s\n' '---ARGS---' >> 'ARGS_PATH'
capture=0
for arg in "$@"; do
  if [ "$capture" -eq 1 ]; then printf '%s\n' "$arg" >> 'PROMPT_PATH'; capture=0; fi
  if [ "$arg" = "-p" ]; then capture=1; fi
  printf '%s\n' "$arg" >> 'ARGS_PATH'
done
printf '%s\n' '---END---' >> 'PROMPT_PATH'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"stable-session"}'
if [ 'FAIL_ON_CALL' != '0' ] && [ "$count" -eq 'FAIL_ON_CALL' ]; then exit 1; fi
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"continued"}]}}'
"##
        .replace("COUNTER_PATH", &counter.to_string_lossy())
        .replace("PROMPT_PATH", &prompts.to_string_lossy())
        .replace("ARGS_PATH", &args_log.to_string_lossy())
        .replace(
            "FAIL_ON_CALL",
            &fail_on_call.map_or(0, |call| call).to_string(),
        );
        script.push('\n');
        std::fs::write(binary, script).unwrap();
        std::fs::set_permissions(binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(unix)]
    fn prompt_sections(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap()
            .split("---PROMPT---\n")
            .skip(1)
            .map(|section| section.split("---END---").next().unwrap().to_string())
            .collect()
    }

    #[cfg(unix)]
    fn fake_claude_provider(binary: &Path, session_store: Option<PathBuf>) -> AgentCliProvider {
        let provider = AgentCliProvider {
            cli: CliKind::Claude,
            bin: binary.to_string_lossy().into_owned(),
            model: "claude-opus-5-5".into(),
            workdir: PathBuf::new(),
            policy: AgentPolicy::ReadOnly,
            session: Mutex::new(None),
            session_system_brief: Mutex::new(None),
            last_socket: Mutex::new(None),
            mcp: false,
            timeout: Duration::from_secs(5),
            session_store: Mutex::new(None),
            codex_usage: Mutex::new(None),
            #[cfg(target_os = "linux")]
            guard: None,
        };
        if let Some(path) = session_store {
            provider.with_session_store(path)
        } else {
            provider
        }
    }

    #[test]
    fn codex_output_yields_text_session_and_usage() {
        let episode = fold(CliKind::Codex, CODEX_RUN);
        assert_eq!(episode.text.trim(), "probe-ok");
        assert_eq!(
            episode.session_id.as_deref(),
            Some("01a067db-ea53-7973-b9ae-4b63f2af5026")
        );
        assert_eq!(episode.usage.input_tokens, 24416 - 11264);
        assert_eq!(episode.usage.cache_read_tokens, 11264);
        assert_eq!(episode.usage.output_tokens, 6);
    }

    #[test]
    fn claude_output_yields_text_and_session() {
        let episode = fold(CliKind::Claude, CLAUDE_RUN);
        assert_eq!(episode.text.trim(), "probe-ok");
        assert_eq!(
            episode.session_id.as_deref(),
            Some("95465654-0fdf-4e1a-ab39-4f30c5575580")
        );
        assert_eq!(episode.usage.output_tokens, 7);
        assert_eq!(episode.usage.input_tokens, 15);
        assert_eq!(episode.usage.cache_read_tokens, 3);
        assert_eq!(episode.usage.total_input(), 18);
    }

    #[test]
    fn grok_output_skips_thinking_and_takes_the_text() {
        let episode = fold(CliKind::Grok, GROK_RUN);
        assert_eq!(episode.text.trim(), "probe-ok");
        assert!(!episode.text.contains("inner"));
        assert_eq!(
            episode.session_id.as_deref(),
            Some("01a067dc-92a1-7513-89d6-df5767f0ceda")
        );
    }

    #[tokio::test]
    async fn cli_activity_reports_only_bounded_tool_names_and_outcomes() {
        let (sink, mut events) = mpsc::channel(8);
        let mut episode = Episode::default();
        ingest_line(
            CliKind::Claude,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hidden reasoning secret"},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"echo owner-token"}}]}}"#,
            &Some(sink.clone()),
            &mut episode,
        );
        ingest_line(
            CliKind::Claude,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"owner-token","is_error":false}]}}"#,
            &Some(sink),
            &mut episode,
        );
        let start = events.try_recv().unwrap();
        let end = events.try_recv().unwrap();
        assert_eq!(
            start,
            StreamEvent::ToolUseStart {
                id: "toolu_1".into(),
                name: "Bash".into(),
            }
        );
        assert_eq!(
            end,
            StreamEvent::ToolUseEnd {
                id: "toolu_1".into(),
                succeeded: true,
            }
        );
        let serialized = serde_json::to_string(&[start, end]).unwrap();
        assert!(!serialized.contains("owner-token"));
        assert!(!serialized.contains("hidden reasoning secret"));
        assert!(episode.text.is_empty());
    }

    #[tokio::test]
    async fn codex_process_activity_uses_categories_without_command_text() {
        let (sink, mut events) = mpsc::channel(8);
        let mut episode = Episode::default();
        ingest_line(
            CliKind::Codex,
            r#"{"type":"item.started","item":{"id":"item-1","type":"command_execution","command":"curl https://private.invalid?token=secret"}}"#,
            &Some(sink.clone()),
            &mut episode,
        );
        ingest_line(
            CliKind::Codex,
            r#"{"type":"item.completed","item":{"id":"item-1","type":"command_execution","exit_code":0}}"#,
            &Some(sink),
            &mut episode,
        );
        let start = events.try_recv().unwrap();
        let end = events.try_recv().unwrap();
        assert_eq!(
            start,
            StreamEvent::ToolUseStart {
                id: "item-1".into(),
                name: "Run command".into(),
            }
        );
        assert_eq!(
            end,
            StreamEvent::ToolUseEnd {
                id: "item-1".into(),
                succeeded: true,
            }
        );
        assert!(!serde_json::to_string(&[start, end])
            .unwrap()
            .contains("private.invalid"));
    }

    #[test]
    fn structured_cli_failures_are_parsed_without_losing_usage() {
        let claude = fold(CliKind::Claude, CLAUDE_QUOTA_FAILURE);
        assert!(claude
            .structured_error
            .as_deref()
            .unwrap()
            .contains("weekly limit"));
        assert_eq!(claude.usage.input_tokens, 19);
        assert_eq!(claude.usage.output_tokens, 4);

        let codex = fold(CliKind::Codex, CODEX_RATE_LIMIT_FAILURE);
        assert_eq!(
            codex.structured_error.as_deref(),
            Some("Rate limit exceeded. Try again later.")
        );
    }

    #[test]
    fn explicit_cli_quota_messages_are_retryable_without_guessed_delay() {
        for message in [
            "You've hit your weekly limit · resets 11pm (Africa/Lagos)",
            "rate-limit exceeded",
            "too many requests for this account",
        ] {
            assert!(matches!(
                classify_cli_error(CliKind::Claude, message),
                ProviderError::RateLimited {
                    retry_after_secs: None,
                    ..
                }
            ));
        }
        assert!(matches!(
            classify_cli_error(CliKind::Claude, "MCP server issuer unavailable"),
            ProviderError::Process { .. }
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn structured_error_overrides_exit_success_and_incidental_stderr() {
        use std::os::unix::fs::PermissionsExt;

        let directory = test_directory("orgbot-cli-error-test");
        let binary = directory.join("fake-claude");
        std::fs::write(
            &binary,
            r##"#!/bin/sh
printf '%s\n' 'MCP issuer warning: incidental stderr' >&2
printf '%s\n' '{"type":"result","subtype":"success","is_error":true,"result":"weekly limit reached","usage":{"input_tokens":7,"output_tokens":2}}'
"##,
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let provider = AgentCliProvider {
            cli: CliKind::Claude,
            bin: binary.to_string_lossy().into_owned(),
            model: "claude-opus-5-5".into(),
            workdir: PathBuf::new(),
            policy: AgentPolicy::ReadOnly,
            session: Mutex::new(None),
            session_system_brief: Mutex::new(None),
            last_socket: Mutex::new(None),
            mcp: false,
            timeout: Duration::from_secs(5),
            session_store: Mutex::new(None),
            codex_usage: Mutex::new(None),
            #[cfg(target_os = "linux")]
            guard: None,
        };
        let mut episode = Episode::default();
        let error = provider
            .run_episode("test prompt", None, &None, &mut episode, None, None)
            .await
            .unwrap_err();
        assert!(
            matches!(
                error,
                ProviderError::RateLimited {
                    retry_after_secs: None,
                    ..
                }
            ),
            "expected the structured quota failure to win over stderr, got: {error:?}"
        );
        assert!(error.to_string().contains("weekly limit"));
        assert_eq!(episode.usage.input_tokens, 7);
        assert_eq!(episode.usage.output_tokens, 2);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn codex_usage_without_cache_fields_still_parses() {
        let mut episode = Episode::default();
        ingest_line(
            CliKind::Codex,
            r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":2}}"#,
            &None,
            &mut episode,
        );
        assert_eq!(episode.usage.input_tokens, 10);
        assert_eq!(episode.usage.output_tokens, 2);
        assert_eq!(episode.usage.cache_read_tokens, 0);
    }

    #[test]
    fn codex_cumulative_usage_is_delta_and_missing_usage_stays_missing() {
        let previous = usage_from_json(&serde_json::json!({
            "input_tokens": 100, "cached_input_tokens": 40,
            "output_tokens": 10, "cache_write_input_tokens": 3
        }));
        let current = usage_from_json(&serde_json::json!({
            "input_tokens": 160, "cached_input_tokens": 55,
            "output_tokens": 18, "cache_write_input_tokens": 5
        }));
        let delta = usage_delta(current, Some(previous));
        assert_eq!(delta.input_tokens, 45);
        assert_eq!(delta.cache_read_tokens, 15);
        assert_eq!(delta.output_tokens, 8);
        assert_eq!(delta.cache_write_tokens, 2);
        assert!(delta.reported && delta.complete);
        let partial_previous = Usage {
            complete: false,
            ..previous
        };
        assert_eq!(usage_delta(current, Some(partial_previous)), Usage::missing());
        let rollback = usage_delta(previous, Some(current));
        assert_eq!(rollback, Usage::missing());

        let missing = usage_delta(Usage::default(), Some(current));
        assert!(!missing.reported);
        assert_eq!(usage_delta(current, None), current);
        assert!(!codex_usage_delta(current, None, Some("same"), Some("same")).reported);
        assert_eq!(
            codex_usage_delta(current, None, Some("old"), Some("new")),
            current
        );
        assert_eq!(codex_usage_delta(current, None, None, Some("new")), current);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn new_codex_session_discards_the_previous_usage_baseline() {
        use std::os::unix::fs::PermissionsExt;

        let directory = test_directory("orgbot-codex-new-session");
        let binary = directory.join("fake-codex");
        std::fs::write(
            &binary,
            "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"new-session\"}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let provider = AgentCliProvider {
            cli: CliKind::Codex,
            bin: binary.to_string_lossy().into_owned(),
            model: "gpt-6.1-sol".into(),
            workdir: PathBuf::new(),
            policy: AgentPolicy::ReadOnly,
            session: Mutex::new(Some("old-session".into())),
            session_system_brief: Mutex::new(None),
            last_socket: Mutex::new(None),
            mcp: false,
            timeout: Duration::from_secs(5),
            session_store: Mutex::new(None),
            codex_usage: Mutex::new(Some(usage_from_json(&serde_json::json!({
                "input_tokens": 100, "output_tokens": 10
            })))),
            #[cfg(target_os = "linux")]
            guard: None,
        };
        let mut episode = Episode::default();
        provider
            .run_episode("prompt", None, &None, &mut episode, None, None)
            .await
            .unwrap();
        assert_eq!(provider.session_id().as_deref(), Some("new-session"));
        assert_eq!(*provider.codex_usage.lock().unwrap(), None);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn codex_complete_deltas_cumulative_usage_after_provider_restarts() {
        use std::os::unix::fs::PermissionsExt;

        let directory = test_directory("orgbot-codex-usage-resume");
        let binary = directory.join("fake-codex");
        let store_path = directory.join("session.json");
        std::fs::write(
            &binary,
            r##"#!/bin/sh
count_file="$(dirname "$0")/count"
count=0
if [ -f "$count_file" ]; then read count < "$count_file"; fi
count=$((count + 1))
printf '%s\n' "$count" > "$count_file"
printf '%s\n' '{"type":"thread.started","thread_id":"persistent-session"}'
case "$count" in
  1) usage='"usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":10}' ;;
  2) usage='"usage":{"input_tokens":160,"cached_input_tokens":50,"output_tokens":16}' ;;
  3) usage='"usage":{"output_tokens":20}' ;;
  4) usage='"usage":{"input_tokens":210,"cached_input_tokens":65,"output_tokens":24}' ;;
  5) usage='' ;;
  6) usage='"usage":{"input_tokens":230,"cached_input_tokens":70,"output_tokens":28}' ;;
esac
printf '{"type":"turn.completed",%s}\n' "$usage"
"##,
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = SessionStore {
            path: store_path.clone(),
            cli: CliKind::Codex,
            model: "gpt-6.1-sol".into(),
            workdir: canonical_workdir(&directory),
            policy: AgentPolicy::ReadOnly,
            mcp: false,
        };
        let make_provider = || {
            AgentCliProvider {
                cli: CliKind::Codex,
                bin: binary.to_string_lossy().into_owned(),
                model: store.model.clone(),
                workdir: directory.clone(),
                policy: store.policy,
                session: Mutex::new(None),
                session_system_brief: Mutex::new(None),
                last_socket: Mutex::new(None),
                mcp: false,
                timeout: Duration::from_secs(5),
                session_store: Mutex::new(None),
                codex_usage: Mutex::new(None),
                #[cfg(target_os = "linux")]
                guard: None,
            }
            .with_session_store(store_path.clone())
        };
        let request = || {
            CompletionRequest::new(
                "gpt-6.1-sol",
                "role brief",
                vec![Turn::user("continue")],
            )
        };

        let first = make_provider().complete(request(), None).await.unwrap();
        assert_eq!(first.usage.input_tokens, 60);
        assert_eq!(first.usage.cache_read_tokens, 40);
        let second = make_provider().complete(request(), None).await.unwrap();
        assert_eq!(second.usage.input_tokens, 50);
        assert_eq!(second.usage.cache_read_tokens, 10);
        assert_eq!(second.usage.output_tokens, 6);
        let missing = make_provider().complete(request(), None).await.unwrap();
        assert_eq!(missing.usage, Usage::missing());
        let after_partial = make_provider().complete(request(), None).await.unwrap();
        assert_eq!(after_partial.usage.input_tokens, 35);
        assert_eq!(after_partial.usage.cache_read_tokens, 15);
        assert_eq!(after_partial.usage.output_tokens, 8);
        let absent = make_provider().complete(request(), None).await.unwrap();
        assert!(!absent.usage.reported);
        let after_absent = make_provider().complete(request(), None).await.unwrap();
        assert_eq!(after_absent.usage.input_tokens, 15);
        assert_eq!(after_absent.usage.cache_read_tokens, 5);
        assert_eq!(after_absent.usage.output_tokens, 4);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn codex_mcp_keeps_sandbox_and_never_approval_for_every_policy() {
        for (policy, expected) in [
            (AgentPolicy::ReadOnly, "read-only"),
            (AgentPolicy::WorkspaceWrite, "workspace-write"),
            (AgentPolicy::FullAccess, "danger-full-access"),
        ] {
            let mut cmd = Command::new("codex");
            build_args(
                &mut cmd,
                CliKind::Codex,
                "test",
                None,
                "",
                policy,
                None,
                true,
            );
            let args: Vec<_> = cmd
                .as_std()
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            assert!(args.windows(2).any(|pair| pair == ["-s", expected]));
            assert!(args
                .windows(2)
                .any(|pair| pair == ["-c", "approval_policy=\"never\""]));
            assert!(!args.iter().any(|arg| arg == "--approve-for-me"));
        }
    }

    #[test]
    fn codex_resume_does_not_enable_automatic_review_for_any_policy() {
        for policy in [
            AgentPolicy::ReadOnly,
            AgentPolicy::WorkspaceWrite,
            AgentPolicy::FullAccess,
        ] {
            let mut cmd = Command::new("codex");
            build_args(
                &mut cmd,
                CliKind::Codex,
                "--- the board, as it stands now ---\nstate follows",
                Some("prior-session"),
                "gpt-6.1-sol",
                policy,
                None,
                true,
            );
            let args: Vec<_> = cmd
                .as_std()
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            assert!(args
                .windows(2)
                .any(|pair| pair == ["-c", "approval_policy=\"never\""]));
            assert!(!args.iter().any(|arg| arg == "-s"));
            assert!(!args.iter().any(|arg| arg == "--approve-for-me"));
            assert!(!args.iter().any(|arg| arg == "--full-auto"));
            let prompt = "--- the board, as it stands now ---\nstate follows";
            let override_index = args.iter().position(|arg| arg == "-c").unwrap();
            let session_index = args.iter().position(|arg| arg == "prior-session").unwrap();
            let prompt_index = args.iter().position(|arg| arg == prompt).unwrap();
            let model_index = args.iter().position(|arg| arg == "-m").unwrap();
            let effort_index = args
                .windows(2)
                .position(|pair| pair == ["-c", "model_reasoning_effort=\"xhigh\""])
                .unwrap();
            assert!(override_index < session_index && session_index < prompt_index);
            assert!(model_index < session_index && effort_index < session_index);
            assert_eq!(args[session_index - 1], "--");
            assert_eq!(prompt_index, args.len() - 1);
        }
    }

    #[test]
    fn only_the_named_codex_model_gets_xhigh_effort() {
        for (cli, model, expected) in [
            (CliKind::Codex, "gpt-6.1-sol", true),
            (CliKind::Codex, "gpt-6.1", false),
            (CliKind::Claude, "gpt-6.1-sol", false),
            (CliKind::Grok, "gpt-6.1-sol", false),
        ] {
            let mut command = Command::new("agent");
            build_args(
                &mut command,
                cli,
                "work",
                None,
                model,
                AgentPolicy::ReadOnly,
                None,
                false,
            );
            let arguments: Vec<String> = command
                .as_std()
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect();
            assert_eq!(
                arguments
                    .windows(2)
                    .any(|pair| pair == ["-c", "model_reasoning_effort=\"xhigh\""]),
                expected,
                "{cli:?} {model}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn saved_session_is_owner_only_and_bound_to_provider_model_and_workdir() {
        use std::os::unix::fs::PermissionsExt;

        let directory = std::env::temp_dir().join(format!(
            "orgbot-session-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("bot.json");
        let store = SessionStore {
            path: path.clone(),
            cli: CliKind::Codex,
            model: "gpt-6.1-sol".into(),
            workdir: "/tmp/project".into(),
            policy: AgentPolicy::WorkspaceWrite,
            mcp: true,
        };
        let codex_usage = usage_from_json(&serde_json::json!({
            "input_tokens": 100, "output_tokens": 10
        }));
        write_session(&store, "session-id", None, Some(codex_usage)).unwrap();
        assert_eq!(read_session(&store).as_deref(), Some("session-id"));
        assert_eq!(
            read_session_record(&store).unwrap().codex_usage,
            Some(codex_usage)
        );
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let restored = AgentCliProvider {
            cli: store.cli,
            bin: "/fake/codex".into(),
            model: store.model.clone(),
            workdir: PathBuf::from("/tmp/project"),
            policy: store.policy,
            session: Mutex::new(None),
            session_system_brief: Mutex::new(None),
            last_socket: Mutex::new(None),
            mcp: store.mcp,
            timeout: Duration::from_secs(5),
            session_store: Mutex::new(None),
            codex_usage: Mutex::new(None),
            #[cfg(target_os = "linux")]
            guard: None,
        }
        .with_session_store(path.clone());
        assert_eq!(*restored.codex_usage.lock().unwrap(), Some(codex_usage));
        let mut mismatched = store.clone();
        mismatched.model = "gpt-6.1".into();
        assert!(read_session(&mismatched).is_none());
        mismatched.model = store.model.clone();
        mismatched.cli = CliKind::Claude;
        assert!(read_session(&mismatched).is_none());
        mismatched.cli = store.cli;
        mismatched.workdir = "/tmp/other".into();
        assert!(read_session(&mismatched).is_none());
        mismatched.workdir = store.workdir.clone();
        mismatched.policy = AgentPolicy::ReadOnly;
        assert!(read_session(&mismatched).is_none());
        mismatched.policy = store.policy;
        mismatched.mcp = false;
        assert!(read_session(&mismatched).is_none());

        let legacy_path = directory.join("legacy.json");
        let legacy_store = SessionStore {
            path: legacy_path.clone(),
            cli: CliKind::Claude,
            model: "claude-opus-5-5".into(),
            workdir: canonical_workdir(&PathBuf::new()),
            policy: AgentPolicy::ReadOnly,
            mcp: false,
        };
        let mut legacy_record = serde_json::to_value(PersistedSession {
            cli: legacy_store.cli,
            model: legacy_store.model.clone(),
            workdir: legacy_store.workdir.clone(),
            policy: legacy_store.policy,
            mcp: legacy_store.mcp,
            session_id: "legacy-session".into(),
            system_brief: None,
            codex_usage: None,
        })
        .unwrap();
        legacy_record
            .as_object_mut()
            .unwrap()
            .remove("system_brief");
        legacy_record.as_object_mut().unwrap().remove("codex_usage");
        std::fs::write(&legacy_path, serde_json::to_vec(&legacy_record).unwrap()).unwrap();
        std::fs::set_permissions(&legacy_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let restored = fake_claude_provider(Path::new("/fake/claude"), Some(legacy_path));
        assert_eq!(restored.session_id().as_deref(), Some("legacy-session"));
        assert_eq!(*restored.session_system_brief.lock().unwrap(), None);
        assert_eq!(*restored.codex_usage.lock().unwrap(), None);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cli_mcp_overlay_copies_config_into_owner_only_temp_files() {
        use std::os::unix::fs::PermissionsExt;

        let source = std::env::temp_dir().join(format!(
            "orgbot-cli-home-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join("config.toml"),
            "api_key = \"synthetic-secret\"\n",
        )
        .unwrap();
        std::fs::write(source.join("auth.json"), "{}").unwrap();
        let overlay = write_agent_home_at("/tmp/room.sock", &source, "test", |config, _, _| {
            config.push_str("\n[mcp_servers.orgbot_room]\nenabled = true\n");
        })
        .unwrap();
        let copied = std::fs::read_to_string(overlay.join("config.toml")).unwrap();
        assert!(copied.contains("synthetic-secret"));
        assert!(copied.contains("mcp_servers.orgbot_room"));
        assert_eq!(
            std::fs::metadata(&overlay).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(overlay.join("config.toml"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(std::fs::symlink_metadata(overlay.join("auth.json"))
            .unwrap()
            .file_type()
            .is_symlink());
        std::fs::remove_dir_all(overlay).unwrap();
        std::fs::remove_dir_all(source).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn first_session_id_is_saved_before_an_interrupted_episode() {
        use std::os::unix::fs::PermissionsExt;

        let directory = std::env::temp_dir().join(format!(
            "orgbot-interrupted-session-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let binary = directory.join("fake-codex");
        std::fs::write(
            &binary,
            "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"session-before-timeout\"}'\nexec sleep 2\n",
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let state_path = directory.join("bot.json");
        let store = SessionStore {
            path: state_path.clone(),
            cli: CliKind::Codex,
            model: "gpt-6.1-sol".into(),
            workdir: canonical_workdir(Path::new("/tmp")),
            policy: AgentPolicy::WorkspaceWrite,
            mcp: false,
        };
        let provider = AgentCliProvider {
            cli: CliKind::Codex,
            bin: binary.to_string_lossy().into_owned(),
            model: store.model.clone(),
            workdir: PathBuf::from("/tmp"),
            policy: store.policy,
            session: Mutex::new(None),
            session_system_brief: Mutex::new(None),
            last_socket: Mutex::new(None),
            mcp: store.mcp,
            timeout: Duration::from_millis(50),
            session_store: Mutex::new(Some(store.clone())),
            codex_usage: Mutex::new(None),
            #[cfg(target_os = "linux")]
            guard: None,
        };
        let mut episode = Episode::default();
        let result = provider
            .run_episode("prompt", None, &None, &mut episode, None, None)
            .await;
        assert!(
            result.is_err(),
            "the fake CLI should exceed its turn timeout"
        );
        assert_eq!(
            read_session(&store).as_deref(),
            Some("session-before-timeout")
        );
        assert_eq!(
            read_session_record(&store)
                .and_then(|record| record.system_brief)
                .as_deref(),
            None
        );
        let resumed = AgentCliProvider::new(
            store.cli,
            store.model.clone(),
            "/tmp",
            store.policy,
            store.mcp,
        )
        .unwrap()
        .with_session_store(state_path);
        assert_eq!(
            resumed.session_id().as_deref(),
            Some("session-before-timeout")
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn protected_fake_cli_resumes_in_private_profile_and_cannot_redirect_writes_home() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = std::env::temp_dir().join(format!(
            "orgbot-protected-cli-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("original-home");
        let project = root.join("project");
        let state = root.join("state/workspaces");
        std::fs::create_dir_all(source.join(".codex/sessions")).unwrap();
        std::fs::create_dir_all(source.join(".codex/skills/local")).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(source.join(".codex/auth.json"), "fixture-auth").unwrap();
        std::fs::write(
            source.join(".codex/config.toml"),
            "model = \"fixture\"\n[mcp_servers.remote]\ncommand = \"remote-fixture\"\n",
        )
        .unwrap();
        std::fs::write(source.join(".codex/skills/local/SKILL.md"), "fixture skill").unwrap();
        std::fs::write(project.join("kept.txt"), "original-project").unwrap();

        let binary = project.join("fake-codex");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$HOME|$CODEX_HOME\" > \"$CODEX_HOME/env-record.txt\"\nif [ ! -f \"$CODEX_HOME/auth.json\" ] || [ ! -f \"$CODEX_HOME/skills/local/SKILL.md\" ]; then exit 41; fi\nif grep -q 'remote\\|mcp_servers' \"$CODEX_HOME/config.toml\"; then exit 43; fi\ncase \"$*\" in *stable-session*) [ -f \"$CODEX_HOME/sessions/stable-session.jsonl\" ] || exit 42 ;; *) mkdir -p \"$CODEX_HOME/sessions\"; printf 'resume state\\n' > \"$CODEX_HOME/sessions/stable-session.jsonl\" ;; esac\nprintf 'modified-private-auth' > \"$CODEX_HOME/auth.json\"\nprintf 'generated-by-cli' > /tmp/orgbot-workspace/generated.txt\nprintf '%s\\n' '{{\"type\":\"thread.started\",\"thread_id\":\"stable-session\"}}'\nprintf '%s\\n' '{{\"type\":\"item.completed\",\"item\":{{\"type\":\"agent_message\",\"text\":\"protected-ok\"}}}}'\n"
        );
        std::fs::write(&binary, script).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();

        let session_path = root.join("state/cli-session.json");
        let mut provider = AgentCliProvider::new(
            CliKind::Codex,
            "fixture-model",
            project.to_string_lossy(),
            AgentPolicy::WorkspaceWrite,
            false,
        )
        .unwrap();
        provider.bin = binary.to_string_lossy().into_owned();
        provider.timeout = Duration::from_secs(10);
        provider = provider.with_session_store(session_path);
        provider = provider.with_workspace_guard_project(
            state.clone(),
            "fake-agent".into(),
            project.clone(),
        );
        provider.guard.as_mut().unwrap().profile_source = Some(source.clone());

        let mut first = Episode::default();
        provider
            .run_episode("first", None, &None, &mut first, None, None)
            .await
            .unwrap();
        assert_eq!(first.session_id.as_deref(), Some("stable-session"));
        assert_eq!(first.text.trim(), "protected-ok");
        assert!(project.join("generated.txt").is_file());
        assert_eq!(
            std::fs::read_to_string(source.join(".codex/auth.json")).unwrap(),
            "fixture-auth"
        );
        let prepared = provider.guard.as_ref().unwrap().prepare().unwrap();
        let recorded =
            std::fs::read_to_string(prepared.private_home.join(".codex/env-record.txt")).unwrap();
        assert!(recorded.contains("/run/user/"));
        assert!(recorded.contains("/orgbot-home/.codex"));

        let mut resumed = Episode::default();
        provider
            .run_episode(
                "second",
                first.session_id.as_deref(),
                &None,
                &mut resumed,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(resumed.session_id.as_deref(), Some("stable-session"));
        assert_eq!(
            std::fs::read_to_string(source.join(".codex/auth.json")).unwrap(),
            "fixture-auth"
        );

        // No room socket means no room bridge, but copied remote MCP servers
        // still must not be inherited by a CLI that is MCP-capable.
        let mut missing_socket = AgentCliProvider::new(
            CliKind::Codex,
            "fixture-model",
            project.to_string_lossy(),
            AgentPolicy::WorkspaceWrite,
            true,
        )
        .unwrap();
        missing_socket.bin = binary.to_string_lossy().into_owned();
        missing_socket.timeout = Duration::from_secs(10);
        missing_socket = missing_socket.with_workspace_guard_project(
            state.clone(),
            "missing-socket-agent".into(),
            project.clone(),
        );
        missing_socket.guard.as_mut().unwrap().profile_source = Some(source.clone());
        let mut no_socket_episode = Episode::default();
        missing_socket
            .run_episode("no socket", None, &None, &mut no_socket_episode, None, None)
            .await
            .unwrap();
        assert_eq!(no_socket_episode.text.trim(), "protected-ok");

        let home = root.join("state/workspaces/agents");
        let private_auth = std::fs::read_dir(&home)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join("fake-agent/profiles/home/.codex/auth.json");
        std::fs::remove_file(&private_auth).unwrap();
        symlink(source.join(".codex/auth.json"), &private_auth).unwrap();
        let mut rejected = Episode::default();
        assert!(provider
            .run_episode(
                "third",
                Some("stable-session"),
                &None,
                &mut rejected,
                None,
                None
            )
            .await
            .is_err());
        assert_eq!(
            std::fs::read_to_string(source.join(".codex/auth.json")).unwrap(),
            "fixture-auth"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_session_the_cli_no_longer_has_is_dropped_and_the_turn_runs_fresh() {
        use std::os::unix::fs::PermissionsExt;
        let directory = std::env::temp_dir().join(format!(
            "orgbot-cli-missing-session-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let binary = directory.join("fake-claude");
        // Resuming fails the way the real CLIs do once a session is gone.
        std::fs::write(
            &binary,
            r##"#!/bin/sh
for arg in "$@"; do
  if [ "$arg" = "--resume" ]; then
    echo 'Error: No conversation found with session ID: gone-session' >&2
    exit 1
  fi
done
printf '%s\n' '{"type":"system","subtype":"init","session_id":"fresh-session"}'
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}'
"##,
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = directory.join("sessions").join("bot.json");
        let provider = fake_claude_provider(&binary, Some(store.clone()));
        *provider.session.lock().unwrap() = Some("gone-session".into());

        let response = provider
            .complete(
                CompletionRequest::new("claude-opus-5-5", "brief", vec![Turn::user("say hi")]),
                None,
            )
            .await
            .expect("the turn should run in a fresh session");
        assert_eq!(response.content.len(), 1);
        assert_eq!(provider.session_id().as_deref(), Some("fresh-session"));
        assert!(std::fs::read_to_string(&store)
            .unwrap()
            .contains("fresh-session"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_first_episode_retries_the_observed_native_session() {
        let directory = std::env::temp_dir().join(format!(
            "orgbot-cli-retry-session-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let binary = directory.join("fake-claude");
        let counter = directory.join("counter");
        let prompts = directory.join("prompts");
        let args = directory.join("args");
        fake_claude_script(&binary, &counter, &prompts, &args, Some(1));
        let provider = fake_claude_provider(&binary, None);

        let first = CompletionRequest::new(
            "claude-opus-5-5",
            "stable role brief",
            vec![Turn::user("first situation")],
        );
        assert!(provider.complete(first, None).await.is_err());
        assert_eq!(provider.session_id().as_deref(), Some("stable-session"));

        let retry = CompletionRequest::new(
            "claude-opus-5-5",
            "stable role brief",
            vec![Turn::user("retry situation")],
        );
        provider.complete(retry, None).await.unwrap();
        let sections = prompt_sections(&prompts);
        assert_eq!(sections.len(), 2);
        assert!(sections[0].contains("stable role brief"));
        assert!(sections[0].contains("first situation"));
        assert!(sections[1].contains("stable role brief"));
        assert!(sections[1].contains("retry situation"));
        assert!(std::fs::read_to_string(&args)
            .unwrap()
            .contains("--resume\nstable-session"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_role_brief_change_is_sent_once_across_native_session_restarts() {
        let directory = std::env::temp_dir().join(format!(
            "orgbot-cli-role-refresh-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let binary = directory.join("fake-claude");
        let counter = directory.join("counter");
        let prompts = directory.join("prompts");
        let args = directory.join("args");
        let state_path = directory.join("cli-session.json");
        fake_claude_script(&binary, &counter, &prompts, &args, Some(2));

        let original = fake_claude_provider(&binary, Some(state_path.clone()));
        original
            .complete(
                CompletionRequest::new(
                    "claude-opus-5-5",
                    "ROLE V1 UNIQUE",
                    vec![Turn::user("initial task")],
                ),
                None,
            )
            .await
            .unwrap();
        assert_eq!(original.session_id().as_deref(), Some("stable-session"));
        drop(original);

        let updated = fake_claude_provider(&binary, Some(state_path.clone()));
        assert_eq!(updated.session_id().as_deref(), Some("stable-session"));
        assert!(updated
            .complete(
                CompletionRequest::new(
                    "claude-opus-5-5",
                    "ROLE V2 UNIQUE",
                    vec![Turn::user("task after role update")],
                ),
                None,
            )
            .await
            .is_err());
        assert_eq!(
            read_session_record(&SessionStore {
                path: state_path.clone(),
                cli: CliKind::Claude,
                model: "claude-opus-5-5".into(),
                workdir: canonical_workdir(&PathBuf::new()),
                policy: AgentPolicy::ReadOnly,
                mcp: false,
            })
            .and_then(|record| record.system_brief)
            .as_deref(),
            Some("ROLE V1 UNIQUE"),
            "the failed episode must not mark its brief as applied"
        );
        drop(updated);

        let retry = fake_claude_provider(&binary, Some(state_path.clone()));
        assert_eq!(retry.session_id().as_deref(), Some("stable-session"));
        retry
            .complete(
                CompletionRequest::new(
                    "claude-opus-5-5",
                    "ROLE V2 UNIQUE",
                    vec![Turn::user("retry after failed role refresh")],
                ),
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            read_session_record(&SessionStore {
                path: state_path.clone(),
                cli: CliKind::Claude,
                model: "claude-opus-5-5".into(),
                workdir: canonical_workdir(&PathBuf::new()),
                policy: AgentPolicy::ReadOnly,
                mcp: false,
            })
            .and_then(|record| record.system_brief)
            .as_deref(),
            Some("ROLE V2 UNIQUE")
        );
        drop(retry);

        let restored = fake_claude_provider(&binary, Some(state_path.clone()));
        restored
            .complete(
                CompletionRequest::new(
                    "claude-opus-5-5",
                    "ROLE V2 UNIQUE",
                    vec![Turn::user("later task")],
                ),
                None,
            )
            .await
            .unwrap();
        let sections = prompt_sections(&prompts);
        assert_eq!(sections.len(), 4);
        assert!(sections[0].contains("ROLE V1 UNIQUE"));
        assert!(sections[1].contains("Updated standing role and operating brief"));
        assert!(sections[1].contains("ROLE V2 UNIQUE"));
        assert!(sections[1].contains("task after role update"));
        assert!(sections[2].contains("ROLE V2 UNIQUE"));
        assert!(sections[2].contains("retry after failed role refresh"));
        assert!(!sections[3].contains("ROLE V2 UNIQUE"));
        assert!(sections[3].contains("later task"));
        assert!(
            std::fs::read_to_string(&args)
                .unwrap()
                .matches("--resume\nstable-session")
                .count()
                >= 2
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn codex_room_approval_override_is_scoped_to_the_local_server() {
        let mut config = "[mcp_servers.external]\nurl = \"https://example.invalid\"\n".to_string();
        append_codex_room_server(&mut config, Path::new("/usr/bin/orgbot"), "/tmp/room.sock");

        let (external, room) = config
            .split_once("[mcp_servers.orgbot_room]")
            .expect("the guarded room server is configured");
        assert!(!external.contains("default_tools_approval_mode"));
        assert!(room.contains("default_tools_approval_mode = \"approve\""));
        assert!(room.contains("args = [\"room-mcp\", \"--socket\", \"/tmp/room.sock\" ]"));
        assert!(!config.contains("approval_policy"));
        assert!(config.contains("[mcp_servers.external]"));
    }

    #[test]
    fn spawned_cli_strips_app_secrets_but_keeps_native_cli_environment() {
        let mut cmd = Command::new("codex");
        cmd.env("PATH", "/native/bin:/usr/bin")
            .env("ANTHROPIC_API_KEY", "native-anthropic-placeholder")
            .env("OPENAI_API_KEY", "native-openai-placeholder")
            .env("ORGBOT_CLI_TURN_TIMEOUT_SECS", "600");
        let app_names = [
            "ORGBOT_ROOM_TOKEN",
            "DEEPSEEK_API_KEY",
            "ORGBOT_DEEPSEEK_TWO_API_KEY",
            "ORGBOT_OLLAMA_API_KEY",
            "ORGBOT_MCP_0_AUTHORIZATION",
        ]
        .map(std::ffi::OsString::from);
        remove_app_credentials(&mut cmd, app_names);

        let environment: std::collections::HashMap<_, _> = cmd
            .as_std()
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        for removed in [
            "ORGBOT_ROOM_TOKEN",
            "DEEPSEEK_API_KEY",
            "ORGBOT_DEEPSEEK_TWO_API_KEY",
            "ORGBOT_OLLAMA_API_KEY",
            "ORGBOT_MCP_0_AUTHORIZATION",
        ] {
            assert_eq!(
                environment.get(removed),
                Some(&None),
                "{removed} is removed"
            );
        }
        for kept in [
            "PATH",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "ORGBOT_CLI_TURN_TIMEOUT_SECS",
        ] {
            assert!(
                environment.get(kept).is_some_and(Option::is_some),
                "{kept} remains"
            );
        }
    }

    #[test]
    fn the_latest_user_prose_is_the_instruction() {
        let messages = vec![
            Turn::user("old situation"),
            Turn::assistant("old answer"),
            Turn::user("what changed now"),
        ];
        assert_eq!(
            latest_instruction(&messages).as_deref(),
            Some("what changed now")
        );
    }

    #[test]
    fn assistant_turns_never_count_as_instructions() {
        let messages = vec![Turn::assistant("an answer without a question")];
        assert_eq!(latest_instruction(&messages), None);
    }

    #[test]
    fn banners_and_progress_lines_are_ignored() {
        let mut episode = Episode::default();
        ingest_line(
            CliKind::Codex,
            "downloading model weights... 42%",
            &None,
            &mut episode,
        );
        assert!(episode.text.is_empty());
        assert!(episode.session_id.is_none());
    }

    #[test]
    fn policies_map_to_each_cli_vocabulary() {
        assert_eq!(
            policy_flags(CliKind::Codex, AgentPolicy::WorkspaceWrite),
            ("-s", "workspace-write")
        );
        assert_eq!(
            policy_flags(CliKind::Codex, AgentPolicy::FullAccess),
            ("-s", "danger-full-access")
        );
        // dontAsk rejects actions requiring approval; room MCP rules remain explicit.
        assert_eq!(
            policy_flags(CliKind::Claude, AgentPolicy::ReadOnly),
            ("--permission-mode", "dontAsk")
        );
        assert_ne!(
            policy_flags(CliKind::Grok, AgentPolicy::ReadOnly).1,
            "plan",
            "plan mode makes a bot unable to touch the board at all"
        );
        assert_eq!(
            policy_flags(CliKind::Grok, AgentPolicy::FullAccess),
            ("--permission-mode", "bypassPermissions")
        );
    }

    #[test]
    fn grok_mcp_rules_are_passed_as_separate_options() {
        for resumed in [None, Some("previous-session")] {
            for policy in [AgentPolicy::ReadOnly, AgentPolicy::FullAccess] {
                let mut cmd = Command::new("grok");
                build_args(
                    &mut cmd,
                    CliKind::Grok,
                    "test",
                    resumed,
                    "",
                    policy,
                    None,
                    true,
                );
                let args: Vec<_> = cmd
                    .as_std()
                    .get_args()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect();
                assert_eq!(args.iter().filter(|arg| *arg == "--allow").count(), 2);
                assert!(args
                    .windows(2)
                    .any(|pair| pair == ["--allow", ROOM_TOOL_PATTERN]));
                assert!(args
                    .windows(2)
                    .any(|pair| pair == ["--allow", ROOM_TOOL_WILDCARD]));
            }
        }
    }

    #[test]
    fn claude_registers_the_room_server_under_the_name_its_rules_allow() {
        // A hyphen/underscore mismatch here permission-blocks every room tool
        // for Claude bots while Codex and Grok bots keep working.
        let path = write_claude_mcp_config("/tmp/orgbot-test.sock").unwrap();
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        let servers = config["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(
            servers["orgbot_room"]["command"].as_str(),
            relay_binary().unwrap().to_str()
        );
        let name = servers.keys().next().unwrap();
        assert_eq!(ROOM_TOOL_PATTERN, format!("mcp__{name}"));
        assert_eq!(ROOM_TOOL_WILDCARD, format!("mcp__{name}__*"));
    }

    #[test]
    fn claude_read_only_mcp_uses_dont_ask_with_explicit_tools() {
        let mut cmd = Command::new("claude");
        build_args(
            &mut cmd,
            CliKind::Claude,
            "test",
            None,
            "",
            AgentPolicy::ReadOnly,
            Some(Path::new("/private/orgbot-room-mcp.json")),
            true,
        );
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--permission-mode", "dontAsk"]));
        assert!(args
            .windows(3)
            .any(|triple| { triple == ["--allowedTools", ROOM_TOOL_PATTERN, ROOM_TOOL_WILDCARD] }));
        assert!(args.iter().any(|arg| arg == "--strict-mcp-config"));
    }
}
