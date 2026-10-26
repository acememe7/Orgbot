//! # orgbot-server
//!
//! Hosts group-chat rooms. A room is defined by a TOML file, run either
//! headless from the CLI or as a WebSocket server that clients — the Flutter
//! app, a browser, another process — attach to.
//!
//! The layering:
//! - [`config`] parses a room file and resolves it into live providers.
//! - [`session`] runs one room as an actor, broadcasting its event log.
//! - [`protocol`] is the JSON contract clients speak.
//! - [`ws`] bridges a WebSocket to a session.
//! - [`runner`] drives a room to completion and prints a transcript.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations, rust_2018_idioms)]

pub mod bodies;
pub mod config;
#[cfg(target_os = "linux")]
pub mod desktop_notifications;
pub mod local_workspace;
pub mod pair;
pub mod protocol;
pub mod runner;
pub mod session;
#[cfg(target_os = "linux")]
pub mod supervisor;
pub mod ws;

pub use config::{ResolvedRoom, RoomConfig};

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use axum::routing::get;
use axum::Router;
pub use protocol::{ClientMessage, ServerMessage, WIRE_VERSION};
pub use session::{Control, RoomSession, SessionHandle};

/// Host one room over WebSocket until the process is told to stop.
///
/// This is the library form of `orgbot serve`: it loads `config`, seats the
/// bots, runs the room on its own task, and serves the HTTP/WS surface at
/// `addr` (the Flutter app, a browser, or the desktop shell attach there).
/// It returns when the listener stops serving.
pub async fn serve_room(config: &Path, addr: SocketAddr, access: Access) -> anyhow::Result<()> {
    serve_room_with_readiness(config, addr, access, None).await
}

/// Host a room with the environment explicitly supplied by its owner process.
/// This is used by the Linux service wrapper so provider secrets stay in the
/// keyring and are never written to a unit file or room TOML.
pub async fn serve_room_with_environment(
    config: &Path,
    addr: SocketAddr,
    access: Access,
    environment: &std::collections::HashMap<String, String>,
) -> anyhow::Result<()> {
    serve_room_inner(config, addr, access, None, Some(environment), true).await
}

/// How a room is reached from outside this process.
///
/// A room's snapshot is the whole thing — roster, board and transcript — and
/// a client can post as a person and drive the bots. So reaching the port is
/// reaching the room, and everything that is not loopback needs a token.
#[derive(Clone)]
pub struct Access {
    /// The token a client must present in its opening `Hello`.
    ///
    /// `None` means anyone who can reach the port is in, which is only
    /// defensible when the operating system is the boundary.
    pub token: Option<String>,
}

impl Default for Access {
    fn default() -> Self {
        Self::with_token(format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ))
    }
}
impl std::fmt::Debug for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Access")
            .field("authenticated", &self.token.is_some())
            .finish()
    }
}

impl Access {
    /// An open room, for loopback only.
    pub fn open() -> Self {
        Self { token: None }
    }

    /// A room that requires this token.
    pub fn with_token(token: impl Into<String>) -> Self {
        Self {
            token: Some(token.into()),
        }
    }
}

/// Like [serve_room], but reports readiness over a channel.
///
/// A host that embeds the server (the desktop shell) needs to know when the
/// room is actually reachable - and *why* it failed when it is not - instead
/// of guessing from the void. The sender receives Ok(local_addr) once the
/// listener is bound and serving, or Err(reason) if loading, resolving or
/// binding failed.
pub async fn serve_room_with_readiness(
    config: &Path,
    addr: SocketAddr,
    access: Access,
    ready: Option<tokio::sync::oneshot::Sender<Result<std::net::SocketAddr, String>>>,
) -> anyhow::Result<()> {
    serve_room_inner(config, addr, access, ready, None, false).await
}

async fn serve_room_inner(
    config: &Path,
    addr: SocketAddr,
    access: Access,
    ready: Option<tokio::sync::oneshot::Sender<Result<std::net::SocketAddr, String>>>,
    environment: Option<&std::collections::HashMap<String, String>>,
    resume_was_running: bool,
) -> anyhow::Result<()> {
    if !addr.ip().is_loopback() && access.token.is_none() {
        anyhow::bail!("a reachable room requires a token");
    }
    if access.token.as_ref().is_some_and(|t| t.trim().len() < 20) {
        anyhow::bail!("room tokens must contain at least 20 characters");
    }
    let mut ready = ready;
    let mut report = |outcome: Result<std::net::SocketAddr, String>| {
        if let Some(tx) = ready.take() {
            let _ = tx.send(outcome);
        }
    };

    let parsed = match RoomConfig::load(config) {
        Ok(p) => p,
        Err(error) => {
            report(Err(format!("cannot load room file: {error}")));
            return Err(error.into());
        }
    };
    let room_name: Arc<str> = Arc::from(parsed.room.name.as_str());
    let resolved = match environment.map_or_else(
        || parsed.resolve(),
        |env| parsed.resolve_with_environment(env),
    ) {
        Ok(r) => r,
        Err(error) => {
            report(Err(format!("cannot resolve room: {error}")));
            return Err(error.into());
        }
    };
    let workspace_spec = resolved.workspace.clone();
    let (mut session, handle) = RoomSession::build_with_resume(resolved, resume_was_running)?;
    #[cfg(target_os = "linux")]
    if resume_was_running {
        session.enable_desktop_notifications();
    }
    let workspace_host = match workspace_spec {
        Some(spec) => Some(local_workspace::LocalWorkspaceHost::attach(spec, &mut session).await?),
        None => None,
    };

    let state = ws::AppState {
        session: handle,
        room_name,
        token: access.token.map(Arc::from),
    };
    let shutdown_session = state.session.clone();
    let app = Router::new()
        .route("/", get(ws::status))
        .route("/health", get(ws::health))
        .route("/ws", get(ws::ws_handler))
        .with_state(state);

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(error) => {
            report(Err(format!(
                "cannot bind {addr} - is another room host already running? ({error})"
            )));
            return Err(error.into());
        }
    };
    // Dropping a host also stops its room, including a model call in flight.
    let session_task = tokio::spawn(session.run());
    struct RunningWorkspace(tokio::task::JoinHandle<()>);
    impl Drop for RunningWorkspace {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _workspace = workspace_host.map(RunningWorkspace);
    let local = listener.local_addr().unwrap_or(addr);
    report(Ok(local));
    tracing::info!(%addr, "serving room; connect a client to ws://{addr}/ws");
    let shutdown_trigger = shutdown_session.clone();
    let serving = axum::serve(listener, app).with_graceful_shutdown(async move {
        #[cfg(unix)]
        {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("install SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = terminate.recv() => {},
            }
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_trigger.send(Control::Stop).await;
    });
    let serving_result = serving.await;
    // Drain an in-flight turn and persist its last committed checkpoint before
    // the service process exits. The room phase itself remains untouched.
    if serving_result.is_err() {
        let _ = shutdown_session.send(Control::Stop).await;
    }
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), session_task).await;
    serving_result?;
    Ok(())
}
