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
    CompletionRequest, CompletionResponse, ContentBlock, Role, StopReason, ToolDef, Turn, Usage,
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
    #[serde(default)]
    capabilities: crate::capabilities::Capabilities,
}

/// Stop reasons the runtime sets for a turn that failed or was put off, as
/// distinct from a provider's own: such a turn says nothing about the bot.
pub const DEFERRED_TURN_MARKERS: &[&str] = &["stopped", "human_input", "provider_wait", "owner_approval", "provider_error"];

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
    /// What an agent CLI did on its own: tool uses inside it and files it
    /// changed, which the room sees only through this count.
    pub own_activity: usize,
    /// Room tool calls that changed something: not reads, listings or
    /// messages.
    pub changes: usize,
}

impl TurnReport {
    /// A turn the owner ended for this bot alone.
    pub fn stopped(agent: AgentId) -> Self {
        Self {
            agent,
            tool_calls: 0,
            tool_errors: 0,
            said: None,
            steps: 0,
            usage: Usage::default(),
            stop_reason: StopReason::Other("stopped".into()),
            own_activity: 0,
            changes: 0,
        }
    }
}

/// Keeps usage from completed provider responses if a later call in the same
/// tool loop fails or its task is cancelled. The room lock is acquired only
/// from `Drop`, never held across a provider call. Normal completion disarms
/// this guard after recording the turn once.
struct UsageCheckpoint {
    engine: SharedEngine,
    agent: AgentId,
    usage: Usage,
    responses: u32,
    armed: bool,
}

impl UsageCheckpoint {
    fn new(engine: &SharedEngine, agent: AgentId) -> Self {
        Self {
            engine: engine.clone(),
            agent,
            usage: Usage::default(),
            responses: 0,
            armed: true,
        }
    }

