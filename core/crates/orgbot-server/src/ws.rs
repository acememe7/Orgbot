//! The WebSocket endpoint.
//!
//! A client connects, immediately receives a [`ServerMessage::Snapshot`], then
//! a live tail of everything that happens: durable [`ServerMessage::Event`]s
//! and ephemeral [`ServerMessage::Stream`] deltas. In the other direction it
//! may send the handful of things a human is allowed to do.
//!
//! Each connection runs two loops against a split socket — one pumping the
//! broadcast out, one reading client control in — so a slow reader never blocks
//! the writer and vice versa.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures::{SinkExt, StreamExt};
use tokio::sync::broadcast;

use crate::protocol::{ClientMessage, ServerMessage, WIRE_VERSION};
use crate::session::{Control, SessionHandle};

/// Shared state for the HTTP layer: the one room this process hosts.
#[derive(Clone)]
pub struct AppState {
    /// A handle to the running room.
    pub session: SessionHandle,
    /// The room's name, for the status page.
    pub room_name: Arc<str>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("room_name", &self.room_name)
            .finish()
    }
}

/// The `GET /ws` upgrade handler.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| serve_client(socket, state))
}

/// Drive one client connection to completion.
async fn serve_client(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    // The client's very first frame may be a `Hello`; it is optional, so a
    // client that just wants to watch can stay silent. Either way the snapshot
    // goes out first.
    let subscription = state.session.subscribe();
    match state.session.snapshot().await {
        Some(snapshot) => {
            if send(&mut sender, &snapshot).await.is_err() {
                return;
            }
        }
        None => {
            let _ = send(
                &mut sender,
                &ServerMessage::Closed {
                    reason: "the room has already finished".into(),
                },
            )
            .await;
            return;
        }
    }

    // Pump broadcast events out to this client.
    let outbound = tokio::spawn(pump_out(sender, subscription));
    // Read client control in.
    let inbound = pump_in(&mut receiver, state.session.clone());
    inbound.await;

    outbound.abort();
}

/// Forward broadcast messages to the socket until it closes or the client lags
/// too far behind.
async fn pump_out(
    mut sender: futures::stream::SplitSink<WebSocket, Message>,
    mut subscription: broadcast::Receiver<ServerMessage>,
) {
    loop {
        match subscription.recv().await {
            Ok(message) => {
                let closing = matches!(message, ServerMessage::Closed { .. });
                if send(&mut sender, &message).await.is_err() {
                    return;
                }
                if closing {
                    return;
                }
            }
            // The client fell behind the broadcast buffer. Tell it to resync
            // rather than dropping it — it can request a fresh snapshot.
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let notice = ServerMessage::Error {
                    message: "fell behind; send a resync".into(),
                };
                if send(&mut sender, &notice).await.is_err() {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Read client messages and turn them into room control until the socket ends.
async fn pump_in(
    receiver: &mut futures::stream::SplitStream<WebSocket>,
    session: SessionHandle,
) {
    while let Some(Ok(frame)) = receiver.next().await {
        let text = match frame {
            Message::Text(text) => text,
            Message::Close(_) => return,
            // Pings are handled by axum; ignore binary and pong frames.
            _ => continue,
        };
        let Ok(message) = serde_json::from_str::<ClientMessage>(&text) else {
            continue;
        };
        if !dispatch(&session, message).await {
            return;
        }
    }
}

/// Apply one client message. Returns `false` once the room has stopped.
async fn dispatch(session: &SessionHandle, message: ClientMessage) -> bool {
    match message {
        ClientMessage::Hello { .. } | ClientMessage::Ping | ClientMessage::Resync => {
            // Hello and Resync are served by the snapshot the client already
            // got on connect; Ping needs no room interaction. Keep the
            // connection alive.
            true
        }
        ClientMessage::SendMessage { author, body, task } => {
            session
                .send(Control::HumanMessage { author, body, task })
                .await
        }
        ClientMessage::AnswerBot { task, answer } => {
            session.send(Control::Answer { task, answer }).await
        }
        ClientMessage::Pause { reason } => session.send(Control::Pause { reason }).await,
        ClientMessage::Resume => session.send(Control::Resume).await,
    }
}

/// Serialize and send one server message.
async fn send(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    message: &ServerMessage,
) -> Result<(), axum::Error> {
    let json = serde_json::to_string(message).map_err(axum::Error::new)?;
    sender.send(Message::Text(json)).await
}

/// The `GET /` status page: a one-line confirmation the server is up.
pub async fn status(State(state): State<AppState>) -> String {
    format!(
        "orgbot server (wire v{WIRE_VERSION}) hosting room `{}`\nconnect a client to /ws\n",
        state.room_name
    )
}

/// The `GET /health` check.
pub async fn health() -> &'static str {
    "ok"
}
