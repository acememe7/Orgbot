//! OpenAI-compatible `/v1/chat/completions` backends.
//!
//! One adapter covers DeepSeek, a plain OpenAI key, a Codex/ChatGPT sign-in,
//! and anything else that speaks the same dialect (vLLM, Ollama, LM Studio,
//! OpenRouter). They differ only in base URL, credential source, and a couple
//! of headers, so those are data rather than separate implementations.
//!
//! The dialect differs from Anthropic's in three ways this adapter absorbs:
//! the system prompt is a message rather than a top-level field, tool call
//! arguments are a JSON *string* rather than an object, and tool results are
//! `role: "tool"` messages rather than blocks inside a user turn.

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

/// A backend speaking the OpenAI chat-completions dialect.
#[derive(Debug)]
pub struct OpenAiProvider {
    http: reqwest::Client,
    name: String,
    base_url: String,
    credential: Credential,
}

impl OpenAiProvider {
    /// DeepSeek, credential from `DEEPSEEK_API_KEY`.
    pub fn deepseek() -> Result<Self> {
        Ok(Self::new(
            "deepseek",
            auth::base_url("deepseek", "https://api.deepseek.com"),
            auth::deepseek()?,
        ))
    }

    /// OpenAI, credential from `OPENAI_API_KEY` or the Codex CLI sign-in.
    pub fn openai() -> Result<Self> {
        Ok(Self::new(
            "openai",
            auth::base_url("openai", "https://api.openai.com"),
            auth::openai_compatible("openai", "OPENAI_API_KEY")?,
        ))
    }

    /// A Codex/ChatGPT sign-in as stored by the Codex CLI.
    ///
    /// A ChatGPT session is not an OpenAI API key: it authenticates against
    /// ChatGPT's own backend, whose request surface is not a published API and
    /// can change without notice. The base URL is therefore configurable via
    /// `OPENAI_BASE_URL` — point it at whatever endpoint the session is valid
    /// for. When `~/.codex/auth.json` holds a plain `OPENAI_API_KEY` instead,
    /// this behaves exactly like [`OpenAiProvider::openai`] and is stable.
    pub fn codex() -> Result<Self> {
        Ok(Self::new(
            "codex",
            auth::base_url("openai", "https://api.openai.com"),
            auth::codex_cli()?,
        ))
    }

    /// Grok API keys and subscription sessions have separate endpoints.
    pub fn grok() -> Result<Self> {
        let mut provider = Self::grok_with_credential(auth::grok()?);
        provider.base_url = auth::base_url("grok", &provider.base_url);
        Ok(provider)
    }

    pub fn grok_with_credential(credential: Credential) -> Self {
        let base = if credential.is_oauth() {
            "https://cli-chat-proxy.grok.com"
        } else {
            "https://api.x.ai"
        };
        Self::new("grok", base, credential)
    }

    /// Any other OpenAI-compatible endpoint.
    pub fn custom(
        name: impl Into<String>,
        base_url: impl Into<String>,
        credential: Credential,
    ) -> Self {
        Self::new(name, base_url, credential)
    }

    fn new(name: impl Into<String>, base_url: impl Into<String>, credential: Credential) -> Self {
        Self {
            http: crate::provider::http_client(),
            name: name.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            credential,
        }
    }

