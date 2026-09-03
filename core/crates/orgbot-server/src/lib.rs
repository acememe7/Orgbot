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

pub mod config;
pub mod protocol;
pub mod runner;
pub mod session;
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
pub async fn serve_room(config: &Path, addr: SocketAddr) -> anyhow::Result<()> {
    serve_room_with_readiness(config, addr, None).await
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
    ready: Option<tokio::sync::oneshot::Sender<Result<std::net::SocketAddr, String>>>,
) -> anyhow::Result<()> {
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
    let resolved = match parsed.resolve() {
        Ok(r) => r,
        Err(error) => {
            report(Err(format!("cannot resolve room: {error}")));
            return Err(error.into());
        }
    };
    let (session, handle) = RoomSession::build(resolved)?;

    tokio::spawn(session.run());

    let state = ws::AppState {
        session: handle,
        room_name,
    };
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
    let local = listener.local_addr().unwrap_or(addr);
    report(Ok(local));
    tracing::info!(%addr, "serving room; connect a client to ws://{addr}/ws");
    axum::serve(listener, app).await?;
    Ok(())
}

