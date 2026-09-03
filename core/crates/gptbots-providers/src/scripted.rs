//! A deterministic provider with no model behind it.
//!
//! Two uses. Rule-based roles — a schema validator, a policy gate — do not need
//! a model and should not cost one. And every test above this crate needs bot
//! turns it can predict, which is impossible against a live model.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::error::Result;
use crate::provider::{emit, Provider};
use crate::types::{
    CompletionRequest, CompletionResponse, ContentBlock, StopReason, StreamEvent, Usage,
};

/// Replays a fixed sequence of responses.
#[derive(Debug)]
pub struct ScriptedProvider {
    name: String,
    /// Remaining responses. Guarded by a mutex because [`Provider`] takes
    /// `&self` — the trait is shared across bot turns.
    responses: Mutex<VecDeque<CompletionResponse>>,
    /// Returned once the script runs out, instead of failing the turn.
    fallback: CompletionResponse,
    /// Requests seen so far, so tests can assert on what was actually sent.
    seen: Mutex<Vec<CompletionRequest>>,
}

impl ScriptedProvider {
    /// A provider that replays `responses` in order.
    pub fn new(responses: Vec<CompletionResponse>) -> Self {
        Self {
            name: "scripted".into(),
            responses: Mutex::new(responses.into()),
            // Deliberately empty: an exhausted script must register as a turn
            // that achieved nothing, so a run ends as stalled instead of
            // chattering until the turn budget is gone.
            fallback: CompletionResponse {
                model: "scripted".into(),
                content: Vec::new(),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
            seen: Mutex::new(Vec::new()),
        }
    }

    /// A provider that always answers with the same text.
    pub fn always_says(text: impl Into<String>) -> Self {
        let mut provider = Self::new(Vec::new());
        provider.fallback = CompletionResponse {
            model: "scripted".into(),
            content: vec![ContentBlock::text(text)],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        provider
    }

    /// Queue one more response.
    pub fn push(&self, response: CompletionResponse) {
        self.responses
            .lock()
            .expect("scripted provider lock")
            .push_back(response);
    }

    /// The requests this provider has been asked to serve.
    pub fn requests(&self) -> Vec<CompletionRequest> {
        self.seen.lock().expect("scripted provider lock").clone()
    }

    /// How many turns are still scripted.
    pub fn remaining(&self) -> usize {
        self.responses.lock().expect("scripted provider lock").len()
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> Result<CompletionResponse> {
        self.seen
            .lock()
            .expect("scripted provider lock")
            .push(request);

        let response = self
            .responses
            .lock()
            .expect("scripted provider lock")
            .pop_front()
            .unwrap_or_else(|| self.fallback.clone());

        // Replay through the sink so streaming consumers behave the same way
        // against a script as against a live model.
        for block in &response.content {
            match block {
                ContentBlock::Text { text } => {
                    emit(&sink, StreamEvent::TextDelta { text: text.clone() }).await;
                }
                ContentBlock::Thinking { text } => {
                    emit(&sink, StreamEvent::ThinkingDelta { text: text.clone() }).await;
                }
                ContentBlock::ToolUse { id, name, .. } => {
                    emit(
                        &sink,
                        StreamEvent::ToolUseStart {
                            id: id.clone(),
                            name: name.clone(),
                        },
                    )
                    .await;
                }
                ContentBlock::ToolResult { .. } => {}
            }
        }
        emit(
            &sink,
            StreamEvent::Done {
                stop_reason: response.stop_reason.clone(),
            },
        )
        .await;
        Ok(response)
    }
}

/// Build a response that calls one tool.
pub fn tool_call(id: &str, name: &str, input: serde_json::Value) -> CompletionResponse {
    CompletionResponse {
        model: "scripted".into(),
        content: vec![ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }],
        stop_reason: StopReason::ToolUse,
        usage: Usage::default(),
    }
}

/// Build a response that says something and stops.
pub fn says(text: &str) -> CompletionResponse {
    CompletionResponse {
        model: "scripted".into(),
        content: vec![ContentBlock::text(text)],
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CompletionRequest;

    fn request() -> CompletionRequest {
        CompletionRequest::new("scripted", "be helpful", vec![])
    }

    #[tokio::test]
    async fn responses_are_replayed_in_order() {
        let provider = ScriptedProvider::new(vec![says("first"), says("second")]);
        assert_eq!(
            provider.complete(request(), None).await.unwrap().text(),
            "first"
        );
        assert_eq!(
            provider.complete(request(), None).await.unwrap().text(),
            "second"
        );
    }

    #[tokio::test]
    async fn an_exhausted_script_ends_the_turn_saying_nothing() {
        let provider = ScriptedProvider::new(vec![]);
        let response = provider.complete(request(), None).await.unwrap();
        assert_eq!(response.stop_reason, StopReason::EndTurn);
        assert!(
            response.text().is_empty(),
            "an exhausted script must not read as progress"
        );
    }

    #[tokio::test]
    async fn requests_are_recorded_for_assertions() {
        let provider = ScriptedProvider::always_says("ok");
        provider.complete(request(), None).await.unwrap();
        let seen = provider.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].system, "be helpful");
    }

    #[tokio::test]
    async fn a_scripted_turn_still_streams_through_the_sink() {
        let provider = ScriptedProvider::new(vec![tool_call(
            "t1",
            "claim_task",
            serde_json::json!({"task": "task_1"}),
        )]);
        let (tx, mut rx) = mpsc::channel(8);
        provider.complete(request(), Some(tx)).await.unwrap();

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert!(matches!(events[0], StreamEvent::ToolUseStart { .. }));
        assert!(matches!(
            events.last().unwrap(),
            StreamEvent::Done {
                stop_reason: StopReason::ToolUse
            }
        ));
    }
}