    /// Build the JSON request body. Crate-visible so the mapping is testable
    /// without a network round trip.
    pub(crate) fn build_body(&self, request: &CompletionRequest) -> Value {
        let mut messages: Vec<Value> = Vec::new();
        if !request.system.is_empty() {
            messages.push(json!({"role": "system", "content": request.system}));
        }
        for turn in &request.messages {
            messages.extend(turn_to_wire(turn));
        }

        let mut body = Map::new();
        body.insert("model".into(), json!(request.model));
        body.insert("messages".into(), json!(messages));
        body.insert("max_tokens".into(), json!(request.max_tokens));
        body.insert("stream".into(), json!(request.stream));
        if request.stream {
            // Without this, usage is omitted entirely from streamed responses.
            body.insert("stream_options".into(), json!({"include_usage": true}));
        }
        if !request.tools.is_empty() {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.input_schema,
                        }
                    })
                })
                .collect();
            body.insert("tools".into(), json!(tools));
        }
        Value::Object(body)
    }

    fn request_builder(&self, model: &str) -> reqwest::RequestBuilder {
        let mut builder = self
            .http
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!("Bearer {}", self.credential.secret()),
            );

        if self.name == "grok" && self.credential.is_oauth() {
            // Grok CLI's subscription wire contract, including model routing
            // and the protocol version required by the proxy.
            builder = builder
                .header("X-XAI-Token-Auth", "xai-grok-cli")
                .header("x-authenticateresponse", "authenticate-response")
                .header("x-grok-model-override", model)
                .header("x-grok-client-version", "1.0.13")
                .header("user-agent", "OrgBot");
        }

        // A ChatGPT session is scoped to an account, which the backend expects
        // as its own header alongside the bearer token.
        if let Credential::ChatGptSession { account_id, .. } = &self.credential {
            if !account_id.is_empty() {
                builder = builder.header("chatgpt-account-id", account_id.as_str());
            }
        }
        builder
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> Result<CompletionResponse> {
        let request = crate::provider::prepare_images(request, self.name != "deepseek").await?;
        let body = self.build_body(&request);
        let response = self
            .request_builder(&request.model)
            .json(&body)
            .send()
            .await
            .map_err(|source| ProviderError::Transport {
                provider: self.name.clone(),
                source,
            })?;
        let response = check_status(&self.name, response).await?;

        if request.stream {
            stream_response(&self.name, response, sink).await
        } else {
            let value: Value = response.json().await.map_err(|e| ProviderError::Decode {
                provider: self.name.clone(),
                detail: e.to_string(),
            })?;
            parse_completion(&value)
        }
    }
}

async fn check_status(provider: &str, response: reqwest::Response) -> Result<reqwest::Response> {
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
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.chars().take(500).collect());

    Err(match status.as_u16() {
        401 | 403 => ProviderError::Unauthorized {
            provider: provider.into(),
            status: status.as_u16(),
        },
        429 => ProviderError::RateLimited {
            provider: provider.into(),
            message,
            retry_after_secs: retry_after,
        },
        400 | 404 | 422 => ProviderError::BadRequest {
            provider: provider.into(),
            message,
        },
        code => ProviderError::Upstream {
            provider: provider.into(),
            status: code,
            message,
        },
    })
}

/// Convert one neutral turn into one or more dialect messages.
///
/// A single assistant turn holding both text and tool calls becomes one
/// message; a user turn holding tool results becomes one message *per result*,
/// because this dialect has no way to carry several results in one message.
fn turn_to_wire(turn: &Turn) -> Vec<Value> {
    let mut messages = Vec::new();
    let mut text = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut images = Vec::new();

    for block in &turn.content {
        match block {
            ContentBlock::Text { text: chunk } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(chunk);
            }
            // Reasoning is not replayed; see the note in the Anthropic adapter.
            ContentBlock::Thinking { .. } => {}
            ContentBlock::Image {
                media_type,
                source: crate::types::ImageSource::Base64 { data },
            } => {
                images.push(json!({"type":"image_url", "image_url":{"url":format!("data:{media_type};base64,{data}")}}));
            }
            ContentBlock::Image { .. } => {
                text.push_str("[Image was not loaded]");
            }
            ContentBlock::ToolUse { id, name, input } => tool_calls.push(json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    // Arguments are a JSON string in this dialect, not an object.
                    "arguments": input.to_string(),
                }
            })),
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let body = if *is_error {
                    format!("ERROR: {content}")
                } else {
                    content.clone()
                };
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": body,
                }));
            }
        }
    }

    let role = match turn.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    if !tool_calls.is_empty() {
        messages.push(json!({
            "role": "assistant",
            "content": if text.is_empty() { Value::Null } else { json!(text) },
            "tool_calls": tool_calls,
        }));
    } else if !images.is_empty() {
        let mut content = vec![json!({"type":"text", "text":text})];
        content.extend(images);
        messages.push(json!({"role":role, "content":content}));
    } else if !text.is_empty() {
        messages.push(json!({"role": role, "content": text}));
    }
    messages
}

