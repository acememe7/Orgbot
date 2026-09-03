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
pub use protocol::{ClientMessage, ServerMessage, WIRE_VERSION};
pub use session::{Control, RoomSession, SessionHandle};
