//! The room as an MCP server, for agent CLI bots.
//!
//! A CLI agent (claude -p, codex exec, grok -p) runs its own tool loop; to
//! let it also act on the room's board mid-episode, this module serves the
//! room's tools over the Model Context Protocol. The protocol part is pure:
//! [McpSession] parses JSON-RPC messages and answers them through a
//! dispatcher closure, so it is fully unit-testable without sockets or
//! processes. The runtime wires a session to the engine while a turn is in
//! flight (see orgbot-runtime::bot), and a transport (unix socket / stdio
//! relay) carries the bytes to the CLI.
//!
//! Only the subset of MCP that agents actually use is implemented:
//! initialize, ping, tools/list, tools/call, and notifications/initialized.
//! Batch frames and resource discovery are not offered.

use orgbot_providers::ToolDef;
use serde_json::{json, Value};
use tokio::sync::mpsc;

/// The MCP protocol version this server speaks.
pub const PROTOCOL_VERSION: &str = "2025-03-26";

/// An inbound MCP request delivered while an agent episode runs.
pub struct McpCall {
    /// The raw JSON-RPC request line.
    pub request: String,
    /// Where to send the JSON-RPC response (None for notifications).
    pub reply: tokio::sync::oneshot::Sender<Option<String>>,
}

impl std::fmt::Debug for McpCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpCall").finish_non_exhaustive()
    }
}

/// The tools a given bot may see through the room MCP server.
pub fn room_tools_for(spec: &orgbot_proto::AgentSpec) -> Vec<ToolDef> {
    orgbot_tools::definitions_for(spec)
}

/// One MCP session: the state of a single client connection (one agent
/// episode). Fresh per episode, because every agent process re-initialises.
#[derive(Debug)]
pub struct McpSession {
    initialized: bool,
    /// The tools offered to this client, fixed at session start.
    tools: Vec<ToolDef>,
}

impl McpSession {
    /// Start a session that offers `tools`.
    pub fn new(tools: Vec<ToolDef>) -> Self {
        Self {
            initialized: false,
            tools,
        }
    }

    /// Answer one JSON-RPC message, dispatching tool calls through `call`.
    ///
    /// `call` receives the tool name and parsed arguments and returns the
    /// outcome the tool produced; the engine does the work. Returns the raw
    /// response line to send back, or None when the message was a
    /// notification and expects no reply.
    pub fn handle(
        &mut self,
        message: &str,
        mut call: impl FnMut(&str, &Value) -> orgbot_tools::ToolOutcome,
    ) -> Option<String> {
        let parsed: Value = match serde_json::from_str(message) {
            Ok(value) => value,
            Err(_) => {
                return Some(response_error(None, -32700, "parse error"));
            }
        };
        let id = parsed.get("id").cloned();
        let method = parsed.get("method").and_then(Value::as_str);

        let Some(method) = method else {
            return Some(response_error(id, -32600, "invalid request"));
        };

        match method {
            "initialize" => {
                self.initialized = true;
                Some(response_ok(
                    id,
                    json!({
                        "protocolVersion": PROTOCOL_VERSION,
                        "capabilities": { "tools": { "listChanged": false } },
                        "serverInfo": {
                            "name": "orgbot-room",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                    }),
                ))
            }
            "notifications/initialized" | "notifications/cancelled" => None,
            "ping" => Some(response_ok(id, json!({}))),
            "tools/list" if self.initialized => {
                let tools: Vec<Value> = self
                    .tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "name": tool.name,
                            "description": tool.description,
                            "inputSchema": tool.input_schema,
                        })
                    })
                    .collect();
                Some(response_ok(id, json!({ "tools": tools })))
            }
            "tools/call" if self.initialized => {
                let name = parsed
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let args = parsed
                    .get("params")
                    .and_then(|p| p.get("arguments"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let outcome = call(name, &args);
                let mut result = json!({
                    "content": [{ "type": "text", "text": outcome.content }],
                });
                if outcome.is_error {
                    result["isError"] = json!(true);
                }
                Some(response_ok(id, result))
            }
            "tools/list" | "tools/call" => {
                Some(response_error(id, -32002, "server not initialized"))
            }
            _ => Some(response_error(
                id,
                -32601,
                format!("method not found: {method}"),
            )),
        }
    }
}

