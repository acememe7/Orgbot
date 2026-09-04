//! A ChatGPT subscription, spoken to directly.
//!
//! A Codex sign-in is not an OpenAI API key. It authenticates against
//! ChatGPT's own backend, which speaks the Responses dialect rather than chat
//! completions, and which expects the Codex CLI's originator headers. That is
//! why this is a separate adapter from [`crate::openai`] rather than a base
//! URL swap: the request shape, the streaming events and the headers all
//! differ.
//!
//! The surface is not a published API and can change without notice. When the
//! sign-in is a plain API key instead, [`crate::openai::OpenAiProvider`] is the
//! stable path and should be preferred.

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;

use crate::auth::{self, Credential};
use crate::error::{ProviderError, Result};
use crate::provider::{emit, Provider};
use crate::sse::SseDecoder;
use crate::types::{
    CompletionRequest, CompletionResponse, ContentBlock, Effort, Role, StopReason, StreamEvent,
    Turn, Usage,
};

/// The default endpoint for a ChatGPT sign-in.
pub const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

/// The Codex CLI version this adapter identifies as. The backend gates on the
/// originator and user agent, so these are part of the wire contract.
const CLIENT_VERSION: &str = "0.144.0";

/// A ChatGPT subscription speaking the Responses dialect.
#[derive(Debug)]
pub struct CodexProvider {
    http: reqwest::Client,
    base_url: String,
    credential: Credential,
    account_id: String,
}

impl CodexProvider {
    /// Build from the environment: `CODEX_AUTH_TOKEN`, else the Codex CLI's
    /// stored sign-in at `~/.codex/auth.json`.
    pub fn from_env() -> Result<Self> {
        let credential = match std::env::var("CODEX_AUTH_TOKEN")
            .ok()
            .filter(|t| !t.trim().is_empty())
        {
            Some(token) => Credential::ChatGptSession {
                account_id: account_from_token(&token).unwrap_or_default(),
                access_token: token,
            },
            None => auth::codex_cli()?,
        };
        Ok(Self::new(
            auth::base_url("codex", DEFAULT_BASE_URL),
            credential,
        ))
    }

    /// Build against an explicit endpoint and credential.
    pub fn new(base_url: impl Into<String>, credential: Credential) -> Self {
        // The CLI does not always record the account id, but the backend wants
        // it; the token carries it in its claims, so fall back to those.
        let account_id = match &credential {
            Credential::ChatGptSession { account_id, .. } if !account_id.is_empty() => {
                account_id.clone()
            }
            other => account_from_token(other.secret()).unwrap_or_default(),
        };
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            credential,
            account_id,
        }
    }

    /// Build the Responses request body. Crate-visible so the mapping is
    /// testable without a network round trip.
    pub(crate) fn build_body(&self, request: &CompletionRequest) -> Value {
        let mut input: Vec<Value> = Vec::new();
        for turn in &request.messages {
            input.extend(turn_to_items(turn));
        }

        let mut body = Map::new();
        body.insert("model".into(), json!(request.model));
        body.insert(
            "instructions".into(),
            json!(if request.system.is_empty() {
                "You are a member of a team working a shared task board."
            } else {
                request.system.as_str()
            }),
        );
        body.insert("input".into(), json!(input));
        body.insert("tool_choice".into(), json!("auto"));
        body.insert("parallel_tool_calls".into(), json!(true));
        // The backend refuses to persist turns for a subscription session, and
        // the room keeps its own history anyway.
        body.insert("store".into(), json!(false));
        body.insert("stream".into(), json!(true));
        body.insert("include".into(), json!([]));

        if !request.tools.is_empty() {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.input_schema,
                        "strict": false,
                    })
                })
                .collect();
            body.insert("tools".into(), json!(tools));
        }

        if let Some(effort) = request.effort {
            body.insert(
                "reasoning".into(),
                json!({ "effort": effort_name(effort), "summary": "auto" }),
            );
        }

        Value::Object(body)
    }
}

/// Map the neutral effort scale onto what the Responses API accepts.
const fn effort_name(effort: Effort) -> &'static str {
    match effort {
        Effort::Low => "low",
        Effort::Medium => "medium",
        // The backend takes only three levels; anything above high saturates.
        Effort::High | Effort::XHigh | Effort::Max => "high",
    }
}

