//! The Anthropic Messages API.
//!
//! Rust has no official Anthropic SDK, so this speaks the REST API directly
//! (`POST /v1/messages`). The request shape follows the current API: adaptive
//! thinking rather than a fixed `budget_tokens`, `effort` inside
//! `output_config`, and server-side refusal fallbacks on the models that
//! support them.

use std::collections::BTreeMap;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;

use crate::auth::{self, Credential};
use crate::error::{ProviderError, Result};
use crate::provider::{emit, Provider};
use crate::sse::SseDecoder;
use crate::types::{
    CompletionRequest, CompletionResponse, ContentBlock, Role, StopReason, StreamEvent, Turn, Usage,
};

/// The API version header value this adapter is written against.
const API_VERSION: &str = "2023-06-01";

/// Beta flag enabling server-side refusal fallbacks in their scalar form.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// Beta flag required when authenticating with an OAuth token.
const OAUTH_BETA: &str = "oauth-2025-04-20";

/// The default public endpoint.
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// A client for the Anthropic Messages API.
#[derive(Debug)]
pub struct AnthropicProvider {
    http: reqwest::Client,
    base_url: String,
    credential: Credential,
}

impl AnthropicProvider {
    /// Build a provider, resolving the credential from the environment.
    pub fn from_env() -> Result<Self> {
        Ok(Self::with_credential(auth::anthropic()?))
    }

    /// Build a provider with an explicit credential.
    pub fn with_credential(credential: Credential) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: auth::base_url("anthropic", DEFAULT_BASE_URL),
            credential,
        }
    }

    /// Override the endpoint. Used by tests and by proxies.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_string();
        self
    }

    /// Build the JSON request body.
    ///
    /// Made public to the crate so the wire shape can be asserted in tests
    /// without a network round trip — the mapping is where the bugs live.
    pub(crate) fn build_body(&self, request: &CompletionRequest) -> Value {
        let capabilities = ModelCapabilities::for_model(&request.model);

        let mut body = Map::new();
        body.insert("model".into(), json!(request.model));
        body.insert("max_tokens".into(), json!(request.max_tokens));
        body.insert("stream".into(), json!(request.stream));

        if !request.system.is_empty() {
            // A single cached system block: it is identical on every turn of a
            // room, so it is the natural cache prefix.
            body.insert(
                "system".into(),
                json!([{
                    "type": "text",
                    "text": request.system,
                    "cache_control": {"type": "ephemeral"},
                }]),
            );
        }

        let messages: Vec<Value> = request
            .messages
            .iter()
            .filter_map(turn_to_wire)
            .collect();
        body.insert("messages".into(), json!(messages));

        if !request.tools.is_empty() {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.input_schema,
                    })
                })
                .collect();
            body.insert("tools".into(), json!(tools));
        }

        if capabilities.adaptive_thinking {
            // `summarized` so the UI can show reasoning as it streams; the
            // default of `omitted` reads as a long pause before any output.
            body.insert(
                "thinking".into(),
                json!({"type": "adaptive", "display": "summarized"}),
            );
        }
        if capabilities.effort {
            if let Some(effort) = request.effort {
                body.insert("output_config".into(), json!({"effort": effort.as_str()}));
            }
        }
        if capabilities.refusal_fallbacks {
            // Route around a policy decline inside the same call rather than
            // failing the bot's turn.
            body.insert("fallbacks".into(), json!("default"));
        }
        Value::Object(body)
    }

    /// The beta flags this request needs.
    fn betas(&self, request: &CompletionRequest) -> Vec<&'static str> {
        let mut betas = Vec::new();
        if ModelCapabilities::for_model(&request.model).refusal_fallbacks {
            betas.push(FALLBACK_BETA);
        }
        if self.credential.is_oauth() {
            betas.push(OAUTH_BETA);
        }
        betas
    }

    fn request_builder(&self, request: &CompletionRequest) -> reqwest::RequestBuilder {
        let mut builder = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .header("content-type", "application/json")
            .header("anthropic-version", API_VERSION);

        // An OAuth token goes on `Authorization`, never on `x-api-key`.
        builder = if self.credential.is_oauth() {
            builder.header(
                "authorization",
                format!("Bearer {}", self.credential.secret()),
            )
        } else {
            builder.header("x-api-key", self.credential.secret())
        };

        let betas = self.betas(request);
        if !betas.is_empty() {
            builder = builder.header("anthropic-beta", betas.join(","));
        }
        builder
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> Result<CompletionResponse> {
        let body = self.build_body(&request);
        let response = self
            .request_builder(&request)
            .json(&body)
            .send()
            .await
            .map_err(|source| ProviderError::Transport {
                provider: "anthropic".into(),
                source,
            })?;

        let response = check_status(response).await?;

        if request.stream {
            stream_response(response, sink).await
        } else {
            let value: Value = response.json().await.map_err(|e| ProviderError::Decode {
                provider: "anthropic".into(),
                detail: e.to_string(),
            })?;
            parse_message(&value)
        }
    }
}

