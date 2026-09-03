//! One bot, and the loop that gives it a turn.
//!
//! A turn is: read what changed, decide, call tools until there is nothing left
//! to call, then stop. The loop is bounded by the bot's own autonomy setting,
//! so a model that gets stuck calling tools in a circle costs one bot's budget
//! rather than the whole room's.

use std::sync::Arc;

use gptbots_engine::{Command, Engine};
use gptbots_proto::{AgentId, AgentSpec, Seq};
use gptbots_providers::types::{CompletionRequest, ContentBlock, StopReason, ToolDef, Turn, Usage};
use gptbots_providers::{Provider, ProviderError, StreamEvent};
use tokio::sync::mpsc;

use crate::prompt;

/// What one turn accomplished.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnReport {
    /// Which bot acted.
    pub agent: AgentId,
    /// Tool calls executed.
    pub tool_calls: usize,
    /// How many of those were rejected.
    pub tool_errors: usize,
    /// What the bot said, if anything.
    pub said: Option<String>,
    /// Model round trips consumed.
    pub steps: u32,
    /// Tokens used across the whole turn.
    pub usage: Usage,
    /// Why the last model call stopped.
    pub stop_reason: StopReason,
}

impl TurnReport {
    /// Whether the turn actually changed anything in the room.
    ///
    /// The orchestrator uses this to notice a room that is awake but going
    /// nowhere, which is otherwise indistinguishable from one that is working.
    pub fn made_progress(&self) -> bool {
        self.tool_calls > self.tool_errors || self.said.is_some()
    }
}

/// A bot: its configuration, its backend, and its memory of the room.
#[derive(Debug)]
pub struct Bot {
    /// The bot's configuration.
    pub spec: AgentSpec,
    provider: Arc<dyn Provider>,
    /// The bot's conversation with its model, across turns.
    history: Vec<Turn>,
    /// The last event this bot has been shown.
    last_seen: Seq,
    /// Cached tool definitions; they depend only on the spec.
    tools: Vec<ToolDef>,
    /// The model id to call.
    model: String,
}

impl Bot {
    /// Seat a bot with a backend.
    pub fn new(spec: AgentSpec, provider: Arc<dyn Provider>) -> Self {
        let tools = gptbots_tools::definitions_for(&spec);
        let model = spec.provider.model().to_string();
        Self {
            spec,
            provider,
            history: Vec::new(),
            last_seen: Seq::ZERO,
            tools,
            model,
        }
    }

    /// The bot's id.
    pub fn id(&self) -> AgentId {
        self.spec.id
    }

    /// How many turns of conversation this bot is carrying.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// The last log position this bot has been shown.
    ///
    /// The scheduler reads this to tell whether a message posted since the
    /// bot's last turn is addressed to it and still unanswered.
    pub fn last_seen(&self) -> Seq {
        self.last_seen
    }

    /// Give the bot a turn.
    ///
    /// `sink` receives streaming deltas for the UI; pass `None` when nobody is
    /// watching. Provider failures are reported to the engine, which faults a
    /// bot that keeps failing and hands its work back.
    pub async fn take_turn(
        &mut self,
        engine: &mut Engine,
        sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> Result<TurnReport, ProviderError> {
        let system = prompt::system_prompt(engine, &self.spec);
        let situation = prompt::situation(engine, self.spec.id, self.last_seen);
        self.last_seen = engine.head();
        self.history.push(Turn::user(situation));

        let mut report = TurnReport {
            agent: self.spec.id,
            tool_calls: 0,
            tool_errors: 0,
            said: None,
            steps: 0,
            usage: Usage::default(),
            stop_reason: StopReason::EndTurn,
        };
        let mut posted_to_chat = false;

        let max_steps = self.spec.autonomy.max_turns_per_task.max(1);
        for _ in 0..max_steps {
            self.trim_history();

            let request = CompletionRequest::new(
                self.model.clone(),
                system.clone(),
                self.history.clone(),
            )
            .with_tools(self.tools.clone());

            let response = match self.provider.complete(request, sink.clone()).await {
                Ok(response) => response,
                Err(error) => {
                    // Tell the engine, so repeated failures fault the bot and
                    // release whatever it is holding.
                    let _ = engine.apply(Command::ReportError {
                        agent: self.spec.id,
                        task: engine.active_tasks_of(self.spec.id).first().copied(),
                        error: error.to_string(),
                    });
                    return Err(error);
                }
            };

            report.steps += 1;
            report.usage.add(response.usage);
            report.stop_reason = response.stop_reason.clone();
            self.history.push(response.as_turn());

            let calls = response.tool_calls();
            if calls.is_empty() {
                let text = response.text();
                if !text.trim().is_empty() {
                    report.said = Some(text);
                }
                break;
            }

            // Every result for one assistant turn must travel back together;
            // splitting them teaches the model to stop calling tools in
            // parallel.
            let mut results = Vec::with_capacity(calls.len());
            for (id, name, input) in calls {
                if name == "post_message" {
                    posted_to_chat = true;
                }
                let outcome = gptbots_tools::dispatch(engine, self.spec.id, name, input);
                report.tool_calls += 1;
                if outcome.is_error {
                    report.tool_errors += 1;
                }
                results.push(ContentBlock::ToolResult {
                    tool_use_id: id.to_string(),
                    content: outcome.content,
                    is_error: outcome.is_error,
                });
            }
            self.history.push(Turn::tool_results(results));
        }

        // A bot that ended with prose and never called post_message still meant
        // to say it — this is a group chat, and silent work is invisible to the
        // other bots. Posted as an ordinary statement, so it does not wake the
        // room; anything needing an answer goes through post_message with
        // intent "ask".
        if let Some(said) = &report.said {
            if !posted_to_chat {
                let message =
                    gptbots_proto::ChatMessage::say(engine.room_id(), self.spec.id, said.clone());
                let _ = engine.apply(Command::PostMessage {
                    message: Box::new(message),
                });
            }
        }

        let _ = engine.apply(Command::RecordTurn {
            agent: self.spec.id,
        });
        Ok(report)
    }

    /// Keep the conversation from growing without bound.
    ///
    /// Cutting is only safe at a boundary where the remaining history does not
    /// begin with tool results — an orphaned `tool_result` whose `tool_use` has
    /// been trimmed away is rejected by every backend.
    fn trim_history(&mut self) {
        if self.history.len() <= MAX_HISTORY_TURNS {
            return;
        }
        let target = self.history.len() - KEEP_HISTORY_TURNS;
        let cut = self.history[target..]
            .iter()
            .position(is_safe_cut_point)
            .map(|offset| target + offset);

        if let Some(cut) = cut {
            self.history.drain(..cut);
        }
    }
}

/// Whether history may be cut so that this turn becomes the first one.
fn is_safe_cut_point(turn: &Turn) -> bool {
    use gptbots_providers::types::Role;
    turn.role == Role::User
        && !turn
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
}

/// Trim once the conversation passes this many turns.
const MAX_HISTORY_TURNS: usize = 60;

/// Turns retained after a trim.
const KEEP_HISTORY_TURNS: usize = 30;

#[cfg(test)]
mod tests {
    use super::*;
    use gptbots_providers::types::Role;
    use gptbots_providers::ScriptedProvider;
    use serde_json::json;