/// Parse a complete (non-streamed) chat completion.
pub(crate) fn parse_completion(value: &Value) -> Result<CompletionResponse> {
    let model = value
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let choice = value
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first());

    let mut content = Vec::new();
    let mut stop_reason = StopReason::EndTurn;

    if let Some(choice) = choice {
        if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
            stop_reason = parse_finish_reason(reason);
        }
        if let Some(message) = choice.get("message") {
            // DeepSeek's reasoning models return their reasoning separately.
            if let Some(reasoning) = message
                .get("reasoning_content")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                content.push(ContentBlock::Thinking {
                    text: reasoning.to_string(),
                });
            }
            if let Some(text) = message
                .get("content")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                content.push(ContentBlock::Text {
                    text: text.to_string(),
                });
            }
            if let Some(calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
                for call in calls {
                    if let Some(block) = tool_call_from_wire(call) {
                        content.push(block);
                    }
                }
            }
        }
    }

    // A model can emit tool calls while reporting `stop`; trust the calls.
    if stop_reason == StopReason::EndTurn && content.iter().any(ContentBlock::is_tool_use) {
        stop_reason = StopReason::ToolUse;
    }

    Ok(CompletionResponse {
        model,
        content,
        stop_reason,
        usage: parse_usage(value.get("usage")),
    })
}

fn tool_call_from_wire(call: &Value) -> Option<ContentBlock> {
    let function = call.get("function")?;
    let arguments = function
        .get("arguments")
        .and_then(|v| v.as_str())
        .unwrap_or("{}");
    Some(ContentBlock::ToolUse {
        id: call.get("id")?.as_str()?.to_string(),
        name: function.get("name")?.as_str()?.to_string(),
        // Arguments arrive as a JSON string; an empty or malformed one becomes
        // an empty object rather than failing the whole turn.
        input: serde_json::from_str(arguments).unwrap_or_else(|_| json!({})),
    })
}

fn parse_finish_reason(raw: &str) -> StopReason {
    match raw {
        "stop" => StopReason::EndTurn,
        "tool_calls" | "function_call" => StopReason::ToolUse,
        "length" => StopReason::MaxTokens,
        "content_filter" => StopReason::Refusal,
        other => StopReason::Other(other.to_string()),
    }
}

fn parse_usage(usage: Option<&Value>) -> Usage {
    let Some(usage) = usage else {
        return Usage::default();
    };
    let field = |name: &str| usage.get(name).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let cached = usage
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    Usage {
        // Cached tokens are reported inside the prompt total here, unlike the
        // Anthropic shape where they are a separate bucket.
        input_tokens: field("prompt_tokens").saturating_sub(cached),
        output_tokens: field("completion_tokens"),
        cache_read_tokens: cached,
        cache_write_tokens: 0,
    }
}

async fn stream_response(
    provider: &str,
    response: reqwest::Response,
    sink: Option<mpsc::Sender<StreamEvent>>,
) -> Result<CompletionResponse> {
    let mut decoder = SseDecoder::new();
    let mut accumulator = Accumulator::default();
    let mut bytes = response.bytes_stream();

    'stream: while let Some(chunk) = bytes.next().await {
        let chunk = chunk.map_err(|source| ProviderError::Transport {
            provider: provider.into(),
            source,
        })?;
        for event in decoder.push(&chunk) {
            if event.data.trim() == "[DONE]" {
                break 'stream;
            }
            let Ok(value) = serde_json::from_str::<Value>(&event.data) else {
                continue;
            };
            accumulator.absorb(&value, &sink).await;
        }
    }
    if let Some(event) = decoder.finish() {
        if event.data.trim() != "[DONE]" {
            if let Ok(value) = serde_json::from_str::<Value>(&event.data) {
                accumulator.absorb(&value, &sink).await;
            }
        }
    }

    if accumulator.stop_reason.is_none() {
        return Err(ProviderError::StreamInterrupted {
            provider: provider.into(),
            message: "stream ended before a finish reason; incomplete tool calls were not run"
                .into(),
        });
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

/// A tool call being assembled from streamed fragments.
#[derive(Debug, Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
    /// Whether the start of this call has already been announced downstream.
    announced: bool,
}

