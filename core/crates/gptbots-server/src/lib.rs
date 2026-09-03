//! # gptbots-server
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
/// This is the library form of `gptbots serve`: it loads `config`, seats the
/// bots, runs the room on its own task, and serves the HTTP/WS surface at
/// `addr` (the Flutter app, a browser, or the desktop shell attach there).
/// It returns when the listener stops serving.
pub async fn serve_room(config: &Path, addr: SocketAddr) -> anyhow::Result<()> {
    let parsed = RoomConfig::load(config)?;
    let room_name: Arc<str> = Arc::from(parsed.room.name.as_str());
    let resolved = parsed.resolve()?;
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

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "serving room; connect a client to ws://{addr}/ws");
    axum::serve(listener, app).await?;
    Ok(())
}