    fn update(&mut self, usage: Usage, responses: u32) {
        self.usage = usage;
        self.responses = responses;
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for UsageCheckpoint {
    fn drop(&mut self) {
        if self.armed && self.responses > 0 {
            let _ = self.engine.with(|engine| {
                engine.apply(Command::RecordTurn {
                    agent: self.agent,
                    usage: self.usage,
                })
            });
        }
    }
}

impl TurnReport {
    /// Whether the turn actually changed anything in the room.
    ///
    /// The orchestrator uses this to notice a room that is awake but going
    /// nowhere, which is otherwise indistinguishable from one that is working.
    pub fn made_progress(&self) -> bool {
        self.tool_calls > self.tool_errors || self.said.is_some()
    }

    /// Whether the bot did anything at all, as opposed to only talking or
    /// looking: a board tool that changed something, or a tool or edit inside
    /// its CLI.
    /// From the 13 November audit: any successful call counted, so a bot
    /// that only listed tasks or read an artifact went again at once.
    pub fn acted(&self) -> bool {
        self.changes > 0 || self.own_activity > 0
    }
}

impl Bot {
    /// Start this bot's next turn in a fresh session (see
    /// `Provider::start_fresh_session`).
    pub fn start_fresh_session(&self) {
        self.provider.start_fresh_session();
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
    capabilities: crate::capabilities::Capabilities,
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
    /// MCP servers this room reaches out to, shared by every bot in it.
    mcp_servers: Option<Arc<crate::mcp_client::McpClient>>,
    pub(crate) attachments_root: Option<std::path::PathBuf>,
    pub(crate) input_interrupt: CancellationToken,
    retry_at: Option<std::time::Instant>,
    /// Since when its model service has refused every turn, if it is.
    unavailable_since: Option<std::time::Instant>,
    /// Refused requests in a row since then.
    refused_attempts: u32,
    /// Told to the bot at the start of its next turn, once.
    next_turn_note: Option<String>,
    /// When this bot last posted the closing words of a turn that changed
    /// nothing, so it does not post another such status straight away.
    last_idle_post: Option<std::time::Instant>,
}

impl Bot {
    pub fn memory(&self) -> BotMemory {
        let mut history = self.history.clone();
        self.capabilities.prune_history(&mut history);
        BotMemory {
            history,
            last_seen: self.last_seen,
            capabilities: self.capabilities.clone(),
        }
    }

    pub fn restore_memory(&mut self, memory: BotMemory) {
        self.history = memory.history;
        self.last_seen = memory.last_seen;
        self.capabilities = memory.capabilities;
        self.capabilities.prune_history(&mut self.history);
    }
    /// Seat a bot with a backend.
    pub fn new(spec: AgentSpec, provider: Arc<dyn Provider>) -> Self {
        let tools = crate::mcp::room_tools_for(&spec);
        let model = spec.provider.model().to_string();
        Self {
            spec,
            provider,
            history: Vec::new(),
            capabilities: crate::capabilities::Capabilities::default(),
            last_seen: Seq::ZERO,
            tools,
            model,
            mcp_rx: None,
            mcp_socket: None,
            workspace: None,
            mcp_servers: None,
            attachments_root: None,
            input_interrupt: CancellationToken::new(),
            retry_at: None,
            unavailable_since: None,
            refused_attempts: 0,
            next_turn_note: None,
            last_idle_post: None,
        }
    }

    pub fn attach_workspace(&mut self, workspace: crate::workspace::Workspace) {
        // An agent CLI reads, edits and runs commands in its own copy with its
        // own tools; the room's file and command tools work in another copy,
        // which never had its edits. From the 13 November audit: a CLI bot
        // edited a file, ran workspace_verify, and recorded that tests passed
        // on a copy without the edit. It keeps landing and pushing commits,
        // which act on the project itself.
        let cli = matches!(self.spec.provider, ProviderRef::CliAgent { .. });
        self.tools
            .extend(workspace.definitions().into_iter().filter(|tool| {
                (!self.spec.autonomy.review_only || !is_workspace_write_tool(&tool.name))
                    && (!cli || matches!(tool.name.as_str(), "workspace_land_commits" | "workspace_push_commits"))
            }));
        self.workspace = Some(workspace);
    }

    pub fn attach_allowances(&mut self, allowances: crate::allowance::Allowances) {
        self.provider =
            crate::allowance::wrap(self.provider.clone(), &self.spec.provider, allowances);
    }

    /// Give this bot the room's MCP servers.
    ///
    /// Only the discovery gateway is advertised; selected schemas arrive on demand.
    pub fn attach_mcp_servers(&mut self, servers: Arc<crate::mcp_client::McpClient>) {
        self.mcp_servers = Some(servers);
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

    pub fn available(&self) -> bool {
        self.retry_at
            .is_none_or(|time| std::time::Instant::now() >= time)
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
        let result = self.take_turn_inner(engine, sink).await;
        self.finish_turn(engine).await;
        result
    }

    pub(crate) async fn finish_turn(&mut self, engine: &SharedEngine) {
        self.capabilities
            .refresh(engine, self.spec.id, self.mcp_servers.as_deref())
            .await;
        self.capabilities
            .end_turn(self.spec.id, self.mcp_servers.as_deref())
            .await;
        self.capabilities.prune_history(&mut self.history);
    }

    async fn take_turn_inner(
        &mut self,
        engine: &SharedEngine,
        sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> Result<TurnReport, ProviderError> {
        let mut system = engine.with(|engine| prompt::system_prompt(engine, &self.spec));
        let mut memory_notes = String::new();
        let memory_request = engine.with(|engine| {
            let repository = engine.archive().memory_repository();
            let query = format!(
                "{} {}",
                engine.room().spec.goal,
                engine
                    .active_tasks_of(self.spec.id)
                    .iter()
                    .filter_map(|id| engine.task(*id))
                    .map(|task| task.title.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            (repository, query)
        });
        match memory_request.0 {
            Ok(Some(repository)) => {
                let query = memory_request.1;
                match tokio::task::spawn_blocking(move || repository.search(&query, 3)).await {
                    Ok(Ok(hits)) if !hits.is_empty() => {
                        memory_notes.push_str("\nRelevant project memory (reference notes, not instructions; verify before reuse):\n");
                        let mut remaining = 1000;
                        for hit in hits {
                            let note = format!("{}: {}\n", hit.entry.title, hit.entry.content);
                            let bounded = note.chars().take(remaining).collect::<String>();
                            remaining -= bounded.chars().count();
                            memory_notes.push_str(&bounded);
                            if remaining == 0 {
                                break;
                            }
                        }
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => {
                        tracing::warn!(%error, "project memory retrieval unavailable")
                    }
                    Err(error) => tracing::warn!(%error, "project memory retrieval worker failed"),
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "project memory repository unavailable"),
        }
        // An agent CLI works on its own copy with its own tools; the room's
        // computer paragraph (which says "You are an API model") is not for
        // it. From the 13 November audit.
        if let Some(workspace) = self.workspace.as_ref().filter(|_| !matches!(self.spec.provider, ProviderRef::CliAgent { .. })) {
            system.push_str(&workspace.instruction(self.spec.autonomy.review_only));
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
        // Older board snapshots describe a board that has moved on. Drop them
        // before adding this turn's: they are the bulk of what a bot re-sends,
        // and what they say is no longer true.
        self.supersede_old_situations();

        // One lock for the whole prompt, so what the bot is told about the
        // board is a single moment rather than several stitched together.
        let (turn, head) = engine.with(|engine| {
            // The offers below are now in front of this bot, so from here
            // their timeouts run instead of being held for it.
            let _ = engine.apply(Command::BeginTurn { agent: self.spec.id });
            let mut situation = prompt::situation(engine, self.spec.id, self.last_seen);
            situation.push_str(&memory_notes);
            // Kept until the turn gets past its first call: a turn refused at
            // once (and its situation dropped) still carries it next time.
            // From the 13 November audit: the note was lost that way.
            if let Some(note) = self.next_turn_note.clone() {
                situation.push_str("\n\n");
                situation.push_str(&note);
            }
            let mut turn = Turn::user(situation);
            turn.content.extend(crate::attachments::context(
                engine,
                self.last_seen,
                self.attachments_root.as_deref(),
            ));
            (turn, engine.head())
        });
        // What this turn has read is marked seen only when the turn finishes.
        // A turn cut short (a pause, a stop, a restart) or one that fails must
        // leave its messages unseen, or a question it was about to answer is
        // never asked of it again.
        let seen_through = head;
        let mut mark_seen = true;
        let mut changed = false;
        self.history.push(turn);

        let mut report = TurnReport {
            agent: self.spec.id,
            tool_calls: 0,
            tool_errors: 0,
            said: None,
            steps: 0,
            usage: Usage::default(),
            stop_reason: StopReason::EndTurn,
            own_activity: 0,
            changes: 0,
        };
        let mut usage_checkpoint = UsageCheckpoint::new(engine, self.spec.id);
        let mut posted_to_chat = false;
        let mut generated: Vec<std::path::PathBuf> = Vec::new();

        // No cap on a turn's model calls. From the live room: @ollama-glm's
        // turns were cut at 24 calls three times running, mid-review, and the
        // review expired. A turn now runs until the bot stops calling tools;
        // every `remind_every` calls it is asked to finish if it is done.
        let remind_every = self.spec.autonomy.max_turns_per_task.max(1);
        let mut repeats = Repeats::default();
        let mut step: u32 = 0;
        // From the benchmark room: a QA turn on a slow model ran 75 minutes
        // in 72 calls, a minute a call, so a count of calls reached it rarely.
        // Every ten minutes of a turn it hears how long it has run.
        let mut steer = TurnSteer::new(seen_through);
        let mut cut_off: u32 = 0;
        let mut shrunk_for_context = false;
        loop {
            step += 1;
            self.capabilities
                .refresh(engine, self.spec.id, self.mcp_servers.as_deref())
                .await;
            self.capabilities.prune_history(&mut self.history);
            self.trim_history();

            let mut request =
                CompletionRequest::new(self.model.clone(), system.clone(), self.history.clone())
                    .with_tools(self.tools.clone());
            request = request.with_mcp_socket(
                self.mcp_socket
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
            );
            // Decided afresh each step: a release, block or hold since the
            // last one takes the server key and lent sign-ins away from this one.
            request = request.with_grants(engine.with(|engine| sandbox_grants(engine, self.spec.id)));
            // Reasoning without acting is cut short only while teammates wait.
            if engine.with(|engine| engine.others_wait_on(self.spec.id)) {
                request.thinking_limit = Some(API_THINKING_LIMIT);
            }

            if let Some(tx) = &sink {
                let _ = tx.send(StreamEvent::Thinking).await;
            }

            let mut interrupted = false;
            // Without the room bridge too: from the 13 November audit, a CLI
            // bot with no bridge (or whose socket failed) merged untasked edits.
            self.provider
                .hold_merge(!engine.with(|engine| engine.may_change_project(self.spec.id)));
            let response = match self.mcp_rx.as_mut() {
                Some(rx) => {
                    let (response, mcp) = complete_with_mcp(
                        &self.provider,
                        &self.tools,
                        engine,
                        &self.spec,
                        request,
                        sink.clone(),
                        rx,
                        self.mcp_servers.as_deref(),
                        self.workspace.as_ref(),
                        &mut self.capabilities,
                        &mut steer,
                    )
                    .await;
                    interrupted = mcp.interrupted || mcp.thinking_stopped;
                    if mcp.thinking_stopped {
                        self.next_turn_note = Some(THINKING_STOPPED_NOTE.to_string());
                    }
                    report.tool_calls += mcp.ok;
                    report.tool_errors += mcp.err;
                    report.changes += mcp.changed;
                    // Said through the room tool already; the closing prose
                    // is the CLI's own summary and must not be posted again.
                    posted_to_chat |= mcp.posted;
                    response
                }
                // CLI providers execute their own tools inside complete(); only
                // API waits are safe to abandon for new human input. Workspace
                // calls below finish and retain their results before yielding.
                None if !matches!(self.spec.provider, ProviderRef::CliAgent { .. }) => {
                    tokio::select! {
                        biased;
                        _ = self.input_interrupt.cancelled() => {
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
            report.own_activity += self.provider.last_turn_activity().unwrap_or(0);
            if let Some(count) = self.provider.last_pending_removals() {
                engine.with(|e| note_pending_removals(e, self.spec.id, "its own CLI", count));
            }
            let response = match response {
                Ok(response) => {
                    self.unavailable_since = None;
                    self.refused_attempts = 0;
                    response
                }
                // From the 13 November audit: the model's "too long" refusal
                // faulted the bot and released its tasks. Older tool results
                // are shortened and the call is made once more.
                Err(error) if is_context_too_long(&error) && !shrunk_for_context => {
                    shrunk_for_context = true;
                    tracing::warn!(agent = %self.spec.handle, %error, "the conversation was too long; shortening older tool results");
                    shrink_tool_results(&mut self.history, MAX_HISTORY_CHARS / 4, 2);
                    continue;
                }
                Err(error) => {
                    if matches!(error, ProviderError::RateLimited { .. }) {
                        let since = *self.unavailable_since.get_or_insert_with(std::time::Instant::now);
                        self.refused_attempts += 1;
                        tracing::warn!(
                            agent = %self.spec.handle,
                            refused_for_secs = since.elapsed().as_secs(),
                            %error,
                            "the model service refused a turn; waiting"
                        );
                        // Refused again and again, not twice with a long wait
                        // between. From the 13 November audit: one 429, a
                        // 20-minute retry wait and a second 429 released all
                        // a bot held as "refused for 20 minutes".
                        if since.elapsed() >= SERVICE_DOWN_RELEASE && self.refused_attempts >= SERVICE_DOWN_ATTEMPTS {
                            let minutes = since.elapsed().as_secs() / 60;
                            engine.with(|engine| hand_on_held_work(engine, &self.spec, minutes, &error));
                        }
                    }
                    if matches!(
                        error,
                        ProviderError::Allowance { .. } | ProviderError::RateLimited { .. }
                    ) {
                        self.retry_at = Some(
                            std::time::Instant::now()
                                + std::time::Duration::from_secs(
                                    if matches!(error, ProviderError::Allowance { .. }) {
                                        5
                                    } else {
                                        error.retry_after_secs().unwrap_or(60).clamp(1, 604800)
                                    },
                                ),
                        );
                        mark_seen = false;
                        report.stop_reason = StopReason::Other("provider_wait".into());
                        if report.steps == 0 {
                            self.history.pop();
                            return Ok(report);
                        }
                        break;
                    }
                    // What the failed turn used still counts.
                    if let Some(usage) = self.provider.last_failed_usage() {
                        report.usage.add(usage);
                        engine.with(|engine| {
                            let _ = engine.apply(Command::RecordUsage { agent: self.spec.id, usage });
                        });
                    }
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
            // The owner wrote to this CLI bot and it made no room call to see
            // it: the turn ends here and the next one resumes the same session
            // with the message in front of it.
            if interrupted {
                // Cut short: what it was shown stays unseen, by the rule above.
                // From the 13 November audit: these turns marked it seen.
                mark_seen = false;
                // What the CLI used before it was stopped still counts.
                report.usage.add(response.usage);
                usage_checkpoint.update(report.usage, report.steps + 1);
                report.stop_reason = StopReason::Other(
                    if self.next_turn_note.is_some() { "thinking_limit" } else { "human_input" }.into(),
                );
                if let Some(tx) = &sink {
                    let _ = tx.send(StreamEvent::Done { stop_reason: report.stop_reason.clone() }).await;
                }
                break;
            }

            report.steps += 1;
            if report.steps == 1 && !interrupted {
                self.next_turn_note = None;
            }
            // Pictures an agent CLI drew this turn go to the room, not into
            // the conversation it carries forward.
            let mut response = response;
            response.content.retain(|block| match block {
                ContentBlock::Image {
                    source: orgbot_providers::types::ImageSource::File { path },
                    ..
                } => {
                    generated.push(path.clone());
                    false
                }
                _ => true,
            });
            self.capabilities.after_response(&mut self.history);
            report.usage.add(response.usage);
            usage_checkpoint.update(report.usage, report.steps);
            report.stop_reason = response.stop_reason.clone();
            self.history.push(response.as_turn());

            let calls = response.tool_calls();
            // From the benchmark room: a reasoning model's reply hit the
            // 16,000-token output limit before it called a tool, three turns
            // in a row, and each turn ended with nothing done. A reply cut
            // off there is not the end of the turn.
            if calls.is_empty() && response.stop_reason == StopReason::MaxTokens && cut_off < MAX_CUT_OFF_RETRIES {
                cut_off += 1;
                self.history.push(Turn::user(CUT_OFF_NOTE));
                continue;
            }
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
            // A reply cut off at its output limit may have been cut inside its
            // last call's arguments. From the 13 November audit: such a
            // workspace_write ran with no arguments, failed, and was made
            // again just as large. It is not run; the bot is told why.
            let truncated_call = (response.stop_reason == StopReason::MaxTokens).then(|| calls.len() - 1);
            for (index, (id, name, input)) in calls.into_iter().enumerate() {
                if truncated_call == Some(index) {
                    report.tool_calls += 1;
                    report.tool_errors += 1;
                    results.push(ContentBlock::ToolResult {
                        tool_use_id: id.to_string(),
                        content: TRUNCATED_CALL_NOTE.into(),
                        is_error: true,
                    });
                    continue;
                }
                let outcome = if crate::capabilities::handles(name) {
                    self.capabilities
                        .dispatch(
                            engine,
                            self.spec.id,
                            self.spec.autonomy.review_only,
                            self.mcp_servers.as_deref(),
                            name,
                            input,
                        )
                        .await
                } else if self
                    .mcp_servers
                    .as_ref()
                    .is_some_and(|servers| servers.owns(name))
                {
                    if self.spec.autonomy.review_only {
                        orgbot_tools::ToolOutcome::error(
                            "Review-only bots cannot call external MCP tools.",
                        )
                    } else {
                        orgbot_tools::ToolOutcome::error(
                            "Load the selected tool with load_mcp_tool, then use call_mcp_tool.",
                        )
                    }
                } else if name.starts_with("workspace_") {
                    if self.spec.autonomy.review_only && is_workspace_write_tool(name) {
                        orgbot_tools::ToolOutcome::error(
                            "Review-only bots cannot change workspace files or run commands.",
                        )
                    } else if is_workspace_write_tool(name)
                        && !engine.with(|engine| may_write(engine, self.spec.id, name))
                    {
                        orgbot_tools::ToolOutcome::error(NO_TASK_NO_CHANGES)
                    } else {
                        match &self.workspace {
                            Some(workspace) if name == "workspace_land_commits" => {
                                let grants = engine
                                    .with(|engine| sandbox_grants(engine, self.spec.id));
                                land_commits(engine, workspace, self.spec.id, input, grants).await
                            }
                            Some(workspace) if name == "workspace_push_commits" => {
                                let grants = engine
                                    .with(|engine| sandbox_grants(engine, self.spec.id));
                                push_commits(engine, workspace, self.spec.id, input, grants).await
                            }
                            Some(workspace) => {
                                let grants = engine
                                    .with(|engine| sandbox_grants(engine, self.spec.id));
                                let began = std::time::Instant::now();
                                let outcome = workspace
                                    .call_as(self.spec.id, name, input, grants)
                                    .await;
                                engine.with(|engine| {
                                    engine.note_workspace_time(self.spec.id, began.elapsed().as_millis() as i64)
                                });
                                note_room_removals(engine, self.spec.id, name, &outcome);
                                outcome
                            }
                            None => orgbot_tools::ToolOutcome {
                                content: "No workspace attached".into(),
                                is_error: true,
                            },
                        }
                    }
                } else {
                    engine.with(|engine| orgbot_tools::dispatch(engine, self.spec.id, name, input))
                };
                if let Some(tx) = &sink {
                    let _ = tx
                        .send(StreamEvent::ToolUseEnd {
                            id: id.to_string(),
                            succeeded: !outcome.is_error,
                        })
                        .await;
                }
                report.tool_calls += 1;
                // Posted only if it went through, as on the CLI path. From
                // the 13 November audit: a refused post still counted, so the
                // bot's answer, given as prose next, was dropped.
                if name == "post_message" && !outcome.is_error {
                    posted_to_chat = true;
                }
                if !outcome.is_error && !QUIET_TOOLS.contains(&name) {
                    changed = true;
                    report.changes += 1;
                }
                if outcome.is_error {
                    report.tool_errors += 1;
                }
                let mut content = bound_tool_result(outcome.content, MAX_TOOL_RESULT_CHARS);
                if let Some(note) = repeats.note(name, input) {
                    content.push_str(&note);
                }
                results.push(ContentBlock::ToolResult {
                    tool_use_id: id.to_string(),
                    content,
                    is_error: outcome.is_error,
                });
            }
            let reminder = long_turn_reminder(remind_every, step)
                .or_else(|| engine.with(|engine| steer.minutes(engine, self.spec.id)));
            if let Some(reminder) = reminder {
                if let Some(ContentBlock::ToolResult { content, .. }) = results.last_mut() {
                    content.push_str(&reminder);
                }
            }
            self.history.push(Turn::tool_results(results));
            let waiting_for_owner = engine.with(|engine| {
                engine
                    .decisions()
                    .decisions
                    .iter()
                    .any(|d| d.answer.is_none() && d.asked_by.contains(&self.spec.id))
            });
            if waiting_for_owner {
                report.stop_reason = StopReason::Other("owner_approval".into());
                break;
            }
        }

        // A bot that ended with prose and never called post_message still meant
        // to say it — this is a group chat, and silent work is invisible to the
        // other bots. Posted as an ordinary statement, so it does not wake the
        // room; anything needing an answer goes through post_message with
        // intent "ask".
        // Generated images become room attachments, shown in the chat and
        // open to a reply, with the bot's closing words when it has any.
        let files: Vec<orgbot_proto::chat::ChatFile> = match self.attachments_root.as_deref() {
            Some(root) => generated
                .iter()
                .filter_map(|path| {
                    crate::attachments::store_generated(root, path, &format!("Generated by @{}", self.spec.handle))
                        .map_err(|error| tracing::warn!(%error, "a generated image was not kept"))
                        .ok()
                })
                .take(5)
                .collect(),
            None => Vec::new(),
        };
        // From the live room: bots ended turns that changed nothing with
        // "Aece, nothing needs you" or "still waiting", turn after turn. Such
        // a closing is posted at most once in QUIET_REPEAT, unless the turn
        // answered a person.
        let changed = changed || report.own_activity > 0;
        let answering = engine.with(|engine| {
            engine.events_since(self.last_seen).any(|envelope| {
                matches!(&envelope.event, orgbot_proto::Event::MessagePosted { message }
                    if matches!(message.author, orgbot_proto::Author::Human { .. })
                        && message.wakes(self.spec.id, &self.spec.roles))
            })
        });
        let quiet = !changed
            && !answering
            && self.last_idle_post.is_some_and(|at| at.elapsed() < QUIET_REPEAT);
        if changed {
            self.last_idle_post = None;
        } else if report.said.is_some() && !posted_to_chat && !quiet {
            self.last_idle_post = Some(std::time::Instant::now());
        }
        engine.with(|engine| {
            let closing = report.said.clone().filter(|_| !posted_to_chat && !quiet);
            if closing.is_some() || !files.is_empty() {
                let body = closing.unwrap_or_else(|| match files.len() {
                    1 => "Here is the image I generated.".into(),
                    count => format!("Here are the {count} images I generated."),
                });
                let mut message = orgbot_proto::ChatMessage::say(engine.room_id(), self.spec.id, body);
                message.files = files;
                let _ = engine.apply(Command::PostMessage {
                    message: Box::new(message),
                });
            }
            let _ = engine.apply(Command::RecordTurn {
                agent: self.spec.id,
                usage: report.usage,
            });
        });
        usage_checkpoint.disarm();
        // What was passed on mid-turn has been seen too. From the 13 November
        // audit: an owner's question relayed and answered mid-turn woke the
        // bot again at once and was shown to it as new.
        if mark_seen {
            self.last_seen = seen_through.max(steer.relayed_through);
        }
        // Where a turn's time went, for the room's log, which keeps warnings
        // only: a long turn is one.
        let seconds = steer.began.elapsed().as_secs();
        if seconds >= TURN_MINUTES_NOTE * 60 {
            tracing::warn!(
                agent = %self.spec.handle,
                seconds,
                steps = report.steps,
                tool_calls = report.tool_calls,
                tool_errors = report.tool_errors,
                "long turn ended"
            );
        } else {
            tracing::info!(
                agent = %self.spec.handle,
                seconds,
                steps = report.steps,
                tool_calls = report.tool_calls,
                tool_errors = report.tool_errors,
                "turn ended"
            );
        }
        Ok(report)
    }

    /// How much text this bot would re-send on its next request.
    fn history_chars(&self) -> usize {
        history_chars(&self.history)
    }

    /// Replace every older board snapshot with the current state summary.
    /// Binary attachments and other text remain available in their turn.
    fn supersede_old_situations(&mut self) {
        for turn in &mut self.history {
            if turn.role != Role::User {
                continue;
            }
            for block in &mut turn.content {
                if let ContentBlock::Text { text } = block {
                    if text.starts_with(prompt::SITUATION_MARKER) {
                        *text = prompt::SUPERSEDED_SITUATION.to_string();
                    }
                }
            }
        }
    }

    /// Keep the conversation from growing without bound.
    ///
    /// Cutting is only safe at a boundary where the remaining history does not
    /// begin with tool results — an orphaned `tool_result` whose `tool_use` has
    /// been trimmed away is rejected by every backend.
    fn trim_history(&mut self) {
        // Counting turns is the wrong unit on its own: one turn carrying a
        // read artifact can outweigh fifty carrying a sentence. Trim on
        // whichever ceiling is reached first.
        let heavy = self.history_chars() > MAX_HISTORY_CHARS;
        if self.history.len() <= MAX_HISTORY_TURNS && !heavy {
            return;
        }
        let keep = if heavy {
            KEEP_HISTORY_TURNS.min(self.history.len() / 2)
        } else {
            KEEP_HISTORY_TURNS
        };
        let target = self.history.len().saturating_sub(keep);
        let mut cut = self.history[target..]
            .iter()
            .position(is_safe_cut_point)
            .map(|offset| target + offset);

        // If one half still exceeds the byte budget, advance to another safe
        // user boundary. Keep as much recent history as fits, while reserving
        // room for the compacted summary.
        // A first cut that already fits is kept (13 November audit: the
        // history was always cut one segment further).
        if heavy
            && cut.is_some_and(|first| {
                history_chars(&self.history[first..]) > MAX_HISTORY_CHARS - MAX_COMPACTED_SUMMARY_CHARS
            })
        {
            if let Some(mut chosen) = cut {
                for next in chosen + 1..self.history.len() {
                    if is_safe_cut_point(&self.history[next]) {
                        chosen = next;
                        if history_chars(&self.history[chosen..])
                            <= MAX_HISTORY_CHARS - MAX_COMPACTED_SUMMARY_CHARS
                        {
                            break;
                        }
                    }
                }
                cut = Some(chosen);
            }
        }

        if let Some(cut) = cut {
            let summary = compact_history(&self.history[..cut]);
            self.history.drain(..cut);
            if !summary.is_empty()
                && self
                    .history
                    .first()
                    .is_some_and(|turn| turn.role == Role::User)
            {
                self.history[0]
                    .content
                    .insert(0, ContentBlock::text(summary));
            }
        }
        // Inside one turn every user turn carries tool results, so there is
        // no safe place to cut. From the 13 November audit: the ceiling then
        // did nothing, and a long turn grew until the model refused it.
        if self.history_chars() > MAX_HISTORY_CHARS {
            shrink_tool_results(&mut self.history, MAX_HISTORY_CHARS, 2);
        }
    }
}

/// The most of one tool result a bot's conversation keeps.
const MAX_TOOL_RESULT_CHARS: usize = 60_000;
/// What an older tool result is shortened to when the conversation is full.
const SHORTENED_RESULT_CHARS: usize = 1_500;

/// Keep the start and end of a long tool result, saying what was left out.
fn bound_tool_result(content: String, limit: usize) -> String {
    if content.len() <= limit {
        return content;
    }
    let head_end = floor_char_boundary(&content, limit * 2 / 3);
    let tail_start = ceil_char_boundary(&content, content.len() - (limit - head_end).min(content.len()));
    let cut = content.len() - head_end - (content.len() - tail_start);
    format!(
        "{}\n[OrgBots: {cut} characters cut from the middle of this result to keep the conversation in bounds. \
         Read a narrower part, for example with workspace_run and sed -n or grep, if you need it.]\n{}",
        &content[..head_end],
        &content[tail_start..]
    )
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

/// Shorten tool results in place, oldest first, until the conversation fits
/// `budget`, leaving the last `keep_recent` turns whole. Calls and results
/// stay paired, which every backend requires.
fn shrink_tool_results(history: &mut [Turn], budget: usize, keep_recent: usize) {
    let end = history.len().saturating_sub(keep_recent);
    for index in 0..end {
        if history_chars(history) <= budget {
            return;
        }
        for block in &mut history[index].content {
            if let ContentBlock::ToolResult { content, .. } = block {
                // A result already shortened (its note included) stays as is.
                if content.len() > SHORTENED_RESULT_CHARS + 400 {
                    let shortened = bound_tool_result(std::mem::take(content), SHORTENED_RESULT_CHARS);
                    *content = shortened;
                }
            }
        }
    }
}

/// Whether a model refused a request for being longer than it can read.
fn is_context_too_long(error: &ProviderError) -> bool {
    let message = match error {
        ProviderError::BadRequest { message, .. } | ProviderError::Upstream { message, .. } => message.to_ascii_lowercase(),
        _ => return false,
    };
    [
        "prompt is too long",
        "context length",
        "context_length_exceeded",
        "maximum context",
        "context window",
        "too many tokens",
        "input is too long",
        "request too large",
    ]
    .iter()
    .any(|phrase| message.contains(phrase))
}

fn history_chars(history: &[Turn]) -> usize {
    history
        .iter()
        .flat_map(|turn| turn.content.iter())
        .map(|block| match block {
            ContentBlock::Text { text } => text.len(),
            ContentBlock::ToolResult { content, .. } => content.len(),
            other => serde_json::to_string(other).map(|s| s.len()).unwrap_or(0),
        })
        .sum()
}

/// Compress removed turns without an inference call. Complete tool interactions
/// retain their call id, tool name, arguments and result together.
fn compact_history(turns: &[Turn]) -> String {
    use std::collections::{BTreeMap, BTreeSet};

    let mut facts = BTreeSet::new();
    let mut calls: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut results: BTreeMap<String, (String, bool)> = BTreeMap::new();
    let mut recent_human = Vec::new();
    let mut prior_tool_pairs = Vec::new();

    for turn in turns {
        for block in &turn.content {
            match block {
                ContentBlock::Text { text } => {
                    if text.starts_with(COMPACTED_HISTORY_MARKER) {
                        for line in text.lines().skip(1) {
                            let line = line.trim();
                            if line.starts_with("tool ") || line.starts_with("tool result ") {
                                prior_tool_pairs.push(line.to_string());
                            } else if !line.is_empty() {
                                facts.insert(line.to_string());
                            }
                        }
                    } else if turn.role == Role::User {
                        if !text.starts_with(prompt::SITUATION_MARKER)
                            && text != prompt::SUPERSEDED_SITUATION
                        {
                            let snippet = compact_snippet(text, 360);
                            if !snippet.is_empty() {
                                recent_human.push(snippet);
                            }
                        }
                        for line in text.lines() {
                            let line = compact_snippet(line, 240);
                            if is_durable_fact(&line) {
                                facts.insert(format!("operator context: {line}"));
                            }
                        }
                    } else {
                        for line in text.lines() {
                            let line = compact_snippet(line, 240);
                            if is_durable_fact(&line) {
                                facts.insert(format!("outcome: {line}"));
                            }
                        }
                    }
                }
                ContentBlock::ToolUse { id, name, input } => {
                    let args = serde_json::to_string(input).unwrap_or_default();
                    calls.insert(id.clone(), (name.clone(), compact_snippet(&args, 180)));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    // Loaded context must not survive unloading through a summary.
                    let transient = calls.get(tool_use_id).is_some_and(|(name, _)| {
                        matches!(
                            name.as_str(),
                            "use_skill" | "load_mcp_tool" | "find_capabilities"
                        )
                    });
                    results.insert(
                        tool_use_id.clone(),
                        (
                            if transient && !is_error {
                                "context omitted; discover or reload if needed".into()
                            } else {
                                compact_snippet(content, 300)
                            },
                            *is_error,
                        ),
                    );
                }
                // Original files remain available through room artifacts and
                // workspace tools; binary payloads and hidden reasoning are
                // unsuitable for durable text summaries.
                ContentBlock::Image { .. } | ContentBlock::Thinking { .. } => {}
            }
        }
    }

    let mut lines = vec![COMPACTED_HISTORY_MARKER.to_string()];
    for snippet in recent_human.into_iter().rev().take(4).rev() {
        lines.push(format!("recent operator context: {snippet}"));
    }
    let mut paired = BTreeSet::new();
    for (id, (name, args)) in calls {
        if let Some((result, is_error)) = results.get(&id) {
            paired.insert(id.clone());
            lines.push(format!(
                "tool {id} `{name}` {args} => {}{result}",
                if *is_error { "error: " } else { "" }
            ));
        } else {
            lines.push(format!(
                "tool {id} `{name}` {args} => result not present in compacted range"
            ));
        }
    }
    for id in results.keys().filter(|id| !paired.contains(*id)) {
        lines.push(format!("tool result {id} had no call in compacted range; inspect current room state before relying on it"));
    }
    lines.extend(prior_tool_pairs);
    lines.extend(facts);

    let mut summary = String::new();
    for line in lines {
        let added = line.chars().count() + usize::from(!summary.is_empty());
        if summary.chars().count() + added > MAX_COMPACTED_SUMMARY_CHARS {
            break;
        }
        if !summary.is_empty() {
            summary.push('\n');
        }
        summary.push_str(&line);
    }
    summary
}

fn compact_snippet(text: &str, max_chars: usize) -> String {
    let flattened = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut snippet: String = flattened.chars().take(max_chars).collect();
    if flattened.chars().count() > max_chars {
        snippet.push('…');
    }
    snippet
}

fn is_durable_fact(text: &str) -> bool {
    let line = text.to_ascii_lowercase();
    [
        "decision",
        "decided",
        "agreed",
        "must",
        "required",
        "constraint",
        "artifact",
        "deliver",
        "submitted",
        "accepted",
        "reject",
        "approved",
        "chosen",
        "use ",
        "avoid ",
    ]
    .iter()
    .any(|marker| line.contains(marker))
}

/// Push a branch of the owner's repository once the owner has approved
/// exactly those commits to that remote branch for the task the bot holds.
/// Asked every time, whatever the access mode: a push leaves the computer.
async fn push_commits(
    engine: &SharedEngine,
    workspace: &crate::workspace::Workspace,
    actor: AgentId,
    input: &serde_json::Value,
    grants: orgbot_providers::SandboxGrants,
) -> orgbot_tools::ToolOutcome {
    use orgbot_tools::ToolOutcome;
    let text = |key: &str| input.get(key).and_then(serde_json::Value::as_str).unwrap_or("").trim().to_string();
    let remote = match text("remote") {
        remote if remote.is_empty() => "origin".to_string(),
        remote => remote,
    };
    let (branch, reason) = (text("branch"), text("reason"));
    let Ok(task) = serde_json::from_value::<orgbot_proto::TaskId>(serde_json::json!(text("task"))) else {
        return ToolOutcome::error("Name the task you hold that this push is for.");
    };
    let inspected = workspace
        .call_as(actor, "workspace_push_commits", &serde_json::json!({"remote": remote, "branch": branch}), grants.clone())
        .await;
    if inspected.is_error {
        return inspected;
    }
    let Ok(found) = serde_json::from_str::<serde_json::Value>(&inspected.content) else {
        return ToolOutcome::error("The workspace could not describe the push.");
    };
    let field = |key: &str| found.get(key).and_then(serde_json::Value::as_str).unwrap_or("").to_string();
    let (base, head) = (field("base"), field("head"));
    let commits: Vec<String> = found
        .get("commits")
        .and_then(serde_json::Value::as_array)
        .map(|list| list.iter().filter_map(|c| c.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let count = commits.len();
    let approved = |engine: &SharedEngine| engine.with(|e| e.push_approved(task, actor, &remote, &branch, &base, &head));
    if !approved(engine) {
        let asked = engine.with(|e| {
            e.apply(Command::RequestPush {
                by: actor,
                task,
                remote: remote.clone(),
                url: field("url"),
                branch: branch.clone(),
                base: base.clone(),
                head: head.clone(),
                commits,
                reason,
            })
        });
        if let Err(error) = asked {
            return ToolOutcome::error(error.to_string());
        }
        if !approved(engine) {
            return ToolOutcome::ok(format!(
                "Asked the owner to push these {count} commit(s) to {branch} on {remote}. You will be \
                 woken when they answer; then call workspace_push_commits again with the same remote \
                 and branch. Do not ask again meanwhile."
            ));
        }
    }
    let pushed = workspace
        .call_as(
            actor,
            "workspace_push_commits",
            &serde_json::json!({
                "remote": remote, "branch": branch, "push": true,
                "expect_base": base, "expect_head": head,
            }),
            grants,
        )
        .await;
    if !pushed.is_error {
        engine.with(|e| {
            let handle = e.agent(actor).map(|spec| spec.handle.clone()).unwrap_or_default();
            let note = orgbot_proto::chat::ChatMessage::system(
                e.room().id,
                format!(
                    "@{handle} pushed {count} commit(s) to {branch} on {remote}, moving it from {} to {} as the owner approved.",
                    base.get(..7).unwrap_or(&base),
                    head.get(..7).unwrap_or(&head)
                ),
            )
            .about_task(task);
            let _ = e.apply(Command::PostMessage { message: Box::new(note) });
        });
    }
    pushed
}

/// Record the removals a workspace command left waiting in the bot's room
/// copy, for the app to show the owner.
fn note_room_removals(engine: &SharedEngine, actor: AgentId, name: &str, outcome: &orgbot_tools::ToolOutcome) {
    if name != "workspace_run" {
        return;
    }
    let Ok(result) = serde_json::from_str::<serde_json::Value>(&outcome.content) else {
        return;
    };
    let Some(deleted) = result.get("integration").and_then(|i| i.get("deleted")).and_then(serde_json::Value::as_array) else {
        return;
    };
    engine.with(|e| note_pending_removals(e, actor, "room tools", deleted.len()));
}

fn note_pending_removals(engine: &mut orgbot_engine::Engine, agent: AgentId, copy: &str, count: usize) {
    let _ = engine.apply(Command::RecordPendingRemovals {
        agent,
        copy: copy.into(),
        count: u32::try_from(count).unwrap_or(u32::MAX),
    });
}

/// Tools that only read or talk: a turn that used nothing else changed nothing.
const QUIET_TOOLS: &[&str] = &[
    "post_message",
    "list_tasks",
    "room_status",
    "read_artifact",
    "read_prior_artifact",
    "find_prior_work",
    "list_handoffs",
    "search_project_memory",
    "find_capabilities",
    "workspace_read",
    "workspace_list",
    "workspace_git",
];

/// How long a bot whose turns change nothing waits before posting another
/// such closing status.
const QUIET_REPEAT: std::time::Duration = std::time::Duration::from_secs(20 * 60);

/// What to tell a bot after `step` calls of a long turn, every `every` calls.
/// It is never cut off; it is asked to finish if it has finished.
fn long_turn_reminder(every: u32, step: u32) -> Option<String> {
    (step % every.max(1) == 0).then(|| {
        format!(
            "\n\n[OrgBots] {step} model calls so far this turn. If the work is done, \
             finish now: complete any handoff you accepted, record the result and end \
             the turn. If you are going round in circles, say what is blocking you."
        )
    })
}

/// How many replies cut off at the output limit a turn carries on past.
const MAX_CUT_OFF_RETRIES: u32 = 2;

const CUT_OFF_NOTE: &str = "[OrgBots] Your reply was cut off before you called a tool or \
     finished: it reached the output limit, or reasoned too long without acting. Keep \
     your reasoning short and act now: make the next tool call, or give your final answer in a \
     few sentences.";

/// How long a bot's model service may refuse every turn before the work it
/// holds goes back to the board.
const SERVICE_DOWN_RELEASE: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Refused requests in a row, with [`SERVICE_DOWN_RELEASE`] gone by, before
/// a bot's work is handed back.
const SERVICE_DOWN_ATTEMPTS: u32 = 3;

/// Put a bot's work back on the board while its model service refuses it,
/// and tell the coordinators.
///
/// From speed test 1: DeepSeek answered "Service is too busy" for every turn
/// of @deepseek-builder, which held the render pipeline for 24 minutes with
/// nothing to show while two teammates waited on its renders.
fn hand_on_held_work(engine: &mut orgbot_engine::Engine, spec: &AgentSpec, minutes: u64, error: &ProviderError) {
    let held = engine.active_tasks_of(spec.id);
    if held.is_empty() {
        return;
    }
    let reason = format!("its model service has refused it for {minutes} minutes");
    let mut titles = Vec::new();
    for task in held {
        let title = engine.task(task).map(|task| task.title.clone()).unwrap_or_default();
        if engine
            .apply(Command::ReleaseTask { task, by: spec.id, reason: reason.clone() })
            .is_ok()
        {
            titles.push(format!("\"{title}\""));
        }
    }
    if titles.is_empty() {
        return;
    }
    let detail: String = error.to_string().chars().take(160).collect();
    let coordinators: Vec<AgentId> = engine
        .agents()
        .filter(|other| other.id != spec.id && other.roles.iter().any(|role| role == "coordinator"))
        .map(|other| other.id)
        .collect();
    let mut message = orgbot_proto::ChatMessage::system(
        engine.room_id(),
        format!(
            "@{}'s model service has refused it for {minutes} minutes ({detail}), so its work went back to the board: {}.",
            spec.handle,
            titles.join(", ")
        ),
    );
    if !coordinators.is_empty() {
        message = message.to_agents(coordinators);
    }
    let _ = engine.apply(Command::PostMessage { message: Box::new(message) });
}

/// How often, in minutes of one turn, a bot hears how long it has run.
const TURN_MINUTES_NOTE: u64 = 10;

/// Minutes into a turn on one task past which its holder is asked whether it
/// should be split.
const SPLIT_AFTER_MINUTES: u64 = 20;

fn turn_minutes_reminder(minutes: u64, holds_work: bool) -> String {
    if !holds_work {
        // From benchmark 2: @iris ran one turn of 92 minutes holding no task
        // and posting nothing.
        return format!(
            "\n\n[OrgBots] This turn has run {minutes} minutes and you hold no task or \
             review. Finish now: post what you found, if anything, and end the turn. To \
             keep working, claim a task first. At {} minutes holding nothing, the room \
             benches you until the owner resumes you.",
            orgbot_engine::engine::IDLE_TURN_LIMIT_MS / 60_000
        );
    }
    let mut note = format!(
        "\n\n[OrgBots] This turn has run {minutes} minutes, and teammates may be waiting \
         on it. If the task's acceptance criteria are met, finish now: record the result \
         and end the turn. Do not polish past the bar or re-run checks that already passed."
    );
    if minutes >= SPLIT_AFTER_MINUTES {
        note.push_str(
            " If the task needs much more than this, tell the coordinator now so it can be \
             split, and hand on the parts you have not started.",
        );
    }
    note
}

/// How long an agent CLI may go in one turn without using a tool or calling
/// the room, while teammates wait on it, before its turn is ended to be
/// resumed with a note to act.
///
/// From speed test 1: @grok-quick's Grok CLI reasoned for long stretches
/// without a tool call and held the 3D model for two hours while three tasks
/// waited on it. Only API replies had a limit on reasoning.
const CLI_THINKING_LIMIT: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// The same, when nobody waits on the bot: long enough for a coordinator to
/// think a plan through, which took @opus thirteen minutes in speed test 1.
const CLI_THINKING_CAP: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// How long one API reply may only reason, while teammates wait on the bot.
/// When nobody does, the provider's long default applies.
const API_THINKING_LIMIT: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// What a CLI bot is told at the start of the turn after one stopped for
/// reasoning without acting.
const THINKING_STOPPED_NOTE: &str = "[OrgBots] Your last turn was stopped after a long time \
     without a tool call. Do not reason further before acting: make the next tool call now. If \
     the task is bigger than you can finish soon, submit what works, say what is left, and ask the \
     coordinator to split it.";

/// How long the owner's message to a CLI bot waits for the bot to see it
/// through a room call before its turn is ended so the next one shows it.
const OWNER_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// What a turn tells its bot as it goes: how long it has run, and messages
/// for it that arrived since the turn began.
///
/// From benchmark 2: an agent CLI runs its own tool loop, so the reminders
/// in the room's own loop never reached it; @grok-quick ran one turn of 94
/// minutes, and the owner's "@grok-quick, what is the hold up" went unseen
/// until it ended. The room reaches a CLI through its room tools' results.
pub(crate) struct TurnSteer {
    began: std::time::Instant,
    minutes_told: u64,
    /// Through where in the room's log messages for the bot were passed on.
    relayed_through: Seq,
}

impl TurnSteer {
    pub(crate) fn new(head: Seq) -> Self {
        Self { began: std::time::Instant::now(), minutes_told: 0, relayed_through: head }
    }

    /// The time note, once every [`TURN_MINUTES_NOTE`] minutes of the turn.
    fn minutes(&mut self, engine: &orgbot_engine::Engine, agent: AgentId) -> Option<String> {
        let minutes = self.began.elapsed().as_secs() / 60;
        if minutes < self.minutes_told + TURN_MINUTES_NOTE {
            return None;
        }
        self.minutes_told = minutes - minutes % TURN_MINUTES_NOTE;
        let holds_work = engine.has_reason_to_work(agent);
        Some(turn_minutes_reminder(minutes, holds_work))
    }

    /// Messages for the bot posted since they were last passed on.
    fn messages(&mut self, engine: &orgbot_engine::Engine, spec: &AgentSpec) -> Option<String> {
        let lines: Vec<String> = engine
            .events_since(self.relayed_through)
            .filter_map(|envelope| match &envelope.event {
                orgbot_proto::Event::MessagePosted { message } if message.wakes(spec.id, &spec.roles) => {
                    let from = match &message.author {
                        orgbot_proto::Author::Human { name } => name.clone(),
                        orgbot_proto::Author::Agent { agent } => engine
                            .agent(*agent)
                            .map_or_else(|| "a teammate".into(), |other| format!("@{}", other.handle)),
                        orgbot_proto::Author::System => "the room".into(),
                    };
                    let body = message.body.trim();
                    let body = match body.char_indices().nth(1200) {
                        Some((cut, _)) => format!("{}…", &body[..cut]),
                        None => body.to_string(),
                    };
                    Some(format!("From {from}: {body}"))
                }
                _ => None,
            })
            .collect();
        self.relayed_through = engine.head();
        (!lines.is_empty()).then(|| {
            format!(
                "\n\n[OrgBots] New for you since this turn began. Answer anything that asks you \
                 something with post_message now, then carry on:\n{}",
                lines.join("\n")
            )
        })
    }

    /// Whether the owner has written to the bot at least [`OWNER_WAIT`] ago
    /// and it has not been passed on yet.
    fn owner_kept_waiting(&self, engine: &orgbot_engine::Engine, spec: &AgentSpec) -> bool {
        let before = orgbot_proto::Timestamp::now().plus_millis(-(OWNER_WAIT.as_millis() as i64));
        engine.events_since(self.relayed_through).any(|envelope| {
            matches!(&envelope.event, orgbot_proto::Event::MessagePosted { message }
                if matches!(message.author, orgbot_proto::Author::Human { .. })
                    && message.wakes(spec.id, &spec.roles)
                    && message.sent_at <= before)
        })
    }

    /// Everything the bot should hear now, appended to a room tool's result.
    fn note(&mut self, engine: &orgbot_engine::Engine, spec: &AgentSpec) -> String {
        let mut note = self.messages(engine, spec).unwrap_or_default();
        note.push_str(&self.minutes(engine, spec.id).unwrap_or_default());
        note
    }
}

/// The same call made again and again in one turn.
#[derive(Default)]
struct Repeats {
    last: Option<(String, String)>,
    count: u32,
}

impl Repeats {
    /// Note the call, and say so when it is the third identical one in a row.
    fn note(&mut self, name: &str, input: &serde_json::Value) -> Option<String> {
        let call = (name.to_string(), input.to_string());
        if self.last.as_ref() == Some(&call) {
            self.count += 1;
        } else {
            self.last = Some(call);
            self.count = 1;
        }
        (self.count >= 3).then(|| {
            format!(
                "\n\n[OrgBots] That is the same `{name}` call {} times in a row, and \
                 its result will not change. Try something different, or record what \
                 you have.",
                self.count
            )
        })
    }
}

/// What a bot holding no task is told when it tries to change the project.
const NO_TASK_NO_CHANGES: &str = "You hold no task, so changes you make would reach the owner's project \
     with nobody reviewing them. Claim an open task (or accept a handoff) first. If you found something \
     that needs fixing, tell the coordinators in one message so they can open a task for it.";

/// Whether `agent` may use the workspace tool `name` that changes the project.
fn may_write(engine: &orgbot_engine::Engine, agent: AgentId, name: &str) -> bool {
    match name {
        "workspace_land_commits" | "workspace_push_commits" => engine.may_land(agent),
        _ => engine.may_change_project(agent),
    }
}

fn is_workspace_write_tool(name: &str) -> bool {
    matches!(
        name,
        "workspace_import_attachment"
            | "workspace_write"
            | "workspace_mkdir"
            | "workspace_run"
            | "workspace_land_commits"
            | "workspace_push_commits"
    )
}

/// Land commits `actor` bundled in the project, once the owner has approved
/// exactly those commits onto that branch for the task it holds.
///
/// The workspace inspects the bundle on the computer first: what it holds,
/// where the branch stands, whether the project already reads as the commits
/// leave it. Without the owner's approval for exactly that, the owner is asked
/// (or the room's access mode answers); with it, the workspace lands it after
/// checking nothing has moved since, and the room is told.
async fn land_commits(
    engine: &SharedEngine,
    workspace: &crate::workspace::Workspace,
    actor: AgentId,
    input: &serde_json::Value,
    grants: orgbot_providers::SandboxGrants,
) -> orgbot_tools::ToolOutcome {
    use orgbot_tools::ToolOutcome;
    let text = |key: &str| input.get(key).and_then(serde_json::Value::as_str).unwrap_or("").trim().to_string();
    let (bundle, branch, reason) = (text("bundle"), text("branch"), text("reason"));
    let checks = match text("checks") {
        raw if raw.is_empty() => None,
        raw => match serde_json::from_value::<orgbot_proto::ArtifactId>(serde_json::json!(raw)) {
            Ok(id) => Some(id),
            Err(_) => return ToolOutcome::error("checks must be the id of an artifact on your task."),
        },
    };
    let Ok(task) = serde_json::from_value::<orgbot_proto::TaskId>(serde_json::json!(text("task"))) else {
        return ToolOutcome::error("Name the task you hold that these commits are for.");
    };
    let inspected = workspace
        .call_as(actor, "workspace_land_commits", &serde_json::json!({"bundle": bundle, "branch": branch}), grants.clone())
        .await;
    if inspected.is_error {
        return inspected;
    }
    let Ok(found) = serde_json::from_str::<serde_json::Value>(&inspected.content) else {
        return ToolOutcome::error("The workspace could not describe the bundle.");
    };
    let field = |key: &str| found.get(key).and_then(serde_json::Value::as_str).unwrap_or("").to_string();
    let (base, head, sha) = (field("base"), field("head"), field("bundle_sha256"));
    let commits: Vec<String> = found
        .get("commits")
        .and_then(serde_json::Value::as_array)
        .map(|list| list.iter().filter_map(|c| c.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let count = commits.len();
    let approved = |engine: &SharedEngine| engine.with(|e| e.landing_approved(task, actor, &branch, &base, &head));
    if !approved(engine) {
        let asked = engine.with(|e| {
            e.apply(Command::RequestLanding {
                by: actor,
                task,
                branch: branch.clone(),
                base: base.clone(),
                head: head.clone(),
                commits,
                changes: field("changes"),
                checks,
                reason,
            })
        });
        if let Err(error) = asked {
            return ToolOutcome::error(error.to_string());
        }
        if !approved(engine) {
            return ToolOutcome::ok(format!(
                "Asked the owner to land these {count} commit(s) on {branch}. You will be woken when \
                 they answer; then call workspace_land_commits again with the same bundle and branch. \
                 Do not ask again meanwhile."
            ));
        }
    }
    let landed = workspace
        .call_as(
            actor,
            "workspace_land_commits",
            &serde_json::json!({
                "bundle": bundle, "branch": branch, "land": true,
                "expect_base": base, "expect_head": head, "expect_sha256": sha,
            }),
            grants,
        )
        .await;
    if !landed.is_error {
        engine.with(|e| {
            let handle = e.agent(actor).map(|spec| spec.handle.clone()).unwrap_or_default();
            let approval = if e.landing_granted_by_mode(task, actor, &branch, &base, &head) {
                format!("as the room's {} mode allows", e.room().spec.access.mode.label())
            } else {
                "as the owner approved".to_string()
            };
            let note = orgbot_proto::chat::ChatMessage::system(
                e.room().id,
                format!(
                    "@{handle} landed {count} commit(s) on {branch}, moving it from {} to {} {approval}. Nothing was pushed.",
                    base.get(..7).unwrap_or(&base),
                    head.get(..7).unwrap_or(&head)
                ),
            )
            .about_task(task);
            let _ = e.apply(Command::PostMessage { message: Box::new(note) });
        });
    }
    landed
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
            // A request that ran into the client's total time limit is not
            // retried: it was producing, slowly, and would run as long again.
            Err(error)
                if attempt < MAX_RETRIES
                    && error.is_retryable()
                    && !matches!(&error, ProviderError::Transport { source, .. } if source.is_timeout())
                    && !matches!(
                        error,
                        ProviderError::RateLimited { .. } | ProviderError::Allowance { .. }
                    ) =>
            {
                attempt += 1;
                tracing::warn!(provider = provider.name(), attempt, %error, "reconnecting interrupted model response");
                if let Some(tx) = &sink {
                    let _ = tx
                        .send(StreamEvent::Retry {
                            attempt,
                            max_retries: MAX_RETRIES,
                        })
                        .await;
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

/// What a bot is told about a call its reply was cut off inside.
const TRUNCATED_CALL_NOTE: &str = "Your reply reached its output limit inside this call, so its arguments \
     may be incomplete and it was not run. Make it again in smaller parts: for a large file, write \
     it in several workspace_write calls or build it with workspace_run.";

/// How long a CLI asked to end its turn early may take to stop and finish it
/// (its merge included) before the room stops waiting.
const END_TURN_GRACE: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// What the room-MCP calls of one episode amounted to.
#[derive(Debug, Default)]
struct McpTally {
    ok: usize,
    err: usize,
    /// Successful calls that changed something (see [`QUIET_TOOLS`]).
    changed: usize,
    /// A post_message call succeeded during the episode.
    posted: bool,
    /// The episode was ended for the owner's message; see [`OWNER_WAIT`].
    interrupted: bool,
    /// The episode was ended because the CLI reasoned without acting; see
    /// [`CLI_THINKING_LIMIT`].
    thinking_stopped: bool,
}

/// Run one provider turn while also serving inbound room-MCP calls.
///
/// Agent CLIs (claude -p, codex exec, grok -p) run their own tool loop inside
/// one [crate::mcp::McpSession]; every tools/call they make arrives here while
/// the provider future is still running and is dispatched against the engine
/// immediately, so the bot acts on the board mid-episode. Returns the provider
/// result plus how many MCP calls succeeded or were rejected, which the caller
/// folds into the turn report so a board action counts as progress.
#[allow(clippy::too_many_arguments)]
async fn complete_with_mcp(
    provider: &Arc<dyn Provider>,
    tools: &[ToolDef],
    engine: &SharedEngine,
    spec: &AgentSpec,
    request: CompletionRequest,
    sink: Option<mpsc::Sender<StreamEvent>>,
    rx: &mut mpsc::Receiver<crate::mcp::McpCall>,
    servers: Option<&crate::mcp_client::McpClient>,
    workspace: Option<&crate::workspace::Workspace>,
    capabilities: &mut crate::capabilities::Capabilities,
    steer: &mut TurnSteer,
) -> (Result<CompletionResponse, ProviderError>, McpTally) {
    let mut session = crate::mcp::McpSession::new(tools.to_vec());
    let model = request.model.clone();
    let mut turn = std::pin::pin!(provider.complete(request, sink));
    let mut tally = McpTally::default();
    let mut check = tokio::time::interval_at(
        tokio::time::Instant::now() + OWNER_WAIT / 4,
        OWNER_WAIT / 4,
    );
    let mut progress = (provider.live_activity(), tokio::time::Instant::now());
    // Kept current through the turn: a bot that claims a task mid-turn has
    // its changes merged at the end.
    let hold_merge = |engine: &SharedEngine| {
        provider.hold_merge(!engine.with(|engine| engine.may_change_project(spec.id)));
    };
    hold_merge(engine);
    loop {
        tokio::select! {
            response = &mut turn => {
                while let Ok(call) = rx.try_recv() {
                    let reply = dispatch_room_mcp(&mut session, &call.request, engine, spec,
                        servers, workspace, &mut tally, capabilities, steer).await;
                    let _ = call.reply.send(reply);
                }
                return (response, tally);
            }
            inbound = rx.recv() => {
                let Some(call) = inbound else { return (turn.await, tally) };
                progress.1 = tokio::time::Instant::now();
                let reply = dispatch_room_mcp(&mut session, &call.request, engine, spec,
                    servers, workspace, &mut tally, capabilities, steer).await;
                let _ = call.reply.send(reply);
                hold_merge(engine);
            }
            // Dropping the episode stops the CLI; its session was saved as it
            // began, so the next turn resumes it.
            _ = check.tick() => {
                hold_merge(engine);
                let activity = provider.live_activity();
                if activity != progress.0 {
                    progress = (activity, tokio::time::Instant::now());
                }
                let limit = if engine.with(|engine| engine.others_wait_on(spec.id)) {
                    CLI_THINKING_LIMIT
                } else {
                    CLI_THINKING_CAP
                };
                // The clock stops while the CLI runs a tool of its own: a long
                // test run is acting, not reasoning.
                if provider.tool_in_flight() {
                    progress.1 = tokio::time::Instant::now();
                }
                let thinking_only = activity.is_some() && progress.1.elapsed() >= limit;
                if thinking_only {
                    tracing::warn!(
                        agent = %spec.handle,
                        minutes = limit.as_secs() / 60,
                        "an agent CLI went without acting; stopping its turn"
                    );
                    tally.thinking_stopped = true;
                }
                if thinking_only || engine.with(|engine| steer.owner_kept_waiting(engine, spec)) {
                    tally.interrupted = !thinking_only;
                    // The CLI stops and finishes its turn: its edits merge,
                    // its usage and words are kept. From the 13 November
                    // audit: the turn was dropped and all of that was lost.
                    provider.end_turn_early();
                    let finished = tokio::time::timeout(END_TURN_GRACE, &mut turn).await;
                    let mut stopped = match finished {
                        Ok(Ok(response)) => response,
                        _ => CompletionResponse {
                            model,
                            content: Vec::new(),
                            stop_reason: StopReason::EndTurn,
                            usage: Usage::default(),
                        },
                    };
                    stopped.stop_reason = StopReason::Other("human_input".into());
                    return (Ok(stopped), tally);
                }
            }
        }
    }
}

/// Validate the MCP session synchronously, then await only the selected external operation.
#[allow(clippy::too_many_arguments)]
async fn dispatch_room_mcp(
    session: &mut crate::mcp::McpSession,
    request: &str,
    engine: &SharedEngine,
    spec: &AgentSpec,
    servers: Option<&crate::mcp_client::McpClient>,
    workspace: Option<&crate::workspace::Workspace>,
    tally: &mut McpTally,
    capabilities: &mut crate::capabilities::Capabilities,
    steer: &mut TurnSteer,
) -> Option<String> {
    let reply = room_mcp_reply(session, request, engine, spec, servers, workspace, tally, capabilities).await;
    // Only a reply that can carry it takes the note. From the 13 November
    // audit: the handshake, tool list and error replies used it up, and a
    // message written while the CLI started was never heard.
    if !can_carry_note(reply.as_deref()) {
        return reply;
    }
    let note = engine.with(|engine| steer.note(engine, spec));
    with_note(reply, &note)
}

/// Whether a room tool reply has a text result a note can be added to.
fn can_carry_note(reply: Option<&str>) -> bool {
    reply
        .and_then(|reply| serde_json::from_str::<serde_json::Value>(reply).ok())
        .is_some_and(|response| response.pointer("/result/content/0/text").is_some_and(serde_json::Value::is_string))
}

/// A room tool's MCP reply with `note` after its result text.
fn with_note(reply: Option<String>, note: &str) -> Option<String> {
    if note.is_empty() {
        return reply;
    }
    let mut response: serde_json::Value = serde_json::from_str(reply.as_deref()?).ok()?;
    match response.pointer_mut("/result/content/0/text") {
        Some(serde_json::Value::String(text)) => text.push_str(note),
        _ => return reply,
    }
    Some(response.to_string())
}

#[allow(clippy::too_many_arguments)]
async fn room_mcp_reply(
    session: &mut crate::mcp::McpSession,
    request: &str,
    engine: &SharedEngine,
    spec: &AgentSpec,
    servers: Option<&crate::mcp_client::McpClient>,
    workspace: Option<&crate::workspace::Workspace>,
    tally: &mut McpTally,
    capabilities: &mut crate::capabilities::Capabilities,
) -> Option<String> {
    let (actor, review_only) = (spec.id, spec.autonomy.review_only);
    capabilities.refresh(engine, actor, servers).await;
    let mut pending = None;
    let reply = session.handle(request, |name, args| {
        if crate::capabilities::handles(name)
            || servers.is_some_and(|s| s.owns(name))
            || name.starts_with("workspace_")
        {
            pending = Some((name.to_owned(), args.clone()));
            orgbot_tools::ToolOutcome::error("pending")
        } else {
            let outcome = engine.with(|e| orgbot_tools::dispatch(e, actor, name, args));
            tally.count(name, outcome.is_error);
            outcome
        }
    });
    let Some((name, args)) = pending else {
        return reply;
    };
    let outcome = if crate::capabilities::handles(&name) {
        capabilities
            .dispatch(engine, actor, review_only, servers, &name, &args)
            .await
    } else if servers.is_some_and(|s| s.owns(&name)) {
        if review_only {
            orgbot_tools::ToolOutcome::error("Review-only bots cannot call external MCP tools.")
        } else {
            orgbot_tools::ToolOutcome::error(
                "Load the selected tool with load_mcp_tool, then use call_mcp_tool.",
            )
        }
    } else if review_only && is_workspace_write_tool(&name) {
        orgbot_tools::ToolOutcome::error(
            "Review-only bots cannot change workspace files or run commands.",
        )
    } else if is_workspace_write_tool(&name) && !engine.with(|engine| may_write(engine, actor, &name)) {
        orgbot_tools::ToolOutcome::error(NO_TASK_NO_CHANGES)
    } else if let Some(workspace) = workspace {
        // A CLI bot's room workspace tools arrive here mid-episode; the
        // owner's grants apply to them as to an API bot's.
        let grants = engine.with(|engine| sandbox_grants(engine, actor));
        if name == "workspace_land_commits" {
            land_commits(engine, workspace, actor, &args, grants).await
        } else if name == "workspace_push_commits" {
            push_commits(engine, workspace, actor, &args, grants).await
        } else {
            let began = std::time::Instant::now();
            let outcome = workspace.call_as(actor, &name, &args, grants).await;
            engine.with(|engine| engine.note_workspace_time(actor, began.elapsed().as_millis() as i64));
            note_room_removals(engine, actor, &name, &outcome);
            outcome
        }
    } else {
        orgbot_tools::ToolOutcome::error("No workspace attached")
    };
    tally.count(&name, outcome.is_error);
    let mut response: serde_json::Value = serde_json::from_str(reply.as_deref()?).ok()?;
    response["result"] = serde_json::json!({
        "content": [{"type":"text", "text":outcome.content}], "isError":outcome.is_error,
    });
    Some(response.to_string())
}

impl McpTally {
    fn count(&mut self, tool: &str, failed: bool) {
        if failed {
            self.err += 1;
        } else {
            self.ok += 1;
            self.posted |= tool == "post_message";
            if !QUIET_TOOLS.contains(&tool) {
                self.changed += 1;
            }
        }
    }
}

/// Trim once the conversation passes this many turns.
const MAX_HISTORY_TURNS: usize = 60;

/// Turns retained after a trim.
const KEEP_HISTORY_TURNS: usize = 30;

/// The most text a bot carries into its next request.
///
/// A ceiling in the unit that actually costs: one turn holding a read artifact
/// can outweigh fifty holding a sentence, so a turn count alone lets a history
/// grow to megabytes while looking well within its limit. A real room reached
/// two megabytes a bot — roughly half a million tokens re-sent on every step
/// of every turn.
const MAX_HISTORY_CHARS: usize = 240_000;

/// Marker on deterministic local summaries inserted during history compaction.
const COMPACTED_HISTORY_MARKER: &str = "[OrgBots local conversation summary v1]";
/// Bound the summary so recent work retains most of the context budget.
const MAX_COMPACTED_SUMMARY_CHARS: usize = 8_000;

/// What `agent`'s sandbox may carry of the owner's right now: the bot server
/// key and lent sign-ins it was granted for the work under way, with every
/// other lendable path hidden.
pub fn sandbox_grants(engine: &orgbot_engine::Engine, agent: AgentId) -> orgbot_providers::SandboxGrants {
    orgbot_providers::SandboxGrants {
        server_access: engine.server_access_for(agent),
        lend: engine.lent_paths_for(agent),
        hide: engine.lendable_paths(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_client::{McpServerSpec, McpTransport};
    use orgbot_engine::Engine;
    use orgbot_providers::types::Role;
    use orgbot_providers::ScriptedProvider;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    #[test]
    fn a_long_turn_is_reminded_to_finish_and_a_repeated_call_is_named() {
        // From the live room: @ollama-glm's turns were cut at 24 calls with an
        // accepted review unanswered. Turns are no longer cut.
        assert!(long_turn_reminder(24, 24).is_some_and(|r| r.contains("24 model calls so far")));
        assert!(long_turn_reminder(24, 48).is_some());
        assert_eq!(long_turn_reminder(24, 25), None);
        // From the benchmark room: a 75-minute turn of a minute a call.
        assert!(turn_minutes_reminder(20, true).contains("This turn has run 20 minutes"));
        assert!(turn_minutes_reminder(20, true).contains("so it can be split"));
        assert!(!turn_minutes_reminder(10, true).contains("split"));
        // From benchmark 2: @iris ran 92 minutes holding nothing.
        assert!(turn_minutes_reminder(20, false).contains("you hold no task or review"));
        assert!(turn_minutes_reminder(20, false).contains("At 30 minutes holding nothing, the room benches you"));
        let mut repeats = Repeats::default();
        let read = json!({"path": "a.ts"});
        assert_eq!(repeats.note("workspace_read", &read), None);
        assert_eq!(repeats.note("workspace_read", &read), None);
        assert!(repeats.note("workspace_read", &read).is_some_and(|n| n.contains("3 times in a row")));
        assert_eq!(repeats.note("workspace_read", &json!({"path": "b.ts"})), None, "a different call resets it");
    }

    #[derive(Debug)]
    struct PendingProvider(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl Provider for PendingProvider {
        fn name(&self) -> &str {
            "pending-test-provider"
        }

        async fn complete(
            &self,
            _request: CompletionRequest,
            _sink: Option<mpsc::Sender<StreamEvent>>,
        ) -> orgbot_providers::Result<CompletionResponse> {
            let mut delay =
                std::pin::pin!(tokio::time::sleep(std::time::Duration::from_millis(40)));
            std::future::poll_fn(|cx| {
                self.0.fetch_add(1, Ordering::SeqCst);
                std::future::Future::poll(delay.as_mut(), cx)
            })
            .await;
            Ok(CompletionResponse {
                model: "fixture".into(),
                content: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            })
        }
    }

    #[tokio::test]
    async fn disconnected_mcp_channel_does_not_busy_poll_pending_provider() {
        let polls = Arc::new(AtomicUsize::new(0));
        let provider: Arc<dyn Provider> = Arc::new(PendingProvider(polls.clone()));
        let spec = spec();
        let engine = mcp_test_engine(&spec).await;
        let (tx, mut rx) = mpsc::channel(1);
        drop(tx);
        let (result, _) = complete_with_mcp(
            &provider,
            &[],
            &engine,
            &spec,
            CompletionRequest::new("fixture", "", vec![]),
            None,
            &mut rx,
            None,
            None,
            &mut crate::capabilities::Capabilities::default(),
            &mut TurnSteer::new(engine.with(|engine| engine.head())),
        )
        .await;
        assert!(result.is_ok());
        assert!(
            polls.load(Ordering::SeqCst) <= 4,
            "a closed MCP channel must not wake a pending provider repeatedly"
        );
    }

    #[derive(Debug)]
    struct FailAfterToolResponse(AtomicUsize);

    #[async_trait::async_trait]
    impl Provider for FailAfterToolResponse {
        fn name(&self) -> &str {
            "fail-after-tool"
        }

        async fn complete(
            &self,
            _request: CompletionRequest,
            _sink: Option<mpsc::Sender<StreamEvent>>,
        ) -> orgbot_providers::Result<CompletionResponse> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(CompletionResponse {
                    model: "test-model".into(),
                    content: vec![ContentBlock::ToolUse {
                        id: "tool-1".into(),
                        name: "unknown_tool".into(),
                        input: json!({}),
                    }],
                    stop_reason: StopReason::ToolUse,
                    usage: Usage {
                        reported: true,
                        complete: true,
                        calls: 1,
                        input_tokens: 13,
                        output_tokens: 5,
                        ..Usage::default()
                    },
                })
            } else {
                Err(ProviderError::Unsupported(
                    "synthetic second-call failure".into(),
                ))
            }
        }
    }

    #[derive(Debug)]
    struct CancelAfterToolResponse {
        calls: AtomicUsize,
        second_started: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl Provider for CancelAfterToolResponse {
        fn name(&self) -> &str {
            "cancel-after-tool"
        }

        async fn complete(
            &self,
            _request: CompletionRequest,
            _sink: Option<mpsc::Sender<StreamEvent>>,
        ) -> orgbot_providers::Result<CompletionResponse> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(CompletionResponse {
                    model: "test-model".into(),
                    content: vec![ContentBlock::ToolUse {
                        id: "tool-1".into(),
                        name: "unknown_tool".into(),
                        input: json!({}),
                    }],
                    stop_reason: StopReason::ToolUse,
                    usage: Usage {
                        reported: true,
                        complete: true,
                        calls: 1,
                        input_tokens: 13,
                        output_tokens: 5,
                        ..Usage::default()
                    },
                })
            } else {
                self.second_started.notify_one();
                std::future::pending().await
            }
        }
    }

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

    #[tokio::test]
    async fn completed_call_usage_survives_a_later_hard_failure_and_archive_replay() {
        use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};

        let mut agent = spec();
        agent.provider = ProviderRef::Anthropic {
            model: "test-model".into(),
        };
        let mut room = Engine::new(RoomSpec {
            name: "usage recovery".into(),
            goal: "retain completed provider usage".into(),
            roster: vec![],
            completion: CompletionRule::Manual,
            limits: RoomLimits::default(),
            access: Default::default(),
        });
        room.apply(Command::JoinAgent {
            spec: Box::new(agent.clone()),
        })
        .unwrap();
        room.apply(Command::StartRoom).unwrap();
        let engine = SharedEngine::new(room);
        let provider = Arc::new(FailAfterToolResponse(AtomicUsize::new(0)));
        let mut bot = Bot::new(agent, provider);

        assert!(bot.take_turn(&engine, None).await.is_err());
        let (room, log, digest) = engine.with(|engine| {
            (
                engine.room().clone(),
                engine.log().to_vec(),
                engine.digest(),
            )
        });
        let agent_spend = digest
            .agent_spend
            .first()
            .expect("agent spend was checkpointed");
        assert_eq!(agent_spend.turns, 1);
        assert_eq!(agent_spend.input_tokens, 13);
        assert_eq!(agent_spend.output_tokens, 5);
        assert!(agent_spend.usage_known);
        assert!(agent_spend.usage_complete);

        let replayed = Engine::from_archive(room, log).expect("usage event replays");
        let replayed_agent = replayed
            .digest()
            .agent_spend
            .into_iter()
            .next()
            .expect("replayed agent spend");
        assert_eq!(replayed_agent.input_tokens, 13);
        assert_eq!(replayed_agent.output_tokens, 5);
        assert_eq!(replayed_agent.turns, 1);
        assert!(replayed_agent.usage_complete);
    }

    #[tokio::test]
    async fn completed_call_usage_survives_cancellation_of_the_next_provider_call() {
        use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};

        let mut agent = spec();
        agent.provider = ProviderRef::Anthropic {
            model: "test-model".into(),
        };
        let mut room = Engine::new(RoomSpec {
            name: "cancelled usage".into(),
            goal: "retain completed provider usage".into(),
            roster: vec![],
            completion: CompletionRule::Manual,
            limits: RoomLimits::default(),
            access: Default::default(),
        });
        room.apply(Command::JoinAgent {
            spec: Box::new(agent.clone()),
        })
        .unwrap();
        room.apply(Command::StartRoom).unwrap();
        let engine = SharedEngine::new(room);
        let second_started = Arc::new(Notify::new());
        let provider = Arc::new(CancelAfterToolResponse {
            calls: AtomicUsize::new(0),
            second_started: second_started.clone(),
        });
        let mut bot = Bot::new(agent, provider);
        let ready = second_started.notified();
        let turn_engine = engine.clone();
        let running = tokio::spawn(async move { bot.take_turn(&turn_engine, None).await });
        tokio::time::timeout(std::time::Duration::from_secs(2), ready)
            .await
            .expect("the second provider call started");
        running.abort();
        let _ = running.await;

        let (room, log, digest) = engine.with(|engine| {
            (
                engine.room().clone(),
                engine.log().to_vec(),
                engine.digest(),
            )
        });
        let agent_spend = digest
            .agent_spend
            .first()
            .expect("agent usage was checkpointed");
        assert_eq!(agent_spend.input_tokens, 13);
        assert_eq!(agent_spend.output_tokens, 5);
        assert_eq!(agent_spend.turns, 1);
        let replayed = Engine::from_archive(room, log).expect("usage event replays");
        assert_eq!(replayed.digest().agent_spend[0].input_tokens, 13);
    }

    fn bot() -> Bot {
        Bot::new(spec(), Arc::new(ScriptedProvider::always_says("ok")))
    }

    /// From the 13 November audit: a CLI bot's room workspace tools worked in
    /// a copy that never had its CLI's edits.
    #[test]
    fn an_agent_cli_bot_is_offered_only_the_commit_tools() {
        use crate::workspace::{Workspace, WorkspaceCapabilities, WorkspaceSpec};
        let cli = ProviderRef::CliAgent {
            cli: orgbot_proto::agent::CliKind::Claude,
            model: String::new(),
            workdir: String::new(),
            policy: orgbot_proto::agent::AgentPolicy::WorkspaceWrite,
            mcp: true,
            effort: String::new(),
        };
        let mut bot = Bot::new(AgentSpec::new("c1", "Claude", cli), Arc::new(ScriptedProvider::always_says("ok")));
        let (tx, _calls) = mpsc::channel(1);
        bot.attach_workspace(Workspace::with_capabilities(
            WorkspaceSpec { label: "test".into(), root: "/work".into(), writable: true, commands: true },
            tx,
            WorkspaceCapabilities { verification: true, private_home: false, ..WorkspaceCapabilities::default() },
        ));
        let names: Vec<&str> = bot.tools.iter().map(|tool| tool.name.as_str()).filter(|name| name.starts_with("workspace_")).collect();
        assert!(names.iter().all(|name| matches!(*name, "workspace_land_commits" | "workspace_push_commits")), "{names:?}");
    }

    #[tokio::test]
    async fn refreshed_project_memory_stays_in_situation_not_system_prompt() {
        use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};

        static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
        let memory_root = std::env::temp_dir().join(format!(
            "orgbot-memory-refresh-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&memory_root).unwrap();

        let agent = spec();
        let provider = Arc::new(ScriptedProvider::always_says("ok"));
        let mut room = Engine::new(RoomSpec {
            name: "memory-refresh".into(),
            goal: "runtime memory refresh architecture".into(),
            roster: vec![],
            completion: CompletionRule::Manual,
            limits: RoomLimits::default(),
            access: Default::default(),
        });
        room.apply(Command::JoinAgent {
            spec: Box::new(agent.clone()),
        })
        .unwrap();
        room.apply(Command::StartRoom).unwrap();
        room.archive()
            .configure_memory(&memory_root, "memory-refresh-project")
            .unwrap();
        room.archive()
            .remember(
                "Runtime memory refresh architecture",
                "The runtime memory refresh architecture uses old snapshot markers.",
                Some("prior-room"),
                Some("memory-task"),
            )
            .unwrap();

        let engine = SharedEngine::new(room);
        let mut bot = Bot::new(agent, provider.clone());
        bot.take_turn(&engine, None).await.unwrap();
        let first_request = provider.requests().pop().unwrap();
        let first_situation = first_request
            .messages
            .iter()
            .find(|turn| turn.role == Role::User)
            .unwrap();
        assert!(first_situation.content.iter().any(|block| matches!(
            block,
            ContentBlock::Text { text }
                if text.contains("old snapshot markers")
        )));

        engine.with(|engine| {
            engine
                .archive()
                .remember(
                    "Runtime memory refresh architecture",
                    "The runtime memory refresh architecture now supersedes old snapshots and carries updated notes in the current situation.",
                    Some("prior-room"),
                    Some("memory-task"),
                )
                .unwrap();
        });
        bot.take_turn(&engine, None).await.unwrap();
        let requests = provider.requests();
        let second_request = requests.last().unwrap();
        assert_eq!(
            first_request.system.as_bytes(),
            second_request.system.as_bytes()
        );

        let user_situations = second_request
            .messages
            .iter()
            .filter(|turn| turn.role == Role::User)
            .collect::<Vec<_>>();
        assert_eq!(user_situations.len(), 2);
        assert!(matches!(
            &user_situations[0].content[..],
            [ContentBlock::Text { text }] if text == prompt::SUPERSEDED_SITUATION
        ));
        assert!(user_situations[1].content.iter().any(|block| matches!(
            block,
            ContentBlock::Text { text }
                if text.contains("now supersedes old snapshots")
                    && !text.contains("uses old snapshot markers")
        )));

        std::fs::remove_dir_all(memory_root).unwrap();
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

    #[tokio::test]
    async fn review_only_bots_can_verify_but_cannot_write_or_run_workspace_tools() {
        use crate::workspace::{Workspace, WorkspaceCapabilities, WorkspaceSpec};
        use orgbot_engine::Engine;
        use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};

        let provider = Arc::new(ScriptedProvider::new(vec![
            orgbot_providers::types::CompletionResponse {
                model: "test".into(),
                content: vec![
                    ContentBlock::ToolUse {
                        id: "write1".into(),
                        name: "workspace_write".into(),
                        input: json!({"path":"report.md", "content":"changed"}),
                    },
                    ContentBlock::ToolUse {
                        id: "verify1".into(),
                        name: "workspace_verify".into(),
                        input: json!({"command":"python3 -B -m unittest -v"}),
                    },
                ],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            },
            orgbot_providers::types::CompletionResponse {
                model: "test".into(),
                content: vec![ContentBlock::text("review complete")],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ]));
        let mut spec = spec();
        spec.autonomy.review_only = true;
        let mut room = Engine::new(RoomSpec {
            name: "review-only".into(),
            goal: "inspect safely".into(),
            roster: vec![],
            completion: CompletionRule::Manual,
            limits: RoomLimits::default(),
            access: Default::default(),
        });
        room.apply(Command::JoinAgent {
            spec: Box::new(spec.clone()),
        })
        .unwrap();
        room.apply(Command::StartRoom).unwrap();
        let engine = SharedEngine::new(room);
        let mut bot = Bot::new(spec, provider);
        let (tx, mut calls) = mpsc::channel(2);
        bot.attach_workspace(Workspace::with_capabilities(
            WorkspaceSpec {
                label: "test".into(),
                root: "/work".into(),
                writable: true,
                commands: true,
            },
            tx,
            WorkspaceCapabilities {
                verification: true,
                private_home: false,
                ..WorkspaceCapabilities::default()
            },
        ));

        let listed: Vec<_> = bot.tools.iter().map(|tool| tool.name.as_str()).collect();
        assert!(listed.contains(&"workspace_read"));
        assert!(listed.contains(&"workspace_verify"));
        for blocked in ["workspace_write", "workspace_mkdir", "workspace_run"] {
            assert!(!listed.contains(&blocked));
        }

        let host = async {
            let call = calls.recv().await.unwrap();
            assert_eq!(call.name, "workspace_verify");
            call.reply
                .send(orgbot_tools::ToolOutcome::ok("tests passed"))
                .unwrap();
        };
        let (report, ()) = tokio::join!(bot.take_turn(&engine, None), host);
        let report = report.unwrap();
        assert_eq!(report.tool_errors, 1);
        assert!(
            calls.try_recv().is_err(),
            "blocked mutation never reaches the workspace"
        );
        assert_eq!(report.said.as_deref(), Some("review complete"));
    }

    #[tokio::test]
    async fn a_bot_whose_turns_change_nothing_posts_its_status_once_until_someone_asks() {
        // From the live room: "Aece, nothing needs you" turn after turn.
        use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
        let agent = spec();
        let actor = agent.id;
        let mut room = Engine::new(RoomSpec {
            name: "quiet".into(),
            goal: "wait well".into(),
            roster: vec![],
            completion: CompletionRule::Manual,
            limits: RoomLimits::default(),
            access: Default::default(),
        });
        room.apply(Command::JoinAgent { spec: Box::new(agent.clone()) }).unwrap();
        room.apply(Command::StartRoom).unwrap();
        let engine = SharedEngine::new(room);
        let mut bot = Bot::new(agent, Arc::new(ScriptedProvider::always_says("Aece, nothing needs you.")));
        let posted = |engine: &SharedEngine| {
            engine.with(|e| e.transcript().iter().filter(|m| m.author.agent() == Some(actor)).count())
        };
        bot.take_turn(&engine, None).await.unwrap();
        bot.take_turn(&engine, None).await.unwrap();
        assert_eq!(posted(&engine), 1, "the same do-nothing status is not posted twice");

        engine.with(|e| {
            let room = e.room_id();
            let message = orgbot_proto::ChatMessage::human(room, "Aece", "@w1 anything for me?").to_agents(vec![actor]);
            e.apply(Command::PostMessage { message: Box::new(message) }).unwrap();
        });
        bot.take_turn(&engine, None).await.unwrap();
        assert_eq!(posted(&engine), 2, "asked directly, it answers");
        bot.take_turn(&engine, None).await.unwrap();
        assert_eq!(posted(&engine), 2);
    }

    #[tokio::test]
    async fn a_reply_cut_off_at_the_output_limit_does_not_end_the_turn() {
        // From the benchmark room: three turns of @ollama-glm each ended on
        // one reply cut at the 16,000-token limit, with nothing done.
        use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
        let agent = spec();
        let mut room = Engine::new(RoomSpec {
            name: "cut".into(),
            goal: "finish the review".into(),
            roster: vec![],
            completion: CompletionRule::Manual,
            limits: RoomLimits::default(),
            access: Default::default(),
        });
        room.apply(Command::JoinAgent { spec: Box::new(agent.clone()) }).unwrap();
        room.apply(Command::StartRoom).unwrap();
        let engine = SharedEngine::new(room);
        let reply = |text: &str, stop_reason| CompletionResponse {
            model: "test".into(),
            content: vec![ContentBlock::Text { text: text.into() }],
            stop_reason,
            usage: Usage::default(),
        };
        let provider = Arc::new(ScriptedProvider::new(vec![
            reply("thinking…", StopReason::MaxTokens),
            reply("Done: the review is recorded.", StopReason::EndTurn),
        ]));
        let mut bot = Bot::new(agent.clone(), provider.clone());
        let report = bot.take_turn(&engine, None).await.unwrap();
        assert_eq!(provider.requests().len(), 2, "the turn carried on past the cut");
        assert_eq!(report.said.as_deref(), Some("Done: the review is recorded."));
        let asked = provider.requests()[1].messages.iter().any(|turn| format!("{turn:?}").contains("reached the output limit, or reasoned too long"));
        assert!(asked, "the model is told why it is asked again");

        // It does not carry on for ever.
        let provider = Arc::new(ScriptedProvider::new(vec![reply("thinking…", StopReason::MaxTokens); 4]));
        let mut bot = Bot::new(agent, provider.clone());
        bot.take_turn(&engine, None).await.unwrap();
        assert_eq!(provider.requests().len(), 1 + MAX_CUT_OFF_RETRIES as usize);
    }

    #[tokio::test]
    async fn landing_asks_the_owner_once_then_lands_exactly_what_was_approved() {
        // From the live room: a bot's commits stayed in its own copy, so it
        // handed the owner a bundle and a script to run.
        use crate::workspace::{Workspace, WorkspaceCapabilities, WorkspaceSpec};
        use orgbot_proto::decision::{Answer, LAND_COMMITS_ALLOW};
        use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
        let agent = spec();
        let actor = agent.id;
        let mut room = Engine::new(RoomSpec {
            name: "landing".into(),
            goal: "commit the release".into(),
            roster: vec![],
            completion: CompletionRule::Manual,
            limits: RoomLimits::default(),
            access: Default::default(),
        });
        room.apply(Command::JoinAgent { spec: Box::new(agent) }).unwrap();
        room.apply(Command::StartRoom).unwrap();
        let created = room
            .apply(Command::CreateTask {
                title: "commit in separate commits".into(),
                brief: String::new(),
                by: None,
                required_role: None,
                priority: orgbot_proto::task::Priority::Normal,
                acceptance_criteria: vec![],
                depends_on: vec![],
                origin: orgbot_proto::graph::TaskOrigin::plain(),
            })
            .unwrap();
        let task = created
            .iter()
            .find_map(|e| match &e.event {
                orgbot_proto::Event::TaskCreated { task } => Some(task.id),
                _ => None,
            })
            .unwrap();
        for command in [Command::OpenTask { task }, Command::ClaimTask { task, by: actor }, Command::StartTask { task, by: actor }] {
            room.apply(command).unwrap();
        }
        let engine = SharedEngine::new(room);
        let (tx, mut calls) = mpsc::channel(4);
        let workspace = Workspace::with_capabilities(
            WorkspaceSpec { label: "test".into(), root: "/work".into(), writable: true, commands: true },
            tx,
            WorkspaceCapabilities { git_read: true, ..WorkspaceCapabilities::default() },
        );
        assert!(workspace.definitions().iter().any(|tool| tool.name == "workspace_land_commits"));
        let (base, head) = ("a".repeat(40), "b".repeat(40));
        let found = json!({
            "branch": "main", "base": base, "head": head, "bundle_sha256": "f00",
            "commits": ["bbbbbbb tests: add the squad test", "ccccccc tests: add the duration test"],
            "changes": "2 files changed", "checked_out": true,
        });
        let host = tokio::spawn(async move {
            let mut seen = Vec::new();
            while let Some(call) = calls.recv().await {
                assert_eq!(call.name, "workspace_land_commits");
                let reply = if call.arguments.get("land").is_some() {
                    orgbot_tools::ToolOutcome::ok(json!({"landed": true}).to_string())
                } else {
                    orgbot_tools::ToolOutcome::ok(found.to_string())
                };
                seen.push(call.arguments.clone());
                call.reply.send(reply).unwrap();
            }
            seen
        });
        let input = json!({
            "bundle": "artifacts/land.bundle", "branch": "main", "task": task,
            "reason": "the owner asked for separate commits",
        });
        let first = land_commits(&engine, &workspace, actor, &input, Default::default()).await;
        assert!(!first.is_error && first.content.contains("Asked the owner"), "{}", first.content);
        let decision = engine.with(|e| e.decisions().pending().next().map(|d| d.id)).unwrap();
        engine
            .with(|e| {
                e.apply(Command::AnswerDecision {
                    decision,
                    answer: Answer { chosen: LAND_COMMITS_ALLOW.into(), note: String::new(), at: orgbot_proto::Timestamp::now() },
                })
            })
            .unwrap();
        let second = land_commits(&engine, &workspace, actor, &input, Default::default()).await;
        assert!(!second.is_error, "{}", second.content);
        drop(workspace);
        let seen = host.await.unwrap();
        assert_eq!(seen.len(), 3, "inspect, inspect again, land: {seen:?}");
        assert!(seen[..2].iter().all(|args| args.get("land").is_none()), "nothing lands before the owner's yes");
        assert_eq!(seen[2]["expect_base"], base.as_str());
        assert_eq!(seen[2]["expect_head"], head.as_str());
        assert_eq!(seen[2]["expect_sha256"], "f00");
        let told = engine.with(|e| e.transcript().iter().any(|m| m.body.contains("landed 2 commit(s) on main")));
        assert!(told, "the room is told what landed");
    }

    async fn local_mcp_server() -> (McpServerSpec, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed_calls = calls.clone();
        let server = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let mut stream = BufReader::new(stream);
                let mut request_line = String::new();
                if stream.read_line(&mut request_line).await.unwrap_or(0) == 0 {
                    continue;
                }
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if stream.read_line(&mut line).await.unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; length];
                if stream.read_exact(&mut body).await.is_err() {
                    continue;
                }
                let Ok(request) = serde_json::from_slice::<serde_json::Value>(&body) else {
                    continue;
                };
                let id = request.get("id").cloned();
                let result = match request.get("method").and_then(|v| v.as_str()) {
                    Some("initialize") => json!({
                        "protocolVersion":"2025-06-18",
                        "capabilities":{},
                        "serverInfo":{"name":"test-mcp","version":"1"}
                    }),
                    Some("tools/list") => json!({"tools":[{
                        "name":"mutate",
                        "description":"A test-only external mutation",
                        "inputSchema":{"type":"object"}
                    }]}),
                    Some("tools/call") => {
                        observed_calls.fetch_add(1, Ordering::SeqCst);
                        json!({"content":[{"type":"text","text":"called external MCP mutation"}]})
                    }
                    _ => json!({}),
                };
                let response = id.map(|id| json!({"jsonrpc":"2.0", "id":id, "result":result}));
                let response_body = response.map_or_else(String::new, |v| v.to_string());
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                );
                let _ = stream.get_mut().write_all(reply.as_bytes()).await;
                let _ = stream.get_mut().flush().await;
            }
        });
        (
            McpServerSpec {
                name: "external".into(),
                transport: McpTransport::Http {
                    url: format!("http://{address}/mcp"),
                    headers: Default::default(),
                },
            },
            calls,
            server,
        )
    }

    fn mcp_call_provider() -> Arc<dyn Provider> {
        Arc::new(ScriptedProvider::new(vec![
            CompletionResponse {
                model: "test".into(),
                content: vec![ContentBlock::ToolUse {
                    id: "mcp1".into(),
                    name: "external__mutate".into(),
                    input: json!({}),
                }],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            },
            CompletionResponse {
                model: "test".into(),
                content: vec![ContentBlock::text("turn complete")],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ]))
    }

    async fn mcp_test_engine(spec: &AgentSpec) -> SharedEngine {
        use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
        let mut room = Engine::new(RoomSpec {
            name: "mcp-review-gate".into(),
            goal: "verify external MCP access".into(),
            roster: vec![],
            completion: CompletionRule::Manual,
            limits: RoomLimits::default(),
            access: Default::default(),
        });
        room.apply(Command::JoinAgent {
            spec: Box::new(spec.clone()),
        })
        .unwrap();
        room.apply(Command::StartRoom).unwrap();
        SharedEngine::new(room)
    }

    /// From speed test 1: @grok-quick's CLI reasoned for long stretches
    /// without a tool call and held the 3D model for two hours while three
    /// tasks waited on it; @opus thought its plan through for thirteen
    /// minutes with nobody waiting, and must not be cut short.
    #[tokio::test(start_paused = true)]
    async fn a_cli_that_only_reasons_is_stopped_only_while_teammates_wait_on_it() {
        #[derive(Debug)]
        struct Cli(Arc<AtomicUsize>, bool, tokio::sync::Notify);
        #[async_trait::async_trait]
        impl Provider for Cli {
            fn name(&self) -> &str {
                "grok-cli"
            }
            fn live_activity(&self) -> Option<usize> {
                // An acting CLI uses a tool every time it is asked.
                Some(if self.1 { self.0.fetch_add(1, Ordering::SeqCst) + 1 } else { self.0.load(Ordering::SeqCst) })
            }
            fn end_turn_early(&self) {
                self.2.notify_one();
            }
            async fn complete(
                &self,
                _request: CompletionRequest,
                _sink: Option<mpsc::Sender<StreamEvent>>,
            ) -> orgbot_providers::Result<CompletionResponse> {
                // Asked to end early, it finishes its turn with what it has.
                let text = tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(15 * 60)) => "done",
                    _ = self.2.notified() => "stopped part way",
                };
                Ok(CompletionResponse {
                    model: "grok".into(),
                    content: vec![ContentBlock::text(text)],
                    stop_reason: StopReason::EndTurn,
                    usage: Default::default(),
                })
            }
        }
        let run = |engine: SharedEngine, spec: AgentSpec, acting: bool| async move {
            let provider: Arc<dyn Provider> = Arc::new(Cli(Arc::new(AtomicUsize::new(0)), acting, tokio::sync::Notify::new()));
            let (_tx, mut rx) = mpsc::channel(1);
            let mut steer = TurnSteer::new(engine.with(|engine| engine.head()));
            let (result, tally) = complete_with_mcp(
                &provider,
                &[],
                &engine,
                &spec,
                CompletionRequest::new("grok", "", vec![]),
                None,
                &mut rx,
                None,
                None,
                &mut crate::capabilities::Capabilities::default(),
                &mut steer,
            )
            .await;
            (tally.thinking_stopped, result.unwrap().text())
        };
        let spec = spec();

        // Nobody waits on it: fifteen minutes of thought finish.
        let engine = mcp_test_engine(&spec).await;
        assert_eq!(run(engine.clone(), spec.clone(), false).await, (false, "done".to_string()));

        // A teammate's task depends on what it holds: ten minutes, then stopped.
        let created = |title: &str, depends_on: Vec<orgbot_proto::TaskId>| Command::CreateTask {
            title: title.into(),
            brief: String::new(),
            by: None,
            required_role: None,
            priority: orgbot_proto::task::Priority::Normal,
            acceptance_criteria: vec![],
            depends_on,
            origin: orgbot_proto::graph::TaskOrigin::plain(),
        };
        let id = |events: Vec<orgbot_proto::EventEnvelope>| {
            events
                .iter()
                .find_map(|e| match &e.event {
                    orgbot_proto::Event::TaskCreated { task } => Some(task.id),
                    _ => None,
                })
                .unwrap()
        };
        engine.with(|room| {
            let model = id(room.apply(created("3D model", vec![])).unwrap());
            for command in [Command::OpenTask { task: model }, Command::ClaimTask { task: model, by: spec.id }] {
                room.apply(command).unwrap();
            }
            room.apply(created("render pipeline", vec![model])).unwrap();
            assert!(room.others_wait_on(spec.id));
        });
        // Stopped, it still finishes its turn: what it said is kept.
        assert_eq!(run(engine.clone(), spec.clone(), false).await, (true, "stopped part way".to_string()));
        // Using tools all along, it is never stopped.
        assert_eq!(run(engine.clone(), spec.clone(), true).await, (false, "done".to_string()));
    }

    /// API replies are cut short only while teammates wait on the bot.
    #[tokio::test]
    async fn an_api_bot_gets_a_short_thinking_limit_only_while_teammates_wait() {
        let spec = spec();
        let engine = mcp_test_engine(&spec).await;
        let provider = Arc::new(ScriptedProvider::always_says("done"));
        let mut bot = Bot::new(spec.clone(), provider.clone());
        bot.take_turn(&engine, None).await.unwrap();
        assert_eq!(provider.requests()[0].thinking_limit, None, "nobody waits: the long default");
        engine.with(|room| {
            let task = room
                .apply(Command::CreateTask {
                    title: "render pipeline".into(),
                    brief: String::new(),
                    by: None,
                    required_role: None,
                    priority: orgbot_proto::task::Priority::Normal,
                    acceptance_criteria: vec![],
                    depends_on: vec![],
                    origin: orgbot_proto::graph::TaskOrigin::plain(),
                })
                .unwrap()
                .iter()
                .find_map(|e| match &e.event {
                    orgbot_proto::Event::TaskCreated { task } => Some(task.id),
                    _ => None,
                })
                .unwrap();
            room.apply(Command::OpenTask { task }).unwrap();
            room.apply(Command::ClaimTask { task, by: spec.id }).unwrap();
            room.apply(Command::CreateTask {
                title: "hero".into(),
                brief: String::new(),
                by: None,
                required_role: None,
                priority: orgbot_proto::task::Priority::Normal,
                acceptance_criteria: vec![],
                depends_on: vec![task],
                origin: orgbot_proto::graph::TaskOrigin::plain(),
            })
            .unwrap();
        });
        bot.take_turn(&engine, None).await.unwrap();
        assert_eq!(provider.requests().last().unwrap().thinking_limit, Some(API_THINKING_LIMIT));
    }

    /// From speed test 1: DeepSeek refused every turn of @deepseek-builder,
    /// which held the render pipeline for 24 minutes with nothing to show.
    #[tokio::test]
    async fn work_goes_back_to_the_board_when_a_bots_service_keeps_refusing_it() {
        #[derive(Debug)]
        struct Busy;
        #[async_trait::async_trait]
        impl Provider for Busy {
            fn name(&self) -> &str {
                "deepseek"
            }
            async fn complete(
                &self,
                _request: CompletionRequest,
                _sink: Option<mpsc::Sender<StreamEvent>>,
            ) -> orgbot_providers::Result<CompletionResponse> {
                Err(ProviderError::RateLimited {
                    provider: "deepseek".into(),
                    message: "Service is too busy".into(),
                    retry_after_secs: Some(1),
                })
            }
        }
        use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
        let worker = spec();
        let lead = AgentSpec::new("lead", "Lead", orgbot_proto::agent::ProviderRef::Scripted { script: "t".into() })
            .with_roles(["coordinator"]);
        let mut room = Engine::new(RoomSpec {
            name: "busy".into(),
            goal: "ship".into(),
            roster: vec![],
            completion: CompletionRule::Manual,
            limits: RoomLimits::default(),
            access: Default::default(),
        });
        for agent in [&worker, &lead] {
            room.apply(Command::JoinAgent { spec: Box::new(agent.clone()) }).unwrap();
        }
        room.apply(Command::StartRoom).unwrap();
        let created = room
            .apply(Command::CreateTask {
                title: "render pipeline".into(),
                brief: String::new(),
                by: None,
                required_role: None,
                priority: orgbot_proto::task::Priority::Normal,
                acceptance_criteria: vec![],
                depends_on: vec![],
                origin: orgbot_proto::graph::TaskOrigin::plain(),
            })
            .unwrap();
        let task = created
            .iter()
            .find_map(|e| match &e.event {
                orgbot_proto::Event::TaskCreated { task } => Some(task.id),
                _ => None,
            })
            .unwrap();
        for command in [Command::OpenTask { task }, Command::ClaimTask { task, by: worker.id }, Command::StartTask { task, by: worker.id }] {
            room.apply(command).unwrap();
        }
        let engine = SharedEngine::new(room);
        let mut bot = Bot::new(worker.clone(), Arc::new(Busy));

        bot.take_turn(&engine, None).await.unwrap();
        assert_eq!(engine.with(|e| e.task(task).unwrap().assignee), Some(worker.id), "a first refusal is waited out");

        // A second refusal long after the first is not "refused for 5
        // minutes" on its own (13 November audit); a third in a row is.
        bot.unavailable_since = Some(std::time::Instant::now() - SERVICE_DOWN_RELEASE);
        bot.take_turn(&engine, None).await.unwrap();
        assert_eq!(engine.with(|e| e.task(task).unwrap().assignee), Some(worker.id), "two refusals are not enough");
        bot.take_turn(&engine, None).await.unwrap();
        assert_ne!(engine.with(|e| e.task(task).unwrap().assignee), Some(worker.id), "its work went back to the board");
        let told = engine.with(|e| {
            e.transcript()
                .iter()
                .find(|m| m.body.contains("model service has refused it"))
                .cloned()
        });
        let told = told.expect("the coordinators are told");
        assert!(told.body.contains("\"render pipeline\"") && told.wakes(lead.id, &lead.roles), "{}", told.body);
    }

    /// From benchmark 2: the owner asked "@grok-quick, what is the hold up"
    /// during a 94-minute Grok CLI turn, and it went unseen until the turn
    /// ended. It now rides on the bot's next room tool result, once.
    #[tokio::test]
    async fn a_cli_bot_hears_a_message_for_it_on_its_next_room_tool_result() {
        let spec = spec();
        let engine = mcp_test_engine(&spec).await;
        let mut session = crate::mcp::McpSession::new(crate::mcp::room_tools_for(&spec));
        session.handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#, |_, _| unreachable!());
        let mut steer = TurnSteer::new(engine.with(|engine| engine.head()));
        let (mut tally, mut capabilities) = (McpTally::default(), crate::capabilities::Capabilities::default());
        let list = json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"list_tasks","arguments":{}}})
        .to_string();
        engine.with(|room| {
            let message = orgbot_proto::ChatMessage::human(room.room_id(), "Aece", "what is the hold up?")
                .to_agents(vec![spec.id]);
            room.apply(Command::PostMessage { message: Box::new(message) }).unwrap();
        });
        let reply = dispatch_room_mcp(&mut session, &list, &engine, &spec, None, None, &mut tally, &mut capabilities, &mut steer)
            .await
            .unwrap();
        let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("From Aece: what is the hold up?"), "{text}");
        let again = dispatch_room_mcp(&mut session, &list, &engine, &spec, None, None, &mut tally, &mut capabilities, &mut steer)
            .await
            .unwrap();
        let again: serde_json::Value = serde_json::from_str(&again).unwrap();
        assert!(!again["result"]["content"][0]["text"].as_str().unwrap().contains("hold up"), "said once");
    }

    /// A CLI bot that makes no room call for a minute after the owner wrote
    /// to it has its turn ended, so the next one, resuming the same session,
    /// shows the message.
    #[tokio::test(start_paused = true)]
    async fn a_cli_turn_that_keeps_the_owner_waiting_is_ended_for_them() {
        #[derive(Debug)]
        struct Endless;
        #[async_trait::async_trait]
        impl Provider for Endless {
            fn name(&self) -> &str {
                "endless"
            }
            async fn complete(
                &self,
                _request: CompletionRequest,
                _sink: Option<mpsc::Sender<StreamEvent>>,
            ) -> orgbot_providers::Result<CompletionResponse> {
                std::future::pending().await
            }
        }
        let spec = spec();
        let engine = mcp_test_engine(&spec).await;
        let mut steer = TurnSteer::new(engine.with(|engine| engine.head()));
        engine.with(|room| {
            let mut message = orgbot_proto::ChatMessage::human(room.room_id(), "Aece", "what is the hold up?")
                .to_agents(vec![spec.id]);
            message.sent_at = orgbot_proto::Timestamp::now().plus_millis(-2 * 60_000);
            room.apply(Command::PostMessage { message: Box::new(message) }).unwrap();
        });
        let provider: Arc<dyn Provider> = Arc::new(Endless);
        let (_tx, mut rx) = mpsc::channel(1);
        let (result, tally) = complete_with_mcp(
            &provider,
            &[],
            &engine,
            &spec,
            CompletionRequest::new("fixture", "", vec![]),
            None,
            &mut rx,
            None,
            None,
            &mut crate::capabilities::Capabilities::default(),
            &mut steer,
        )
        .await;
        assert!(tally.interrupted);
        assert_eq!(result.unwrap().stop_reason, StopReason::Other("human_input".into()));
    }

    #[tokio::test]
    async fn review_only_bots_cannot_advertise_or_call_external_mcp_tools() {
        let (server_spec, calls, server) = local_mcp_server().await;
        let mut spec = spec();
        spec.autonomy.review_only = true;
        let client = Arc::new(crate::mcp_client::McpClient::lazy(&[server_spec], None));
        let mut bot = Bot::new(spec.clone(), mcp_call_provider());
        bot.attach_mcp_servers(client);
        assert!(
            !bot.tools.iter().any(|tool| tool.name == "external__mutate"),
            "review-only tools must not advertise external MCP capabilities"
        );

        let report = bot
            .take_turn(&mcp_test_engine(&spec).await, None)
            .await
            .unwrap();
        assert_eq!(report.tool_errors, 1);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "dispatch must not reach server"
        );
        server.abort();
    }

    #[tokio::test]
    async fn ordinary_bots_load_external_mcp_tools_on_demand() {
        let (server_spec, calls, server) = local_mcp_server().await;
        let spec = spec();
        let client = Arc::new(crate::mcp_client::McpClient::lazy(&[server_spec], None));
        let provider = Arc::new(ScriptedProvider::new(vec![
            orgbot_providers::scripted::tool_call(
                "load",
                "load_mcp_tool",
                json!({"server":"external", "tool":"external__mutate", "lifetime":"once"}),
            ),
            orgbot_providers::scripted::tool_call(
                "call",
                "call_mcp_tool",
                json!({"tool":"external__mutate", "arguments":{}}),
            ),
            orgbot_providers::scripted::says("done"),
        ]));
        let mut bot = Bot::new(spec.clone(), provider.clone());
        bot.attach_mcp_servers(client);
        assert!(!bot.tools.iter().any(|tool| tool.name == "external__mutate"));
        assert!(bot.tools.iter().any(|tool| tool.name == "load_mcp_tool"));

        let report = bot
            .take_turn(&mcp_test_engine(&spec).await, None)
            .await
            .unwrap();
        assert_eq!(report.tool_errors, 0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let requests = provider.requests();
        let serialized: Vec<_> = requests
            .iter()
            .map(|request| serde_json::to_string(request).unwrap())
            .collect();
        assert!(!serialized[0].contains("A test-only external mutation"));
        assert!(serialized[1].contains("A test-only external mutation"));
        assert!(!serialized[2].contains("A test-only external mutation"));
        server.abort();
    }

    #[tokio::test]
    async fn api_skill_bodies_are_consumed_released_and_reloadable() {
        use orgbot_providers::scripted::{says, tool_call};
        let spec = spec();
        let engine = mcp_test_engine(&spec).await;
        engine.with(|room| {
            room.set_skills(vec![orgbot_proto::skill::Skill {
                name: "fixture-skill".into(),
                description: "fixture summary absent from the startup prompt".into(),
                body: "PRIVATE_SKILL_BODY_MARKER".repeat(500),
            }])
        });
        let provider = Arc::new(ScriptedProvider::new(vec![
            tool_call(
                "load-once",
                "use_skill",
                json!({"skill":"fixture-skill", "lifetime":"once"}),
            ),
            tool_call("next", "list_tasks", json!({})),
            tool_call("reload", "use_skill", json!({"skill":"fixture-skill"})),
            tool_call(
                "release",
                "unload_capability",
                json!({"kind":"skill", "name":"fixture-skill"}),
            ),
            says("done"),
        ]));
        let mut bot = Bot::new(spec, provider.clone());
        let report = bot.take_turn(&engine, None).await.unwrap();
        assert_eq!(report.tool_errors, 0);
        let requests = provider.requests();
        let bodies: Vec<_> = requests
            .iter()
            .map(|r| serde_json::to_string(&r.messages).unwrap())
            .collect();
        assert!(!requests[0].system.contains("fixture summary"));
        assert!(!bodies[0].contains("PRIVATE_SKILL_BODY_MARKER"));
        assert!(bodies[1].contains("PRIVATE_SKILL_BODY_MARKER"));
        assert!(!bodies[2].contains("PRIVATE_SKILL_BODY_MARKER"));
        assert!(bodies[3].contains("PRIVATE_SKILL_BODY_MARKER"));
        assert!(!bodies[4].contains("PRIVATE_SKILL_BODY_MARKER"));
        assert!(!serde_json::to_string(&bot.memory())
            .unwrap()
            .contains("PRIVATE_SKILL_BODY_MARKER"));
        for request in requests {
            let mut uses = std::collections::BTreeSet::new();
            for block in request.messages.iter().flat_map(|turn| &turn.content) {
                match block {
                    ContentBlock::ToolUse { id, .. } => {
                        uses.insert(id);
                    }
                    ContentBlock::ToolResult { tool_use_id, .. } => {
                        assert!(uses.contains(tool_use_id))
                    }
                    _ => {}
                }
            }
        }
        let saved = serde_json::to_value(bot.memory()).unwrap();
        let mut legacy = saved;
        legacy.as_object_mut().unwrap().remove("capabilities");
        let memory: BotMemory = serde_json::from_value(legacy).unwrap();
        bot.restore_memory(memory);
        assert!(!serde_json::to_string(&bot.memory())
            .unwrap()
            .contains("PRIVATE_SKILL_BODY_MARKER"));
    }

    #[tokio::test]
    async fn interrupted_cleanup_expires_mcp_lease_and_connection_before_restore() {
        let (server_spec, calls, server) = local_mcp_server().await;
        let spec = spec();
        let engine = mcp_test_engine(&spec).await;
        let client = Arc::new(crate::mcp_client::McpClient::lazy(&[server_spec], None));
        let mut bot = Bot::new(
            spec.clone(),
            Arc::new(ScriptedProvider::always_says("done")),
        );
        bot.attach_mcp_servers(client.clone());
        let loaded = bot
            .capabilities
            .dispatch(
                &engine,
                spec.id,
                false,
                Some(&client),
                "load_mcp_tool",
                &json!({"server":"external", "tool":"external__mutate"}),
            )
            .await;
        assert!(!loaded.is_error);
        bot.finish_turn(&engine).await;
        let saved = serde_json::to_value(bot.memory()).unwrap();
        bot.restore_memory(serde_json::from_value(saved).unwrap());
        let rejected = bot
            .capabilities
            .dispatch(
                &engine,
                spec.id,
                false,
                Some(&client),
                "call_mcp_tool",
                &json!({"tool":"external__mutate", "arguments":{}}),
            )
            .await;
        assert!(rejected.is_error);
        assert!(
            client
                .call_loaded_for(&engine, spec.id, "external", "external__mutate", &json!({}))
                .await
                .is_error
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn active_skill_can_be_refreshed_after_history_compaction() {
        use orgbot_providers::scripted::{says, tool_call};
        let spec = spec();
        let engine = mcp_test_engine(&spec).await;
        engine.with(|room| {
            room.set_skills(vec![orgbot_proto::skill::Skill {
                name: "compact-fixture".into(),
                description: "fixture".into(),
                body: "COMPACT_SKILL_BODY".into(),
            }]);
            let events = room
                .apply(Command::CreateTask {
                    title: "active work".into(),
                    brief: String::new(),
                    by: None,
                    required_role: None,
                    priority: orgbot_proto::task::Priority::Normal,
                    acceptance_criteria: vec![],
                    depends_on: vec![],
                    origin: orgbot_proto::graph::TaskOrigin::plain(),
                })
                .unwrap();
            let task = events
                .iter()
                .find_map(|e| match &e.event {
                    orgbot_proto::Event::TaskCreated { task } => Some(task.id),
                    _ => None,
                })
                .unwrap();
            room.apply(Command::OpenTask { task }).unwrap();
            room.apply(Command::ClaimTask { task, by: spec.id })
                .unwrap();
        });
        let provider = Arc::new(ScriptedProvider::new(vec![
            tool_call("refresh", "use_skill", json!({"skill":"compact-fixture"})),
            says("done"),
        ]));
        let mut bot = Bot::new(spec.clone(), provider.clone());
        let result = bot
            .capabilities
            .dispatch(
                &engine,
                spec.id,
                false,
                None,
                "use_skill",
                &json!({"skill":"compact-fixture"}),
            )
            .await;
        assert!(!result.is_error);
        bot.history
            .push(tool_call("original", "use_skill", json!({"skill":"compact-fixture"})).as_turn());
        bot.history
            .push(Turn::tool_results(vec![ContentBlock::ToolResult {
                tool_use_id: "original".into(),
                content: result.content,
                is_error: false,
            }]));
        for _ in 0..40 {
            bot.history.push(Turn::user("Continue the current task."));
            bot.history.push(Turn::assistant("Working."));
        }
        bot.trim_history();
        assert!(!serde_json::to_string(&bot.history)
            .unwrap()
            .contains("COMPACT_SKILL_BODY"));
        assert_eq!(bot.take_turn(&engine, None).await.unwrap().tool_errors, 0);
        assert!(serde_json::to_string(&provider.requests()[1].messages)
            .unwrap()
            .contains("COMPACT_SKILL_BODY"));
    }

    #[tokio::test]
    async fn api_batch_cannot_use_a_task_lease_after_releasing_its_task() {
        use orgbot_providers::scripted::{says, tool_call};
        let (server_spec, calls, server) = local_mcp_server().await;
        let spec = spec();
        let engine = mcp_test_engine(&spec).await;
        let task = engine.with(|room| {
            let events = room
                .apply(Command::CreateTask {
                    title: "bounded work".into(),
                    brief: String::new(),
                    by: None,
                    required_role: None,
                    priority: orgbot_proto::task::Priority::Normal,
                    acceptance_criteria: vec![],
                    depends_on: vec![],
                    origin: orgbot_proto::graph::TaskOrigin::plain(),
                })
                .unwrap();
            let task = events
                .iter()
                .find_map(|e| match &e.event {
                    orgbot_proto::Event::TaskCreated { task } => Some(task.id),
                    _ => None,
                })
                .unwrap();
            room.apply(Command::OpenTask { task }).unwrap();
            room.apply(Command::ClaimTask { task, by: spec.id })
                .unwrap();
            task
        });
        let mut batch = tool_call(
            "release",
            "release_task",
            json!({"task":task,"reason":"handoff"}),
        );
        batch.content.extend(
            tool_call(
                "late-call",
                "call_mcp_tool",
                json!({"tool":"external__mutate","arguments":{}}),
            )
            .content,
        );
        let provider = Arc::new(ScriptedProvider::new(vec![
            tool_call(
                "load",
                "load_mcp_tool",
                json!({"server":"external","tool":"external__mutate"}),
            ),
            batch,
            says("done"),
        ]));
        let client = Arc::new(crate::mcp_client::McpClient::lazy(&[server_spec], None));
        let mut bot = Bot::new(spec, provider.clone());
        bot.attach_mcp_servers(client);
        let report = bot.take_turn(&engine, None).await.unwrap();
        assert_eq!(report.tool_errors, 1);
        assert_eq!(
            engine.borrow().task(task).unwrap().state,
            orgbot_proto::TaskState::Ready
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(provider.requests()[2]
            .messages
            .iter()
            .flat_map(|turn| &turn.content)
            .any(|block| {
                matches!(block, ContentBlock::ToolResult {tool_use_id,content,is_error:true}
                if tool_use_id == "late-call" && content.contains("not loaded"))
            }));
        server.abort();
    }

    /// From the live room: the owner allowed Sol the server key, but Sol's
    /// workspace commands through the room bridge still ran without it,
    /// because that path never asked the room whether access was granted.
    #[tokio::test]
    async fn room_bridge_workspace_commands_carry_the_owners_server_access_grant() {
        use crate::workspace::{Workspace, WorkspaceSpec};
        use orgbot_proto::decision::{Answer, SERVER_ACCESS_ALLOW};
        let spec = spec();
        let engine = mcp_test_engine(&spec).await;
        let task = engine.with(|room| {
            let created = room
                .apply(Command::CreateTask {
                    title: "Deploy the reviewed build".into(),
                    brief: String::new(),
                    by: None,
                    required_role: None,
                    priority: orgbot_proto::task::Priority::Normal,
                    acceptance_criteria: vec![],
                    depends_on: vec![],
                    origin: orgbot_proto::graph::TaskOrigin::plain(),
                })
                .unwrap();
            let task = created
                .iter()
                .find_map(|event| match &event.event {
                    orgbot_proto::Event::TaskCreated { task } => Some(task.id),
                    _ => None,
                })
                .unwrap();
            for command in [
                Command::OpenTask { task },
                Command::ClaimTask { task, by: spec.id },
                Command::StartTask { task, by: spec.id },
            ] {
                room.apply(command).unwrap();
            }
            task
        });
        let (tx, mut calls) = mpsc::channel(4);
        let workspace = Workspace::new(
            WorkspaceSpec {
                label: "test".into(),
                root: "/work".into(),
                writable: true,
                commands: true,
            },
            tx,
        );
        let mut tools = crate::mcp::room_tools_for(&spec);
        tools.extend(workspace.definitions());
        let mut session = crate::mcp::McpSession::new(tools);
        session.handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#, |_, _| {
            panic!("initialization must not dispatch")
        });
        let request = json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"workspace_run","arguments":{"command":"ssh hermes-vps true"}}})
        .to_string();
        let mut tally = McpTally::default();
        let mut capabilities = crate::capabilities::Capabilities::default();
        let mut steer = TurnSteer::new(engine.with(|engine| engine.head()));
        let host = async {
            let call = calls.recv().await.unwrap();
            let access = call.grants.server_access;
            call.reply.send(orgbot_tools::ToolOutcome::ok("ran")).unwrap();
            access
        };
        let (_, before) = tokio::join!(
            dispatch_room_mcp(
                &mut session, &request, &engine, &spec, None, Some(&workspace),
                &mut tally, &mut capabilities, &mut steer,
            ),
            host
        );
        assert!(!before, "no key before the owner allows it");

        let decision = engine.with(|room| {
            let events = room
                .apply(Command::RequestServerAccess {
                    by: spec.id,
                    task,
                    reason: "deploy the reviewed build".into(),
                    plan: Default::default(),
                })
                .unwrap();
            let decision = events
                .iter()
                .find_map(|event| match &event.event {
                    orgbot_proto::Event::DecisionAsked { decision } => Some(decision.id),
                    _ => None,
                })
                .unwrap();
            room.apply(Command::AnswerDecision {
                decision,
                answer: Answer {
                    chosen: SERVER_ACCESS_ALLOW.into(),
                    note: String::new(),
                    at: orgbot_proto::Timestamp::now(),
                },
            })
            .unwrap();
            decision
        });
        let _ = decision;
        let host = async {
            let call = calls.recv().await.unwrap();
            let access = call.grants.server_access;
            call.reply.send(orgbot_tools::ToolOutcome::ok("ran")).unwrap();
            access
        };
        let (_, after) = tokio::join!(
            dispatch_room_mcp(
                &mut session, &request, &engine, &spec, None, Some(&workspace),
                &mut tally, &mut capabilities, &mut steer,
            ),
            host
        );
        assert!(after, "allowed, the bridge's workspace command carries the key");
    }

    #[tokio::test]
    async fn native_room_bridge_enforces_initialization_review_and_one_use_approval() {
        let (server_spec, calls, server) = local_mcp_server().await;
        let spec = spec();
        let engine = mcp_test_engine(&spec).await;
        let state = std::env::temp_dir().join(format!("orgbots-bridge-{}", spec.id));
        std::fs::create_dir(&state).unwrap();
        let client =
            crate::mcp_client::McpClient::lazy(&[server_spec], None).with_owner_approval(state);
        let mut session = crate::mcp::McpSession::new(crate::mcp::room_tools_for(&spec));
        let mut tally = McpTally::default();
        let mut capabilities = crate::capabilities::Capabilities::default();
        let request = json!({"jsonrpc":"2.0","id":7,"method":"tools/call",
            "params":{"name":"call_mcp_tool","arguments":{"tool":"external__mutate","arguments":{}}}})
        .to_string();
        let mut steer = TurnSteer::new(engine.with(|engine| engine.head()));
        let reply = dispatch_room_mcp(
            &mut session,
            &request,
            &engine,
            &spec,
            Some(&client),
            None,
            &mut tally,
            &mut capabilities,
            &mut steer,
        )
        .await
        .unwrap();
        assert!(reply.contains("server not initialized"));
        assert_eq!(engine.borrow().decisions().pending().count(), 0);
        session.handle(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            |_, _| panic!("initialization must not dispatch"),
        );
        let mut reviewer = spec.clone();
        reviewer.autonomy.review_only = true;
        let reply = dispatch_room_mcp(
            &mut session,
            &request,
            &engine,
            &reviewer,
            Some(&client),
            None,
            &mut tally,
            &mut capabilities,
            &mut steer,
        )
        .await
        .unwrap();
        assert!(reply.to_lowercase().contains("review-only"));
        assert_eq!(engine.borrow().decisions().pending().count(), 0);
        let loaded = capabilities
            .dispatch(
                &engine,
                spec.id,
                false,
                Some(&client),
                "load_mcp_tool",
                &json!({"server":"external", "tool":"external__mutate"}),
            )
            .await;
        assert!(!loaded.is_error, "{}", loaded.content);
        let reply = dispatch_room_mcp(
            &mut session,
            &request,
            &engine,
            &spec,
            Some(&client),
            None,
            &mut tally,
            &mut capabilities,
            &mut steer,
        )
        .await
        .unwrap();
        assert!(reply.contains("Waiting for owner approval"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let decision = engine.borrow().decisions().pending().next().unwrap().id;
        engine.with(|room| {
            room.apply(Command::AnswerDecision {
                decision,
                answer: orgbot_proto::decision::Answer {
                    chosen: "Allow once".into(),
                    note: String::new(),
                    at: orgbot_proto::Timestamp::now(),
                },
            })
            .unwrap()
        });
        let reply = dispatch_room_mcp(
            &mut session,
            &request,
            &engine,
            &spec,
            Some(&client),
            None,
            &mut tally,
            &mut capabilities,
            &mut steer,
        )
        .await
        .unwrap();
        let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["result"]["isError"], false);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let reply = dispatch_room_mcp(
            &mut session,
            &request,
            &engine,
            &spec,
            Some(&client),
            None,
            &mut tally,
            &mut capabilities,
            &mut steer,
        )
        .await
        .unwrap();
        assert!(reply.contains("Waiting for owner approval"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(tally.ok, 1);
        assert_eq!(tally.err, 3);
        server.abort();
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
    fn compaction_keeps_decisions_artifact_references_and_paired_tool_results() {
        let turns = vec![
            Turn::user("Decision: use the existing checkpoint and preserve artifact art_abc123."),
            Turn {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call-7".into(),
                    name: "read_artifact".into(),
                    input: json!({"artifact":"art_abc123"}),
                }],
            },
            Turn::tool_results(vec![ContentBlock::ToolResult {
                tool_use_id: "call-7".into(),
                content: "Loaded the signed decision record.".into(),
                is_error: false,
            }]),
        ];
        let summary = compact_history(&turns);
        assert!(summary.starts_with(COMPACTED_HISTORY_MARKER));
        assert!(summary.contains("Decision: use the existing checkpoint"));
        assert!(summary.contains("art_abc123"));
        assert!(summary.contains("tool call-7 `read_artifact`"));
        assert!(summary.contains("Loaded the signed decision record"));
    }

    #[test]
    fn compaction_keeps_capability_receipts_without_carrying_loaded_bodies() {
        let turns = vec![
            Turn {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "skill-load".into(),
                    name: "use_skill".into(),
                    input: json!({"skill":"fixture"}),
                }],
            },
            Turn::tool_results(vec![ContentBlock::ToolResult {
                tool_use_id: "skill-load".into(),
                content: "SKILL fixture\nPRIVATE_PLAYBOOK_BODY".into(),
                is_error: false,
            }]),
        ];
        let summary = compact_history(&turns);
        assert!(summary.contains("skill-load"));
        assert!(!summary.contains("PRIVATE_PLAYBOOK_BODY"));
    }

    /// From the 13 November audit: inside one turn nothing could be cut, a
    /// tool result of any size went into the conversation, and the model's
    /// "too long" refusal faulted the bot.
    #[test]
    fn a_long_turn_stays_within_the_conversation_ceiling() {
        let big = format!("{}é{}", "a".repeat(200_000), "z".repeat(200_000));
        let bounded = bound_tool_result(big.clone(), MAX_TOOL_RESULT_CHARS);
        assert!(bounded.len() < MAX_TOOL_RESULT_CHARS + 400);
        assert!(bounded.starts_with("aaa") && bounded.ends_with("zzz"));
        assert!(bounded.contains("characters cut from the middle"));
        assert_eq!(bound_tool_result("short".into(), MAX_TOOL_RESULT_CHARS), "short");

        let mut subject = bot();
        subject.history.push(Turn::user("do the long job"));
        for index in 0..40 {
            subject.history.push(Turn {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse { id: format!("c{index}"), name: "workspace_read".into(), input: json!({}) }],
            });
            subject.history.push(Turn::tool_results(vec![ContentBlock::ToolResult {
                tool_use_id: format!("c{index}"),
                content: bound_tool_result("x".repeat(100_000), MAX_TOOL_RESULT_CHARS),
                is_error: false,
            }]));
        }
        assert!(subject.history_chars() > MAX_HISTORY_CHARS);
        subject.trim_history();
        assert!(subject.history_chars() <= MAX_HISTORY_CHARS, "{}", subject.history_chars());
        // Every call keeps its result, and the newest result is whole.
        let calls = subject.history.iter().flat_map(|t| &t.content).filter(|b| matches!(b, ContentBlock::ToolUse { .. })).count();
        let results = subject.history.iter().flat_map(|t| &t.content).filter(|b| matches!(b, ContentBlock::ToolResult { .. })).count();
        assert_eq!(calls, results);
        assert!(matches!(&subject.history.last().unwrap().content[0], ContentBlock::ToolResult { content, .. } if content.len() > 50_000));

        let too_long = ProviderError::BadRequest { provider: "anthropic".into(), message: "prompt is too long: 210000 tokens > 200000 maximum".into() };
        assert!(is_context_too_long(&too_long));
        assert!(!is_context_too_long(&ProviderError::BadRequest { provider: "x".into(), message: "unknown model".into() }));
    }

    /// From the 13 November audit: handshake and error replies used up the
    /// note of new messages, which then never reached the CLI.
    #[test]
    fn only_a_tool_result_carries_a_note() {
        assert!(!can_carry_note(Some(r#"{"jsonrpc":"2.0","id":0,"result":{"protocolVersion":"2025-06-18"}}"#)));
        assert!(!can_carry_note(Some(r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"no"}}"#)));
        assert!(!can_carry_note(None));
        assert!(can_carry_note(Some(r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"ok"}]}}"#)));
    }

    #[test]
    fn trimming_persists_summary_in_the_restorable_bot_memory() {
        let mut subject = bot();
        for index in 0..80 {
            subject.history.push(Turn::user(format!(
                "Decision: retain artifact art_{index:08}"
            )));
            subject.history.push(Turn::assistant("completed"));
        }
        subject.trim_history();
        let snapshot = subject.memory();
        let snapshot: BotMemory = serde_json::from_value(serde_json::to_value(snapshot).unwrap())
            .expect("compacted history remains serializable in the saved checkpoint");
        assert!(snapshot.history.first().is_some_and(|turn| {
            turn.content.iter().any(|block| matches!(block, ContentBlock::Text { text } if text.starts_with(COMPACTED_HISTORY_MARKER)))
        }));
        let mut restored = bot();
        restored.restore_memory(snapshot);
        assert!(restored.history[0].content.iter().any(|block| {
            matches!(block, ContentBlock::Text { text } if text.contains("artifact art_"))
        }));
    }

    #[test]
    fn byte_heavy_history_is_compacted_below_the_context_ceiling() {
        let mut subject = bot();
        let payload = "x".repeat(40_000);
        for _ in 0..9 {
            subject.history.push(Turn::user(payload.clone()));
        }
        subject.trim_history();
        assert!(subject.history_chars() <= MAX_HISTORY_CHARS);
        assert!(subject.history[0].content.iter().any(|block| {
            matches!(block, ContentBlock::Text { text } if text.starts_with(COMPACTED_HISTORY_MARKER))
        }));
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
            own_activity: 0,
            changes: 0,
        };
        assert!(!base.made_progress(), "an empty turn is not progress");
        assert!(!TurnReport { said: Some("I'm changing it".into()), ..base.clone() }.acted(), "talking is not acting");
        assert!(TurnReport { own_activity: 2, ..base.clone() }.acted(), "edits inside a CLI are");
        assert!(!TurnReport { tool_calls: 3, ..base.clone() }.acted(), "reads and listings are not");
        assert!(TurnReport { tool_calls: 3, changes: 1, ..base.clone() }.acted(), "a change is");

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
            access: Default::default(),
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
            fn name(&self) -> &str {
                "disconnected"
            }
            async fn complete(
                &self,
                _: CompletionRequest,
                _: Option<mpsc::Sender<StreamEvent>>,
            ) -> Result<CompletionResponse, ProviderError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Err(ProviderError::StreamInterrupted {
                    provider: "test".into(),
                    message: "peer disconnected".into(),
                })
            }
        }
        let backend = Arc::new(Disconnects::default());
        let provider: Arc<dyn Provider> = backend.clone();
        let result = super::complete_api(
            &provider,
            CompletionRequest::new("test", "test", vec![]),
            None,
        )
        .await;
        assert!(matches!(
            result,
            Err(ProviderError::StreamInterrupted { .. })
        ));
        assert_eq!(
            backend.0.load(Ordering::SeqCst),
            3,
            "stop after two retries"
        );

        backend.0.store(0, Ordering::SeqCst);
        let spec = AgentSpec::new(
            "worker",
            "Worker",
            ProviderRef::Scripted {
                script: "test".into(),
            },
        );
        let engine = SharedEngine::new(engine_with(&spec));
        let mut bot = Bot::new(spec, provider);
        let interrupt = bot.input_interrupt.clone();
        let (tx, mut rx) = mpsc::channel(8);
        let running = tokio::spawn(async move { bot.take_turn(&engine, Some(tx)).await });
        assert_eq!(rx.recv().await.unwrap(), StreamEvent::Thinking);
        assert!(matches!(
            rx.recv().await.unwrap(),
            StreamEvent::Retry { attempt: 1, .. }
        ));
        interrupt.cancel();
        let report = tokio::time::timeout(std::time::Duration::from_millis(300), running)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(report.stop_reason, StopReason::Other("human_input".into()));
        assert_eq!(
            backend.0.load(Ordering::SeqCst),
            1,
            "cancel the pending retry immediately"
        );
    }

        /// From speed test 2: @iris, holding no task, edited delivered files and
    /// others' tests, and every edit merged unreviewed.
    #[tokio::test]
    async fn a_bot_holding_no_task_cannot_change_the_project() {
        use crate::workspace::{Workspace, WorkspaceSpec};
        use orgbot_providers::ScriptedProvider;
        let provider = Arc::new(ScriptedProvider::new(vec![
            CompletionResponse {
                model: "test".into(),
                content: vec![ContentBlock::ToolUse {
                    id: "write1".into(),
                    name: "workspace_write".into(),
                    input: serde_json::json!({"path":"jockey.js", "content":"tweak"}),
                }],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            },
            CompletionResponse {
                model: "test".into(),
                content: vec![ContentBlock::text("ok")],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ]));
        let spec = AgentSpec::new("idle", "Idle", ProviderRef::Scripted { script: "x".into() });
        let engine = SharedEngine::new(engine_with(&spec));
        let mut bot = Bot::new(spec, provider.clone());
        let (tx, mut calls) = mpsc::channel(2);
        bot.attach_workspace(Workspace::new(
            WorkspaceSpec { label: "test".into(), root: "/work".into(), writable: true, commands: true },
            tx,
        ));
        bot.take_turn(&engine, None).await.unwrap();
        assert!(calls.try_recv().is_err(), "the write never reached the workspace");
        let second = &provider.requests()[1];
        let refused = second.messages.iter().flat_map(|turn| turn.content.iter()).any(|block| matches!(
            block,
            ContentBlock::ToolResult { content, is_error: true, .. } if content.contains("You hold no task")
        ));
        assert!(refused, "the bot is told why");
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
        let spec = AgentSpec::new(
            "writer",
            "Writer",
            ProviderRef::Scripted { script: "x".into() },
        );
        let mut room = engine_with(&spec);
        // A bot changes the project only while it holds work.
        let created = room
            .apply(Command::CreateTask {
                title: "Research notes".into(),
                brief: String::new(),
                by: None,
                required_role: None,
                priority: orgbot_proto::task::Priority::Normal,
                acceptance_criteria: vec![],
                depends_on: vec![],
                origin: orgbot_proto::graph::TaskOrigin::plain(),
            })
            .unwrap();
        let task = created
            .iter()
            .find_map(|event| match &event.event {
                orgbot_proto::Event::TaskCreated { task } => Some(task.id),
                _ => None,
            })
            .unwrap();
        for command in [
            Command::OpenTask { task },
            Command::ClaimTask { task, by: spec.id },
            Command::StartTask { task, by: spec.id },
        ] {
            room.apply(command).unwrap();
        }
        let engine = SharedEngine::new(room);
        let mut bot = Bot::new(spec, provider.clone());
        let token = bot.input_interrupt.clone();
        let (activity_tx, mut activity_rx) = mpsc::channel(16);
        let (tx, mut calls) = mpsc::channel(2);
        bot.attach_workspace(Workspace::new(
            WorkspaceSpec {
                label: "test".into(),
                root: "/work".into(),
                writable: true,
                commands: false,
            },
            tx,
        ));
        let mut running = tokio::spawn(async move {
            let result = bot.take_turn(&engine, Some(activity_tx)).await;
            (bot, engine, result)
        });
        let call = calls.recv().await.unwrap();
        assert_eq!(call.name, "workspace_write");
        token.cancel();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), &mut running)
                .await
                .is_err(),
            "input must not drop the pending write"
        );
        call.reply
            .send(orgbot_tools::ToolOutcome {
                content: "write committed once".into(),
                is_error: false,
            })
            .unwrap();
        let (mut bot, engine, result) = running.await.unwrap();
        let report = result.unwrap();
        let mut activity_events = Vec::new();
        while let Ok(event) = activity_rx.try_recv() {
            activity_events.push(event);
        }
        assert!(activity_events.contains(&StreamEvent::ToolUseStart {
            id: "write1".into(),
            name: "workspace_write".into(),
        }));
        assert!(activity_events.contains(&StreamEvent::ToolUseEnd {
            id: "write1".into(),
            succeeded: true,
        }));
        assert_eq!(report.stop_reason, StopReason::Other("human_input".into()));
        assert_eq!(report.tool_calls, 1);
        assert_eq!(
            provider.requests().len(),
            1,
            "no second API call before processing input"
        );
        assert_eq!(
            bot.last_seen(),
            orgbot_proto::Seq::ZERO,
            "interrupted requests remain actionable"
        );
        assert!(
            matches!(&bot.history.last().unwrap().content[0], ContentBlock::ToolResult { content, .. } if content == "write committed once")
        );
        bot.input_interrupt = tokio_util::sync::CancellationToken::new();
        bot.take_turn(&engine, None).await.unwrap();
        assert!(calls.try_recv().is_err(), "the write must not be replayed");
        assert!(serde_json::to_string(&provider.requests()[1].messages)
            .unwrap()
            .contains("write committed once"));
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

        let bot_task = tokio::spawn(async move { (bot.take_turn(&engine, None).await, engine) });

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
