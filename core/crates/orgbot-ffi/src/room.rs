//! A room running in this process, driven from another language.
//!
//! The engine is async and the caller is not, so each room owns a Tokio
//! runtime on its own thread. The FFI layer never blocks on it: commands go in
//! over a channel, events come out into a buffer the caller drains when it
//! likes. That is what lets a phone UI stay responsive while a model turn runs
//! for a minute.

use orgbot_runtime::workspace::Workspace;
use orgbot_server::protocol::ClientMessage;
use orgbot_tools::ToolOutcome;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

use orgbot_providers::{CompletionRequest, CompletionResponse, Provider, StreamEvent};
use orgbot_server::config::RoomConfig;
use orgbot_server::protocol::ServerMessage;
use orgbot_server::session::{Control, RoomSession, SessionHandle};
use tokio::runtime::Runtime;

/// Swap credentials between API calls, keeping in-flight calls and bot memory.
#[derive(Debug)]
struct LiveProvider {
    name: String,
    current: Mutex<Arc<dyn Provider>>,
}

#[async_trait::async_trait]
impl Provider for LiveProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> orgbot_providers::Result<CompletionResponse> {
        let provider = self.current.lock().unwrap().clone();
        provider.complete(request, sink).await
    }
}

/// A live room, with the thread and runtime keeping it alive.
pub struct Room {
    config: RoomConfig,
    providers: HashMap<String, Arc<LiveProvider>>,
    handle: SessionHandle,
    /// Events received but not yet drained by the caller.
    inbox: Arc<Mutex<Vec<ServerMessage>>>,
    /// Kept so the runtime outlives the session; dropping it stops the room.
    runtime: Option<Runtime>,
    resync: Arc<AtomicBool>,
    calls: Arc<Mutex<Vec<Value>>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<ToolOutcome>>>>,
}

/// How many undrained events to keep.
///
/// A caller that stops polling must not grow this without bound; the oldest
/// are dropped, and the client resyncs from a snapshot if it notices a gap.
const INBOX_LIMIT: usize = 4096;

impl Room {
    /// Start a room from a TOML room file.
    ///
    /// `environment` carries credentials — an API key, or an OAuth bearer
    /// token — as `KEY=value` pairs, so they never have to be written into the
    /// room file itself.
    pub fn open(toml: &str, environment: &[(String, String)]) -> Result<Self, String> {
        let config = RoomConfig::parse(toml, "room.toml").map_err(|e| e.to_string())?;
        let mut resolved = config
            .resolve_with_environment(&environment.iter().cloned().collect())
            .map_err(|e| e.to_string())?;
        let mut providers = HashMap::new();
        for bot in &mut resolved.bots {
            let provider = Arc::new(LiveProvider {
                name: bot.provider.name().into(),
                current: Mutex::new(bot.provider.clone()),
            });
            providers.insert(bot.spec.handle.clone(), provider.clone());
            bot.provider = provider;
        }
        let (mut session, handle) = RoomSession::build(resolved).map_err(|e| e.to_string())?;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| format!("could not start a runtime: {e}"))?;

        let resync = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        if let Some(spec) = config.workspace.clone() {
            let (tx, mut rx) = mpsc::channel::<orgbot_runtime::workspace::WorkspaceCall>(8);
            session.attach_workspace(Workspace { spec, tx });
            let calls = Arc::clone(&calls);
            let pending = Arc::clone(&pending);
            runtime.spawn(async move {
                let mut id = 0u64;
                while let Some(call) = rx.recv().await {
                    if call.reply.is_closed() { continue; }
                    id += 1;
                    pending.lock().unwrap().insert(id, call.reply);
                    calls.lock().unwrap().push(json!({"type":"workspace_call","id":id,"agent":call.agent,"name":call.name,"arguments":call.arguments}));
                }
            });
        }
        let inbox: Arc<Mutex<Vec<ServerMessage>>> = Arc::new(Mutex::new(Vec::new()));