/// Convert one neutral turn into Responses input items.
///
/// Tool results are their own top-level items in this dialect rather than
/// messages with a role, so one turn can yield several items.
fn turn_to_items(turn: &Turn) -> Vec<Value> {
    let mut items = Vec::new();
    let mut text = String::new();
    let mut calls: Vec<Value> = Vec::new();

    for block in &turn.content {
        match block {
            ContentBlock::Text { text: chunk } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(chunk);
            }
            // Reasoning is not replayed; the backend re-derives it, and
            // echoing a summary back as prose confuses the next turn.
            ContentBlock::Thinking { .. } => {}
            ContentBlock::ToolUse { id, name, input } => calls.push(json!({
                "type": "function_call",
                "call_id": id,
                "name": name,
                // Arguments are a JSON string here, not an object.
                "arguments": input.to_string(),
            })),
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => items.push(json!({
                "type": "function_call_output",
                "call_id": tool_use_id,
                "output": if *is_error {
                    format!("ERROR: {content}")
                } else {
                    content.clone()
                },
            })),
        }
    }

    if !text.is_empty() {
        let (role, kind) = match turn.role {
            Role::User => ("user", "input_text"),
            Role::Assistant => ("assistant", "output_text"),
        };
        items.push(json!({
            "type": "message",
            "role": role,
            "content": [{ "type": kind, "text": text }],
        }));
    }
    items.extend(calls);
    items
}

#[async_trait]
impl Provider for CodexProvider {
    fn name(&self) -> &str {
        "codex"
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> Result<CompletionResponse> {
        let body = self.build_body(&request);
        let mut builder = self
            .http
            .post(format!("{}/responses", self.base_url))
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            // The backend gates on these; without them it refuses the session.
            .header("originator", "codex_cli_rs")
            .header("user-agent", format!("codex_cli_rs/{CLIENT_VERSION} (OrgBot)"))
            .header(
                "authorization",
                format!("Bearer {}", self.credential.secret()),
            );
        if !self.account_id.is_empty() {
            builder = builder.header("chatgpt-account-id", self.account_id.as_str());
        }

        let response = builder
            .json(&body)
            .send()
            .await
            .map_err(|source| ProviderError::Transport {
                provider: "codex".into(),
                source,
            })?;
        let response = check_status(response).await?;

        // The endpoint only streams, so a non-streaming caller still reads the
        // event stream and is simply handed the assembled result.
        collect_stream(response, sink).await
    }
}

async fn check_status(response: reqwest::Response) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let body = response.text().await.unwrap_or_default();
    let message = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message").or(Some(e)))
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.chars().take(500).collect());

    Err(match status.as_u16() {
        401 | 403 => ProviderError::Unauthorized {
            provider: "codex".into(),
            status: status.as_u16(),
        },
        429 => ProviderError::RateLimited {
            provider: "codex".into(),
            retry_after_secs: retry_after,
        },
        400 | 404 | 422 => ProviderError::BadRequest {
            provider: "codex".into(),
            message,
        },
        code => ProviderError::Upstream {
            provider: "codex".into(),
            status: code,
            message,
        },
    })
}

/// Read the event stream and assemble the finished turn.
async fn collect_stream(
    response: reqwest::Response,
    sink: Option<mpsc::Sender<StreamEvent>>,
) -> Result<CompletionResponse> {
    let mut decoder = SseDecoder::new();
    let mut state = StreamState::default();
    let mut body = response.bytes_stream();

    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|source| ProviderError::Transport {
            provider: "codex".into(),
            source,
        })?;
        for event in decoder.push(&chunk) {
            if event.data == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(&event.data) else {
                continue;
            };
            state.apply(&value, &sink).await?;
        }
    }

    let response = state.finish();
    emit(
        &sink,
        StreamEvent::Done {
            stop_reason: response.stop_reason.clone(),
        },
    )
    .await;
    Ok(response)
}

/// What has been seen so far on the stream.
#[derive(Default)]
struct StreamState {
    model: String,
    text: String,
    thinking: String,
    calls: Vec<ContentBlock>,
    usage: Usage,
    /// Whether the terminal `response.completed` event arrived. Without it the
    /// stream was cut short and the turn must not be reported as complete.
    completed: bool,
}

