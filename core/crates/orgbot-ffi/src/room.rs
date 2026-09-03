//! A room running in this process, driven from another language.
//!
//! The engine is async and the caller is not, so each room owns a Tokio
//! runtime on its own thread. The FFI layer never blocks on it: commands go in
//! over a channel, events come out into a buffer the caller drains when it
//! likes. That is what lets a phone UI stay responsive while a model turn runs
//! for a minute.

use std::sync::{Arc, Mutex};

use orgbot_server::config::RoomConfig;
use orgbot_server::protocol::ServerMessage;
use orgbot_server::session::{Control, RoomSession, SessionHandle};
use tokio::runtime::Runtime;

/// A live room, with the thread and runtime keeping it alive.
pub struct Room {
    handle: SessionHandle,
    /// Events received but not yet drained by the caller.
    inbox: Arc<Mutex<Vec<ServerMessage>>>,
    /// Kept so the runtime outlives the session; dropping it stops the room.
    runtime: Option<Runtime>,
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
        // Credentials are read from the process environment by the provider
        // layer, so they are set before the room resolves its bots.
        for (key, value) in environment {
            // SAFETY: called before any room thread starts, so no other thread
            // is reading the environment concurrently.
            unsafe { std::env::set_var(key, value) };
        }

        let config = RoomConfig::parse(toml, "room.toml").map_err(|e| e.to_string())?;
        let resolved = config.resolve().map_err(|e| e.to_string())?;
        let (session, handle) = RoomSession::build(resolved).map_err(|e| e.to_string())?;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| format!("could not start a runtime: {e}"))?;

        let inbox: Arc<Mutex<Vec<ServerMessage>>> = Arc::new(Mutex::new(Vec::new()));

        // Collect everything the room broadcasts, so nothing is missed between
        // polls.
        let mut events = handle.subscribe();
        let collector = Arc::clone(&inbox);
        runtime.spawn(async move {
            while let Ok(message) = events.recv().await {
                let mut inbox = collector.lock().expect("inbox lock");
                if inbox.len() >= INBOX_LIMIT {
                    inbox.remove(0);
                }
                inbox.push(message);
            }
        });
        runtime.spawn(session.run());

        Ok(Self {
            handle,
            inbox,
            runtime: Some(runtime),
        })
    }

    /// Take everything received since the last call.
    pub fn drain(&self) -> Vec<ServerMessage> {
        let mut inbox = self.inbox.lock().expect("inbox lock");
        std::mem::take(&mut inbox)
    }

    /// The room's current state.
    pub fn snapshot(&self) -> Option<ServerMessage> {
        let runtime = self.runtime.as_ref()?;
        runtime.block_on(self.handle.snapshot())
    }

    /// Say something to the room as a person.
    pub fn say(&self, author: &str, body: &str) -> bool {
        let Some(runtime) = self.runtime.as_ref() else {
            return false;
        };
        runtime.block_on(self.handle.send(Control::HumanMessage {
            author: author.to_string(),
            body: body.to_string(),
            task: None,
        }))
    }

    /// Pause the room.
    pub fn pause(&self, reason: &str) -> bool {
        let Some(runtime) = self.runtime.as_ref() else {
            return false;
        };
        runtime.block_on(self.handle.send(Control::Pause {
            reason: reason.to_string(),
        }))
    }

    /// Resume a paused room.
    pub fn resume(&self) -> bool {
        let Some(runtime) = self.runtime.as_ref() else {
            return false;
        };
        runtime.block_on(self.handle.send(Control::Resume))
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

impl std::fmt::Debug for Room {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Room").finish_non_exhaustive()
    }
}
