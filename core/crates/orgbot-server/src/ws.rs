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
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| serve_client(socket, state))
}

/// Drive one client connection to completion.
async fn serve_client(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    // Nothing is sent before the client has proved it belongs here. The
    // snapshot is the whole room — roster, board, and transcript — so
    // answering an unauthenticated socket would hand all of it over to
    // anyone who can reach the port.
    let Some(since) = handshake(&mut sender, &mut receiver, state.token.as_deref()).await
    else {
        return;
    };

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

    // Replay what already happened.
    //
    // The snapshot carries the roster and the board, but the transcript lives
    // in the event log — so without this a client that attaches after the room
    // started never sees anything said before it connected, and a message can
    // simply vanish. The subscription was taken before the snapshot, so a few
    // events may arrive twice; clients drop anything at or below the highest
    // sequence they have already applied.
    for envelope in state.session.backlog(since).await {
        let event = ServerMessage::Event {
            envelope: Box::new(envelope),
        };
        if send(&mut sender, &event).await.is_err() {
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
            ..
        }) if secret_eq(&offered, expected) => Admission::Accept(since),
        _ => Admission::Refuse("that token is not valid for this room"),
    }
}

/// Tell a client why it is being hung up on, then hang up.
async fn refuse(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    reason: &str,
) {
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
        for offered in ["s3cret-token-long-enoug", "s3cret-token-long-enough!", "S3CRET-TOKEN-LONG-ENOUGH", ""] {
            assert!(
                matches!(admit(Some(&hello(Some(offered), 0)), Some(expected)), Admission::Refuse(_)),
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
