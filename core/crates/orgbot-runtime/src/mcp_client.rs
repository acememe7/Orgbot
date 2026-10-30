//! Reaching *out* to MCP servers, so a bot can use tools the room does not own.
//!
//! [`crate::mcp`] is the other direction: the room served as tools to an agent
//! CLI. This is the client. It matters because of an asymmetry that quietly
//! decided what a bot could do — an agent-CLI bot inherits its machine's MCP
//! servers for free, and an API-backed bot inherits nothing. On a phone every
//! bot is API-backed, so a roster of twenty-one had no MCP tools at all while
//! the same roster on a laptop would have had every one the machine offers.
//!
//! Two transports, because the honest answer differs by device:
//!
//! - **stdio** spawns the server as a process. This is how most MCP servers
//!   are written and how they are registered with the CLIs. It works wherever
//!   the room can start a process — a desktop, a server — and not on Android,
//!   which does not let an app run arbitrary binaries.
//! - **http** talks to a server over the network, which works anywhere the
//!   room has one, including a phone.
//!
//! A server that cannot be reached is dropped with a warning rather than
//! failing the room. A room file is shared between machines that do not have
//! the same things installed, and a room that refuses to open because one
//! optional tool server is missing is worse than one that opens without it.
//!
//! Tool names are prefixed with the server they came from — `history__search`
//! — for two reasons: two servers may both offer `search`, and a bot's own
//! board tools must never be shadowed by something a server decided to call
//! `claim_task`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;
use orgbot_engine::Command;
use orgbot_proto::AgentId;
use orgbot_providers::types::ToolDef;
use orgbot_tools::ToolOutcome;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

/// The protocol version this client speaks.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26"];

/// Separates the server's name from the tool's own.
///
/// Two underscores because tool names are conventionally snake_case, and a
/// single one would be ambiguous about where the server name ended.
pub const NAMESPACE: &str = "__";

/// Keep an unreachable optional tool server from holding room startup open for
/// a minute. Multiple configured servers are also attempted concurrently.
const MCP_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// How to reach one MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum McpTransport {
    /// Run the server as a child process and speak JSON-RPC over its pipes.
    Stdio {
        /// The executable.
        command: String,
        /// Its arguments.
        #[serde(default)]
        args: Vec<String>,
        /// Extra environment for the child.
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    /// Run the server on the connected computer, over the workspace.
    ///
    /// The phone cannot start a process; the machine it is already connected
    /// to can, and that machine is usually the one the servers were installed
    /// on in the first place. Each call is one invocation carrying its own
    /// handshake, so a server that keeps no state between calls — which is
    /// what a read-only data server is — works with no persistent channel at
    /// all. One that does keep state will not, and that is worth knowing
    /// before relying on it.
    Workspace {
        /// The executable on that machine.
        command: String,
        /// Its arguments.
        #[serde(default)]
        args: Vec<String>,
        /// Environment for the invocation.
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    /// Post JSON-RPC to an HTTP endpoint.
    Http {
        /// Where the server listens.
        url: String,
        /// Headers to send, for servers that want a token.
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

/// One MCP server a room may use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerSpec {
    /// What its tools are prefixed with.
    pub name: String,
    /// How to reach it.
    #[serde(flatten)]
    pub transport: McpTransport,
}

/// A server that answered, and what it offers.
struct Connected {
    name: String,
    config_hash: [u8; 32],
    tools: Vec<ToolDef>,
    channel: Channel,
}

/// The live end of a connection.
enum Channel {
    /// A child process, held open across calls. Behind a lock because several
    /// bots take turns at once and one pipe cannot interleave two exchanges.
    Stdio(Arc<Mutex<StdioPipe>>),
    Http {
        url: String,
        headers: BTreeMap<String, String>,
        client: reqwest::Client,
        session: Arc<Mutex<HttpSession>>,
    },
    /// A fresh invocation on the connected computer for every exchange.
    Workspace {
        workspace: crate::workspace::Workspace,
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
}

#[derive(Default)]
struct HttpSession {
    session_id: Option<String>,
    protocol_version: Option<String>,
}

struct StdioPipe {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
    next_id: u64,
}

/// Every MCP server a room has, and the tools they offer.
#[derive(Default)]
pub struct McpClient {
    servers: Vec<Connected>,
    approval_state: Option<PathBuf>,
    catalog: Vec<String>,
    lazy: Option<LazyRegistry>,
}

struct LazyRegistry {
    specs: BTreeMap<String, McpServerSpec>,
    workspace: Option<crate::workspace::Workspace>,
    loaded: Mutex<HashMap<AgentId, BTreeMap<String, Arc<Connected>>>>,
    locks: Mutex<HashMap<(AgentId, String), Arc<Mutex<()>>>>,
}

impl LazyRegistry {
    async fn connection_lock(&self, actor: AgentId, server: &str) -> Arc<Mutex<()>> {
        let key = (actor, server.to_string());
        let mut locks = self.locks.lock().await;
        Arc::clone(locks.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))))
    }
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field(
                "servers",
                &self
                    .servers
                    .iter()
                    .filter_map(|s| safe_server_name(&s.name).then_some(s.name.as_str()))
                    .collect::<Vec<_>>(),
            )
            .field("server_count", &self.catalog.len())
            .field("lazy", &self.lazy.is_some())
            .finish()
    }
}

impl McpClient {
    /// Eagerly connect to each server, keeping the ones that answer.
    ///
    /// This legacy constructor is not lifecycle-capable: use [`Self::lazy`]
    /// when tools must be loaded and released per actor.
    pub async fn connect(specs: &[McpServerSpec]) -> Self {
        Self::connect_with(specs, None).await
    }

    /// As [`Self::connect`], with a computer the room can run servers on.
    /// This eager constructor is not lifecycle-capable; use [`Self::lazy`]
    /// for per-actor loading and release.
    pub async fn connect_with(
        specs: &[McpServerSpec],
        workspace: Option<&crate::workspace::Workspace>,
    ) -> Self {
        let workspace = workspace.cloned();
        let catalog = specs
            .iter()
            .filter(|spec| safe_server_name(&spec.name))
            .map(|spec| spec.name.clone())
            .collect::<Vec<_>>();
        let attempts = futures::stream::iter(specs.iter().cloned())
            .map(move |spec| {
                let workspace = workspace.clone();
                async move {
                    let result = tokio::time::timeout(
                        MCP_CONNECT_TIMEOUT,
                        connect_one(&spec, workspace.as_ref()),
                    )
                    .await;
                    (spec, result)
                }
            })
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await;
        let mut servers = Vec::new();
        for (spec, result) in attempts {
            match result {
                Ok(Ok(server)) => {
                    tracing::info!(
                        server = %server.name,
                        tools = server.tools.len(),
                        "mcp server connected"
                    );
                    servers.push(server);
                }
                Ok(Err(_)) | Err(_) => {
                    // Not fatal, deliberately: see the note at the top. Config
                    // values and server errors can carry credentials.
                    tracing::warn!(server = %spec.name, "mcp server unavailable; details withheld");
                }
            }
        }
        Self {
            servers,
            approval_state: None,
            catalog,
            lazy: None,
        }
    }

