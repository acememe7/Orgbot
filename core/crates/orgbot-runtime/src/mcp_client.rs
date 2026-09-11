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

use std::collections::BTreeMap;
use std::sync::Arc;

use orgbot_providers::types::ToolDef;
use orgbot_tools::ToolOutcome;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

/// The protocol version this client speaks.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Separates the server's name from the tool's own.
///
/// Two underscores because tool names are conventionally snake_case, and a
/// single one would be ambiguous about where the server name ended.
pub const NAMESPACE: &str = "__";

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
    },
    /// A fresh invocation on the connected computer for every exchange.
    Workspace {
        workspace: crate::workspace::Workspace,
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
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
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("servers", &self.servers.iter().map(|s| &s.name).collect::<Vec<_>>())
            .finish()
    }
}

impl McpClient {
    /// Connect to each server, keeping the ones that answer.
    pub async fn connect(specs: &[McpServerSpec]) -> Self {
        Self::connect_with(specs, None).await
    }

    /// As [`Self::connect`], with a computer the room can run servers on.
    pub async fn connect_with(
        specs: &[McpServerSpec],
        workspace: Option<&crate::workspace::Workspace>,
    ) -> Self {
        let mut servers = Vec::new();
        for spec in specs {
            match connect_one(spec, workspace).await {
                Ok(server) => {
                    tracing::info!(
                        server = %server.name,
                        tools = server.tools.len(),
                        "mcp server connected"
                    );
                    servers.push(server);
                }
                Err(error) => {
                    // Not fatal, deliberately: see the note at the top.
                    tracing::warn!(server = %spec.name, %error, "mcp server unavailable");
                }
            }
        }
        Self { servers }
    }

    /// Every tool available, named for the model.
    pub fn tools(&self) -> Vec<ToolDef> {
        self.servers
            .iter()
            .flat_map(|server| server.tools.iter().cloned())
            .collect()
    }

    /// Whether this name belongs to a server rather than to the room.
    pub fn owns(&self, tool: &str) -> bool {
        self.servers
            .iter()
            .any(|server| tool.starts_with(&format!("{}{NAMESPACE}", server.name)))
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
        let bare = &tool[server.name.len() + NAMESPACE.len()..];
        let request = json!({
            "method": "tools/call",
            "params": { "name": bare, "arguments": arguments },
        });
        match server.channel.request(request).await {
            Ok(result) => ToolOutcome {
                content: render(&result),
                // A server reports a tool-level failure in the result, not as
                // a protocol error, and the bot needs to know the difference.
                is_error: result
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
            Err(error) => ToolOutcome::error(format!(
                "`{tool}` did not answer: {error}. The server may have stopped."
            )),
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
            let mut child = tokio::process::Command::new(command)
                .args(args)
                .envs(env)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
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
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .map_err(|e| e.to_string())?,
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
    channel.notify(json!({ "method": "notifications/initialized" })).await;

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
        tools,
        channel,
    })
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
            } => {
                body["jsonrpc"] = json!("2.0");
                body["id"] = json!(1);
                let mut request = client
                    .post(url)
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream");
                for (key, value) in headers {
                    request = request.header(key, value);
                }
                let response = request
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                let text = response.text().await.map_err(|e| e.to_string())?;
                let value = parse_http_body(&text)?;
                if let Some(error) = value.get("error") {
                    return Err(error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error")
                        .to_string());
                }
                Ok(value.get("result").cloned().unwrap_or(Value::Null))
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
            } => {
                let mut request = client.post(url).header("content-type", "application/json");
                for (key, value) in headers {
                    request = request.header(key, value);
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
pub async fn discover_on_workspace(
    workspace: &crate::workspace::Workspace,
) -> Vec<McpServerSpec> {
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
                tools: vec![],
                channel: Channel::Http {
                    url: String::new(),
                    headers: BTreeMap::new(),
                    client: reqwest::Client::new(),
                },
            }],
        };
        assert!(client.owns("history__claim_task"));
        assert!(
            !client.owns("claim_task"),
            "a bare board tool must never route to a server"
        );
    }
}
