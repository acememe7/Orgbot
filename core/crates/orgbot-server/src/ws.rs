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
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use tokio::sync::broadcast;

use orgbot_proto::event::Seq;

use crate::protocol::{ClientMessage, ServerMessage, WIRE_VERSION};
use crate::session::{Control, SessionHandle};

/// Shared state for the HTTP layer: the one room this process hosts.
#[derive(Clone)]
pub struct AppState {
    /// A handle to the running room.
    pub session: SessionHandle,
    /// The room's name, for the status page.
    pub room_name: Arc<str>,
    /// The token a client must present, when the room has one.
    ///
    /// `None` only for a room bound to loopback, where the operating system
    /// is already the boundary. Anything reachable from off the machine has
    /// one; see the bind checks in `main`.
    pub token: Option<Arc<str>>,
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
    headers: HeaderMap,
) -> Response {
    if let Some(origin) = headers.get("origin") {
        if !matches!(
            origin.to_str(),
            Ok("tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost")
        ) {
            return (StatusCode::FORBIDDEN, "web origin is not permitted").into_response();
        }
    }
    ws.max_message_size(256 * 1024)
        .max_frame_size(256 * 1024)
        .on_upgrade(move |socket| serve_client(socket, state))
}

/// Drive one client connection to completion.
async fn serve_client(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    // Nothing is sent before the client has proved it belongs here. The
    // snapshot is the whole room — roster, board, and transcript — so
    // answering an unauthenticated socket would hand all of it over to
    // anyone who can reach the port.
    let Some(since) = handshake(&mut sender, &mut receiver, state.token.as_deref()).await else {
        return;
    };

    let mut subscription = state.session.subscribe();
    let _ = since; // A replacement snapshot always carries a full replay.
    if replay(&mut sender, &state.session).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            frame = receiver.next() => {
                let text = match frame {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    _ => return,
                };
                let Ok(message) = serde_json::from_str::<ClientMessage>(&text) else {
                    let _ = send(&mut sender, &ServerMessage::Error { message: "invalid room message".into() }).await;
                    continue;
                };
                match message {
                    ClientMessage::Hello { .. } | ClientMessage::Resync => {
                        if replay(&mut sender, &state.session).await.is_err() { return; }
                    }
                    ClientMessage::Ping => {
                        if send(&mut sender, &ServerMessage::Pong).await.is_err() { return; }
                    }
                    message => if !dispatch(&state.session, message).await {
                        let _ = send(&mut sender, &ServerMessage::Error { message: "room control queue is full or closed".into() }).await;
                    },
                }
            }
            message = subscription.recv() => match message {
                Ok(message) => {
                    let closing = matches!(message, ServerMessage::Closed { .. });
                    if send(&mut sender, &message).await.is_err() || closing { return; }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    if replay(&mut sender, &state.session).await.is_err() { return; }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

async fn replay(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    session: &SessionHandle,
) -> Result<(), axum::Error> {
    if let Some(snapshot) = session.snapshot().await {
        send(sender, &snapshot).await?;
    }
    for envelope in session.backlog(Seq::ZERO).await {
        send(
            sender,
            &ServerMessage::Event {
                envelope: Box::new(envelope),
            },
        )
        .await?;
    }
    // Historical task events may include older phases; finish with the current phase.
    if let Some(ServerMessage::Snapshot { room, .. }) = session.snapshot().await {
        send(sender, &ServerMessage::Phase { phase: room.phase }).await?;
    }
    Ok(())
}

/// How long a client has to introduce itself before the room hangs up. Short,
/// so an unauthenticated socket cannot sit and hold resources.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Read the client's opening `Hello` and check its token.
///
/// Returns the sequence the client wants the tail to resume from, or `None`
/// when the connection was refused and has already been told why.
async fn handshake(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    receiver: &mut futures::stream::SplitStream<WebSocket>,
    token: Option<&str>,
) -> Option<Seq> {
    let Some(expected) = token else {
        // A room with no token is bound to loopback only, and a client there
        // may stay silent and just watch, as it always could.
        return Some(Seq::ZERO);
    };

    // Anything but a text frame in time — a timeout, a close, a binary frame,
    // a dropped socket — is not a client that can authenticate.
    let opening = match tokio::time::timeout(HANDSHAKE_TIMEOUT, receiver.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => Some(text),
        _ => None,
    };

    match admit(opening.as_deref(), Some(expected)) {
        Admission::Accept(since) => Some(since),
        Admission::Refuse(reason) => {
            refuse(sender, reason).await;
            None
        }
    }
}

/// Whether an opening frame admits a client, and from where it resumes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Admission {
    /// Let it in, resuming the tail after this sequence.
    Accept(Seq),
    /// Turn it away, with the reason to send back.
    Refuse(&'static str),
}

/// Decide whether to admit a client.
///
/// Kept free of I/O so every way in can be tested directly: this is the only
/// thing standing between a stranger on the network and the whole room.
/// `opening` is `None` when nothing arrived before the deadline.
pub(crate) fn admit(opening: Option<&str>, token: Option<&str>) -> Admission {
    let Some(expected) = token else {
        // No token: loopback only, and a silent watcher is allowed.
        return Admission::Accept(Seq::ZERO);
    };
    let Some(opening) = opening else {
        return Admission::Refuse("this room needs a token; none was sent");
    };
    match serde_json::from_str::<ClientMessage>(opening) {
        Ok(ClientMessage::Hello {
            since,
            token: Some(offered),
            wire_version: WIRE_VERSION,
        }) if secret_eq(&offered, expected) => Admission::Accept(since),
        _ => Admission::Refuse("that token is not valid for this room"),
    }
}

/// Tell a client why it is being hung up on, then hang up.
async fn refuse(sender: &mut futures::stream::SplitSink<WebSocket, Message>, reason: &str) {
    let _ = send(
        sender,
        &ServerMessage::Closed {
            reason: reason.to_string(),
        },
    )
    .await;
    let _ = sender.close().await;
}

/// Compare two secrets in constant time.
///
/// A plain `==` returns as soon as two bytes differ, which lets a caller
/// discover a token one character at a time by measuring how long the refusal
/// takes. The length difference is not hidden, and does not need to be.
fn secret_eq(offered: &str, expected: &str) -> bool {
    let (offered, expected) = (offered.as_bytes(), expected.as_bytes());
    if offered.len() != expected.len() {
        return false;
    }
    offered
        .iter()
        .zip(expected)
        .fold(0u8, |differences, (a, b)| differences | (a ^ b))
        == 0
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
        ClientMessage::SendMessage {
            author,
            body,
            task,
            files,
        } => {
            session
                .send(Control::HumanMessage {
                    author,
                    body,
                    task,
                    files,
                })
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
    let _ = state;
    format!("orgbot server (wire v{WIRE_VERSION})\n")
}

/// The `GET /health` check.
pub async fn health() -> &'static str {
    "ok"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello(token: Option<&str>, since: u64) -> String {
        let token = match token {
            Some(t) => format!(r#","token":"{t}""#),
            None => String::new(),
        };
        format!(r#"{{"type":"hello","wire_version":1,"since":{since}{token}}}"#)
    }

    /// A room with no token is loopback-only, where the operating system is
    /// the boundary — a silent watcher stays allowed.
    #[test]
    fn a_room_without_a_token_admits_anyone() {
        assert_eq!(admit(None, None), Admission::Accept(Seq::ZERO));
        assert_eq!(
            admit(Some(&hello(None, 0)), None),
            Admission::Accept(Seq::ZERO)
        );
    }

    #[test]
    fn the_right_token_gets_in_and_keeps_its_resume_point() {
        let opening = hello(Some("s3cret-token-long-enough"), 42);
        match admit(Some(&opening), Some("s3cret-token-long-enough")) {
            Admission::Accept(since) => assert_eq!(since, Seq(42)),
            other => panic!("expected admission, got {other:?}"),
        }
    }

    #[test]
    fn a_wrong_token_is_turned_away() {
        let opening = hello(Some("wrong"), 0);
        assert!(matches!(
            admit(Some(&opening), Some("s3cret-token-long-enough")),
            Admission::Refuse(_)
        ));
    }

    /// The case that mattered: a client that says nothing used to be handed
    /// the entire room.
    #[test]
    fn saying_nothing_is_turned_away() {
        assert!(matches!(
            admit(None, Some("s3cret-token-long-enough")),
            Admission::Refuse(_)
        ));
    }

    #[test]
    fn a_hello_with_no_token_is_turned_away() {
        assert!(matches!(
            admit(Some(&hello(None, 0)), Some("s3cret-token-long-enough")),
            Admission::Refuse(_)
        ));
    }

    /// Opening with something other than a hello must not be a way past the
    /// check — posting a message, for instance.
    #[test]
    fn another_message_type_is_not_a_way_in() {
        let post = r#"{"type":"send_message","author":"me","body":"hi"}"#;
        assert!(matches!(
            admit(Some(post), Some("s3cret-token-long-enough")),
            Admission::Refuse(_)
        ));
    }

    #[test]
    fn rubbish_is_turned_away_rather_than_panicking() {
        for opening in ["", "{", "null", "[]", "not json at all"] {
            assert!(
                matches!(
                    admit(Some(opening), Some("s3cret-token-long-enough")),
                    Admission::Refuse(_)
                ),
                "{opening:?} should not be admitted"
            );
        }
    }

    /// A prefix of the token must not be accepted, and neither must a longer
    /// string that starts with it.
    #[test]
    fn near_misses_are_not_close_enough() {
        let expected = "s3cret-token-long-enough";
        for offered in [
            "s3cret-token-long-enoug",
            "s3cret-token-long-enough!",
            "S3CRET-TOKEN-LONG-ENOUGH",
            "",
        ] {
            assert!(
                matches!(
                    admit(Some(&hello(Some(offered), 0)), Some(expected)),
                    Admission::Refuse(_)
                ),
                "{offered:?} should not be admitted"
            );
        }
    }

    #[test]
    fn secrets_compare_equal_only_when_identical() {
        assert!(secret_eq("abc", "abc"));
        assert!(!secret_eq("abc", "abd"));
        assert!(!secret_eq("abc", "ab"));
        assert!(!secret_eq("", "a"));
        assert!(secret_eq("", ""));
    }
}