impl StreamState {
    async fn apply(&mut self, event: &Value, sink: &Option<mpsc::Sender<StreamEvent>>) -> Result<()> {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or_default();
        match kind {
            "response.output_text.delta" => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    self.text.push_str(delta);
                    emit(sink, StreamEvent::TextDelta { text: delta.into() }).await;
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    self.thinking.push_str(delta);
                    emit(sink, StreamEvent::ThinkingDelta { text: delta.into() }).await;
                }
            }
            "response.output_item.done" => {
                if let Some(item) = event.get("item") {
                    self.take_item(item, sink).await;
                }
            }
            "response.completed" => {
                self.completed = true;
                if let Some(response) = event.get("response") {
                    if let Some(model) = response.get("model").and_then(Value::as_str) {
                        self.model = model.to_string();
                    }
                    self.take_usage(response.get("usage"));
                }
            }
            "error" | "response.failed" | "response.incomplete" => {
                let message = event
                    .pointer("/response/error/message")
                    .or_else(|| event.pointer("/error/message"))
                    .or_else(|| event.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("the ChatGPT backend ended the turn without a result")
                    .to_string();
                return Err(ProviderError::Upstream {
                    provider: "codex".into(),
                    status: 502,
                    message,
                });
            }
            _ => {}
        }
        Ok(())
    }

    /// Record a completed output item. Text arrives as deltas too, so only
    /// tool calls are taken here to avoid doubling the prose.
    async fn take_item(&mut self, item: &Value, sink: &Option<mpsc::Sender<StreamEvent>>) {
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return;
        }
        let id = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if id.is_empty() || name.is_empty() {
            return;
        }
        // Arguments come as a JSON string; a malformed one becomes an empty
        // object so the tool layer reports a argument error rather than the
        // whole turn failing to parse.
        let input = item
            .get("arguments")
            .and_then(Value::as_str)
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .unwrap_or_else(|| json!({}));
        emit(
            sink,
            StreamEvent::ToolUseStart {
                id: id.clone(),
                name: name.clone(),
            },
        )
        .await;
        self.calls.push(ContentBlock::ToolUse { id, name, input });
    }

    fn take_usage(&mut self, usage: Option<&Value>) {
        let Some(usage) = usage else { return };
        let number = |key: &str| {
            usage
                .get(key)
                .and_then(Value::as_u64)
                .unwrap_or_default() as u32
        };
        self.usage = Usage {
            input_tokens: number("input_tokens"),
            output_tokens: number("output_tokens"),
            cache_read_tokens: usage
                .pointer("/input_tokens_details/cached_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default() as u32,
            cache_write_tokens: 0,
        };
    }

    fn finish(self) -> CompletionResponse {
        let mut content = Vec::new();
        if !self.thinking.is_empty() {
            content.push(ContentBlock::Thinking { text: self.thinking });
        }
        if !self.text.is_empty() {
            content.push(ContentBlock::Text { text: self.text });
        }
        let had_calls = !self.calls.is_empty();
        content.extend(self.calls);

        CompletionResponse {
            model: self.model,
            content,
            stop_reason: if had_calls {
                StopReason::ToolUse
            } else if self.completed {
                StopReason::EndTurn
            } else {
                // No terminal event: the connection dropped mid-turn. Say so
                // rather than presenting a truncated turn as finished.
                StopReason::Other("stream ended without response.completed".into())
            },
            usage: self.usage,
        }
    }
}

