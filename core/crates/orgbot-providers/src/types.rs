//! The provider-neutral request and response shapes.
//!
//! Every backend is normalised to this vocabulary so the agent loop never
//! branches on which model a bot happens to use. The vocabulary follows the
//! Anthropic content-block model — a turn is a list of typed blocks rather than
//! a string — because that is the shape tool use actually has, and flattening
//! it to text loses the tool-call/tool-result pairing that a room of bots
//! depends on.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImageSource {
    File { path: std::path::PathBuf },
    Base64 { data: String },
}

/// One typed piece of a turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Image {
        media_type: String,
        source: ImageSource,
    },
    /// Ordinary prose.
    Text {
        /// The text.
        text: String,
    },
    /// The model's reasoning, where the backend exposes it.
    Thinking {
        /// The reasoning text, which may be a summary rather than the raw chain.
        text: String,
    },
    /// The model asking to call a tool.
    ToolUse {
        /// Correlation id; the matching result must echo it.
        id: String,
        /// Tool name.
        name: String,
        /// Arguments, as validated JSON.
        input: serde_json::Value,
    },
    /// The result of a tool call, sent back on the next turn.
    ToolResult {
        /// The `ToolUse::id` this answers.
        tool_use_id: String,
        /// The result, rendered for the model.
        content: String,
        /// Whether the call failed. A failed call still needs a result block —
        /// dropping it leaves the model with a dangling call.
        is_error: bool,
    },
}

impl ContentBlock {
    /// Convenience constructor for a text block.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// The text carried by this block, if it carries any.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } | Self::Thinking { text } => Some(text),
            _ => None,
        }
    }

    /// Whether this block is a tool call.
    pub const fn is_tool_use(&self) -> bool {
        matches!(self, Self::ToolUse { .. })
    }
}

/// Who produced a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The caller.
    User,
    /// The model.
    Assistant,
}

/// One turn of a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    /// Who is speaking.
    pub role: Role,
    /// The blocks making up the turn.
    pub content: Vec<ContentBlock>,
}

impl Turn {
    /// A user turn carrying a single text block.
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::text(text)],
        }
    }

    /// An assistant turn carrying a single text block.
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::text(text)],
        }
    }

    /// A user turn carrying tool results.
    ///
    /// All results for one assistant turn must travel together in a single
    /// user turn: splitting them across several turns teaches the model to
    /// stop issuing parallel calls.
    pub fn tool_results(results: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::User,
            content: results,
        }
    }

    /// Every tool call in this turn.
    pub fn tool_uses(&self) -> impl Iterator<Item = &ContentBlock> {
        self.content.iter().filter(|b| b.is_tool_use())
    }

    /// The concatenated text of this turn, thinking excluded.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A tool the model may call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    /// The name the model calls.
    pub name: String,
    /// What the tool does and when to reach for it.
    pub description: String,
    /// A JSON Schema object describing the arguments.
    pub input_schema: serde_json::Value,
}

/// How much thinking budget to spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    /// Cheap: routine work and subagents.
    Low,
    /// Balanced.
    Medium,
    /// The default for work that matters.
    High,
    /// Best for most coding and agentic work.
    XHigh,
    /// Correctness over cost.
    Max,
}

impl Effort {
    /// The wire value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// A request for one model turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionRequest {
    /// The model id.
    pub model: String,
    /// Standing instructions. Stable across a room's turns so it caches well.
    pub system: String,
    /// The conversation so far.
    pub messages: Vec<Turn>,
    /// Tools available this turn.
    pub tools: Vec<ToolDef>,
    /// Output ceiling.
    pub max_tokens: u32,
    /// Thinking depth, where the backend supports it.
    pub effort: Option<Effort>,
    /// Whether to stream. Required for large `max_tokens` on some backends to
    /// avoid an HTTP timeout mid-response.
    pub stream: bool,
    /// A unix socket where the room's MCP bridge is listening, for agent-CLI
    /// bots that act on the board mid-episode.
    pub mcp_socket: Option<String>,
}

impl CompletionRequest {
    /// A request with the defaults this project uses: streaming on, high
    /// effort, and an output ceiling large enough that a bot's turn is not
    /// truncated mid-thought.
    pub fn new(model: impl Into<String>, system: impl Into<String>, messages: Vec<Turn>) -> Self {
        Self {
            model: model.into(),
            system: system.into(),
            messages,
            tools: Vec::new(),
            max_tokens: DEFAULT_MAX_TOKENS,
            effort: Some(Effort::High),
            stream: true,
            mcp_socket: None,
        }
    }

    /// Builder: attach the tool set.
    pub fn with_tools(mut self, tools: Vec<ToolDef>) -> Self {
        self.tools = tools;
        self
    }

    /// Builder: set the effort level.
    pub fn with_effort(mut self, effort: Effort) -> Self {
        self.effort = Some(effort);
        self
    }

    /// Builder: point an agent-CLI bot at its room's MCP bridge socket.
    pub fn with_mcp_socket(mut self, socket: Option<String>) -> Self {
        self.mcp_socket = socket;
        self
    }
}

/// Default output ceiling: generous enough for a full bot turn, small enough
/// to stay well inside a streamed response.
pub const DEFAULT_MAX_TOKENS: u32 = 16_000;