/// Map an HTTP failure onto a typed error, preserving retry information.
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
    let message = extract_error_message(&body);

    Err(match status.as_u16() {
        401 | 403 => ProviderError::Unauthorized {
            provider: "anthropic".into(),
            status: status.as_u16(),
        },
        429 => ProviderError::RateLimited {
            provider: "anthropic".into(),
            retry_after_secs: retry_after,
        },
        400 | 404 | 422 => ProviderError::BadRequest {
            provider: "anthropic".into(),
            message,
        },
        code => ProviderError::Upstream {
            provider: "anthropic".into(),
            status: code,
            message,
        },
    })
}

/// Pull `error.message` out of an error body, falling back to the raw text.
fn extract_error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.chars().take(500).collect())
}

/// Convert one turn to the wire format, or drop it if it carries nothing
/// sendable.
fn turn_to_wire(turn: &Turn) -> Option<Value> {
    let content: Vec<Value> = turn.content.iter().filter_map(block_to_wire).collect();
    if content.is_empty() {
        // The API rejects an empty content array, and a turn that held only a
        // thinking block legitimately becomes empty here.
        return None;
    }
    Some(json!({
        "role": match turn.role { Role::User => "user", Role::Assistant => "assistant" },
        "content": content,
    }))
}

/// Convert one block to the wire format.
///
/// Thinking blocks are deliberately **not** replayed. The API requires them to
/// come back byte-identical with their signature intact, and this crate
/// normalises across three backends rather than preserving that; sending a
/// reconstructed thinking block is a 400. Dropping them is safe — the model
/// re-derives its reasoning each turn.
fn block_to_wire(block: &ContentBlock) -> Option<Value> {
    match block {
        ContentBlock::Text { text } if text.is_empty() => None,
        ContentBlock::Text { text } => Some(json!({"type": "text", "text": text})),
        ContentBlock::Thinking { .. } => None,
        ContentBlock::ToolUse { id, name, input } => Some(json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": input,
        })),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => Some(json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
            "is_error": is_error,
        })),
    }
}

/// Parse a complete (non-streamed) Message response.
pub(crate) fn parse_message(value: &Value) -> Result<CompletionResponse> {
    let model = value
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let content = value
        .get("content")
        .and_then(|v| v.as_array())
        .map(|blocks| blocks.iter().filter_map(block_from_wire).collect())
        .unwrap_or_default();

    let stop_reason = value
        .get("stop_reason")
        .and_then(|v| v.as_str())
        .map(parse_stop_reason)
        .unwrap_or(StopReason::EndTurn);

    Ok(CompletionResponse {
        model,
        content,
        stop_reason,
        usage: parse_usage(value.get("usage")),
    })
}