/// Serve one relay connection: forward every newline-delimited request to the
/// room's in-turn dispatcher and write its reply back. The dispatcher lives in
/// the bot's turn loop, so a request that arrives between turns simply waits.
pub async fn serve_stream(stream: tokio::net::UnixStream, tx: mpsc::Sender<McpCall>) {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                let id = request_id(&trimmed);
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                if tx
                    .send(McpCall {
                        request: trimmed,
                        reply: reply_tx,
                    })
                    .await
                    .is_err()
                {
                    // The turn loop has gone. Answer rather than vanish: a
                    // client left waiting on a request that never returns
                    // decides the server died, which is what
                    // "the orgbot-room server dropped" was.
                    let _ = write_line(&mut reader, &response_error(id, -32603, BETWEEN_TURNS)).await;
                    break;
                }
                let reply = match reply_rx.await {
                    Ok(Some(reply)) => Some(reply),
                    Ok(None) => None, // a notification; nothing to answer
                    // The turn ended while this call was in flight and the
                    // sender was dropped. Same reasoning as above.
                    Err(_) => Some(response_error(id, -32603, BETWEEN_TURNS)),
                };
                if let Some(reply) = reply {
                    if write_line(&mut reader, &reply).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}

/// Told to a client whose call arrived outside a bot's turn.
///
/// The room only dispatches tool calls while the bot that owns them is taking
/// a turn. A call outside that window cannot be served, but it must still be
/// answered — an unanswered JSON-RPC request looks to the client like the
/// server has died, and it tears the session down and reconnects.
const BETWEEN_TURNS: &str =
    "the room is between turns; retry this call during your turn";

/// Write one newline-delimited JSON-RPC message back to the client.
async fn write_line(
    reader: &mut tokio::io::BufReader<tokio::net::UnixStream>,
    line: &str,
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    reader.get_mut().write_all(format!("{line}\n").as_bytes()).await
}

/// The `id` of a request, so a failure can be attributed to it.
fn request_id(request: &str) -> Option<Value> {
    serde_json::from_str::<Value>(request)
        .ok()
        .and_then(|value| value.get("id").cloned())
}

fn response_ok(id: Option<Value>, result: Value) -> String {
    let mut body = serde_json::Map::new();
    body.insert("jsonrpc".into(), json!("2.0"));
    if let Some(id) = id {
        body.insert("id".into(), id);
    } else {
        body.insert("id".into(), Value::Null);
    }
    body.insert("result".into(), result);
    serde_json::to_string(&Value::Object(body)).expect("response serialises")
}

fn response_error(id: Option<Value>, code: i64, message: impl Into<String>) -> String {
    let mut body = serde_json::Map::new();
    body.insert("jsonrpc".into(), json!("2.0"));
    if let Some(id) = id {
        body.insert("id".into(), id);
    } else {
        body.insert("id".into(), Value::Null);
    }
    body.insert(
        "error".into(),
        json!({ "code": code, "message": message.into() }),
    );
    serde_json::to_string(&Value::Object(body)).expect("error serialises")
}

#[cfg(test)]
mod tests {
    use super::*;
    use orgbot_providers::types::ToolDef;

    fn ping_tool() -> ToolDef {
        ToolDef {
            name: "room_ping".into(),
            description: "Check the room is there.".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
        }
    }

    fn session() -> McpSession {
        McpSession::new(vec![ping_tool()])
    }

    fn dispatch_ok(name: &str, _args: &Value) -> orgbot_tools::ToolOutcome {
        orgbot_tools::ToolOutcome::ok(format!("ran {name}"))
    }

    #[test]
    fn initialize_names_the_server_and_negotiates() {
        let mut s = session();
        let reply = s
            .handle(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
                dispatch_ok,
            )
            .expect("initialize replies");
        let parsed: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(parsed["result"]["serverInfo"]["name"], "orgbot-room");
        assert_eq!(parsed["result"]["protocolVersion"], "2025-03-26",);
        assert_eq!(parsed["id"], 1);
    }

    #[test]
    fn tool_calls_are_dispatched_and_their_text_returned() {
        let mut s = session();
        s.handle(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            dispatch_ok,
        );
        let reply = s
            .handle(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"room_ping","arguments":{}}}"#,
                dispatch_ok,
            )
            .expect("tool call replies");
        let parsed: Value = serde_json::from_str(&reply).unwrap();
        assert!(parsed["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("ran room_ping"));
        assert!(parsed["result"].get("isError").is_none());
    }

    #[test]
    fn tools_list_exposes_the_room_tools_with_schemas() {
        let mut s = session();
        s.handle(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            dispatch_ok,
        );
        let reply = s
            .handle(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
                dispatch_ok,
            )
            .expect("tools/list replies");
        let parsed: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(parsed["result"]["tools"][0]["name"], "room_ping");
        assert_eq!(
            parsed["result"]["tools"][0]["inputSchema"]["type"],
            "object",
        );
    }

    #[test]
    fn failing_tools_are_marked_is_error_but_still_return_text() {
        let mut s = session();
        s.handle(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            dispatch_ok,
        );
        let failing = |_name: &str, _args: &Value| orgbot_tools::ToolOutcome::error("not allowed");
        let reply = s
            .handle(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"room_ping"}}"#,
                failing,
            )
            .expect("tool call replies");
        let parsed: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(parsed["result"]["isError"], true);
    }

    #[test]
    fn tools_are_locked_until_initialize() {
        let mut s = session();
        let reply = s
            .handle(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
                dispatch_ok,
            )
            .expect("replies with an error");
        let parsed: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(parsed["error"]["code"], -32002);
    }

    #[test]
    fn notifications_get_no_reply_and_unknown_methods_are_errors() {
        let mut s = session();
        assert!(s
            .handle(
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                dispatch_ok,
            )
            .is_none());
        let reply = s
            .handle(
                r#"{"jsonrpc":"2.0","id":2,"method":"what/ever"}"#,
                dispatch_ok,
            )
            .expect("replies");
        let parsed: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(parsed["error"]["code"], -32601);
    }

    #[test]
    fn garbage_lines_yield_a_parse_error_not_a_panic() {
        let mut s = session();
        let reply = s.handle("not json at all", dispatch_ok).expect("replies");
        let parsed: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(parsed["error"]["code"], -32700);
    }
}
