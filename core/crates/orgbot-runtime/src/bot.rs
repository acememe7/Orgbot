//! One bot, and the loop that gives it a turn.
//!
//! A turn is: read what changed, decide, call tools until there is nothing left
//! to call, then stop. The loop is bounded by the bot's own autonomy setting,
//! so a model that gets stuck calling tools in a circle costs one bot's budget
//! rather than the whole room's.

use std::sync::Arc;

use orgbot_engine::Command;

use crate::shared::SharedEngine;
use orgbot_proto::agent::ProviderRef;
use orgbot_proto::{AgentId, AgentSpec, Seq};
use orgbot_providers::types::{
    CompletionRequest, CompletionResponse, ContentBlock, StopReason, ToolDef, Turn, Usage,
};
use orgbot_providers::{Provider, ProviderError, StreamEvent};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::prompt;

/// The provider conversation retained between app launches; no credentials.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BotMemory {
    history: Vec<Turn>,
    last_seen: Seq,
}

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
    /// Inbound MCP calls from the agent's own tool loop, delivered while a
    /// turn is in flight. Only agent-CLI bots with the room bridge enabled
    /// carry one.
    mcp_rx: Option<mpsc::Receiver<crate::mcp::McpCall>>,
    /// The unix socket where this bot's room-MCP bridge listens; handed to
    /// the CLI so its tool loop can reach the board.
    mcp_socket: Option<std::path::PathBuf>,
    workspace: Option<crate::workspace::Workspace>,
    pub(crate) attachments_root: Option<std::path::PathBuf>,
    pub(crate) input_interrupt: CancellationToken,
}

impl Bot {
    pub fn memory(&self) -> BotMemory {
        BotMemory {
            history: self.history.clone(),
            last_seen: self.last_seen,
        }
    }

    pub fn restore_memory(&mut self, memory: BotMemory) {
        self.history = memory.history;
        self.last_seen = memory.last_seen;
    }
    /// Seat a bot with a backend.
    pub fn new(spec: AgentSpec, provider: Arc<dyn Provider>) -> Self {
        let tools = orgbot_tools::definitions_for(&spec);
        let model = spec.provider.model().to_string();
        Self {
            spec,
            provider,
            history: Vec::new(),
            last_seen: Seq::ZERO,
            tools,
            model,
            mcp_rx: None,
            mcp_socket: None,
            workspace: None,
            attachments_root: None,
            input_interrupt: CancellationToken::new(),
        }
    }

    pub fn attach_workspace(&mut self, workspace: crate::workspace::Workspace) {
        self.tools.extend(workspace.definitions());
        self.workspace = Some(workspace);
    }

    /// Wire this bot to the room MCP bridge: while a turn runs, inbound
    /// [crate::mcp::McpCall]s are dispatched against the engine in-band.
    pub fn attach_mcp(
        &mut self,
        rx: mpsc::Receiver<crate::mcp::McpCall>,
        socket: std::path::PathBuf,
    ) {
        self.mcp_socket = Some(socket);
        self.mcp_rx = Some(rx);
    }