#[derive(Debug, Default)]
struct Accumulator {
    model: String,
    text: String,
    thinking: String,
    /// Keyed by the `index` field, which is how this dialect correlates
    /// fragments of concurrent tool calls.
    calls: BTreeMap<u64, PartialCall>,
    stop_reason: Option<StopReason>,
    usage: Usage,
}

impl Accumulator {
    async fn absorb(&mut self, value: &Value, sink: &Option<mpsc::Sender<StreamEvent>>) {
        if let Some(model) = value.get("model").and_then(|v| v.as_str()) {
            if self.model.is_empty() {
                self.model = model.to_string();
            }
        }
        // The final chunk carries usage and usually has no choices.
        if let Some(usage) = value.get("usage") {
            if !usage.is_null() {
                self.usage = parse_usage(Some(usage));
            }
        }

        let Some(choice) = value
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|c| c.first())
        else {
            return;
        };
        if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
            self.stop_reason = Some(parse_finish_reason(reason));
        }
        let Some(delta) = choice.get("delta") else {
            return;
        };

        if let Some(reasoning) = delta
            .get("reasoning_content")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            self.thinking.push_str(reasoning);
            emit(
                sink,
                StreamEvent::ThinkingDelta {
                    text: reasoning.into(),
                },
            )
            .await;
        }
        if let Some(text) = delta
            .get("content")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            self.text.push_str(text);
            emit(sink, StreamEvent::TextDelta { text: text.into() }).await;
        }

        let Some(calls) = delta.get("tool_calls").and_then(|v| v.as_array()) else {
            return;
        };
        for call in calls {
            let index = call.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
            let entry = self.calls.entry(index).or_default();
            if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                entry.id = id.to_string();
            }
            if let Some(function) = call.get("function") {
                if let Some(name) = function.get("name").and_then(|v| v.as_str()) {
                    entry.name.push_str(name);
                }
                if let Some(arguments) = function.get("arguments").and_then(|v| v.as_str()) {
                    entry.arguments.push_str(arguments);
                }
            }
            // Announce once the call has both halves of its identity.
            if !entry.announced && !entry.id.is_empty() && !entry.name.is_empty() {
                entry.announced = true;
                emit(
                    sink,
                    StreamEvent::ToolUseStart {
                        id: entry.id.clone(),
                        name: entry.name.clone(),
                    },
                )
                .await;
            }
        }
    }

    fn finish(self) -> CompletionResponse {
        let mut content = Vec::new();
        if !self.thinking.is_empty() {
            content.push(ContentBlock::Thinking {
                text: self.thinking,
            });
        }
        if !self.text.is_empty() {
            content.push(ContentBlock::Text { text: self.text });
        }
        for call in self.calls.into_values() {
            if call.id.is_empty() && call.name.is_empty() {
                continue;
            }
            content.push(ContentBlock::ToolUse {
                id: call.id,
                name: call.name,
                input: serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({})),
            });
        }

        let mut stop_reason = self.stop_reason.unwrap_or(StopReason::EndTurn);
        if stop_reason == StopReason::EndTurn && content.iter().any(ContentBlock::is_tool_use) {
            stop_reason = StopReason::ToolUse;
        }
        CompletionResponse {
            model: self.model,
            content,
            stop_reason,
            usage: self.usage,
        }
    }
}

/// DeepSeek's general-purpose model.
pub const DEEPSEEK_CHAT: &str = "deepseek-chat";