        // Collect everything the room broadcasts, so nothing is missed between
        // polls.
        let mut events = handle.subscribe();
        let collector = Arc::clone(&inbox);
        let needs_resync = Arc::clone(&resync);
        runtime.spawn(async move {
            loop {
                match events.recv().await {
                    Ok(message) => {
                        let mut inbox = collector.lock().unwrap();
                        if inbox.len() >= INBOX_LIMIT {
                            inbox.clear();
                            needs_resync.store(true, Ordering::Release);
                        }
                        inbox.push(message);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        needs_resync.store(true, Ordering::Release);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        runtime.spawn(session.run());

        Ok(Self {
            config,
            providers,
            handle,
            inbox,
            runtime: Some(runtime),
            resync,
            calls,
            pending,
        })
    }

    /// Take everything received since the last call.
    pub fn drain(&self) -> Vec<Value> {
        let messages = std::mem::take(&mut *self.inbox.lock().unwrap());
        let mut output: Vec<Value> = if self.resync.swap(false, Ordering::AcqRel) {
            let mut replay = vec![];
            if let Some(snapshot) = self.snapshot() {
                replay.push(serde_json::to_value(snapshot).unwrap());
            }
            if let Some(runtime) = &self.runtime {
                for envelope in runtime.block_on(self.handle.backlog(orgbot_proto::Seq::ZERO)) {
                    replay.push(json!({"type":"event","envelope":envelope}));
                }
            }
            replay
        } else {
            messages
                .into_iter()
                .map(|message| serde_json::to_value(message).unwrap())
                .collect()
        };
        let mut pending = self.pending.lock().unwrap();
        pending.retain(|id, reply| {
            if reply.is_closed() {
                output.push(json!({"type":"workspace_cancel","id":id}));
                false
            } else {
                true
            }
        });
        let calls = std::mem::take(&mut *self.calls.lock().unwrap());
        output.extend(
            calls
                .into_iter()
                .filter(|call| pending.contains_key(&call["id"].as_u64().unwrap())),
        );
        output
    }

    /// Native clients use the same controls as socket clients, plus tool replies.
    pub fn control(&self, raw: &str) -> bool {
        if raw.len() > 2 * 1024 * 1024 {
            return false;
        }
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            return false;
        };
        if value["type"] == "update_credentials" {
            let Ok(environment) =
                serde_json::from_value::<HashMap<String, String>>(value["environment"].clone())
            else {
                return false;
            };
            let Ok(resolved) = self.config.resolve_with_environment(&environment) else {
                return false;
            };
            for bot in resolved.bots {
                if let Some(provider) = self.providers.get(&bot.spec.handle) {
                    *provider.current.lock().unwrap() = bot.provider;
                }
            }
            // Credentials are never broadcast, checkpointed or journalled.
            return true;
        }
        if value["type"] == "workspace_result" {
            let Some(id) = value["id"].as_u64() else {
                return false;
            };
            let Some(content) = value["content"].as_str() else {
                return false;
            };
            let reply = self.pending.lock().unwrap().remove(&id);
            return reply.is_some_and(|reply| {
                reply
                    .send(ToolOutcome {
                        content: content.into(),
                        is_error: value["is_error"].as_bool().unwrap_or(true),
                    })
                    .is_ok()
            });
        }
        let Ok(message) = serde_json::from_value::<ClientMessage>(value) else {
            return false;
        };
        let control = match message {
            ClientMessage::SendMessage {
                author,
                body,
                task,
                files,
            } => Control::HumanMessage {
                author,
                body,
                task,
                files,
            },
            ClientMessage::AnswerBot { task, answer } => Control::Answer { task, answer },
            ClientMessage::AnswerDecision {
                decision,
                chosen,
                note,
            } => Control::AnswerDecision {
                decision,
                chosen,
                note,
            },
            ClientMessage::Pause { reason } => Control::Pause { reason },
            ClientMessage::Resume => Control::Resume,
            ClientMessage::Resync | ClientMessage::Hello { .. } => {
                self.resync.store(true, Ordering::Release);
                return true;
            }
            ClientMessage::Ping => return true,
        };
        self.handle.try_send(control)
    }

    /// The room's current state.
    pub fn snapshot(&self) -> Option<ServerMessage> {
        let runtime = self.runtime.as_ref()?;
        runtime.block_on(self.handle.snapshot())
    }

    /// Say something to the room as a person.
    pub fn say(&self, author: &str, body: &str) -> bool {
        let Some(_) = self.runtime.as_ref() else {
            return false;
        };
        self.handle.try_send(Control::HumanMessage {
            files: vec![],
            author: author.to_string(),
            body: body.to_string(),
            task: None,
        })
    }

    /// Pause the room.
    pub fn pause(&self, reason: &str) -> bool {
        let Some(_) = self.runtime.as_ref() else {
            return false;
        };
        self.handle.try_send(Control::Pause {
            reason: reason.to_string(),
        })
    }

    /// Resume a paused room.
    pub fn resume(&self) -> bool {
        let Some(_) = self.runtime.as_ref() else {
            return false;
        };
        self.handle.try_send(Control::Resume)
    }
}

impl Drop for Room {
    fn drop(&mut self) {
        // Shut the runtime down without waiting on tasks that may be mid-turn:
        // a model call can take a minute, and closing a screen should not hang
        // the UI thread for it.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orgbot_providers::{ScriptedProvider, Turn};

    #[tokio::test]
    async fn renewal_changes_the_next_call_without_replacing_the_bot() {
        let provider = LiveProvider {
            name: "test".into(),
            current: Mutex::new(Arc::new(ScriptedProvider::always_says("before renewal"))),
        };
        let request = CompletionRequest::new("model", "system", vec![Turn::user("hello")]);
        let first = provider.complete(request.clone(), None).await.unwrap();
        *provider.current.lock().unwrap() =
            Arc::new(ScriptedProvider::always_says("after renewal"));
        let second = provider.complete(request, None).await.unwrap();
        assert_ne!(first.text(), second.text());
        assert_eq!(second.text(), "after renewal");
    }
}

impl std::fmt::Debug for Room {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Room").finish_non_exhaustive()
    }
}
