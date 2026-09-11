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
        let mut servers = Vec::new();
        for spec in specs {
            match connect_one(spec).await {
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

async fn connect_one(spec: &McpServerSpec) -> Result<Connected, String> {
    let channel = match &spec.transport {
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