/// Why the model stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model finished its turn.
    EndTurn,
    /// The model wants to call tools; the loop must run them and continue.
    ToolUse,
    /// The output ceiling was hit. The turn is truncated.
    MaxTokens,
    /// A safety classifier declined.
    Refusal,
    /// A stop sequence matched.
    StopSequence,
    /// Something the adapter did not recognise.
    Other(String),
}

impl StopReason {
    /// Whether the agent loop should run tools and take another turn.
    pub const fn wants_another_turn(&self) -> bool {
        matches!(self, Self::ToolUse)
    }

    /// Whether the turn ended badly enough to surface to a human.
    pub const fn is_problem(&self) -> bool {
        matches!(self, Self::MaxTokens | Self::Refusal)
    }
}

/// Token accounting for one turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens read.
    pub input_tokens: u32,
    /// Tokens written.
    pub output_tokens: u32,
    /// Tokens served from the prompt cache.
    pub cache_read_tokens: u32,
    /// Tokens written into the prompt cache.
    pub cache_write_tokens: u32,
}

impl Usage {
    /// Fold another turn's usage into this one.
    pub fn add(&mut self, other: Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
    }

    /// Total tokens billed as input, cache reads included.
    pub const fn total_input(&self) -> u32 {
        self.input_tokens + self.cache_read_tokens
    }
}

/// One completed model turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionResponse {
    /// The model that actually answered, which may differ from the one asked
    /// for when a server-side fallback fired.
    pub model: String,
    /// The blocks the model produced.
    pub content: Vec<ContentBlock>,
    /// Why it stopped.
    pub stop_reason: StopReason,
    /// Token accounting.
    pub usage: Usage,
}

impl CompletionResponse {
    /// The response as an assistant turn, ready to append to the history.
    pub fn as_turn(&self) -> Turn {
        Turn {
            role: Role::Assistant,
            content: self.content.clone(),
        }
    }

    /// The concatenated text, thinking excluded.
    pub fn text(&self) -> String {
        self.as_turn().text()
    }

    /// Every tool call the model asked for.
    pub fn tool_calls(&self) -> Vec<(&str, &str, &serde_json::Value)> {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.as_str(), name.as_str(), input))
                }
                _ => None,
            })
            .collect()
    }
}

/// An incremental update while a turn streams.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// An interrupted API response is being retried; partial output is discarded.
    Retry { attempt: u32, max_retries: u32 },
    /// More prose.
    TextDelta {
        /// The appended text.
        text: String,
    },
    /// More reasoning.
    ThinkingDelta {
        /// The appended reasoning.
        text: String,
    },
    /// A tool call has begun; arguments follow.
    ToolUseStart {
        /// Correlation id.
        id: String,
        /// Tool name.
        name: String,
    },
    /// The turn finished.
    Done {
        /// Why it stopped.
        stop_reason: StopReason,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turns_text_excludes_thinking_and_tool_blocks() {
        let turn = Turn {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    text: "internal".into(),
                },
                ContentBlock::text("visible"),
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "read".into(),
                    input: serde_json::json!({}),
                },
            ],
        };
        assert_eq!(turn.text(), "visible");
        assert_eq!(turn.tool_uses().count(), 1);
    }

    #[test]
    fn tool_calls_are_extracted_with_their_arguments() {
        let response = CompletionResponse {
            model: "claude-opus-5".into(),
            content: vec![ContentBlock::ToolUse {
                id: "toolu_1".into(),
                name: "claim_task".into(),
                input: serde_json::json!({"task": "task_abc"}),
            }],
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
        };
        let calls = response.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "claim_task");
        assert_eq!(calls[0].2["task"], "task_abc");
        assert!(response.stop_reason.wants_another_turn());
    }

    #[test]
    fn only_tool_use_asks_for_another_turn() {
        assert!(StopReason::ToolUse.wants_another_turn());
        for done in [
            StopReason::EndTurn,
            StopReason::MaxTokens,
            StopReason::Refusal,
            StopReason::StopSequence,
        ] {
            assert!(!done.wants_another_turn(), "{done:?}");
        }
    }

    #[test]
    fn truncation_and_refusal_are_flagged_as_problems() {
        assert!(StopReason::MaxTokens.is_problem());
        assert!(StopReason::Refusal.is_problem());
        assert!(!StopReason::EndTurn.is_problem());
    }

    #[test]
    fn usage_accumulates_across_turns() {
        let mut total = Usage::default();
        total.add(Usage {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 100,
            cache_write_tokens: 0,
        });
        total.add(Usage {
            input_tokens: 3,
            output_tokens: 7,
            cache_read_tokens: 50,
            cache_write_tokens: 2,
        });
        assert_eq!(total.input_tokens, 13);
        assert_eq!(total.output_tokens, 12);
        assert_eq!(total.total_input(), 13 + 150);
    }

    #[test]
    fn requests_default_to_streaming_at_high_effort() {
        let request = CompletionRequest::new("claude-opus-5", "be helpful", vec![]);
        assert!(request.stream);
        assert_eq!(request.effort, Some(Effort::High));
        assert_eq!(request.max_tokens, DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn content_blocks_survive_a_json_round_trip() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "toolu_1".into(),
            content: "ok".into(),
            is_error: false,
        };
        let json = serde_json::to_string(&block).unwrap();
        assert_eq!(serde_json::from_str::<ContentBlock>(&json).unwrap(), block);
    }
}
