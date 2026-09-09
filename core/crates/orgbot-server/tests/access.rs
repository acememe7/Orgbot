//! The door, tested through a real socket.
//!
//! The unit tests in `ws` cover the admission decision; these cover the
//! wiring around it — specifically that nothing is written to a client that
//! has not been admitted. A snapshot is the whole room: roster, board and
//! transcript. Sending it before the token is checked would hand everything
//! to anyone who can reach the port, and no amount of correct decision logic
//! helps if the answer goes out first.

use std::net::SocketAddr;
use std::path::PathBuf;

use futures::{SinkExt, StreamExt};
use orgbot_server::Access;
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "a-token-long-enough-to-pass";

const ROOM: &str = r#"
[room]
name = "locked"
goal = "prove the door is locked"

[[bots]]
handle = "lead"
roles = ["coordinator"]
[bots.provider]
kind = "scripted"
script = "ok"
"#;

/// Start a room on an ephemeral port and return where it is listening.
async fn room_with(access: Access) -> (SocketAddr, PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "orgbot-access-{}-{}.toml",
        std::process::id(),
        stamp()
    ));
    std::fs::write(&path, ROOM).unwrap();

    let (tx, rx) = tokio::sync::oneshot::channel();
    let served = path.clone();
    tokio::spawn(async move {
        let _ = orgbot_server::serve_room_with_readiness(
            &served,
            "127.0.0.1:0".parse().unwrap(),
            access,
            Some(tx),
        )
        .await;
    });
    let addr = rx.await.unwrap().expect("the room should bind");
    (addr, path)
}

/// A unique-enough suffix so parallel tests do not share a room file.
fn stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Connect, optionally send an opening frame, and return the first thing the
/// server says back.
async fn first_message(addr: SocketAddr, opening: Option<&str>) -> String {
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("the socket should connect; what comes after is the test");
    if let Some(opening) = opening {
        socket
            .send(Message::Text(opening.to_string().into()))
            .await
            .unwrap();
    }
    match socket.next().await {
        Some(Ok(Message::Text(text))) => text.to_string(),
        other => panic!("expected a text frame, got {other:?}"),
    }
}

fn hello(token: Option<&str>) -> String {
    match token {
        Some(token) => format!(r#"{{"type":"hello","wire_version":1,"token":"{token}"}}"#),
        None => r#"{"type":"hello","wire_version":1}"#.to_string(),
    }
}

#[tokio::test]
async fn the_right_token_gets_the_room() {
    let (addr, path) = room_with(Access::with_token(TOKEN)).await;
    let first = first_message(addr, Some(&hello(Some(TOKEN)))).await;
    assert!(
        first.contains(r#""type":"snapshot""#),
        "a valid token should be answered with the room, got {first}"
    );
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn a_wrong_token_is_told_no_and_shown_nothing() {
    let (addr, path) = room_with(Access::with_token(TOKEN)).await;
    let first = first_message(addr, Some(&hello(Some("not-the-token-at-all")))).await;
    assert!(
        first.contains(r#""type":"closed""#),
        "expected a refusal, got {first}"
    );
    assert!(
        !first.contains("snapshot") && !first.contains("roster"),
        "a refusal must not carry any part of the room: {first}"
    );
    let _ = std::fs::remove_file(path);
}

/// The original hole: the snapshot went out before any `Hello` was read, so a
/// client that simply connected was handed the whole room.
#[tokio::test]
async fn a_hello_without_a_token_gets_nothing() {
    let (addr, path) = room_with(Access::with_token(TOKEN)).await;
    let first = first_message(addr, Some(&hello(None))).await;
    assert!(
        first.contains(r#""type":"closed""#),
        "expected a refusal, got {first}"
    );
    let _ = std::fs::remove_file(path);
}

/// Posting as a person must not be a way past the check.
#[tokio::test]
async fn a_stranger_cannot_talk_their_way_in() {
    let (addr, path) = room_with(Access::with_token(TOKEN)).await;
    let post = r#"{"type":"send_message","author":"intruder","body":"delete everything"}"#;
    let first = first_message(addr, Some(post)).await;
    assert!(
        first.contains(r#""type":"closed""#),
        "expected a refusal, got {first}"
    );
    let _ = std::fs::remove_file(path);
}

/// A room with no token is loopback-only and stays as open as it was, so the
/// desktop shell keeps working without ceremony.
#[tokio::test]
async fn an_open_room_still_answers_a_silent_client() {
    let (addr, path) = room_with(Access::open()).await;
    let first = first_message(addr, None).await;
    assert!(
        first.contains(r#""type":"snapshot""#),
        "an open room should answer immediately, got {first}"
    );
    let _ = std::fs::remove_file(path);
}

// ------------------------------------------------------------- the role table
//
// Declaring roles in the room file is what makes the fallback machinery do
// anything: an engine with no table matches names exactly, which is how a
// sixteen-bot room ended up with nine bots free and work stranded on the one
// busy holder of a role.

const ROOM_WITH_ROLES: &str = r#"
[room]
name = "staffed"
goal = "get the work done"

[[bots]]
handle = "ops1"
roles = ["ops"]
[bots.provider]
kind = "scripted"
script = "ok"

[[bots]]
handle = "eng1"
roles = ["engineer"]
[bots.provider]
kind = "scripted"
script = "ok"

[[roles]]
name = "ops"
capabilities = ["infra", "deploy"]
fallbacks = ["engineer"]

[[tasks]]
title = "provision the broker"
brief = "ops work"
required_role = "ops"
"#;

#[test]
fn a_room_file_can_declare_who_covers_for_whom() {
    let path = std::env::temp_dir().join(format!("orgbot-roles-{}.toml", stamp()));
    std::fs::write(&path, ROOM_WITH_ROLES).unwrap();

    let resolved = orgbot_server::config::RoomConfig::load(&path)
        .expect("the file should parse")
        .resolve()
        .expect("and resolve");

    let table = &resolved.roles;
    let ops_holder = vec!["ops".to_string()];
    let engineer = vec!["engineer".to_string()];

    assert_eq!(
        table.eligibility(&ops_holder, Some("ops")),
        orgbot_proto::roles::Eligibility::Exact
    );
    assert_eq!(
        table.eligibility(&engineer, Some("ops")),
        orgbot_proto::roles::Eligibility::Fallback,
        "an engineer may stand in for ops work"
    );
    assert_eq!(
        table.eligibility(&engineer, Some("infra")),
        orgbot_proto::roles::Eligibility::No,
        "and capabilities do not leak the other way"
    );
    let _ = std::fs::remove_file(path);
}

/// A room that declares nothing keeps the behaviour it always had.
#[test]
fn a_room_file_without_roles_matches_by_name() {
    let path = std::env::temp_dir().join(format!("orgbot-noroles-{}.toml", stamp()));
    std::fs::write(&path, ROOM).unwrap();

    let resolved = orgbot_server::config::RoomConfig::load(&path)
        .expect("parse")
        .resolve()
        .expect("resolve");

    assert_eq!(resolved.roles, orgbot_proto::roles::RoleTable::default());
    let _ = std::fs::remove_file(path);
}
