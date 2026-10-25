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

use async_trait::async_trait;
use orgbot_proto::agent::{AgentPolicy, CliKind};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::error::{ProviderError, Result};
use crate::provider::{Provider, emit};
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
    /// The socket handed in on the last request, for diagnostics.
    last_socket: Mutex<Option<String>>,
    /// Whether the room's board is served to the agent as MCP tools.
    mcp: bool,
    /// Turn ceiling.
    timeout: Duration,
    /// Optional owner-only record that lets one bot resume its native session
    /// after the room process is rebuilt.
    session_store: Mutex<Option<SessionStore>>,
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
            last_socket: Mutex::new(None),
            timeout,
            session_store: Mutex::new(None),
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
        if let Some(session_id) = read_session(&store) {
            *self.session.lock().unwrap() = Some(session_id);
        }
        *self.session_store.lock().unwrap() = Some(store);
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

fn read_session(store: &SessionStore) -> Option<String> {
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
        .then_some(state.session_id)
}

fn write_session(store: &SessionStore, session_id: &str) -> std::io::Result<()> {
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
        let prompt = match (&resumed, situation) {
            // A fresh session gets the full standing brief plus what changed.
            (None, Some(situation)) => format!("{}\n\n{}", request.system, situation),
            (None, None) => request.system,
            // A resumed agent remembers its role; it only needs what changed.
            (Some(_), Some(situation)) => situation,
            (Some(_), None) => request.system,
        };

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
        let result = self
            .run_episode(
                &prompt,
                resumed.as_deref(),
                &sink,
                &mut episode,
                claude_config.as_deref(),
                codex_home.as_deref(),
            )
            .await;
        if let Some(path) = &claude_config {
            let _ = std::fs::remove_file(path);
        }
        if let Some(dir) = &codex_home {
            let _ = std::fs::remove_dir_all(dir);
        }
        result?;

        // Remember the session for the next turn. A resumed session reports
        // its id again on init; either way the freshest id wins.
        if let Some(id) = episode.session_id {
            *self.session.lock().unwrap() = Some(id);
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
        let mut cmd = Command::new(&self.bin);
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
        let stderr_task = tokio::spawn(async move {
            let mut buffer = Vec::new();
            let _ = tokio::io::copy(&mut stderr, &mut buffer).await;
            buffer
        });

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
                    if let (Some(store), Some(session_id)) = (
                        provider.session_store.lock().unwrap().clone(),
                        episode.session_id.as_deref(),
                    ) {
                        write_session(&store, session_id).map_err(|error| {
                            ProviderError::Process {
                                provider: provider.cli.label().to_string(),
                                detail: format!("could not persist native session state: {error}"),
                            }
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
                Ok(())
            }
            Err(_elapsed) => {
                // Kill and reap so the child cannot linger.
                let _ = child.start_kill();
                let _ = child.wait().await;
                Err(ProviderError::Process {
                    provider: self.cli.label().to_string(),
                    detail: format!("timed out after {}s", self.timeout.as_secs()),
                })
            }
        }
    }
}

/// Read one JSONL line of agent output into the episode.
fn ingest_line(
    cli: CliKind,
    line: &str,
    _sink: &Option<mpsc::Sender<StreamEvent>>,
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
        CliKind::Codex => ingest_codex_line(&value, episode),
        CliKind::Claude | CliKind::Grok => ingest_messages_line(&value, episode),
    }
}

