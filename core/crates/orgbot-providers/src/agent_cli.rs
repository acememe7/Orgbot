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
//! session and records its id; every later turn resumes that same session, so
//! the agent remembers its earlier work and conversation the way a human agent
//! would. The id lives on the provider instance — one provider per bot per
//! room run — which is exactly the lifetime of a bot's working memory in one
//! room. (Persisting session ids across room runs, so a bot wakes up
//! remembering last week's project, is a desktop-app concern: store the id
//! with the room state and feed it back through the bot's configuration.)
//!
//! Two notes on what this adapter does *not* do yet:
//!
//! - Room tools. The CLI agent speaks its own tool protocol, not the room's
//!   `claim_task` / `handoff_request` vocabulary, so a turn returns prose and
//!   the engine treats it as such. Giving agent bots the room's board as MCP
//!   tools is the next step.
//! - Text is what crosses the boundary. The rich event stream an agent produces
//!   (file edits, shell runs, tool results) is summarised into the agent's final
//!   message. Full action telemetry can stream later over the same pipe.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use orgbot_proto::agent::{AgentPolicy, CliKind};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::timeout;

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
    /// The socket handed in on the last request, for diagnostics.
    last_socket: Mutex<Option<String>>,
    /// Whether the room's board is served to the agent as MCP tools.
    mcp: bool,
    /// Turn ceiling.
    timeout: Duration,
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
        })
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

fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// The sandbox/permission flag this CLI expects for a policy.
fn policy_flags(cli: CliKind, policy: AgentPolicy) -> (&'static str, &'static str) {
    let value = match (cli, policy) {
        (CliKind::Codex, AgentPolicy::ReadOnly) => "read-only",
        (CliKind::Codex, AgentPolicy::WorkspaceWrite) => "workspace-write",
        (CliKind::Codex, AgentPolicy::FullAccess) => "danger-full-access",
        // Not "plan": plan mode blocks *every* tool, including the room
        // board — and the board is not a file operation, it is the bot's job.
        // A read-only bot should still be able to claim, inspect and submit;
        // what it must not do is change the project, and `default` refuses
        // that in a headless session because nothing can approve it. The room
        // tools come through because they are allow-listed explicitly.
        (CliKind::Claude | CliKind::Grok, AgentPolicy::ReadOnly) => "default",
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
                ingest_line(provider.cli, &line, sink, episode);
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
    write_agent_home(socket, ".codex", "codex", |config, relay, socket| {
        config.push_str("\n[mcp_servers.orgbot_room]\n");
        config.push_str(&format!(
            "command = {}\n",
            serde_json::to_string(&relay.to_string_lossy()).unwrap()
        ));
        config.push_str(&format!(
            "args = [\"room-mcp\", \"--socket\", \"{socket}\" ]\n"
        ));
    })
}

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
    use std::os::unix::fs::symlink;

    let relay = relay_binary()?;
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| {
            ProviderError::Unsupported(format!(
                "HOME is not set; cannot build a {label} overlay"
            ))
        })?
        .join(dot_dir);
    if !home.is_dir() {
        return Err(ProviderError::Unsupported(format!(
            "{} is not a directory; {label} is probably not set up",
            home.display()
        )));
    }

    let overlay = std::env::temp_dir().join(format!(
        "orgbot-{label}-{}-{}",
        std::process::id(),
        socket.replace(['/', '.'], "_")
    ));
    let _ = std::fs::remove_dir_all(&overlay);
    std::fs::create_dir_all(&overlay).map_err(|error| {
        ProviderError::Unsupported(format!("cannot create {label} overlay: {error}"))
    })?;

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
    std::fs::write(overlay.join("config.toml"), config).map_err(|error| {
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
    let path = std::env::temp_dir().join(format!("orgbot-mcp-{}.json", std::process::id()));
    let bytes = serde_json::to_vec(&config).map_err(|error| {
        ProviderError::Unsupported(format!("MCP config serialisation failed: {error}"))
    })?;
    std::fs::write(&path, bytes).map_err(|error| {
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
        .map(|turn| turn.text())
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
    let _ = policy_flag;
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
            if mcp {
                // Headless codex refuses MCP tool calls under a plain sandbox;
                // --approve-for-me routes them through automatic review (and
                // implies the workspace-write sandbox, so -s must not be set).
                cmd.arg("--approve-for-me");
            } else {
                cmd.arg("-s").arg(policy_value);
            }
        }
        (CliKind::Codex, Some(session)) => {
            // `codex exec resume` does not take a sandbox flag (the resumed
            // session keeps the one it started with), and its trailing prompt
            // means options must precede the positionals.
            cmd.arg("exec")
                .arg("resume")
                .arg("--json")
                .arg("--skip-git-repo-check")
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

    // Let the room's own tools through the permission gate.
    //
    // Under a normal permission mode an MCP tool call needs approval, and a
    // headless session has nobody to approve it — the agent sees the tools,
    // calls one, and is refused, which shows up in the transcript as "room
    // tooling is permission-blocked". The room board is this bot's job, so it
    // is allowed explicitly rather than by loosening the sandbox generally.
    if mcp {
        match cli {
            CliKind::Claude | CliKind::Grok => {
                // Both the bare server name and an explicit wildcard: the
                // CLIs differ on which form matches a whole server, and an
                // unmatched rule is simply ignored.
                cmd.arg("--allowedTools")
                    .arg(ROOM_TOOL_PATTERN)
                    .arg(ROOM_TOOL_WILDCARD);
            }
            // codex routes MCP approvals through --approve-for-me, set above.
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
        // Deliberately not "plan". Plan mode blocks every tool, including
        // the room board, so a read-only bot could see its tools, be refused
        // on every call, and sit there reporting that it is blocked. Read-only
        // means "do not change the project", not "do nothing".
        assert_eq!(
            policy_flags(CliKind::Claude, AgentPolicy::ReadOnly),
            ("--permission-mode", "default")
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
}