    fn spec() -> AgentSpec {
        AgentSpec::new(
            "w1",
            "Worker",
            gptbots_proto::agent::ProviderRef::Scripted {
                script: "test".into(),
            },
        )
        .with_roles(["writer"])
    }

    fn bot() -> Bot {
        Bot::new(spec(), Arc::new(ScriptedProvider::always_says("ok")))
    }

    #[test]
    fn a_user_turn_without_tool_results_is_a_safe_cut_point() {
        assert!(is_safe_cut_point(&Turn::user("what changed?")));
    }

    #[test]
    fn a_turn_of_tool_results_is_not_a_safe_cut_point() {
        let turn = Turn::tool_results(vec![ContentBlock::ToolResult {
            tool_use_id: "t1".into(),
            content: "done".into(),
            is_error: false,
        }]);
        assert!(
            !is_safe_cut_point(&turn),
            "cutting here orphans the tool_result and every backend rejects it"
        );
    }

    #[test]
    fn an_assistant_turn_is_not_a_safe_cut_point() {
        assert!(!is_safe_cut_point(&Turn::assistant("on it")));
    }

    #[test]
    fn short_histories_are_left_alone() {
        let mut bot = bot();
        for _ in 0..10 {
            bot.history.push(Turn::user("tick"));
        }
        bot.trim_history();
        assert_eq!(bot.history_len(), 10);
    }

    #[test]
    fn trimming_never_orphans_a_tool_result() {
        let mut bot = bot();
        // A long history of complete call/result cycles.
        for _ in 0..40 {
            bot.history.push(Turn::user("what changed?"));
            bot.history.push(Turn {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "list_tasks".into(),
                    input: json!({}),
                }],
            });
            bot.history.push(Turn::tool_results(vec![ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: "nothing".into(),
                is_error: false,
            }]));
        }
        bot.trim_history();

        assert!(bot.history_len() < 120, "history should have been trimmed");
        assert!(
            is_safe_cut_point(&bot.history[0]),
            "the surviving history must start on a clean boundary"
        );

        // Every tool result must still have its call ahead of it.
        let mut open: Vec<String> = Vec::new();
        for turn in &bot.history {
            for block in &turn.content {
                match block {
                    ContentBlock::ToolUse { id, .. } => open.push(id.clone()),
                    ContentBlock::ToolResult { tool_use_id, .. } => assert!(
                        open.contains(tool_use_id),
                        "orphaned tool_result {tool_use_id}"
                    ),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn progress_needs_a_successful_action_or_something_said() {
        let base = TurnReport {
            agent: AgentId::new(),
            tool_calls: 0,
            tool_errors: 0,
            said: None,
            steps: 1,
            usage: Usage::default(),
            stop_reason: StopReason::EndTurn,
        };
        assert!(!base.made_progress(), "an empty turn is not progress");

        assert!(TurnReport {
            said: Some("done".into()),
            ..base.clone()
        }
        .made_progress());

        assert!(TurnReport {
            tool_calls: 1,
            ..base.clone()
        }
        .made_progress());

        assert!(
            !TurnReport {
                tool_calls: 2,
                tool_errors: 2,
                ..base
            }
            .made_progress(),
            "a turn of nothing but rejected calls changed nothing"
        );
    }
}