/// DeepSeek's reasoning model, which returns `reasoning_content`.
pub const DEEPSEEK_REASONER: &str = "deepseek-reasoner";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolDef;

    #[tokio::test]
    async fn streams_finish_at_done_and_reject_truncated_tool_turns() {
        let tool = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"shell\",\"arguments\":\"{}\"}}]}}]}\n\n";
        for (terminal, keep_open, succeeds) in [
            ("data: {\"choices\":[{\"finish_reason\":\"tool_calls\"}]}\n\ndata: {\"usage\":{\"completion_tokens\":9}}\n\ndata: [DONE]\n\n", true, true),
            ("data: {\"choices\":[{\"finish_reason\":\"tool_calls\"}]}", false, true),
            ("", false, false),
        ] {
            let (http, _release) = crate::sse::test_response(&format!("{tool}{terminal}"), keep_open).await;
            let result = tokio::time::timeout(std::time::Duration::from_secs(1), stream_response("grok", http, None))
                .await.expect("a completed turn must not wait for the server to close HTTP");
            assert_eq!(result.is_ok(), succeeds, "{result:?}");
            if let Ok(turn) = result {
                assert_eq!(turn.stop_reason, StopReason::ToolUse);
                if keep_open { assert_eq!(turn.usage.output_tokens, 9); }
            }
        }
    }

    #[test]
    fn grok_subscription_routes_the_selected_model_without_changing_api_keys() {
        let provider = OpenAiProvider::grok_with_credential(Credential::Bearer("session".into()));
        let request = provider.request_builder("grok-4.6").build().unwrap();
        assert_eq!(
            request.url().as_str(),
            "https://cli-chat-proxy.grok.com/v1/chat/completions"
        );
        assert_eq!(request.headers()["x-grok-model-override"], "grok-4.6");
        assert_eq!(request.headers()["x-grok-client-version"], "1.0.13");
        assert_eq!(request.headers()["x-xai-token-auth"], "xai-grok-cli");
        assert_eq!(request.headers()["authorization"], "Bearer session");
        let api = OpenAiProvider::grok_with_credential(Credential::ApiKey("key".into()));
        let request = api.request_builder("api-model").build().unwrap();
        assert_eq!(request.url().host_str(), Some("api.x.ai"));
        assert!(!request.headers().contains_key("x-grok-model-override"));
    }

    #[tokio::test]
    async fn rate_limit_errors_preserve_the_server_reason_and_retry_delay() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for body in [
            r#"{"error":{"message":"Temporary model capacity limit"}}"#,
            r#"{"code":"subscription:free-usage-exhausted","error":"Weekly allowance exhausted"}"#,
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                socket.read(&mut request).await.unwrap();
                socket.write_all(format!(
                    "HTTP/1.1 429 Too Many Requests\r\nContent-Length: {}\r\nRetry-After: 12\r\nConnection: close\r\n\r\n{body}", body.len()
                ).as_bytes()).await.unwrap();
            });
            let response = reqwest::get(format!("http://{address}")).await.unwrap();
            let error = check_status("grok", response).await.unwrap_err();
            let report = crate::error::ErrorReport::from(&error);
            let expected = if body.contains("capacity") { "Temporary model capacity limit" } else { "subscription:free-usage-exhausted" };
            assert!(report.message.contains(expected), "{}", report.message);
            assert!(report.message.contains("retry after 12s"));
            assert_eq!(report.kind, "rate_limited");
            assert_eq!(error.retry_after_secs(), Some(12));
            server.await.unwrap();
        }
    }

    fn provider() -> OpenAiProvider {
        OpenAiProvider::custom(
            "deepseek",
            "https://api.deepseek.com",
            Credential::ApiKey("sk-test".into()),
        )
    }

    #[test]
    fn ollama_cloud_uses_its_own_key_and_the_existing_tool_protocol() {
        use orgbot_proto::agent::ProviderRef;
        let reference = ProviderRef::OpenAiCompatible {
            profile: "ollama".into(),
            model: "gpt-oss:120b".into(),
        };
        let mut environment = std::collections::HashMap::from([
            ("ORGBOT_OLLAMA_BASE_URL".into(), "https://ollama.com".into()),
            ("CODEX_AUTH_TOKEN".into(), "unrelated-session".into()),
        ]);
        assert!(crate::build_with_environment(&reference, &environment).is_err());
        environment.insert("ORGBOT_OLLAMA_API_KEY".into(), "ollama-only".into());
        assert_eq!(
            crate::build_with_environment(&reference, &environment)
                .unwrap()
                .name(),
            "ollama"
        );
        let provider = OpenAiProvider::custom(
            "ollama",
            "https://ollama.com",
            Credential::ApiKey("ollama-only".into()),
        );
        let request = CompletionRequest::new(
            "gpt-oss:120b",
            "Research",
            vec![Turn::user("Read the brief")],
        )
        .with_tools(vec![ToolDef {
            name: "workspace_read".into(),
            description: "Read a file".into(),
            input_schema: json!({"type":"object"}),
        }]);
        let http = provider
            .request_builder(&request.model)
            .json(&provider.build_body(&request))
            .build()
            .unwrap();
        assert_eq!(
            http.url().as_str(),
            "https://ollama.com/v1/chat/completions"
        );
        assert_eq!(http.headers()["authorization"], "Bearer ollama-only");
        assert!(!http.headers().contains_key("chatgpt-account-id"));
        let body: Value = serde_json::from_slice(http.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body["model"], "gpt-oss:120b");
        assert_eq!(body["tools"][0]["function"]["name"], "workspace_read");
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    fn request() -> CompletionRequest {
        CompletionRequest::new(DEEPSEEK_CHAT, "you are a bot", vec![Turn::user("hello")])
    }

    // ------------------------------------------------------------ request body

    #[test]
    fn the_system_prompt_becomes_the_first_message() {
        let body = provider().build_body(&request());
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "you are a bot");
        assert_eq!(body["messages"][1]["role"], "user");
        assert!(
            body.get("system").is_none(),
            "this dialect has no top-level system field"
        );
    }

    #[test]
    fn streaming_requests_ask_for_usage_explicitly() {
        let body = provider().build_body(&request());
        assert_eq!(
            body["stream_options"]["include_usage"], true,
            "usage is omitted from streamed responses without this"
        );
    }

    #[test]
    fn a_non_streaming_request_omits_stream_options() {
        let mut request = request();
        request.stream = false;
        let body = provider().build_body(&request);
        assert!(body.get("stream_options").is_none());
    }

    #[test]
    fn tools_are_wrapped_in_the_function_envelope() {
        let request = request().with_tools(vec![ToolDef {
            name: "claim_task".into(),
            description: "take a task".into(),
            input_schema: json!({"type": "object"}),
        }]);
        let body = provider().build_body(&request);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "claim_task");
        assert_eq!(
            body["tools"][0]["function"]["parameters"]["type"], "object",
            "the schema field is `parameters` here, not `input_schema`"
        );
    }

    /// The single most error-prone difference from the Anthropic dialect.
    #[test]
    fn tool_call_arguments_are_serialised_as_a_json_string() {
        let mut request = request();
        request.messages = vec![Turn {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "claim_task".into(),
                input: json!({"task": "task_1"}),
            }],
        }];
        let body = provider().build_body(&request);
        let arguments = &body["messages"][1]["tool_calls"][0]["function"]["arguments"];
        assert!(
            arguments.is_string(),
            "arguments must be a string, got {arguments}"
        );
        assert_eq!(
            serde_json::from_str::<Value>(arguments.as_str().unwrap()).unwrap()["task"],
            "task_1"
        );
    }

    #[test]
    fn tool_results_become_their_own_tool_role_messages() {
        let mut request = request();
        request.messages = vec![Turn::tool_results(vec![
            ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: "claimed".into(),
                is_error: false,
            },
            ContentBlock::ToolResult {
                tool_use_id: "call_2".into(),
                content: "not yours".into(),
                is_error: true,
            },
        ])];
        let body = provider().build_body(&request);
        let messages = body["messages"].as_array().unwrap();
        // system + one message per result: this dialect cannot batch them.
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(messages[1]["tool_call_id"], "call_1");
        assert_eq!(messages[1]["content"], "claimed");
        assert!(
            messages[2]["content"]
                .as_str()
                .unwrap()
                .starts_with("ERROR:"),
            "there is no is_error flag here, so failure has to be in the text"
        );
    }

    #[test]
    fn an_assistant_turn_with_text_and_calls_becomes_one_message() {
        let mut request = request();
        request.messages = vec![Turn {
            role: Role::Assistant,
            content: vec![
                ContentBlock::text("let me take that"),
                ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "claim_task".into(),
                    input: json!({}),
                },
            ],
        }];
        let body = provider().build_body(&request);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["content"], "let me take that");
        assert_eq!(messages[1]["tool_calls"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn thinking_blocks_are_not_replayed() {
        let mut request = request();
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
        assert_eq!(body["messages"][1]["content"], "answer");
    }

    // ------------------------------------------------------------- responses

    #[test]
    fn a_complete_response_is_parsed() {
        let value = json!({
            "model": "deepseek-chat",
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "content": "on it",
                    "tool_calls": [{
                        "id": "call_1", "type": "function",
                        "function": {"name": "claim_task", "arguments": "{\"task\":\"t\"}"}
                    }]
                }
            }],
            "usage": {"prompt_tokens": 100, "completion_tokens": 20}
        });
        let parsed = parse_completion(&value).unwrap();
        assert_eq!(parsed.text(), "on it");
        assert_eq!(parsed.stop_reason, StopReason::ToolUse);
        assert_eq!(parsed.tool_calls()[0].2["task"], "t");
    }

    #[test]
    fn deepseek_reasoning_is_captured_as_a_thinking_block() {
        let value = json!({
            "model": "deepseek-reasoner",
            "choices": [{"finish_reason": "stop", "message": {
                "reasoning_content": "step by step", "content": "the answer"
            }}]
        });
        let parsed = parse_completion(&value).unwrap();
        assert!(matches!(parsed.content[0], ContentBlock::Thinking { .. }));
        assert_eq!(parsed.text(), "the answer");
    }

    /// Cached tokens are counted inside `prompt_tokens` here, unlike the
    /// Anthropic shape where they are a separate bucket. Double counting them
    /// would inflate every cost estimate.
    #[test]
    fn cached_tokens_are_split_out_of_the_prompt_total() {
        let value = json!({
            "choices": [],
            "usage": {
                "prompt_tokens": 1000, "completion_tokens": 50,
                "prompt_tokens_details": {"cached_tokens": 900}
            }
        });
        let usage = parse_completion(&value).unwrap().usage;
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.cache_read_tokens, 900);
        assert_eq!(usage.total_input(), 1000);
    }

    /// Some servers report `stop` even while emitting tool calls; the loop must
    /// still run them or the turn silently does nothing.
    #[test]
    fn tool_calls_override_a_plain_stop_finish_reason() {
        let value = json!({
            "choices": [{"finish_reason": "stop", "message": {
                "tool_calls": [{"id": "call_1", "type": "function",
                    "function": {"name": "list_tasks", "arguments": "{}"}}]
            }}]
        });
        assert_eq!(
            parse_completion(&value).unwrap().stop_reason,
            StopReason::ToolUse
        );
    }

    #[test]
    fn malformed_tool_arguments_degrade_to_an_empty_object() {
        let value = json!({
            "choices": [{"finish_reason": "tool_calls", "message": {
                "tool_calls": [{"id": "call_1", "type": "function",
                    "function": {"name": "x", "arguments": "{not json"}}]
            }}]
        });
        let parsed = parse_completion(&value).unwrap();
        assert_eq!(parsed.tool_calls()[0].2, &json!({}));
    }

    #[test]
    fn finish_reasons_map_onto_stop_reasons() {
        assert_eq!(parse_finish_reason("stop"), StopReason::EndTurn);
        assert_eq!(parse_finish_reason("tool_calls"), StopReason::ToolUse);
        assert_eq!(parse_finish_reason("length"), StopReason::MaxTokens);
        assert_eq!(parse_finish_reason("content_filter"), StopReason::Refusal);
        assert!(matches!(parse_finish_reason("novel"), StopReason::Other(_)));
    }

    // ------------------------------------------------------------- streaming

    async fn assemble(events: &[Value]) -> CompletionResponse {
        let mut accumulator = Accumulator::default();
        for event in events {
            accumulator.absorb(event, &None).await;
        }
        accumulator.finish()
    }

    #[tokio::test]
    async fn a_streamed_turn_assembles_text_and_a_fragmented_tool_call() {
        let response = assemble(&[
            json!({"model": "deepseek-chat", "choices": [{"delta": {"content": "Hel"}}]}),
            json!({"choices": [{"delta": {"content": "lo"}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{
                "index": 0, "id": "call_1", "type": "function",
                "function": {"name": "claim_task", "arguments": ""}
            }]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{
                "index": 0, "function": {"arguments": "{\"task\":"}
            }]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{
                "index": 0, "function": {"arguments": "\"task_1\"}"}
            }]}}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
            json!({"choices": [], "usage": {"prompt_tokens": 100, "completion_tokens": 20}}),
        ])
        .await;

        assert_eq!(response.model, "deepseek-chat");
        assert_eq!(response.text(), "Hello");
        assert_eq!(response.stop_reason, StopReason::ToolUse);
        assert_eq!(response.tool_calls()[0].2["task"], "task_1");
        assert_eq!(response.usage.output_tokens, 20);
    }

    /// Concurrent calls are correlated only by `index`; mixing them up would
    /// send one tool the other's arguments.
    #[tokio::test]
    async fn parallel_tool_calls_stay_separate_by_index() {
        let response = assemble(&[
            json!({"choices": [{"delta": {"tool_calls": [
                {"index": 0, "id": "call_a", "function": {"name": "first", "arguments": "{\"a\":"}},
                {"index": 1, "id": "call_b", "function": {"name": "second", "arguments": "{\"b\":"}}
            ]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [
                {"index": 1, "function": {"arguments": "2}"}},
                {"index": 0, "function": {"arguments": "1}"}}
            ]}}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
        ])
        .await;

        let calls = response.tool_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1, "first");
        assert_eq!(calls[0].2["a"], 1);
        assert_eq!(calls[1].1, "second");
        assert_eq!(calls[1].2["b"], 2);
    }

    #[tokio::test]
    async fn streamed_reasoning_is_kept_out_of_the_answer() {
        let response = assemble(&[
            json!({"choices": [{"delta": {"reasoning_content": "thinking..."}}]}),
            json!({"choices": [{"delta": {"content": "the answer"}}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        ])
        .await;
        assert_eq!(response.text(), "the answer");
        assert!(matches!(response.content[0], ContentBlock::Thinking { .. }));
    }

    #[tokio::test]
    async fn a_tool_call_start_is_announced_once_it_has_an_id_and_a_name() {
        let (tx, mut rx) = mpsc::channel(16);
        let sink = Some(tx);
        let mut accumulator = Accumulator::default();
        for event in [
            json!({"choices": [{"delta": {"tool_calls": [
                {"index": 0, "id": "call_1", "function": {"name": "claim_task"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [
                {"index": 0, "function": {"arguments": "{}"}}]}}]}),
        ] {
            accumulator.absorb(&event, &sink).await;
        }
        drop(sink);

        let mut starts = 0;
        while let Some(event) = rx.recv().await {
            if matches!(event, StreamEvent::ToolUseStart { .. }) {
                starts += 1;
            }
        }
        assert_eq!(starts, 1, "the start must be announced exactly once");
    }

    #[tokio::test]
    async fn a_usage_only_final_chunk_does_not_lose_the_stop_reason() {
        let response = assemble(&[
            json!({"choices": [{"delta": {"content": "done"}, "finish_reason": "stop"}]}),
            json!({"choices": [], "usage": {"prompt_tokens": 5, "completion_tokens": 1}}),
        ])
        .await;
        assert_eq!(response.stop_reason, StopReason::EndTurn);
        assert_eq!(response.usage.output_tokens, 1);
    }
}