fn block_from_wire(block: &Value) -> Option<ContentBlock> {
    match block.get("type").and_then(|v| v.as_str())? {
        "text" => Some(ContentBlock::Text {
            text: block.get("text")?.as_str()?.to_string(),
        }),
        "thinking" => Some(ContentBlock::Thinking {
            text: block
                .get("thinking")
                .or_else(|| block.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        }),
        "tool_use" => Some(ContentBlock::ToolUse {
            id: block.get("id")?.as_str()?.to_string(),
            name: block.get("name")?.as_str()?.to_string(),
            input: block.get("input").cloned().unwrap_or_else(|| json!({})),
        }),
        _ => None,
    }
}

fn parse_stop_reason(raw: &str) -> StopReason {
    match raw {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "refusal" => StopReason::Refusal,
        "stop_sequence" => StopReason::StopSequence,
        other => StopReason::Other(other.to_string()),
    }
}

fn parse_usage(usage: Option<&Value>) -> Usage {
    let Some(usage) = usage else {
        return Usage::default();
    };
    let field = |name: &str| {
        usage
            .get(name)
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32
    };
    Usage {
        input_tokens: field("input_tokens"),
        output_tokens: field("output_tokens"),
        cache_read_tokens: field("cache_read_input_tokens"),
        cache_write_tokens: field("cache_creation_input_tokens"),
    }
}

/// Read an SSE stream to completion, emitting deltas and assembling the turn.
async fn stream_response(
    response: reqwest::Response,
    sink: Option<mpsc::Sender<StreamEvent>>,
) -> Result<CompletionResponse> {
    let mut decoder = SseDecoder::new();
    let mut accumulator = Accumulator::default();
    let mut bytes = response.bytes_stream();

    while let Some(chunk) = bytes.next().await {
        let chunk = chunk.map_err(|source| ProviderError::Transport {
            provider: "anthropic".into(),
            source,
        })?;
        for event in decoder.push(&chunk) {
            if event.data == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(&event.data) else {
                continue;
            };
            accumulator.absorb(&value, &sink).await;
        }
    }
    if let Some(event) = decoder.finish() {
        if let Ok(value) = serde_json::from_str::<Value>(&event.data) {
            accumulator.absorb(&value, &sink).await;
        }
    }

    let response = accumulator.finish();
    emit(
        &sink,
        StreamEvent::Done {
            stop_reason: response.stop_reason.clone(),
        },
    )
    .await;
    Ok(response)
}

/// A block being assembled from streamed deltas.
#[derive(Debug)]
enum PartialBlock {
    Text(String),
    Thinking(String),
    ToolUse {
        id: String,
        name: String,
        /// `input_json_delta` fragments, concatenated. Parsed once at the end:
        /// the fragments are not individually valid JSON.
        json: String,
    },
}

/// Assembles a complete response from a stream of SSE events.
#[derive(Debug, Default)]
struct Accumulator {
    model: String,
    blocks: BTreeMap<usize, PartialBlock>,
    stop_reason: Option<StopReason>,
    usage: Usage,
}

impl Accumulator {
    async fn absorb(&mut self, value: &Value, sink: &Option<mpsc::Sender<StreamEvent>>) {
        match value.get("type").and_then(|v| v.as_str()) {
            Some("message_start") => {
                if let Some(message) = value.get("message") {
                    if let Some(model) = message.get("model").and_then(|v| v.as_str()) {
                        self.model = model.to_string();
                    }
                    self.usage = parse_usage(message.get("usage"));
                }
            }
            Some("content_block_start") => {
                let Some(index) = value.get("index").and_then(|v| v.as_u64()) else {
                    return;
                };
                let Some(block) = value.get("content_block") else {
                    return;
                };
                match block.get("type").and_then(|v| v.as_str()) {
                    Some("text") => {
                        self.blocks.insert(index as usize, PartialBlock::Text(String::new()));
                    }
                    Some("thinking") => {
                        self.blocks
                            .insert(index as usize, PartialBlock::Thinking(String::new()));
                    }
                    Some("tool_use") => {
                        let id = block
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let name = block
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        emit(
                            sink,
                            StreamEvent::ToolUseStart {
                                id: id.clone(),
                                name: name.clone(),
                            },
                        )
                        .await;
                        self.blocks.insert(
                            index as usize,
                            PartialBlock::ToolUse {
                                id,
                                name,
                                json: String::new(),
                            },
                        );
                    }
                    _ => {}
                }
            }
            Some("content_block_delta") => {
                let Some(index) = value.get("index").and_then(|v| v.as_u64()) else {
                    return;
                };
                let Some(delta) = value.get("delta") else {
                    return;
                };
                let Some(block) = self.blocks.get_mut(&(index as usize)) else {
                    return;
                };
                match (delta.get("type").and_then(|v| v.as_str()), block) {
                    (Some("text_delta"), PartialBlock::Text(buffer)) => {
                        let text = delta.get("text").and_then(|v| v.as_str()).unwrap_or_default();
                        buffer.push_str(text);
                        emit(sink, StreamEvent::TextDelta { text: text.into() }).await;
                    }
                    (Some("thinking_delta"), PartialBlock::Thinking(buffer)) => {
                        let text = delta
                            .get("thinking")
                            .or_else(|| delta.get("text"))
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        buffer.push_str(text);
                        emit(sink, StreamEvent::ThinkingDelta { text: text.into() }).await;
                    }
                    (Some("input_json_delta"), PartialBlock::ToolUse { json, .. }) => {
                        json.push_str(
                            delta
                                .get("partial_json")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default(),
                        );
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(reason) = value
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(|v| v.as_str())
                {
                    self.stop_reason = Some(parse_stop_reason(reason));
                }
                // The final usage arrives here, and only output counts update.
                if let Some(usage) = value.get("usage") {
                    let update = parse_usage(Some(usage));
                    if update.output_tokens > 0 {
                        self.usage.output_tokens = update.output_tokens;
                    }
                }
            }
            _ => {}
        }
    }

    fn finish(self) -> CompletionResponse {
        let content = self
            .blocks
            .into_values()
            .filter_map(|block| match block {
                PartialBlock::Text(text) if text.is_empty() => None,
                PartialBlock::Text(text) => Some(ContentBlock::Text { text }),
                PartialBlock::Thinking(text) if text.is_empty() => None,
                PartialBlock::Thinking(text) => Some(ContentBlock::Thinking { text }),
                PartialBlock::ToolUse { id, name, json } => Some(ContentBlock::ToolUse {
                    id,
                    name,
                    // An empty argument list streams as no deltas at all.
                    input: serde_json::from_str(&json).unwrap_or_else(|_| json!({})),
                }),
            })
            .collect();

        CompletionResponse {
            model: self.model,
            content,
            stop_reason: self.stop_reason.unwrap_or(StopReason::EndTurn),
            usage: self.usage,
        }
    }
}

/// Which request features a given model accepts.
///
/// The API rejects `effort` and adaptive `thinking` on older models, so the
/// adapter gates them rather than sending a body that 400s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelCapabilities {
    /// Accepts `thinking: {type: "adaptive"}`.
    pub adaptive_thinking: bool,
    /// Accepts `output_config.effort`.
    pub effort: bool,
    /// Accepts the `fallbacks` parameter.
    pub refusal_fallbacks: bool,
}

impl ModelCapabilities {
    /// Capabilities for a model id.
    pub fn for_model(model: &str) -> Self {
        // The current families all take adaptive thinking and effort.
        const MODERN: [&str; 8] = [
            "claude-fable-5",
            "claude-mythos-5",
            "claude-opus-5",
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-sonnet-5",
            "claude-sonnet-4-6",
        ];
        // Only the frontier models route refusals to a fallback model.
        const FALLBACK_CAPABLE: [&str; 3] =
            ["claude-fable-5", "claude-mythos-5", "claude-opus-5"];

        let modern = MODERN.iter().any(|m| model.starts_with(m));
        Self {
            adaptive_thinking: modern,
            effort: modern,
            refusal_fallbacks: FALLBACK_CAPABLE.iter().any(|m| model.starts_with(m)),
        }
    }
}

/// The model this project defaults to for bots that do the thinking.
pub const DEFAULT_MODEL: &str = "claude-opus-5";

/// A cheaper model for inspectors, routers, and other high-volume roles.
pub const CHEAP_MODEL: &str = "claude-haiku-4-5";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Effort, ToolDef};

    fn provider() -> AnthropicProvider {
        AnthropicProvider::with_credential(Credential::ApiKey("sk-test".into()))
    }

    fn request(model: &str) -> CompletionRequest {
        CompletionRequest::new(model, "you are a bot", vec![Turn::user("hello")])
    }

    // ------------------------------------------------------------ request body

    #[test]
    fn the_body_carries_the_core_fields() {
        let body = provider().build_body(&request(DEFAULT_MODEL));
        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["max_tokens"], 16_000);
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"][0]["text"], "hello");
    }

    #[test]
    fn the_system_prompt_is_a_cached_block() {
        let body = provider().build_body(&request(DEFAULT_MODEL));
        assert_eq!(body["system"][0]["text"], "you are a bot");
        assert_eq!(
            body["system"][0]["cache_control"]["type"], "ephemeral",
            "the system prompt is identical every turn, so it is the cache prefix"
        );
    }

    #[test]
    fn an_empty_system_prompt_is_omitted_entirely() {
        let mut request = request(DEFAULT_MODEL);
        request.system = String::new();
        let body = provider().build_body(&request);
        assert!(body.get("system").is_none());
    }

    #[test]
    fn modern_models_get_adaptive_thinking_and_effort() {
        let body = provider().build_body(&request(DEFAULT_MODEL).with_effort(Effort::XHigh));
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(
            body["thinking"]["display"], "summarized",
            "the default of omitted reads as a long pause in a chat UI"
        );
        assert_eq!(body["output_config"]["effort"], "xhigh");
    }

    /// `budget_tokens` was removed on the current models and returns a 400.
    #[test]
    fn the_body_never_sends_a_thinking_budget() {
        let body = provider().build_body(&request(DEFAULT_MODEL));
        assert!(body["thinking"].get("budget_tokens").is_none());
    }

    #[test]
    fn older_models_get_neither_thinking_nor_effort() {
        let body = provider().build_body(&request("claude-3-5-sonnet-20241022"));
        assert!(
            body.get("thinking").is_none() && body.get("output_config").is_none(),
            "sending these to a model that predates them is a 400"
        );
    }

    #[test]
    fn refusal_fallbacks_are_enabled_only_on_the_models_that_take_them() {
        let opus = provider().build_body(&request("claude-opus-5"));
        assert_eq!(opus["fallbacks"], "default");
        assert!(provider().betas(&request("claude-opus-5")).contains(&FALLBACK_BETA));

        let sonnet = provider().build_body(&request("claude-sonnet-5"));
        assert!(
            sonnet.get("fallbacks").is_none(),
            "the parameter is rejected on models without server-side fallbacks"
        );
        assert!(!provider().betas(&request("claude-sonnet-5")).contains(&FALLBACK_BETA));
    }

    #[test]
    fn an_oauth_credential_adds_its_beta_flag() {
        let oauth = AnthropicProvider::with_credential(Credential::Bearer("tok".into()));
        assert!(oauth.betas(&request(DEFAULT_MODEL)).contains(&OAUTH_BETA));
        assert!(!provider().betas(&request(DEFAULT_MODEL)).contains(&OAUTH_BETA));
    }

    #[test]
    fn tools_are_sent_with_their_json_schema() {
        let request = request(DEFAULT_MODEL).with_tools(vec![ToolDef {
            name: "claim_task".into(),
            description: "take a task".into(),
            input_schema: json!({"type": "object", "properties": {"task": {"type": "string"}}}),
        }]);
        let body = provider().build_body(&request);
        assert_eq!(body["tools"][0]["name"], "claim_task");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
    }

    #[test]
    fn tool_calls_and_results_round_trip_into_the_body() {
        let mut request = request(DEFAULT_MODEL);
        request.messages = vec![
            Turn {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "toolu_1".into(),
                    name: "claim_task".into(),
                    input: json!({"task": "task_1"}),
                }],
            },
            Turn::tool_results(vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_1".into(),
                content: "claimed".into(),
                is_error: false,
            }]),
        ];
        let body = provider().build_body(&request);
        assert_eq!(body["messages"][0]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][0]["content"][0]["input"]["task"], "task_1");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_result");
        assert_eq!(body["messages"][1]["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(body["messages"][1]["content"][0]["is_error"], false);
    }

    /// Replaying a reconstructed thinking block is a 400, so they are dropped.
    #[test]
    fn thinking_blocks_are_not_replayed() {
        let mut request = request(DEFAULT_MODEL);
        request.messages = vec![Turn {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    text: "reasoning".into(),
                },
                ContentBlock::text("answer"),
            ],
        }];
        let body = provider().build_body(&request);
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["text"], "answer");
    }

    /// A turn holding only thinking becomes empty, and the API rejects an
    /// empty content array — so the turn must be dropped, not sent hollow.
    #[test]
    fn a_turn_left_empty_after_dropping_thinking_is_omitted() {
        let mut request = request(DEFAULT_MODEL);
        request.messages = vec![
            Turn::user("question"),
            Turn {
                role: Role::Assistant,
                content: vec![ContentBlock::Thinking {
                    text: "just reasoning".into(),
                }],
            },
        ];
        let body = provider().build_body(&request);
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    }

    // ------------------------------------------------------------- responses

    #[test]
    fn a_complete_message_is_parsed() {
        let value = json!({
            "model": "claude-opus-5",
            "content": [
                {"type": "text", "text": "on it"},
                {"type": "tool_use", "id": "toolu_1", "name": "claim_task", "input": {"task": "t"}}
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 10, "output_tokens": 25,
                "cache_read_input_tokens": 900, "cache_creation_input_tokens": 100
            }
        });
        let parsed = parse_message(&value).unwrap();
        assert_eq!(parsed.text(), "on it");
        assert_eq!(parsed.stop_reason, StopReason::ToolUse);
        assert_eq!(parsed.tool_calls().len(), 1);
        assert_eq!(parsed.usage.cache_read_tokens, 900);
        assert_eq!(parsed.usage.cache_write_tokens, 100);
    }

    #[test]
    fn every_stop_reason_maps_to_a_known_variant() {
        assert_eq!(parse_stop_reason("end_turn"), StopReason::EndTurn);
        assert_eq!(parse_stop_reason("tool_use"), StopReason::ToolUse);
        assert_eq!(parse_stop_reason("max_tokens"), StopReason::MaxTokens);
        assert_eq!(parse_stop_reason("refusal"), StopReason::Refusal);
        assert_eq!(parse_stop_reason("stop_sequence"), StopReason::StopSequence);
        assert!(matches!(parse_stop_reason("brand_new"), StopReason::Other(_)));
    }

    #[test]
    fn an_error_body_yields_its_message() {
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad model"}}"#;
        assert_eq!(extract_error_message(body), "bad model");
    }

    #[test]
    fn a_non_json_error_body_is_passed_through_truncated() {
        assert_eq!(extract_error_message("<html>502</html>"), "<html>502</html>");
    }

    // ------------------------------------------------------------- streaming

    /// Drive the accumulator over a realistic event sequence.
    async fn assemble(events: &[Value]) -> CompletionResponse {
        let mut accumulator = Accumulator::default();
        for event in events {
            accumulator.absorb(event, &None).await;
        }
        accumulator.finish()
    }

    #[tokio::test]
    async fn a_streamed_turn_assembles_text_and_a_tool_call() {
        let response = assemble(&[
            json!({"type": "message_start", "message": {
                "model": "claude-opus-5", "usage": {"input_tokens": 10}
            }}),
            json!({"type": "content_block_start", "index": 0,
                   "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 0,
                   "delta": {"type": "text_delta", "text": "Hel"}}),
            json!({"type": "content_block_delta", "index": 0,
                   "delta": {"type": "text_delta", "text": "lo"}}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "content_block_start", "index": 1,
                   "content_block": {"type": "tool_use", "id": "toolu_1", "name": "claim_task"}}),
            // Argument fragments are not individually valid JSON.
            json!({"type": "content_block_delta", "index": 1,
                   "delta": {"type": "input_json_delta", "partial_json": "{\"task\":"}}),
            json!({"type": "content_block_delta", "index": 1,
                   "delta": {"type": "input_json_delta", "partial_json": "\"task_1\"}"}}),
            json!({"type": "content_block_stop", "index": 1}),
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"},
                   "usage": {"output_tokens": 25}}),
            json!({"type": "message_stop"}),
        ])
        .await;

        assert_eq!(response.model, "claude-opus-5");
        assert_eq!(response.text(), "Hello");
        assert_eq!(response.stop_reason, StopReason::ToolUse);
        let calls = response.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "claim_task");
        assert_eq!(calls[0].2["task"], "task_1", "fragments must reassemble");
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 25);
    }

    #[tokio::test]
    async fn streamed_blocks_keep_their_wire_order() {
        // Blocks arrive interleaved by index; the assembled order must follow
        // the index, not arrival.
        let response = assemble(&[
            json!({"type": "content_block_start", "index": 1,
                   "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_start", "index": 0,
                   "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 1,
                   "delta": {"type": "text_delta", "text": "second"}}),
            json!({"type": "content_block_delta", "index": 0,
                   "delta": {"type": "text_delta", "text": "first"}}),
        ])
        .await;
        assert_eq!(response.text(), "first\nsecond");
    }

    #[tokio::test]
    async fn a_tool_call_with_no_arguments_becomes_an_empty_object() {
        let response = assemble(&[
            json!({"type": "content_block_start", "index": 0,
                   "content_block": {"type": "tool_use", "id": "toolu_1", "name": "list_tasks"}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}),
        ])
        .await;
        assert_eq!(response.tool_calls()[0].2, &json!({}));
    }

    #[tokio::test]
    async fn streamed_thinking_is_captured_separately_from_the_answer() {
        let response = assemble(&[
            json!({"type": "content_block_start", "index": 0,
                   "content_block": {"type": "thinking", "thinking": ""}}),
            json!({"type": "content_block_delta", "index": 0,
                   "delta": {"type": "thinking_delta", "thinking": "weighing it up"}}),
            json!({"type": "content_block_start", "index": 1,
                   "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 1,
                   "delta": {"type": "text_delta", "text": "the answer"}}),
        ])
        .await;
        assert_eq!(response.text(), "the answer", "thinking is not part of the answer");
        assert!(matches!(
            response.content[0],
            ContentBlock::Thinking { .. }
        ));
    }

    #[tokio::test]
    async fn a_refusal_survives_the_stream() {
        let response = assemble(&[
            json!({"type": "message_start", "message": {"model": "claude-opus-5"}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "refusal"}}),
        ])
        .await;
        assert_eq!(response.stop_reason, StopReason::Refusal);
        assert!(response.stop_reason.is_problem());
    }

    #[tokio::test]
    async fn stream_events_reach_the_sink_as_they_arrive() {
        let (tx, mut rx) = mpsc::channel(16);
        let sink = Some(tx);
        let mut accumulator = Accumulator::default();
        for event in [
            json!({"type": "content_block_start", "index": 0,
                   "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 0,
                   "delta": {"type": "text_delta", "text": "hi"}}),
            json!({"type": "content_block_start", "index": 1,
                   "content_block": {"type": "tool_use", "id": "toolu_1", "name": "claim_task"}}),
        ] {
            accumulator.absorb(&event, &sink).await;
        }
        drop(sink);

        let mut seen = Vec::new();
        while let Some(event) = rx.recv().await {
            seen.push(event);
        }
        assert_eq!(seen[0], StreamEvent::TextDelta { text: "hi".into() });
        assert!(matches!(seen[1], StreamEvent::ToolUseStart { .. }));
    }

    #[test]
    fn capabilities_track_the_model_families() {
        let opus5 = ModelCapabilities::for_model("claude-opus-5");
        assert!(opus5.adaptive_thinking && opus5.effort && opus5.refusal_fallbacks);

        let sonnet = ModelCapabilities::for_model("claude-sonnet-4-6");
        assert!(sonnet.adaptive_thinking && sonnet.effort);
        assert!(!sonnet.refusal_fallbacks);

        let legacy = ModelCapabilities::for_model("claude-3-opus-20240229");
        assert!(!legacy.adaptive_thinking && !legacy.effort && !legacy.refusal_fallbacks);
    }
}