/// Read the ChatGPT account id out of an access token's claims.
///
/// The claims are read without verifying the signature, which is safe here
/// because this is only ever used to fill in a routing header — the backend
/// still authenticates the token itself.
fn account_from_token(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let claims: Value = serde_json::from_slice(&base64url_decode(payload)?).ok()?;
    claims
        .pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")
        .or_else(|| claims.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Decode unpadded base64url. Written out rather than pulled in as a
/// dependency because a JWT payload is the only thing this crate decodes.
fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let value = |byte: u8| -> Option<u32> {
        Some(match byte {
            b'A'..=b'Z' => u32::from(byte - b'A'),
            b'a'..=b'z' => u32::from(byte - b'a') + 26,
            b'0'..=b'9' => u32::from(byte - b'0') + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        })
    };
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        acc = (acc << 6) | value(byte)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> CodexProvider {
        CodexProvider::new(
            DEFAULT_BASE_URL,
            Credential::ChatGptSession {
                access_token: "tok".into(),
                account_id: "acct_1".into(),
            },
        )
    }

    #[test]
    fn a_user_turn_becomes_an_input_message() {
        let request = CompletionRequest::new(
            "gpt-5.6-sol",
            "be brief",
            vec![Turn::user("hello")],
        );
        let body = provider().build_body(&request);
        assert_eq!(body["instructions"], "be brief");
        assert_eq!(body["input"][0]["type"], "message");
        assert_eq!(body["input"][0]["role"], "user");
        // A user message uses input_text; an assistant one uses output_text.
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["input"][0]["content"][0]["text"], "hello");
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn tool_calls_and_results_use_the_responses_shapes() {
        let assistant = Turn {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "create_task".into(),
                input: json!({"title": "ship it"}),
            }],
        };
        let result = Turn {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: "done".into(),
                is_error: false,
            }],
        };
        let request =
            CompletionRequest::new("gpt-5.6-sol", "", vec![assistant, result]);
        let body = provider().build_body(&request);
        assert_eq!(body["input"][0]["type"], "function_call");
        assert_eq!(body["input"][0]["call_id"], "call_1");
        // Arguments are a JSON string in this dialect, not an object.
        assert_eq!(body["input"][0]["arguments"], r#"{"title":"ship it"}"#);
        assert_eq!(body["input"][1]["type"], "function_call_output");
        assert_eq!(body["input"][1]["output"], "done");
    }

    #[test]
    fn a_failed_tool_result_is_marked_for_the_model() {
        let turn = Turn {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "c".into(),
                content: "no such task".into(),
                is_error: true,
            }],
        };
        let body = provider().build_body(&CompletionRequest::new("m", "", vec![turn]));
        assert_eq!(body["input"][0]["output"], "ERROR: no such task");
    }

    #[test]
    fn tools_are_declared_flat_rather_than_nested() {
        let mut request = CompletionRequest::new("m", "", vec![Turn::user("hi")]);
        request.tools = vec![crate::types::ToolDef {
            name: "claim_task".into(),
            description: "take a task".into(),
            input_schema: json!({"type": "object"}),
        }];
        let body = provider().build_body(&request);
        // Chat completions nests under `function`; Responses does not.
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "claim_task");
    }

    #[tokio::test]
    async fn a_stream_assembles_text_and_tool_calls() {
        let mut state = StreamState::default();
        for event in [
            json!({"type": "response.output_text.delta", "delta": "on "}),
            json!({"type": "response.output_text.delta", "delta": "it"}),
            json!({"type": "response.output_item.done", "item": {
                "type": "function_call", "call_id": "c1",
                "name": "claim_task", "arguments": "{\"id\":\"t1\"}"
            }}),
            json!({"type": "response.completed", "response": {
                "model": "gpt-5.6-sol",
                "usage": {"input_tokens": 10, "output_tokens": 3,
                          "input_tokens_details": {"cached_tokens": 4}}
            }}),
        ] {
            state.apply(&event, &None).await.unwrap();
        }
        let response = state.finish();
        assert_eq!(response.model, "gpt-5.6-sol");
        assert_eq!(response.stop_reason, StopReason::ToolUse);
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.cache_read_tokens, 4);
        assert!(matches!(
            &response.content[0],
            ContentBlock::Text { text } if text == "on it"
        ));
        assert!(matches!(
            &response.content[1],
            ContentBlock::ToolUse { name, .. } if name == "claim_task"
        ));
    }

    /// A cut connection must not look like a finished turn.
    #[tokio::test]
    async fn a_stream_without_a_completion_event_is_not_end_turn() {
        let mut state = StreamState::default();
        state
            .apply(
                &json!({"type": "response.output_text.delta", "delta": "half"}),
                &None,
            )
            .await
            .unwrap();
        assert!(matches!(state.finish().stop_reason, StopReason::Other(_)));
    }

    #[tokio::test]
    async fn an_error_event_fails_the_turn() {
        let mut state = StreamState::default();
        let error = state
            .apply(
                &json!({"type": "response.failed",
                        "response": {"error": {"message": "quota exhausted"}}}),
                &None,
            )
            .await
            .unwrap_err();
        assert!(format!("{error}").contains("quota exhausted"));
    }

    #[test]
    fn the_account_id_is_recovered_from_the_token_claims() {
        // {"https://api.openai.com/auth":{"chatgpt_account_id":"acct_from_jwt"}}
        let claims = "eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdF9mcm9tX2p3dCJ9fQ";
        assert_eq!(
            account_from_token(&format!("header.{claims}.signature")).as_deref(),
            Some("acct_from_jwt")
        );
    }

    #[test]
    fn a_token_that_is_not_a_jwt_yields_no_account() {
        assert_eq!(account_from_token("not-a-jwt"), None);
        assert_eq!(account_from_token("a.!!!.c"), None);
    }

    #[test]
    fn effort_saturates_at_the_highest_level_the_backend_takes() {
        assert_eq!(effort_name(Effort::Max), "high");
        assert_eq!(effort_name(Effort::Low), "low");
    }
}