/// Codex --json events: thread.started gives the session id, item.completed
/// carries finished messages, and turn.completed reports usage.
fn ingest_codex_line(value: &Value, episode: &mut Episode) {
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
        "item.completed" => {
            let Some(item) = value.get("item") else {
                return;
            };
            let Some(item_type) = item.get("type").and_then(Value::as_str) else {
                return;
            };
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
fn ingest_messages_line(value: &Value, episode: &mut Episode) {
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
    if value.get("type").and_then(Value::as_str) != Some("assistant") {
        return;
    }
    let Some(message) = value.get("message") else {
        return;
    };
    let Some(blocks) = message.get("content").and_then(Value::as_array) else {
        return;
    };
    for block in blocks {
        if block.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(text) = block.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    episode.text.push_str(text);
                    episode.text.push('\n');
                }
            }
        }
    }
    if let Some(usage) = message.get("usage") {
        episode.usage = usage_from_json(usage);
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
    Usage {
        input_tokens: num("input_tokens"),
        output_tokens: num("output_tokens"),
        cache_read_tokens: num("cached_input_tokens").max(num("cache_read_input_tokens")),
        cache_write_tokens: num("cache_write_input_tokens").max(num("cache_creation_input_tokens")),
    }
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
    if let Some(path) = std::env::var_os("ORGBOT_MCP_RELAY_BIN") {
        return Ok(std::path::PathBuf::from(path));
    }
    std::env::current_exe().map_err(|error| {
        ProviderError::Unsupported(format!("cannot locate own binary for MCP relay: {error}"))
    })
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
        config.push_str("\n[mcp_servers.orgbot_room]\n");
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
    config.push_str("\n[mcp_servers.orgbot_room]\n");
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
            "orgbot-room": {
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
            cmd.arg("--mcp-config").arg(path);
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
        (CliKind::Codex, Some(session)) => {
            // `codex exec resume` does not take a sandbox flag (the resumed
            // session keeps the one it started with), and its trailing prompt
            // means options must precede the positionals.
            cmd.arg("exec")
                .arg("resume")
                .arg("--json")
                .arg("--skip-git-repo-check")
                .arg("-c")
                .arg("approval_policy=\"never\"")
                .arg(session)
                .arg(prompt);
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
}

/// Matches every tool the room's MCP server exposes.
///
/// Both Claude Code and Grok namespace MCP tools as `mcp__<server>__<tool>`
/// in their permission rules; naming the server allows all of its tools.
const ROOM_TOOL_PATTERN: &str = "mcp__orgbot_room";

/// The same set, spelled as a wildcard for CLIs that want one.
const ROOM_TOOL_WILDCARD: &str = "mcp__orgbot_room__*";

#[cfg(test)]
mod tests {
    use super::*;

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

    fn fold(cli: CliKind, run: &str) -> Episode {
        let mut episode = Episode::default();
        for line in run.lines() {
            ingest_line(cli, line, &None, &mut episode);
        }
        episode
    }

    #[test]
    fn codex_output_yields_text_session_and_usage() {
        let episode = fold(CliKind::Codex, CODEX_RUN);
        assert_eq!(episode.text.trim(), "probe-ok");
        assert_eq!(
            episode.session_id.as_deref(),
            Some("01a067db-ea53-7973-b9ae-4b63f2af5026")
        );
        assert_eq!(episode.usage.input_tokens, 24416);
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

    #[test]
    fn structured_cli_failures_are_parsed_without_losing_usage() {
        let claude = fold(CliKind::Claude, CLAUDE_QUOTA_FAILURE);
        assert!(
            claude
                .structured_error
                .as_deref()
                .unwrap()
                .contains("weekly limit")
        );
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

        let directory = std::env::temp_dir().join(format!(
            "orgbot-cli-error-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
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
            last_socket: Mutex::new(None),
            mcp: false,
            timeout: Duration::from_secs(5),
            session_store: Mutex::new(None),
        };
        let mut episode = Episode::default();
        let error = provider
            .run_episode("test prompt", None, &None, &mut episode, None, None)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ProviderError::RateLimited {
                retry_after_secs: None,
                ..
            }
        ), "expected the structured quota failure to win over stderr, got: {error:?}");
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
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["-c", "approval_policy=\"never\""])
            );
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
                "test",
                Some("prior-session"),
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
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["-c", "approval_policy=\"never\""])
            );
            assert!(!args.iter().any(|arg| arg == "-s"));
            assert!(!args.iter().any(|arg| arg == "--approve-for-me"));
            assert!(!args.iter().any(|arg| arg == "--full-auto"));
            let override_index = args.iter().position(|arg| arg == "-c").unwrap();
            let session_index = args.iter().position(|arg| arg == "prior-session").unwrap();
            let prompt_index = args.iter().position(|arg| arg == "test").unwrap();
            assert!(override_index < session_index && session_index < prompt_index);
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
        write_session(&store, "session-id").unwrap();
        assert_eq!(read_session(&store).as_deref(), Some("session-id"));
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
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
        assert!(
            std::fs::symlink_metadata(overlay.join("auth.json"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
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
            last_socket: Mutex::new(None),
            mcp: store.mcp,
            timeout: Duration::from_millis(50),
            session_store: Mutex::new(Some(store.clone())),
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
                assert!(
                    args.windows(2)
                        .any(|pair| pair == ["--allow", ROOM_TOOL_PATTERN])
                );
                assert!(
                    args.windows(2)
                        .any(|pair| pair == ["--allow", ROOM_TOOL_WILDCARD])
                );
            }
        }
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
            None,
            true,
        );
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--permission-mode", "dontAsk"])
        );
        assert!(
            args.windows(3).any(|triple| {
                triple == ["--allowedTools", ROOM_TOOL_PATTERN, ROOM_TOOL_WILDCARD]
            })
        );
    }
}