    /// Keep server configuration without starting any process or opening any
    /// network connection. Each actor connects only after explicitly loading
    /// a server, and owns an independent connection until it releases it.
    pub fn lazy(specs: &[McpServerSpec], workspace: Option<&crate::workspace::Workspace>) -> Self {
        let mut configured = BTreeMap::new();
        for spec in specs.iter().filter(|spec| safe_server_name(&spec.name)) {
            configured
                .entry(spec.name.clone())
                .or_insert_with(|| spec.clone());
        }
        let specs = configured;
        let catalog = specs.keys().cloned().collect();
        Self {
            servers: Vec::new(),
            approval_state: None,
            catalog,
            lazy: Some(LazyRegistry {
                specs,
                workspace: workspace.cloned(),
                loaded: Mutex::new(HashMap::new()),
                locks: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Require a fresh owner approval for every external server call.
    pub fn with_owner_approval(mut self, state_dir: PathBuf) -> Self {
        self.approval_state = Some(state_dir);
        self
    }

    /// Every tool available, named for the model.
    pub fn tools(&self) -> Vec<ToolDef> {
        self.servers
            .iter()
            .flat_map(|server| server.tools.iter().cloned())
            .collect()
    }

    /// Names of configured MCP servers, without returning their commands,
    /// endpoints, environment, headers, or other configuration.
    pub fn server_names(&self) -> Vec<String> {
        self.catalog.clone()
    }

    /// Connect one configured server for one actor and return its namespaced
    /// tools. Repeated loads by the same actor reuse the same connection.
    pub async fn load_for(&self, actor: AgentId, server: &str) -> Result<Vec<ToolDef>, String> {
        let Some(lazy) = &self.lazy else {
            return Err("MCP server loading is unavailable.".into());
        };
        let Some(spec) = lazy.specs.get(server) else {
            return Err("MCP server is unavailable.".into());
        };
        let connection_lock = lazy.connection_lock(actor, server).await;
        let _connection_guard = connection_lock.lock().await;
        let existing = lazy
            .loaded
            .lock()
            .await
            .get(&actor)
            .and_then(|servers| servers.get(server).cloned());
        if let Some(connected) = existing {
            return Ok(connected.tools.clone());
        }
        {
            let connected = tokio::time::timeout(
                MCP_CONNECT_TIMEOUT,
                connect_one(spec, lazy.workspace.as_ref()),
            )
            .await
            .map_err(|_| "MCP server could not be loaded.".to_string())?
            .map_err(|_| "MCP server could not be loaded.".to_string())?;
            lazy.loaded
                .lock()
                .await
                .entry(actor)
                .or_default()
                .insert(server.to_string(), Arc::new(connected));
        }
        Ok(lazy.loaded.lock().await[&actor][server].tools.clone())
    }

    /// Release only this actor's connection to a configured server. A second
    /// actor using the same server remains connected.
    pub async fn release_for(&self, actor: AgentId, server: &str) -> Result<(), String> {
        let Some(lazy) = &self.lazy else {
            return Err("MCP server release is unavailable.".into());
        };
        if !lazy.specs.contains_key(server) {
            return Err("MCP server is unavailable.".into());
        }
        let connection_lock = lazy.connection_lock(actor, server).await;
        let _connection_guard = connection_lock.lock().await;
        let mut loaded = lazy.loaded.lock().await;
        if let Some(actor_servers) = loaded.get_mut(&actor) {
            actor_servers.remove(server);
            if actor_servers.is_empty() {
                loaded.remove(&actor);
            }
        }
        Ok(())
    }

    /// Whether this name belongs to a server rather than to the room.
    pub fn owns(&self, tool: &str) -> bool {
        self.servers
            .iter()
            .any(|server| tool.starts_with(&format!("{}{NAMESPACE}", server.name)))
            || self.lazy.as_ref().is_some_and(|lazy| {
                lazy.specs
                    .keys()
                    .any(|name| tool.starts_with(&format!("{name}{NAMESPACE}")))
            })
    }

    /// Call a namespaced tool and return what the server said.
    pub async fn call(&self, tool: &str, arguments: &Value) -> ToolOutcome {
        let Some(server) = self
            .servers
            .iter()
            .find(|server| tool.starts_with(&format!("{}{NAMESPACE}", server.name)))
        else {
            return ToolOutcome::error(format!("No MCP server offers `{tool}`."));
        };
        call_connected(server, tool, arguments, false).await
    }

    /// Call an external tool only after the room owner approved this exact
    /// agent, tool, arguments, server configuration, and room state identity.
    pub async fn call_for(
        &self,
        engine: &crate::SharedEngine,
        actor: AgentId,
        tool: &str,
        arguments: &Value,
    ) -> ToolOutcome {
        let Some(server) = self
            .servers
            .iter()
            .find(|server| tool.starts_with(&format!("{}{NAMESPACE}", server.name)))
        else {
            return ToolOutcome::error(format!("No MCP server offers `{tool}`."));
        };
        self.call_for_server(engine, actor, server, tool, arguments, false)
            .await
    }

    /// Call only an exact tool loaded by this actor. Lazy connections and
    /// approval decisions are scoped to the selected actor and server.
    pub async fn call_loaded_for(
        &self,
        engine: &crate::SharedEngine,
        actor: AgentId,
        tool: &str,
        arguments: &Value,
    ) -> ToolOutcome {
        let Some(lazy) = &self.lazy else {
            return ToolOutcome::error("No MCP server offers that loaded tool.");
        };
        let server = lazy.loaded.lock().await.get(&actor).and_then(|servers| {
            servers.values().find_map(|connected| {
                connected
                    .tools
                    .iter()
                    .any(|definition| definition.name == tool)
                    .then(|| Arc::clone(connected))
            })
        });
        let Some(server) = server else {
            return ToolOutcome::error("No MCP server offers that loaded tool.");
        };
        // Keep this Arc for the in-flight operation. Release removes the
        // registry's reference immediately; this call may finish on its own.
        self.call_for_server(engine, actor, server.as_ref(), tool, arguments, true)
            .await
    }

    async fn call_for_server(
        &self,
        engine: &crate::SharedEngine,
        actor: AgentId,
        server: &Connected,
        tool: &str,
        arguments: &Value,
        redact_errors: bool,
    ) -> ToolOutcome {
        let Some(state_dir) = self.approval_state.as_deref() else {
            return call_connected(server, tool, arguments, redact_errors).await;
        };
        // An outside tool call can do anything its server can, so the room's
        // access mode rates it risky: Auto and Full write let it through
        // without asking; Approval and Half approval ask for each call.
        let mode = engine.with(|room| room.room().spec.access.mode);
        if mode.approves(orgbot_proto::room::Risk::Risky) {
            return call_connected(server, tool, arguments, redact_errors).await;
        }
        let args = crate::mcp_approval::canonical_json(arguments);
        let state_identity = match crate::mcp_approval::canonical_state_identity(
            state_dir.to_path_buf(),
            engine.clone(),
        )
        .await
        {
            Ok(identity) => identity,
            Err(error) => return ToolOutcome::error(error),
        };
        let config_hash = crate::mcp_approval::hex_digest(&server.config_hash);
        let base = format!(
            "External MCP call: agent={actor}; server={}; tool={tool}; config_sha256={config_hash}; room_state={state_identity}; args={args}",
            server.name,
        );
        if base.chars().count() > 1_900 {
            return ToolOutcome::error("External MCP approval request is too large; call refused.");
        }

        let prior = engine.with(|room| {
            room.decisions()
                .decisions
                .iter()
                .filter(|decision| {
                    decision.owner_question
                        && decision.asked_by.contains(&actor)
                        && decision.because.starts_with(&base)
                })
                .cloned()
                .collect::<Vec<_>>()
        });
        let denied = prior.iter().any(|decision| {
            decision
                .answer
                .as_ref()
                .is_some_and(|answer| answer.chosen != "Allow once")
        });
        if denied {
            return ToolOutcome::error("Owner denied this external MCP call.");
        }
        let latest = prior.last();
        let latest_pending = latest.filter(|decision| decision.is_pending());
        let latest_approved = latest.filter(|decision| {
            decision.is_authoritative()
                && decision
                    .answer
                    .as_ref()
                    .is_some_and(|answer| answer.chosen == "Allow once")
        });
        let reusable = if let Some(decision) = latest_pending {
            Some(decision.clone())
        } else if let Some(decision) = latest_approved {
            match crate::mcp_approval::approval_consumed(state_dir.to_path_buf(), decision.id).await
            {
                Ok(false) => Some(decision.clone()),
                Ok(true) => None,
                Err(error) => return ToolOutcome::error(error),
            }
        } else {
            None
        };
        let question = reusable
            .as_ref()
            .map(|decision| decision.because.clone())
            .unwrap_or_else(|| format!("{base}; approval_sequence={}", prior.len()));
        let matching = reusable.or_else(|| {
            engine.with(|room| {
                room.decisions()
                    .decisions
                    .iter()
                    .find(|decision| {
                        decision.owner_question
                            && decision.asked_by.contains(&actor)
                            && decision.because == question
                    })
                    .cloned()
            })
        });
        if question.chars().count() > 2_000 {
            return ToolOutcome::error(
                "External MCP approval request exceeds the safe size limit.",
            );
        }
        let Some(decision) = matching else {
            let result = engine.with(|room| {
                room.apply(Command::AskOwner {
                    by: actor,
                    task: None,
                    question: question.clone(),
                    options: vec!["Allow once".into(), "Deny".into()],
                })
            });
            return match result {
                Ok(_) => ToolOutcome::error(
                    "Waiting for owner approval. No external tool call was made.",
                ),
                Err(error) => {
                    ToolOutcome::error(format!("Could not request owner approval: {error}"))
                }
            };
        };
        let Some(answer) = decision.answer.as_ref() else {
            return ToolOutcome::error(
                "Waiting for owner approval. No external tool call was made.",
            );
        };
        if answer.chosen == "Deny" {
            return ToolOutcome::error("Owner denied this external MCP call.");
        }
        if answer.chosen != "Allow once" {
            return ToolOutcome::error("Owner response did not grant this external MCP call.");
        }
        match crate::mcp_approval::consume_approval(state_dir.to_path_buf(), decision.id).await {
            Ok(()) => call_connected(server, tool, arguments, redact_errors).await,
            Err(error) => ToolOutcome::error(error),
        }
    }

    /// How many servers answered.
    pub fn len(&self) -> usize {
        self.servers.len()
    }

    /// Whether nothing is connected.
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }
}

/// Turn an MCP tool result into the text a model reads.
fn render(result: &Value) -> String {
    let Some(content) = result.get("content").and_then(Value::as_array) else {
        return result.to_string();
    };
    let text: Vec<String> = content
        .iter()
        .map(|block| match block.get("text").and_then(Value::as_str) {
            Some(text) => text.to_string(),
            None => block.to_string(),
        })
        .collect();
    if text.is_empty() {
        result.to_string()
    } else {
        text.join("\n")
    }
}

fn safe_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.contains(NAMESPACE)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

async fn call_connected(
    server: &Connected,
    tool: &str,
    arguments: &Value,
    redact_errors: bool,
) -> ToolOutcome {
    let prefix = format!("{}{NAMESPACE}", server.name);
    let Some(bare) = tool.strip_prefix(&prefix) else {
        return ToolOutcome::error("No MCP server offers that tool.");
    };
    let request = json!({
        "method": "tools/call",
        "params": { "name": bare, "arguments": arguments },
    });
    match server.channel.request(request).await {
        Ok(result) => ToolOutcome {
            content: render(&result),
            // A server reports a tool-level failure in the result, not as a
            // protocol error, and the bot needs to know the difference.
            is_error: result
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        Err(error) if redact_errors => {
            let _ = error;
            ToolOutcome::error("The loaded MCP tool did not answer; its details were withheld.")
        }
        Err(error) => ToolOutcome::error(format!(
            "`{tool}` did not answer: {error}. The server may have stopped."
        )),
    }
}

async fn connect_one(
    spec: &McpServerSpec,
    workspace: Option<&crate::workspace::Workspace>,
) -> Result<Connected, String> {
    let channel = match &spec.transport {
        McpTransport::Workspace { command, args, env } => {
            let workspace = workspace
                .ok_or("this room has no connected computer to run it on")?
                .clone();
            Channel::Workspace {
                workspace,
                command: command.clone(),
                args: args.clone(),
                env: env.clone(),
            }
        }
        McpTransport::Stdio { command, args, env } => {
            let mut process = tokio::process::Command::new(command);
            process.args(args);
            strip_inherited_credentials(&mut process, std::env::vars_os().map(|(name, _)| name));
            let mut child = process
                .envs(env)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| format!("could not start `{command}`: {e}"))?;
            let stdin = child.stdin.take().ok_or("no stdin")?;
            let stdout = child.stdout.take().ok_or("no stdout")?;
            Channel::Stdio(Arc::new(Mutex::new(StdioPipe {
                child,
                stdin,
                stdout: BufReader::new(stdout),
                next_id: 0,
            })))
        }
        McpTransport::Http { url, headers } => Channel::Http {
            url: url.clone(),
            headers: headers.clone(),
            client: http_client()?,
            session: Arc::new(Mutex::new(HttpSession::default())),
        },
    };

    channel
        .request(json!({
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "orgbot", "version": env!("CARGO_PKG_VERSION") },
            },
        }))
        .await?;
    channel
        .notify(json!({ "method": "notifications/initialized" }))
        .await;

    let listed = channel.request(json!({ "method": "tools/list" })).await?;
    let tools = listed
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| {
                    let name = tool.get("name")?.as_str()?;
                    Some(ToolDef {
                        name: format!("{}{NAMESPACE}{name}", spec.name),
                        description: tool
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        input_schema: tool
                            .get("inputSchema")
                            .cloned()
                            .unwrap_or_else(|| json!({"type": "object"})),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Connected {
        name: spec.name.clone(),
        config_hash: {
            use sha2::{Digest, Sha256};
            Sha256::digest(serde_json::to_vec(spec).map_err(|error| error.to_string())?).into()
        },
        tools,
        channel,
    })
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        // Managed MCP endpoints can carry owner-provided credentials and
        // session IDs. A redirect may send those headers to another host.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| error.to_string())
}

fn strip_inherited_credentials(
    command: &mut tokio::process::Command,
    names: impl IntoIterator<Item = std::ffi::OsString>,
) {
    for name in names {
        let normalized = name.to_string_lossy().to_ascii_uppercase();
        if normalized.starts_with("ORGBOT_")
            || normalized.contains("_API_KEY")
            || normalized.contains("_TOKEN")
            || normalized.contains("_SECRET")
            || normalized.contains("_PASSWORD")
            || normalized.contains("_CREDENTIAL")
            || normalized.contains("_AUTHORIZATION")
        {
            command.env_remove(name);
        }
    }
}

impl Channel {
    /// Send a request and wait for its result.
    async fn request(&self, mut body: Value) -> Result<Value, String> {
        match self {
            Self::Stdio(pipe) => {
                let mut pipe = pipe.lock().await;
                pipe.next_id += 1;
                let id = pipe.next_id;
                body["jsonrpc"] = json!("2.0");
                body["id"] = json!(id);
                let line = format!("{body}\n");
                pipe.stdin
                    .write_all(line.as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
                pipe.stdin.flush().await.map_err(|e| e.to_string())?;

                // Read until the response to *this* id arrives; a server may
                // interleave notifications of its own.
                for _ in 0..64 {
                    let mut answer = String::new();
                    let read = tokio::time::timeout(
                        std::time::Duration::from_secs(60),
                        pipe.stdout.read_line(&mut answer),
                    )
                    .await
                    .map_err(|_| "timed out".to_string())?
                    .map_err(|e| e.to_string())?;
                    if read == 0 {
                        return Err("the server closed its output".into());
                    }
                    let Ok(value) = serde_json::from_str::<Value>(answer.trim()) else {
                        continue;
                    };
                    if value.get("id").and_then(Value::as_u64) != Some(id) {
                        continue;
                    }
                    if let Some(error) = value.get("error") {
                        return Err(error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown error")
                            .to_string());
                    }
                    return Ok(value.get("result").cloned().unwrap_or(Value::Null));
                }
                Err("the server never answered".into())
            }
            Self::Workspace {
                workspace,
                command,
                args,
                env,
            } => {
                // One invocation carrying its own handshake. A fresh process
                // has not been initialized, so the exchange has to include it;
                // the id below is what distinguishes the answer we want from
                // the handshake's.
                body["jsonrpc"] = json!("2.0");
                body["id"] = json!(2);
                let script = pipeline(command, args, env, &body)?;
                let outcome = workspace
                    .call(
                        orgbot_proto::AgentId::new(),
                        "workspace_run",
                        &json!({ "command": script, "timeout_seconds": 120 }),
                    )
                    .await;
                if outcome.is_error {
                    return Err(outcome.content);
                }
                for line in outcome.content.lines() {
                    let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
                        continue;
                    };
                    if value.get("id").and_then(Value::as_u64) != Some(2) {
                        continue;
                    }
                    if let Some(error) = value.get("error") {
                        return Err(error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown error")
                            .to_string());
                    }
                    return Ok(value.get("result").cloned().unwrap_or(Value::Null));
                }
                Err(format!(
                    "no reply from `{command}` on the connected computer: {}",
                    outcome.content.chars().take(200).collect::<String>()
                ))
            }
            Self::Http {
                url,
                headers,
                client,
                session,
            } => {
                let is_initialize = body["method"] == "initialize";
                body["jsonrpc"] = json!("2.0");
                body["id"] = json!(1);
                let mut request = client
                    .post(url)
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream");
                for (key, value) in headers {
                    request = request.header(key, value);
                }
                {
                    let session = session.lock().await;
                    if let Some(session_id) = &session.session_id {
                        request = request.header("mcp-session-id", session_id);
                    }
                    if let Some(protocol_version) = &session.protocol_version {
                        request = request.header("mcp-protocol-version", protocol_version);
                    }
                }
                let response = request
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                {
                    let mut current = session.lock().await;
                    if let Some(session_id) = response
                        .headers()
                        .get("mcp-session-id")
                        .and_then(|value| value.to_str().ok())
                    {
                        current.session_id = Some(session_id.to_owned());
                    }
                }
                let text = response.text().await.map_err(|e| e.to_string())?;
                let value = parse_http_body(&text)?;
                if let Some(error) = value.get("error") {
                    return Err(error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error")
                        .to_string());
                }
                let result = value.get("result").cloned().unwrap_or(Value::Null);
                if is_initialize {
                    let negotiated = result
                        .get("protocolVersion")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            "MCP initialize result omitted its protocol version".to_string()
                        })?;
                    if !SUPPORTED_PROTOCOL_VERSIONS.contains(&negotiated) {
                        return Err(format!(
                            "MCP server negotiated unsupported protocol version `{negotiated}`"
                        ));
                    }
                    session.lock().await.protocol_version = Some(negotiated.to_owned());
                }
                Ok(result)
            }
        }
    }

    /// Send something that takes no answer.
    async fn notify(&self, mut body: Value) {
        body["jsonrpc"] = json!("2.0");
        match self {
            Self::Stdio(pipe) => {
                let mut pipe = pipe.lock().await;
                let line = format!("{body}\n");
                let _ = pipe.stdin.write_all(line.as_bytes()).await;
                let _ = pipe.stdin.flush().await;
            }
            // A fresh process per exchange has nothing to notify.
            Self::Workspace { .. } => {}
            Self::Http {
                url,
                headers,
                client,
                session,
            } => {
                let mut request = client.post(url).header("content-type", "application/json");
                for (key, value) in headers {
                    request = request.header(key, value);
                }
                {
                    let session = session.lock().await;
                    if let Some(session_id) = &session.session_id {
                        request = request.header("mcp-session-id", session_id);
                    }
                    if let Some(protocol_version) = &session.protocol_version {
                        request = request.header("mcp-protocol-version", protocol_version);
                    }
                }
                let _ = request.json(&body).send().await;
            }
        }
    }
}

/// Every MCP server the connected computer already has registered.
///
/// The point of this is that there is nothing to configure. The machine a room
/// is connected to is usually the machine the servers were installed on, and
/// it already keeps a list of them — the same list the CLI on that machine
/// reads. So a room inherits what that computer offers, which is exactly what
/// an agent-CLI bot running there would have had, and what an API-backed bot
/// has been missing.
///
/// Only `stdio` entries are taken. An `http` server in that file is reachable
/// by the room directly and does not need the computer in the middle.
pub fn servers_in_codex_config(config: &str) -> Vec<McpServerSpec> {
    let Ok(parsed) = config.parse::<toml::Table>() else {
        return Vec::new();
    };
    let Some(servers) = parsed.get("mcp_servers").and_then(toml::Value::as_table) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (name, entry) in servers {
        let Some(entry) = entry.as_table() else {
            continue;
        };
        // A URL here is reachable without a machine in the middle.
        if entry.contains_key("url") {
            continue;
        }
        let Some(command) = entry.get("command").and_then(toml::Value::as_str) else {
            continue;
        };
        let args = entry
            .get("args")
            .and_then(toml::Value::as_array)
            .map(|args| {
                args.iter()
                    .filter_map(|a| a.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let env = entry
            .get("env")
            .and_then(toml::Value::as_table)
            .map(|env| {
                env.iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        found.push(McpServerSpec {
            name: name.clone(),
            transport: McpTransport::Workspace {
                command: command.to_string(),
                args,
                env,
            },
        });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// Read the connected computer's own MCP list.
///
/// Returns nothing if there is no such file, which is the ordinary case for a
/// machine that has never had a CLI installed on it.
pub async fn discover_on_workspace(workspace: &crate::workspace::Workspace) -> Vec<McpServerSpec> {
    let outcome = workspace
        .call(
            orgbot_proto::AgentId::new(),
            "workspace_run",
            &serde_json::json!({
                "command": "cat ~/.codex/config.toml 2>/dev/null || true",
                "timeout_seconds": 30
            }),
        )
        .await;
    if outcome.is_error {
        return Vec::new();
    }
    servers_in_codex_config(&outcome.content)
}

/// The shell line that runs one MCP exchange on the connected computer.
///
/// The handshake and the request go in together on stdin, because the process
/// only lives for this exchange. A quoted heredoc carries the JSON verbatim —
/// no shell expansion, so quotes and `$` in the payload are safe — and the
/// delimiter is distinctive enough that content cannot close it by accident,
/// which is checked rather than assumed.
fn pipeline(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    request: &Value,
) -> Result<String, String> {
    const EOF: &str = "ORGBOT_MCP_EOF";

    let handshake = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "orgbot", "version": env!("CARGO_PKG_VERSION") },
        },
    });
    let initialized = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    let payload = format!("{handshake}\n{initialized}\n{request}\n");
    if payload.contains(EOF) {
        return Err("the request cannot be sent safely".into());
    }

    let mut line = String::new();
    for (key, value) in env {
        line.push_str(&format!("{key}={} ", shell_quote(value)));
    }
    line.push_str(&shell_quote(command));
    for arg in args {
        line.push(' ');
        line.push_str(&shell_quote(arg));
    }
    Ok(format!("{line} <<'{EOF}'\n{payload}{EOF}\n"))
}

/// Wrap a word so the shell takes it literally.
fn shell_quote(word: &str) -> String {
    // A single-quoted word is literal to the shell except for a quote itself,
    // which has to leave the quoting, be escaped, and go back in.
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// An HTTP MCP server may answer with plain JSON or with one SSE event.
fn parse_http_body(text: &str) -> Result<Value, String> {
    if let Ok(value) = serde_json::from_str::<Value>(text.trim()) {
        return Ok(value);
    }
    for line in text.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            if let Ok(value) = serde_json::from_str::<Value>(data.trim()) {
                return Ok(value);
            }
        }
    }
    Err(format!(
        "could not read the reply: {}",
        text.chars().take(160).collect::<String>()
    ))
}

impl Drop for StdioPipe {
    fn drop(&mut self) {
        // A room that closes should not leave tool servers running.
        let _ = self.child.start_kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orgbot_proto::DecisionId;

    fn approval_engine(agents: Vec<AgentId>) -> crate::SharedEngine {
        use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
        let engine = crate::SharedEngine::new(orgbot_engine::Engine::new(RoomSpec {
            name: "approval-test".into(),
            goal: "test exact owner approval".into(),
            roster: vec![],
            completion: CompletionRule::AllTasksTerminal,
            limits: RoomLimits::default(),
            access: Default::default(),
        }));
        for (index, id) in agents.into_iter().enumerate() {
            let mut spec = orgbot_proto::AgentSpec::new(
                format!("agent-{index}"),
                format!("Agent {index}"),
                orgbot_proto::ProviderRef::Scripted {
                    script: "test".into(),
                },
            );
            spec.id = id;
            engine.with(|room| {
                room.apply(orgbot_engine::Command::JoinAgent {
                    spec: Box::new(spec),
                })
                .unwrap()
            });
        }
        engine.with(|room| room.apply(orgbot_engine::Command::StartRoom).unwrap());
        engine
    }

    fn gated_test_client() -> McpClient {
        let path = std::env::temp_dir().join(format!(
            "orgbot-approval-test-{}-{}",
            std::process::id(),
            DecisionId::new()
        ));
        std::fs::create_dir(&path).unwrap();
        McpClient {
            servers: vec![Connected {
                name: "remote".into(),
                config_hash: [7; 32],
                tools: vec![],
                channel: Channel::Http {
                    url: "http://127.0.0.1:1/should-not-be-called".into(),
                    headers: BTreeMap::new(),
                    client: reqwest::Client::new(),
                    session: Arc::new(Mutex::new(HttpSession::default())),
                },
            }],
            approval_state: None,
            catalog: vec!["remote".into()],
            lazy: None,
        }
        .with_owner_approval(path)
    }

    #[cfg(unix)]
    fn lazy_fixture_with_delay(
        root: &std::path::Path,
        name: &str,
        startup_delay: f64,
    ) -> (McpClient, PathBuf) {
        let marker = root.join(format!("mcp-events-{name}"));
        let script = root.join(format!("fake-mcp-{name}.py"));
        std::fs::write(
            &script,
            format!(
                "import json,sys,time\nopen({:?},'a').write('start\\n')\ntime.sleep({})\nfor line in sys.stdin:\n m=json.loads(line)\n method=m.get('method')\n if method=='initialize': r={{'protocolVersion':'2025-06-18','serverInfo':{{'name':'fixture','version':'1'}}}}\n elif method=='tools/list': r={{'tools':[{{'name':'lookup','description':'Synthetic lookup','inputSchema':{{'type':'object','properties':{{'query':{{'type':'string'}}}},'required':['query']}}}}]}}\n elif method=='tools/call':\n  open({:?},'a').write('call\\n')\n  r={{'content':[{{'type':'text','text':'fixture result'}}]}}\n else: r={{}}\n if 'id' in m: print(json.dumps({{'jsonrpc':'2.0','id':m['id'],'result':r}}),flush=True)\n",
                marker.to_string_lossy(),
                startup_delay,
                marker.to_string_lossy()
            ),
        )
        .unwrap();
        let client = McpClient::lazy(
            &[McpServerSpec {
                name: name.into(),
                transport: McpTransport::Stdio {
                    command: "python3".into(),
                    args: vec![script.to_string_lossy().into_owned()],
                    env: BTreeMap::new(),
                },
            }],
            None,
        );
        (client, marker)
    }

    #[cfg(unix)]
    fn lazy_fixture(root: &std::path::Path, name: &str) -> (McpClient, PathBuf) {
        lazy_fixture_with_delay(root, name, 0.0)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn lazy_mcp_load_release_and_calls_are_isolated_per_actor() {
        use orgbot_engine::Command;
        use orgbot_proto::decision::Answer;

        let root = std::env::temp_dir().join(format!(
            "orgbot-lazy-mcp-test-{}-{}",
            std::process::id(),
            DecisionId::new()
        ));
        std::fs::create_dir(&root).unwrap();
        let first = AgentId::new();
        let second = AgentId::new();
        let (client, marker) = lazy_fixture(&root, "fixture");
        let server_names = client.server_names();
        assert_eq!(server_names, vec!["fixture"]);
        assert!(client.tools().is_empty());
        assert!(
            !marker.exists(),
            "lazy construction must not start a process"
        );
        assert!(client.owns("fixture__lookup"));

        let first_tools = client.load_for(first, "fixture").await.unwrap();
        assert_eq!(first_tools[0].name, "fixture__lookup");
        assert!(marker.exists());
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().lines().count(),
            1,
            "repeated load by one actor reuses its process"
        );
        assert_eq!(
            client.load_for(first, "fixture").await.unwrap(),
            first_tools
        );
        assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 1);

        let second_tools = client.load_for(second, "fixture").await.unwrap();
        assert_eq!(second_tools, first_tools);
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().lines().count(),
            2,
            "each actor owns an independent server connection"
        );

        let engine = approval_engine(vec![first, second]);
        let client = client.with_owner_approval(root.clone());

        let unknown = client
            .call_loaded_for(&engine, first, "fixture__not-listed", &json!({}))
            .await;
        assert!(unknown.is_error);
        assert!(unknown.content.contains("No MCP server offers"));
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().lines().count(),
            2,
            "unknown tools are rejected before the external server call"
        );

        let args = json!({"query":"approved"});
        let pending = client
            .call_loaded_for(&engine, second, "fixture__lookup", &args)
            .await;
        assert!(pending.content.contains("Waiting for owner approval"));
        assert_eq!(engine.borrow().decisions().pending().count(), 1);
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().lines().count(),
            2,
            "no tools/call is forwarded before owner approval"
        );
        let decision = engine.borrow().decisions().pending().next().unwrap().id;
        engine.with(|room| {
            room.apply(Command::AnswerDecision {
                decision,
                answer: Answer {
                    chosen: "Allow once".into(),
                    note: String::new(),
                    at: orgbot_proto::Timestamp::now(),
                },
            })
            .unwrap()
        });
        let called = client
            .call_loaded_for(&engine, second, "fixture__lookup", &args)
            .await;
        assert!(!called.is_error, "{}", called.content);
        assert!(called.content.contains("fixture result"));

        client.release_for(first, "fixture").await.unwrap();
        let released = client
            .call_loaded_for(&engine, first, "fixture__lookup", &args)
            .await;
        assert!(released.is_error, "released tools must no longer route");
        let reloaded = client.load_for(first, "fixture").await.unwrap();
        assert_eq!(reloaded, first_tools);
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().lines().count(),
            4,
            "loading after release starts a fresh process"
        );
        let second_still_loaded = client.load_for(second, "fixture").await.unwrap();
        assert_eq!(second_still_loaded, second_tools);
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().lines().count(),
            4,
            "releasing another actor must not stop or recreate this connection"
        );
        let other_actor_still_loaded = client
            .call_loaded_for(&engine, second, "fixture__lookup", &args)
            .await;
        assert!(other_actor_still_loaded
            .content
            .contains("Waiting for owner approval"));
        client.release_for(second, "fixture").await.unwrap();
        client.release_for(first, "fixture").await.unwrap();
        assert!(client.release_for(first, "not-configured").await.is_err());

        assert!(!format!("{client:?}").contains("fake-mcp.py"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn slow_lazy_connection_does_not_block_another_server_or_actor() {
        let root = std::env::temp_dir().join(format!(
            "orgbot-lazy-mcp-parallel-{}-{}",
            std::process::id(),
            DecisionId::new()
        ));
        std::fs::create_dir(&root).unwrap();
        let (slow_client, slow_marker) = lazy_fixture_with_delay(&root, "slow", 0.5);
        let (fast_client, fast_marker) = lazy_fixture(&root, "fast");
        let specs = slow_client
            .lazy
            .as_ref()
            .unwrap()
            .specs
            .values()
            .chain(fast_client.lazy.as_ref().unwrap().specs.values())
            .cloned()
            .collect::<Vec<_>>();
        let client = Arc::new(McpClient::lazy(&specs, None));
        let slow_actor = AgentId::new();
        let fast_actor = AgentId::new();
        let slow = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.load_for(slow_actor, "slow").await })
        };
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !slow_marker.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("slow connection should have started");
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(250),
            client.load_for(fast_actor, "fast")
        )
        .await
        .expect("unrelated load should make progress while another connects")
        .is_ok());
        assert!(fast_marker.exists());
        assert!(slow.await.unwrap().is_ok());
        client.release_for(slow_actor, "slow").await.unwrap();
        client.release_for(fast_actor, "fast").await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn lazy_catalog_rejects_unsafe_names_without_exposing_configuration() {
        let secretish_command = "/private/path/with-token-do-not-print";
        let client = McpClient::lazy(
            &[
                McpServerSpec {
                    name: "safe-server".into(),
                    transport: McpTransport::Stdio {
                        command: secretish_command.into(),
                        args: vec![],
                        env: BTreeMap::from([("TOKEN".into(), "private-value".into())]),
                    },
                },
                McpServerSpec {
                    name: "unsafe/name".into(),
                    transport: McpTransport::Stdio {
                        command: "missing".into(),
                        args: vec![],
                        env: BTreeMap::new(),
                    },
                },
                McpServerSpec {
                    name: "unavailable".into(),
                    transport: McpTransport::Stdio {
                        command: "missing-binary-for-lazy-catalog".into(),
                        args: vec![],
                        env: BTreeMap::new(),
                    },
                },
            ],
            None,
        );
        assert_eq!(client.server_names(), vec!["safe-server", "unavailable"]);
        let debug = format!("{client:?}");
        assert!(!debug.contains(secretish_command));
        assert!(!debug.contains("private-value"));
        let error = client
            .load_for(AgentId::new(), "unsafe/name")
            .await
            .unwrap_err();
        assert!(!error.contains(secretish_command));
        assert!(!error.contains("private-value"));
        let unavailable = client
            .load_for(AgentId::new(), "unavailable")
            .await
            .unwrap_err();
        assert_eq!(unavailable, "MCP server could not be loaded.");
        assert!(!format!("{client:?}").contains("missing-binary-for-lazy-catalog"));
    }

    #[tokio::test]
    async fn external_call_waits_for_exact_agent_and_arguments_without_forwarding() {
        let first = AgentId::new();
        let second = AgentId::new();
        let engine = approval_engine(vec![first, second]);
        let mut client = gated_test_client();
        let args = json!({"query":"private"});
        let pending = client
            .call_for(&engine, first, "remote__lookup", &args)
            .await;
        assert!(
            pending.content.contains("Waiting for owner approval"),
            "{}",
            pending.content
        );
        assert_eq!(engine.borrow().decisions().pending().count(), 1);
        let asked = engine
            .borrow()
            .decisions()
            .pending()
            .next()
            .unwrap()
            .clone();
        assert!(asked.because.contains(&format!("agent={first}")));
        assert!(asked.because.contains("args={\"query\":\"private\"}"));
        assert!(asked.because.contains("config_sha256=07070707"));

        let repeated = client
            .call_for(&engine, first, "remote__lookup", &args)
            .await;
        assert!(repeated.content.contains("Waiting for owner approval"));
        assert_eq!(engine.borrow().decisions().pending().count(), 1);

        let changed = client
            .call_for(
                &engine,
                first,
                "remote__lookup",
                &json!({"query":"different"}),
            )
            .await;
        assert!(changed.content.contains("Waiting for owner approval"));
        let denied_id = engine.borrow().decisions().pending().nth(1).unwrap().id;
        engine.with(|room| {
            room.apply(orgbot_engine::Command::AnswerDecision {
                decision: denied_id,
                answer: orgbot_proto::decision::Answer {
                    chosen: "Deny".into(),
                    note: String::new(),
                    at: orgbot_proto::Timestamp::now(),
                },
            })
            .unwrap()
        });
        let denied = client
            .call_for(
                &engine,
                first,
                "remote__lookup",
                &json!({"query":"different"}),
            )
            .await;
        assert!(denied.content.contains("Owner denied"));
        let other_agent = client
            .call_for(&engine, second, "remote__lookup", &args)
            .await;
        assert!(other_agent.content.contains("Waiting for owner approval"));
        assert_eq!(engine.borrow().decisions().pending().count(), 2);
        client.servers[0].config_hash = [8; 32];
        let new_config = client
            .call_for(&engine, first, "remote__lookup", &args)
            .await;
        assert!(new_config.content.contains("Waiting for owner approval"));
        let restored_room = approval_engine(vec![first]);
        let new_room = client
            .call_for(&restored_room, first, "remote__lookup", &args)
            .await;
        assert!(new_room.content.contains("Waiting for owner approval"));
        let state = client.approval_state.as_deref().unwrap();
        let _ = std::fs::remove_dir_all(state);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn external_call_is_forwarded_only_once_after_allow_once_and_restart_needs_new_approval()
    {
        use orgbot_engine::Command;
        use orgbot_proto::decision::Answer;
        use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};

        let root = std::env::temp_dir().join(format!(
            "orgbot-approval-forward-test-{}-{}",
            std::process::id(),
            DecisionId::new()
        ));
        std::fs::create_dir(&root).unwrap();
        let script = root.join("fake-mcp.py");
        let marker = root.join("called");
        std::fs::write(
            &script,
            format!(
                "import json,sys\nfor line in sys.stdin:\n m=json.loads(line)\n method=m.get('method')\n if method == 'initialize': r={{'protocolVersion':'2025-06-18','capabilities':{{}},'serverInfo':{{'name':'fake','version':'1'}}}}\n elif method == 'tools/list': r={{'tools':[{{'name':'lookup','inputSchema':{{'type':'object'}}}}]}}\n elif method == 'tools/call':\n  open({:?},'a').write('called\\n')\n  r={{'content':[{{'type':'text','text':'ok'}}]}}\n else: r={{}}\n if 'id' in m: print(json.dumps({{'jsonrpc':'2.0','id':m['id'],'result':r}}),flush=True)\n",
                marker.to_string_lossy()
            ),
        )
        .unwrap();
        let actor = AgentId::new();
        let engine = crate::SharedEngine::new(orgbot_engine::Engine::new(RoomSpec {
            name: "approval-forward".into(),
            goal: "test explicit approval".into(),
            roster: vec![],
            completion: CompletionRule::AllTasksTerminal,
            limits: RoomLimits::default(),
            access: Default::default(),
        }));
        let mut agent = orgbot_proto::AgentSpec::new(
            "approval-agent",
            "Approval Agent",
            orgbot_proto::ProviderRef::Scripted {
                script: "test".into(),
            },
        );
        agent.id = actor;
        engine.with(|room| {
            room.apply(Command::JoinAgent {
                spec: Box::new(agent),
            })
            .unwrap()
        });
        engine.with(|room| room.apply(Command::StartRoom).unwrap());
        let spec = McpServerSpec {
            name: "fake".into(),
            transport: McpTransport::Stdio {
                command: "python3".into(),
                args: vec![script.to_string_lossy().into_owned()],
                env: BTreeMap::new(),
            },
        };
        let client = McpClient::connect(&[spec.clone()])
            .await
            .with_owner_approval(root.clone());
        assert_eq!(client.len(), 1, "the local fixture server should connect");
        let args = json!({"query":"approved"});
        let pending = client.call_for(&engine, actor, "fake__lookup", &args).await;
        assert!(
            pending.content.contains("Waiting for owner approval"),
            "{}",
            pending.content
        );
        assert!(
            !marker.exists(),
            "no call may cross the boundary before approval"
        );
        let decision = engine.borrow().decisions().pending().next().unwrap().id;
        engine.with(|room| {
            room.apply(Command::AnswerDecision {
                decision,
                answer: Answer {
                    chosen: "Allow once".into(),
                    note: String::new(),
                    at: orgbot_proto::Timestamp::now(),
                },
            })
            .unwrap()
        });
        let allowed = client.call_for(&engine, actor, "fake__lookup", &args).await;
        assert!(!allowed.is_error, "{}", allowed.content);
        assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 1);

        let replay = client.call_for(&engine, actor, "fake__lookup", &args).await;
        assert!(replay.content.contains("Waiting for owner approval"));
        assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 1);

        let restarted = McpClient::connect(&[spec])
            .await
            .with_owner_approval(root.clone());
        let after_restart = restarted
            .call_for(&engine, actor, "fake__lookup", &args)
            .await;
        assert!(after_restart.content.contains("Waiting for owner approval"));
        assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn auto_and_full_write_rooms_let_outside_calls_through_and_half_approval_asks() {
        use orgbot_engine::Command;
        use orgbot_proto::room::{AccessMode, CompletionRule, RoomAccess, RoomLimits, RoomSpec};

        for (mode, asks) in [
            (AccessMode::Approval, true),
            (AccessMode::HalfApproval, true),
            (AccessMode::Auto, false),
            (AccessMode::FullWrite, false),
        ] {
            let root = std::env::temp_dir().join(format!(
                "orgbot-approval-mode-test-{}-{}",
                std::process::id(),
                DecisionId::new()
            ));
            std::fs::create_dir(&root).unwrap();
            let script = root.join("fake-mcp.py");
            let marker = root.join("called");
            std::fs::write(
                &script,
                format!(
                    "import json,sys\nfor line in sys.stdin:\n m=json.loads(line)\n method=m.get('method')\n if method == 'initialize': r={{'protocolVersion':'2025-06-18','capabilities':{{}},'serverInfo':{{'name':'fake','version':'1'}}}}\n elif method == 'tools/list': r={{'tools':[{{'name':'lookup','inputSchema':{{'type':'object'}}}}]}}\n elif method == 'tools/call':\n  open({:?},'a').write('called\\n')\n  r={{'content':[{{'type':'text','text':'ok'}}]}}\n else: r={{}}\n if 'id' in m: print(json.dumps({{'jsonrpc':'2.0','id':m['id'],'result':r}}),flush=True)\n",
                    marker.to_string_lossy()
                ),
            )
            .unwrap();
            let actor = AgentId::new();
            let engine = crate::SharedEngine::new(orgbot_engine::Engine::new(RoomSpec {
                name: "approval-mode".into(),
                goal: "test the room's access mode".into(),
                roster: vec![],
                completion: CompletionRule::AllTasksTerminal,
                limits: RoomLimits::default(),
                access: RoomAccess { mode, lend: vec![] },
            }));
            let mut agent = orgbot_proto::AgentSpec::new(
                "mode-agent",
                "Mode Agent",
                orgbot_proto::ProviderRef::Scripted {
                    script: "test".into(),
                },
            );
            agent.id = actor;
            engine.with(|room| room.apply(Command::JoinAgent { spec: Box::new(agent) }).unwrap());
            engine.with(|room| room.apply(Command::StartRoom).unwrap());
            let spec = McpServerSpec {
                name: "fake".into(),
                transport: McpTransport::Stdio {
                    command: "python3".into(),
                    args: vec![script.to_string_lossy().into_owned()],
                    env: BTreeMap::new(),
                },
            };
            let client = McpClient::connect(&[spec]).await.with_owner_approval(root.clone());
            let outcome = client
                .call_for(&engine, actor, "fake__lookup", &json!({"query":"mode"}))
                .await;
            assert_eq!(
                outcome.content.contains("Waiting for owner approval"),
                asks,
                "{mode:?}: {}",
                outcome.content
            );
            assert_eq!(marker.exists(), !asks, "{mode:?}");
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn consumed_approval_is_durable_once_and_rejects_symlink_ledger() {
        let root = std::env::temp_dir().join(format!(
            "orgbot-approval-ledger-test-{}-{}",
            std::process::id(),
            DecisionId::new()
        ));
        std::fs::create_dir(&root).unwrap();
        let decision = DecisionId::new();
        crate::mcp_approval::test_consume_approval_sync(&root, decision).unwrap();
        assert!(crate::mcp_approval::test_consume_approval_sync(&root, decision).is_err());
        let file = root.join("mcp-approvals").join(decision.to_string());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let outside = root.join("outside");
        std::fs::create_dir(&outside).unwrap();
        let other = root.join("symlink-root");
        std::fs::create_dir(&other).unwrap();
        std::os::unix::fs::symlink(&outside, other.join("mcp-approvals")).unwrap();
        assert!(
            crate::mcp_approval::test_consume_approval_sync(&other, DecisionId::new()).is_err()
        );
        assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_tool_is_named_for_the_server_it_came_from() {
        let spec: McpServerSpec = serde_json::from_value(json!({
            "name": "history",
            "transport": "stdio",
            "command": "true"
        }))
        .unwrap();
        assert_eq!(spec.name, "history");
        assert!(matches!(spec.transport, McpTransport::Stdio { .. }));
    }

    #[test]
    fn an_http_server_is_configured_by_url() {
        let spec: McpServerSpec = serde_json::from_value(json!({
            "name": "docs",
            "transport": "http",
            "url": "https://example.invalid/mcp"
        }))
        .unwrap();
        assert!(matches!(spec.transport, McpTransport::Http { .. }));
    }

    #[test]
    fn stdio_mcp_process_does_not_inherit_orgbot_or_service_credentials() {
        let mut command = tokio::process::Command::new("true");
        command
            .env("PATH", "/usr/bin")
            .env("ORGBOT_MCP_0_TOKEN", "room-only-secret")
            .env("OPENAI_API_KEY", "unrelated-service-secret")
            .env("GITHUB_TOKEN", "unrelated-service-token")
            .env("LANG", "en_US.UTF-8");
        strip_inherited_credentials(
            &mut command,
            [
                "PATH",
                "ORGBOT_MCP_0_TOKEN",
                "OPENAI_API_KEY",
                "GITHUB_TOKEN",
                "LANG",
            ]
            .map(std::ffi::OsString::from),
        );
        command.env("TOKEN", "explicitly-configured-mcp-token");
        let environment: BTreeMap<_, _> = command
            .as_std()
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(environment.get("ORGBOT_MCP_0_TOKEN"), Some(&None));
        assert_eq!(environment.get("OPENAI_API_KEY"), Some(&None));
        assert_eq!(environment.get("GITHUB_TOKEN"), Some(&None));
        assert_eq!(
            environment.get("TOKEN"),
            Some(&Some("explicitly-configured-mcp-token".into()))
        );
        assert!(environment.contains_key("PATH"));
        assert!(environment.contains_key("LANG"));
    }

    /// A server that is not there must not take the room with it.
    #[tokio::test]
    async fn a_server_that_will_not_start_is_skipped_not_fatal() {
        let client = McpClient::connect(&[McpServerSpec {
            name: "missing".into(),
            transport: McpTransport::Stdio {
                command: "definitely-not-a-real-binary-xyz".into(),
                args: vec![],
                env: BTreeMap::new(),
            },
        }])
        .await;
        assert!(client.is_empty());
        assert!(client.tools().is_empty());
    }

    #[tokio::test]
    async fn an_imported_http_mcp_completes_handshake_and_tool_call() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

        const NEGOTIATED_PROTOCOL_VERSION: &str = "2025-03-26";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..4 {
                let (stream, _) = listener.accept().await.unwrap();
                let mut reader = BufReader::new(stream);
                let mut request_line = String::new();
                reader.read_line(&mut request_line).await.unwrap();
                let mut headers = BTreeMap::new();
                let mut content_length = 0usize;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).await.unwrap();
                    if header == "\r\n" || header.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':') {
                        match name.trim().to_ascii_lowercase().as_str() {
                            "content-length" => content_length = value.trim().parse().unwrap(),
                            _ => {
                                headers.insert(
                                    name.trim().to_ascii_lowercase(),
                                    value.trim().to_owned(),
                                );
                            }
                        }
                    }
                }
                assert_eq!(
                    headers.get("authorization").map(String::as_str),
                    Some("Bearer synthetic-mcp-room-credential")
                );
                let mut body = vec![0; content_length];
                reader.read_exact(&mut body).await.unwrap();
                let message: Value = serde_json::from_slice(&body).unwrap();
                let method = message["method"].as_str().unwrap();
                if method == "initialize" {
                    assert!(!headers.contains_key("mcp-session-id"));
                } else {
                    assert_eq!(
                        headers.get("mcp-session-id").map(String::as_str),
                        Some("synthetic-session")
                    );
                    assert_eq!(
                        headers.get("mcp-protocol-version").map(String::as_str),
                        Some(NEGOTIATED_PROTOCOL_VERSION)
                    );
                }
                if method == "notifications/initialized" {
                    assert!(message.get("id").is_none());
                    let response =
                        "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    reader
                        .get_mut()
                        .write_all(response.as_bytes())
                        .await
                        .unwrap();
                    continue;
                }
                let result = match method {
                    "initialize" => {
                        assert_eq!(message["params"]["protocolVersion"], PROTOCOL_VERSION);
                        json!({ "protocolVersion": NEGOTIATED_PROTOCOL_VERSION })
                    }
                    "tools/list" => {
                        json!({"tools": [{"name": "lookup", "description": "mock", "inputSchema": {"type": "object"}}]})
                    }
                    "tools/call" => {
                        json!({"content": [{"type": "text", "text": "mock tool completed"}]})
                    }
                    other => panic!("unexpected MCP method {other}"),
                };
                let body = serde_json::to_vec(&json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": result,
                }))
                .unwrap();
                let session_headers = if method == "initialize" {
                    "Mcp-Session-Id: synthetic-session\r\n"
                } else {
                    ""
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{session_headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                reader.get_mut().write_all(header.as_bytes()).await.unwrap();
                reader.get_mut().write_all(&body).await.unwrap();
                reader.get_mut().flush().await.unwrap();
            }
        });

        let client = McpClient::connect(&[McpServerSpec {
            name: "imported_docs".into(),
            transport: McpTransport::Http {
                url: format!("http://{address}/mcp"),
                headers: BTreeMap::from([(
                    "Authorization".into(),
                    "Bearer synthetic-mcp-room-credential".into(),
                )]),
            },
        }])
        .await;
        assert_eq!(client.len(), 1);
        assert!(client.owns("imported_docs__lookup"));
        assert_eq!(client.tools()[0].name, "imported_docs__lookup");
        let outcome = client
            .call("imported_docs__lookup", &json!({"query": "safe"}))
            .await;
        assert!(!outcome.is_error);
        assert!(outcome.content.contains("mock tool completed"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn managed_http_mcp_never_follows_cross_host_307_or_308() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

        for status in [307, 308] {
            let destination = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let destination_address = destination.local_addr().unwrap();
            let origin = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin_address = origin.local_addr().unwrap();
            let origin_task = tokio::spawn(async move {
                let (stream, _) = origin.accept().await.unwrap();
                let mut reader = BufReader::new(stream);
                let mut request_line = String::new();
                reader.read_line(&mut request_line).await.unwrap();
                let mut headers = BTreeMap::new();
                let mut content_length = 0usize;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).await.unwrap();
                    if header == "\r\n" || header.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':') {
                        match name.trim().to_ascii_lowercase().as_str() {
                            "content-length" => content_length = value.trim().parse().unwrap(),
                            _ => {
                                headers.insert(
                                    name.trim().to_ascii_lowercase(),
                                    value.trim().to_owned(),
                                );
                            }
                        }
                    }
                }
                let mut body = vec![0; content_length];
                reader.read_exact(&mut body).await.unwrap();
                assert_eq!(
                    headers.get("x-api-key").map(String::as_str),
                    Some("owner-secret")
                );
                assert_eq!(
                    headers.get("mcp-session-id").map(String::as_str),
                    Some("origin-session")
                );
                let response = format!(
                    "HTTP/1.1 {status} Redirect\r\nLocation: http://{destination_address}/steal\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                reader
                    .get_mut()
                    .write_all(response.as_bytes())
                    .await
                    .unwrap();
            });

            let channel = Channel::Http {
                url: format!("http://{origin_address}/mcp"),
                headers: BTreeMap::from([("x-api-key".into(), "owner-secret".into())]),
                client: http_client().unwrap(),
                session: Arc::new(Mutex::new(HttpSession {
                    session_id: Some("origin-session".into()),
                    protocol_version: Some(PROTOCOL_VERSION.into()),
                })),
            };
            assert!(channel
                .request(json!({"method": "tools/call", "params": {}}))
                .await
                .is_err());
            origin_task.await.unwrap();
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(150), destination.accept())
                    .await
                    .is_err(),
                "HTTP {status} must not forward credentials or issue a request to another host"
            );
        }
    }

    /// Text content is what a model reads; the envelope is not.
    #[test]
    fn a_result_is_rendered_as_the_text_the_server_returned() {
        let result = json!({"content": [{"type": "text", "text": "three projects"}]});
        assert_eq!(render(&result), "three projects");
    }

    #[test]
    fn an_sse_reply_is_read_as_readily_as_a_plain_one() {
        let plain = parse_http_body(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#).unwrap();
        let sse = parse_http_body(
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n",
        )
        .unwrap();
        assert_eq!(plain, sse);
    }

    /// A room inherits the connected computer's own list, so there is nothing
    /// to configure — which is the whole point of reading it.
    #[test]
    fn a_computers_registered_servers_are_picked_up_as_they_are() {
        let config = r#"
[mcp_servers.aece-history]
command = "python3"
args = ["/home/theaece/scripts/mcp_history_server.py"]

[mcp_servers.node_repl]
command = "/usr/lib/chatgpt/node_repl"
args = []
[mcp_servers.node_repl.env]
NODE_REPL_NODE_PATH = "/usr/lib/chatgpt/bin/node"

[mcp_servers.openaiDeveloperDocs]
url = "https://developers.openai.com/mcp"

[features]
some = true
"#;
        let found = servers_in_codex_config(config);
        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["aece-history", "node_repl"],
            "stdio entries are taken; a URL needs no computer in the middle"
        );

        let history = &found[0];
        match &history.transport {
            McpTransport::Workspace { command, args, .. } => {
                assert_eq!(command, "python3");
                assert_eq!(args.len(), 1);
                assert!(args[0].ends_with("mcp_history_server.py"));
            }
            other => panic!("expected a workspace transport, got {other:?}"),
        }

        match &found[1].transport {
            McpTransport::Workspace { env, .. } => {
                assert_eq!(
                    env.get("NODE_REPL_NODE_PATH").map(String::as_str),
                    Some("/usr/lib/chatgpt/bin/node"),
                    "a server's environment travels with it"
                );
            }
            other => panic!("expected a workspace transport, got {other:?}"),
        }
    }

    /// A machine with no CLI installed is the ordinary case, not an error.
    #[test]
    fn a_computer_with_no_config_offers_nothing_and_does_not_complain() {
        assert!(servers_in_codex_config("").is_empty());
        assert!(servers_in_codex_config("not valid toml {{{").is_empty());
        assert!(servers_in_codex_config("[features]\nx = 1\n").is_empty());
    }

    /// The shell line has to survive the shell, which is easy to get wrong in
    /// a way that only shows up against a real machine.
    #[test]
    fn the_generated_command_quotes_exactly_once() {
        let line = pipeline(
            "python3",
            &["/home/me/mcp server.py".into()],
            &BTreeMap::new(),
            &json!({"method": "tools/list", "id": 2}),
        )
        .unwrap();
        assert!(
            line.starts_with("'python3' '/home/me/mcp server.py' <<'ORGBOT_MCP_EOF'"),
            "a path with a space must be one argument, quoted once: {line}"
        );
        assert!(
            !line.contains(r"'''"),
            "three quotes in a row is the classic quoting bug: {line}"
        );
        assert!(line.trim_end().ends_with("ORGBOT_MCP_EOF"));
    }

    /// The handshake travels with the request, because the process is new.
    #[test]
    fn the_exchange_carries_its_own_handshake() {
        let line = pipeline("srv", &[], &BTreeMap::new(), &json!({"id": 2})).unwrap();
        assert!(line.contains("initialize"), "{line}");
        assert!(line.contains("notifications/initialized"), "{line}");
    }

    /// A word carrying a quote must not break out of its quoting.
    #[test]
    fn a_quote_in_an_argument_cannot_escape() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    /// Environment is set for the invocation, not left to the far side.
    #[test]
    fn environment_is_carried_into_the_invocation() {
        let mut env = BTreeMap::new();
        env.insert("TOKEN".to_string(), "sec ret".to_string());
        let line = pipeline("srv", &[], &env, &json!({"id": 2})).unwrap();
        assert!(line.starts_with("TOKEN='sec ret' 'srv'"), "{line}");
    }

    /// Nothing a server offers may shadow a board tool.
    #[test]
    fn a_servers_tool_cannot_collide_with_the_rooms_own() {
        let client = McpClient {
            servers: vec![Connected {
                name: "history".into(),
                config_hash: [0; 32],
                tools: vec![],
                channel: Channel::Http {
                    url: String::new(),
                    headers: BTreeMap::new(),
                    client: reqwest::Client::new(),
                    session: Arc::new(Mutex::new(HttpSession::default())),
                },
            }],
            approval_state: None,
            catalog: vec!["history".into()],
            lazy: None,
        };
        assert!(client.owns("history__claim_task"));
        assert!(
            !client.owns("claim_task"),
            "a bare board tool must never route to a server"
        );
    }
}