    /// The unix socket of this bot's room-MCP bridge, if it has one.
    pub fn mcp_socket(&self) -> Option<&std::path::Path> {
        self.mcp_socket.as_deref()
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
        engine: &SharedEngine,
        sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> Result<TurnReport, ProviderError> {
        let mut system = engine.with(|engine| prompt::system_prompt(engine, &self.spec));
        if let Some(workspace) = &self.workspace {
            system.push_str(&workspace.instruction());
        }
        // A cancelled turn can leave unanswered tool uses. Discard that incomplete
        // assistant turn; the next situation reports the actual committed board.
        if self.history.last().is_some_and(|turn| {
            turn.content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
        }) {
            self.history.pop();
        }
        let previous_seen = self.last_seen;
        // One lock for the whole prompt, so what the bot is told about the
        // board is a single moment rather than several stitched together.
        let (turn, head) = engine.with(|engine| {
            let mut turn = Turn::user(prompt::situation(engine, self.spec.id, self.last_seen));
            turn.content.extend(crate::attachments::context(
                engine,
                self.last_seen,
                self.attachments_root.as_deref(),
            ));
            (turn, engine.head())
        });
        self.last_seen = head;
        self.history.push(turn);

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

            let mut request =
                CompletionRequest::new(self.model.clone(), system.clone(), self.history.clone())
                    .with_tools(self.tools.clone());
            request = request.with_mcp_socket(
                self.mcp_socket
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
            );

            let response = match self.mcp_rx.as_mut() {
                Some(rx) => {
                    let (response, mcp_ok, mcp_err) = complete_with_mcp(
                        &self.provider,
                        &self.tools,
                        engine,
                        self.spec.id,
                        request,
                        sink.clone(),
                        rx,
                    )
                    .await;
                    report.tool_calls += mcp_ok;
                    report.tool_errors += mcp_err;
                    response
                }
                // CLI providers execute their own tools inside complete(); only
                // API waits are safe to abandon for new human input. Workspace
                // calls below finish and retain their results before yielding.
                None if !matches!(self.spec.provider, ProviderRef::CliAgent { .. }) => {
                    tokio::select! {
                        biased;
                        _ = self.input_interrupt.cancelled() => {
                            self.last_seen = previous_seen;
                            if report.steps == 0 {
                                self.history.pop();
                            }
                            report.stop_reason = StopReason::Other("human_input".into());
                            if let Some(tx) = &sink {
                                let _ = tx.send(StreamEvent::Done {
                                    stop_reason: report.stop_reason.clone(),
                                }).await;
                            }
                            return Ok(report);
                        }
                        response = complete_api(&self.provider, request, sink.clone()) => response,
                    }
                }
                None => self.provider.complete(request, sink.clone()).await,
            };
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    // Tell the engine, so repeated failures fault the bot and
                    // release whatever it is holding.
                    let _ = engine.with(|engine| {
                        let task = engine.active_tasks_of(self.spec.id).first().copied();
                        engine.apply(Command::ReportError {
                        agent: self.spec.id,
                        task,
                        error: error.to_string(),
                        // A model that does not exist, or a credential the
                        // backend refuses, will answer the same way next turn.
                        permanent: !error.is_retryable(),
                        })
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
                let outcome = if name.starts_with("workspace_") {
                    match &self.workspace {
                        Some(workspace) => workspace.call(self.spec.id, name, input).await,
                        None => orgbot_tools::ToolOutcome {
                            content: "No workspace attached".into(),
                            is_error: true,
                        },
                    }
                } else {
                    engine.with(|engine| orgbot_tools::dispatch(engine, self.spec.id, name, input))
                };
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
        engine.with(|engine| {
            if let Some(said) = &report.said {
                if !posted_to_chat {
                    let message =
                        orgbot_proto::ChatMessage::say(engine.room_id(), self.spec.id, said.clone());
                    let _ = engine.apply(Command::PostMessage {
                        message: Box::new(message),
                    });
                }
            }
            let _ = engine.apply(Command::RecordTurn {
                agent: self.spec.id,
                usage: report.usage,
            });
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

/// Retry only the current API response. Nothing from an incomplete response
/// reaches history or tool execution, and prior tool results remain in request.
async fn complete_api(
    provider: &Arc<dyn Provider>,
    request: CompletionRequest,
    sink: Option<mpsc::Sender<StreamEvent>>,
) -> Result<CompletionResponse, ProviderError> {
    // ponytail: two reconnects per response; persistent failures still surface
    // through the room's existing error budget instead of retrying indefinitely.
    const MAX_RETRIES: u32 = 2;
    let mut attempt = 0;
    loop {
        match provider.complete(request.clone(), sink.clone()).await {
            Err(error) if attempt < MAX_RETRIES && error.is_retryable()
                && !matches!(error, ProviderError::RateLimited { .. }) => {
                attempt += 1;
                tracing::warn!(provider = provider.name(), attempt, %error, "reconnecting interrupted model response");
                if let Some(tx) = &sink {
                    let _ = tx.send(StreamEvent::Retry { attempt, max_retries: MAX_RETRIES }).await;
                }
                tokio::time::sleep(std::time::Duration::from_secs(1 << attempt)).await;
            }
            result => return result,
        }
    }
}

/// Whether history may be cut so that this turn becomes the first one.
fn is_safe_cut_point(turn: &Turn) -> bool {
    use orgbot_providers::types::Role;
    turn.role == Role::User
        && !turn
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
}

/// Run one provider turn while also serving inbound room-MCP calls.
///
/// Agent CLIs (claude -p, codex exec, grok -p) run their own tool loop inside
/// one [crate::mcp::McpSession]; every tools/call they make arrives here while
/// the provider future is still running and is dispatched against the engine
/// immediately, so the bot acts on the board mid-episode. Returns the provider
/// result plus how many MCP calls succeeded or were rejected, which the caller
/// folds into the turn report so a board action counts as progress.
async fn complete_with_mcp(
    provider: &Arc<dyn Provider>,
    tools: &[ToolDef],
    engine: &SharedEngine,
    actor: AgentId,
    request: CompletionRequest,
    sink: Option<mpsc::Sender<StreamEvent>>,
    rx: &mut mpsc::Receiver<crate::mcp::McpCall>,
) -> (Result<CompletionResponse, ProviderError>, usize, usize) {
    let mut session = crate::mcp::McpSession::new(tools.to_vec());
    let mut turn = std::pin::pin!(provider.complete(request, sink));
    let mut calls_ok = 0usize;
    let mut calls_err = 0usize;
    loop {
        tokio::select! {
            response = &mut turn => {
                // The model has finished, but the CLI may have calls already
                // queued. Serve them before returning: dropping the reply
                // channel leaves the client waiting on a request that never
                // comes back, and it concludes the server has died.
                while let Ok(call) = rx.try_recv() {
                    let reply = {
                        let mut dispatch = |name: &str, args: &serde_json::Value| {
                            let outcome = engine
                                .with(|engine| orgbot_tools::dispatch(engine, actor, name, args));
                            if outcome.is_error {
                                calls_err += 1;
                            } else {
                                calls_ok += 1;
                            }
                            outcome
                        };
                        session.handle(&call.request, &mut dispatch)
                    };
                    let _ = call.reply.send(reply);
                }
                return (response, calls_ok, calls_err);
            }
            inbound = rx.recv() => {
                let Some(call) = inbound else { continue };
                let reply = {
                    let mut dispatch = |name: &str, args: &serde_json::Value| {
                        let outcome =
                            engine.with(|engine| orgbot_tools::dispatch(engine, actor, name, args));
                        tracing::debug!(
                            tool = name,
                            error = outcome.is_error,
                            result = %outcome.content.chars().take(160).collect::<String>(),
                            "mcp tool call"
                        );
                        if outcome.is_error {
                            calls_err += 1;
                        } else {
                            calls_ok += 1;
                        }
                        outcome
                    };
                    session.handle(&call.request, &mut dispatch)
                };
                let _ = call.reply.send(reply);
            }
        }
    }
}

/// Trim once the conversation passes this many turns.
const MAX_HISTORY_TURNS: usize = 60;

/// Turns retained after a trim.
const KEEP_HISTORY_TURNS: usize = 30;

#[cfg(test)]
mod tests {
    use super::*;
    use orgbot_providers::types::Role;
    use orgbot_providers::ScriptedProvider;
    use serde_json::json;

    fn spec() -> AgentSpec {
        AgentSpec::new(
            "w1",
            "Worker",
            orgbot_proto::agent::ProviderRef::Scripted {
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
            bot.history
                .push(Turn::tool_results(vec![ContentBlock::ToolResult {
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

#[cfg(test)]
mod mcp_tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use orgbot_engine::{Command, Engine};

    use crate::shared::SharedEngine;
    use orgbot_proto::agent::ProviderRef;
    use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
    use orgbot_proto::AgentSpec;
    use orgbot_providers::types::{
        CompletionRequest, CompletionResponse, ContentBlock, StopReason, Usage,
    };
    use orgbot_providers::{Provider, ProviderError, StreamEvent};
    use tokio::sync::{mpsc, oneshot};

    use crate::bot::Bot;
    use crate::mcp::McpCall;

    /// A provider that signals when its turn starts and then blocks until told
    /// to finish, so the test can serve MCP calls mid-episode.
    #[derive(Debug)]
    struct GateProvider {
        started: Mutex<Option<oneshot::Sender<()>>>,
        release: Mutex<Option<oneshot::Receiver<()>>>,
    }

    #[async_trait]
    impl Provider for GateProvider {
        fn name(&self) -> &str {
            "gate"
        }

        async fn complete(
            &self,
            _request: CompletionRequest,
            _sink: Option<mpsc::Sender<StreamEvent>>,
        ) -> Result<CompletionResponse, ProviderError> {
            if let Some(started) = self.started.lock().unwrap().take() {
                let _ = started.send(());
            }
            let release = self.release.lock().unwrap().take();
            if let Some(rx) = release {
                let _ = rx.await;
            }
            Ok(CompletionResponse {
                model: "gate".into(),
                content: vec![ContentBlock::text("done")],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            })
        }
    }

    fn engine_with(spec: &AgentSpec) -> Engine {
        let mut engine = Engine::new(RoomSpec {
            name: "mcp-room".into(),
            goal: "prove in-band tools".into(),
            roster: vec![],
            completion: CompletionRule::Manual,
            limits: RoomLimits::default(),
        });
        engine
            .apply(Command::JoinAgent {
                spec: Box::new(spec.clone()),
            })
            .unwrap();
        engine.apply(Command::StartRoom).unwrap();
        engine
    }

    async fn rpc(tx: &mpsc::Sender<McpCall>, request: &str) -> Option<String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(McpCall {
            request: request.to_string(),
            reply: reply_tx,
        })
        .await
        .unwrap();
        reply_rx.await.unwrap()
    }

    #[tokio::test]
    async fn reconnects_are_bounded_and_human_input_cancels_the_backoff() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        #[derive(Debug, Default)]
        struct Disconnects(AtomicUsize);
        #[async_trait]
        impl Provider for Disconnects {
            fn name(&self) -> &str { "disconnected" }
            async fn complete(&self, _: CompletionRequest, _: Option<mpsc::Sender<StreamEvent>>) -> Result<CompletionResponse, ProviderError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Err(ProviderError::StreamInterrupted { provider: "test".into(), message: "peer disconnected".into() })
            }
        }
        let backend = Arc::new(Disconnects::default());
        let provider: Arc<dyn Provider> = backend.clone();
        let result = super::complete_api(&provider, CompletionRequest::new("test", "test", vec![]), None).await;
        assert!(matches!(result, Err(ProviderError::StreamInterrupted { .. })));
        assert_eq!(backend.0.load(Ordering::SeqCst), 3, "stop after two retries");

        backend.0.store(0, Ordering::SeqCst);
        let spec = AgentSpec::new("worker", "Worker", ProviderRef::Scripted { script: "test".into() });
        let engine = SharedEngine::new(engine_with(&spec));
        let mut bot = Bot::new(spec, provider);
        let interrupt = bot.input_interrupt.clone();
        let (tx, mut rx) = mpsc::channel(8);
        let running = tokio::spawn(async move { bot.take_turn(&engine, Some(tx)).await });
        assert!(matches!(rx.recv().await.unwrap(), StreamEvent::Retry { attempt: 1, .. }));
        interrupt.cancel();
        let report = tokio::time::timeout(std::time::Duration::from_millis(300), running).await.unwrap().unwrap().unwrap();
        assert_eq!(report.stop_reason, StopReason::Other("human_input".into()));
        assert_eq!(backend.0.load(Ordering::SeqCst), 1, "cancel the pending retry immediately");
    }

    #[tokio::test]
    async fn human_input_preserves_an_inflight_workspace_write_and_its_result() {
        use crate::workspace::{Workspace, WorkspaceSpec};
        use orgbot_providers::ScriptedProvider;
        let provider = Arc::new(ScriptedProvider::new(vec![CompletionResponse {
            model: "test".into(),
            content: vec![ContentBlock::ToolUse {
                id: "write1".into(),
                name: "workspace_write".into(),
                input: serde_json::json!({"path":"research.md", "content":"verified"}),
            }],
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
        }]));
        let spec = AgentSpec::new("writer", "Writer", ProviderRef::Scripted { script: "x".into() });
        let engine = SharedEngine::new(engine_with(&spec));
        let mut bot = Bot::new(spec, provider.clone());
        let token = bot.input_interrupt.clone();
        let (tx, mut calls) = mpsc::channel(2);
        bot.attach_workspace(Workspace::new(
            WorkspaceSpec { label: "test".into(), root: "/work".into(), writable: true, commands: false },
            tx,
        ));
        let mut running = tokio::spawn(async move {
            let result = bot.take_turn(&engine, None).await;
            (bot, engine, result)
        });
        let call = calls.recv().await.unwrap();
        assert_eq!(call.name, "workspace_write");
        token.cancel();
        assert!(tokio::time::timeout(std::time::Duration::from_millis(30), &mut running).await.is_err(),
            "input must not drop the pending write");
        call.reply.send(orgbot_tools::ToolOutcome { content: "write committed once".into(), is_error: false }).unwrap();
        let (mut bot, engine, result) = running.await.unwrap();
        let report = result.unwrap();
        assert_eq!(report.stop_reason, StopReason::Other("human_input".into()));
        assert_eq!(report.tool_calls, 1);
        assert_eq!(provider.requests().len(), 1, "no second API call before processing input");
        assert_eq!(bot.last_seen(), orgbot_proto::Seq::ZERO, "interrupted requests remain actionable");
        assert!(matches!(&bot.history.last().unwrap().content[0], ContentBlock::ToolResult { content, .. } if content == "write committed once"));
        bot.input_interrupt = tokio_util::sync::CancellationToken::new();
        bot.take_turn(&engine, None).await.unwrap();
        assert!(calls.try_recv().is_err(), "the write must not be replayed");
        assert!(serde_json::to_string(&provider.requests()[1].messages).unwrap().contains("write committed once"));
    }

    #[tokio::test]
    async fn mcp_calls_are_dispatched_against_the_engine_mid_turn() {
        let spec = AgentSpec::new("w1", "Worker", ProviderRef::Scripted { script: "x".into() })
            .with_roles(["writer"]);
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let gate = Arc::new(GateProvider {
            started: Mutex::new(Some(started_tx)),
            release: Mutex::new(Some(release_rx)),
        });
        let (mcp_tx, mcp_rx) = mpsc::channel(8);
        let engine = SharedEngine::new(engine_with(&spec));
        let mut bot = Bot::new(spec, gate);
        bot.attach_mcp(mcp_rx, "/tmp/unused.sock".into());
        bot.input_interrupt.cancel(); // MCP tool loops must still finish safely.

        let bot_task =
            tokio::spawn(async move { (bot.take_turn(&engine, None).await, engine) });

        // The episode is now blocked inside complete(); the room bridge must
        // answer MCP calls and act on the engine while it waits.
        started_rx.await.unwrap();

        let init = rpc(&mcp_tx, r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#)
            .await
            .expect("initialize replies");
        assert!(init.contains("orgbot-room"));

        let listed = rpc(&mcp_tx, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .await
            .expect("tools/list replies");
        assert!(listed.contains("room_status"), "{listed}");
        assert!(listed.contains("claim_task"), "{listed}");

        let status = rpc(
            &mcp_tx,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"room_status","arguments":{}}}"#,
        )
        .await
        .expect("tools/call replies");
        assert!(
            status.contains("prove in-band tools"),
            "the engine answered mid-episode: {status}"
        );
        assert!(!status.contains("isError"), "{status}");

        release_tx.send(()).unwrap();
        let (result, engine) = bot_task.await.unwrap();
        let report = result.unwrap();
        assert_eq!(report.said.as_deref(), Some("done"));
        assert!(
            report.tool_calls >= 1,
            "the in-band MCP call must count as progress"
        );
        assert_eq!(engine.with(|e| e.room().spec.name.clone()), "mcp-room");
    }
}
