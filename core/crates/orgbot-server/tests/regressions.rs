//! Regression checks for the defects reproduced during the repository audit.
use futures::{SinkExt, StreamExt};
use orgbot_engine::{Command, Engine};
use orgbot_proto::graph::TaskOrigin;
use orgbot_proto::agent::{Autonomy, ProviderRef};
use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
use orgbot_proto::{AgentSpec, Event, Priority, TaskId};
use orgbot_server::{Access, RoomConfig};
use serde_json::json;
use std::time::Duration;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

const ROOM: &str = r#"
[room]
name = "audit-only"
goal = "no live provider calls"
[[bots]]
handle = "idle"
provider = { kind = "scripted", script = "idle" }
"#;

#[tokio::test]
async fn pause_preserves_the_room_and_resume_wakes_it() {
    let resolved = RoomConfig::parse(ROOM, "test.toml")
        .unwrap()
        .resolve()
        .unwrap();
    let (session, handle) = orgbot_server::RoomSession::build(resolved).unwrap();
    let mut events = handle.subscribe();
    let run = tokio::spawn(session.run());
    for (control, expected) in [
        (
            orgbot_server::Control::Pause {
                reason: "test pause".into(),
            },
            orgbot_proto::RoomPhase::Paused,
        ),
        (
            orgbot_server::Control::Resume,
            orgbot_proto::RoomPhase::Running,
        ),
    ] {
        assert!(handle.send(control).await);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match events.recv().await.unwrap() {
                    orgbot_server::ServerMessage::Phase { phase } if phase == expected => break,
                    orgbot_server::ServerMessage::Closed { .. } => {
                        panic!("pause must not close a room")
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        let orgbot_server::ServerMessage::Snapshot { room, .. } = handle.snapshot().await.unwrap()
        else {
            panic!("snapshot");
        };
        assert_eq!(room.phase, expected);
    }
    run.abort();
}

#[tokio::test]
async fn a_stalled_model_does_not_block_snapshot_human_input_or_pause() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (arrived, mut requests) = tokio::sync::mpsc::channel(8);
    let router = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let arrived = arrived.clone();
            async move {
                arrived.send(body).await.unwrap();
                std::future::pending::<String>().await
            }
        }),
    );
    let dummy = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut resolved = RoomConfig::parse(
        &(ROOM.to_owned() + "\n[[tasks]]\ntitle = \"probe\"\nbrief = \"probe\"\n"),
        "audit.toml",
    )
    .unwrap()
    .resolve()
    .unwrap();
    resolved.bots[0].provider = std::sync::Arc::new(orgbot_providers::OpenAiProvider::custom(
        "audit",
        format!("http://{addr}"),
        orgbot_providers::Credential::ApiKey("synthetic-audit-only".into()),
    ));
    let dir = std::env::temp_dir().join(format!("orgbot-pending-{}", orgbot_proto::RoomId::new()));
    let path = dir.join("state.json");
    resolved.state_path = Some(path.clone());
    let (session, handle) = orgbot_server::RoomSession::build(resolved).unwrap();
    let run = tokio::spawn(session.run());
    tokio::time::timeout(Duration::from_secs(1), requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), handle.snapshot())
            .await
            .unwrap()
            .is_some()
    );
    let mut events = handle.subscribe();
    // The request deliberately never responds. Both messages must still be
    // committed once and shown at once, durable before they are shown, and
    // the stalled turn must not be cut short by them: the owner's input never
    // interrupts a bot that is working.
    let journal = path.with_file_name("owner-input.jsonl");
    for body in ["@idle change direction", "@idle use the new research"] {
        assert!(handle.try_send(orgbot_server::Control::HumanMessage {
            files: vec![],
            author: "Alex".into(),
            body: body.into(),
            task: None,
            reply_to: None,
        }));
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let orgbot_server::ServerMessage::Event { envelope, .. } =
                    events.recv().await.unwrap()
                {
                    if matches!(&envelope.event,
                        Event::MessagePosted { message } if message.body == body)
                    {
                        break;
                    }
                }
            }
        })
        .await
        .expect("the message is shown while the model is still stalled");
        let durable = std::fs::read_to_string(&journal).unwrap_or_default()
            + &std::fs::read_to_string(&path).unwrap_or_default();
        assert!(durable.contains(body), "echo must follow a durable write");
        let backlog = handle.backlog(orgbot_proto::Seq::ZERO).await;
        assert_eq!(backlog.iter().filter(|e| matches!(&e.event,
            Event::MessagePosted { message } if message.body == body)).count(), 1);
        assert!(!backlog.iter().any(|e| matches!(&e.event, Event::AgentErrored { .. })));
        assert!(
            requests.try_recv().is_err(),
            "the stalled turn must not be abandoned and restarted"
        );
    }
    assert!(
        handle
            .send(orgbot_server::Control::StopNow {
                reason: "cancel model".into()
            })
            .await
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let orgbot_server::ServerMessage::Phase {
                phase: orgbot_proto::RoomPhase::Paused,
            } = events.recv().await.unwrap()
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    run.abort();
    dummy.abort();
    let _ = run.await;
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn invalid_review_and_unowned_delegation_are_atomic_rejections() {
    let mut engine = Engine::new(RoomSpec {
        name: "audit".into(),
        goal: "probe".into(),
        roster: vec![],
        completion: CompletionRule::AllTasksTerminal,
        limits: RoomLimits::default(),
        access: Default::default(),
    });
    let bots: Vec<_> = ["owner", "unrelated", "recipient", "qa"]
        .iter()
        .map(|h| {
            AgentSpec::new(*h, *h, ProviderRef::Scripted { script: "".into() }).with_autonomy(
                if *h == "qa" {
                    Autonomy::inspector()
                } else {
                    Autonomy::default()
                },
            )
        })
        .collect();
    for bot in &bots {
        engine
            .apply(Command::JoinAgent {
                spec: Box::new(bot.clone()),
            })
            .unwrap();
    }
    engine.apply(Command::StartRoom).unwrap();
    let events = engine
        .apply(Command::CreateTask {
            title: "owned".into(),
            brief: "probe".into(),
            by: None,
            required_role: None,
            priority: Priority::Normal,
            acceptance_criteria: vec![],
            depends_on: vec![],
            origin: TaskOrigin::plain(),
        })
        .unwrap();
    let task: TaskId = events
        .iter()
        .find_map(|e| match &e.event {
            Event::TaskCreated { task } => Some(task.id),
            _ => None,
        })
        .unwrap();
    engine.apply(Command::OpenTask { task }).unwrap();
    engine
        .apply(Command::ClaimTask {
            task,
            by: bots[0].id,
        })
        .unwrap();
    engine
        .apply(Command::StartTask {
            task,
            by: bots[0].id,
        })
        .unwrap();

    let before = engine.head();
    let review = orgbot_tools::dispatch(
        &mut engine,
        bots[3].id,
        "record_review",
        &json!({"task": task, "summary": "premature review", "findings": []}),
    );
    assert!(review.is_error);
    assert_eq!(engine.head(), before);
    assert_eq!(engine.reviews().len(), 0);

    let offer = orgbot_tools::dispatch(
        &mut engine,
        bots[1].id,
        "offer_handoff",
        &json!({"task": task, "to_agent": "recipient", "kind": "delegate", "brief": "not my task"}),
    );
    assert!(offer.is_error, "{}", offer.content);
    assert_eq!(engine.head(), before);
    assert!(engine.handoffs().next().is_none());
    assert_eq!(engine.task(task).unwrap().assignee, Some(bots[0].id));
}

#[tokio::test]
async fn idle_headless_run_terminates() {
    let resolved = RoomConfig::parse(ROOM, "audit.toml")
        .unwrap()
        .resolve()
        .unwrap();
    assert!(tokio::time::timeout(
        Duration::from_secs(1),
        orgbot_server::runner::run_headless(resolved, true)
    )
    .await
    .is_ok());
}

#[tokio::test]
async fn untrusted_origins_are_refused_and_resync_replays_snapshot() {
    let path = std::env::temp_dir().join(format!(
        "orgbot-audit-synthetic-{}.toml",
        std::process::id()
    ));
    std::fs::write(&path, ROOM).unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let served = path.clone();
    let server = tokio::spawn(async move {
        orgbot_server::serve_room_with_readiness(
            &served,
            "127.0.0.1:0".parse().unwrap(),
            Access::open(),
            Some(tx),
        )
        .await
    });
    let addr = rx.await.unwrap().unwrap();
    let mut request = format!("ws://{addr}/ws").into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Origin", "https://untrusted.example".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(request).await.is_err());
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(1), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(first.to_text().unwrap().contains("\"type\":\"snapshot\""));

    while tokio::time::timeout(Duration::from_millis(100), socket.next())
        .await
        .is_ok()
    {}
    socket
        .send(Message::Text("{\"type\":\"resync\"}".into()))
        .await
        .unwrap();
    let next = tokio::time::timeout(Duration::from_secs(1), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(next.to_text().unwrap().contains("\"type\":\"snapshot\""));
    socket.close(None).await.unwrap();
    server.abort();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn broken_api_streams_recover_without_replaying_workspace_operations() {
    use orgbot_server::{Control, RoomSession, ServerMessage};
    use orgbot_runtime::workspace::{Workspace, WorkspaceSpec};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let network = tokio::spawn(async move {
        let mut requests = Vec::new();
        let write = "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"write1\",\"name\":\"workspace_write\",\"arguments\":\"{\\\"path\\\":\\\"research.md\\\",\\\"content\\\":\\\"verified\\\"}\"}}\n\n";
        let done = "data: {\"type\":\"response.completed\",\"response\":{\"model\":\"test\"}}\n\n";
        for attempt in 0..5 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut headers = Vec::new();
            while !headers.ends_with(b"\r\n\r\n") {
                headers.push(socket.read_u8().await.unwrap());
            }
            let headers = String::from_utf8(headers).unwrap();
            let length: usize = headers.lines().filter_map(|line| line.split_once(':'))
                .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                .unwrap().1.trim().parse().unwrap();
            let mut body = vec![0; length];
            socket.read_exact(&mut body).await.unwrap();
            requests.push(serde_json::from_slice::<serde_json::Value>(&body).unwrap());
            if attempt == 0 {
                socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 21\r\nConnection: close\r\n\r\nTemporary server load").await.unwrap();
                continue;
            }
            let attempt = attempt - 1;
            let response = match attempt {
                0 => format!("{write}{done}"),
                // Even a fully parsed tool proposal must be discarded when the
                // enclosing response drops before completion.
                1 => write.to_string(),
                2 => "data: {\"type\":\"response.output_text.delta\",\"delta\":\"discard this partial answer\"}\n\n".into(),
                _ => format!("data: {{\"type\":\"response.output_text.delta\",\"delta\":\"Recovered after reconnecting.\"}}\n\n{done}"),
            };
            let length = response.len() + if attempt == 1 { 1000 } else { 0 };
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{response}").as_bytes()).await.unwrap();
        }
        requests
    });
    let mut resolved = RoomConfig::parse(ROOM, "recovery.toml").unwrap().resolve().unwrap();
    resolved.bots[0].provider = std::sync::Arc::new(orgbot_providers::CodexProvider::new(
        format!("http://{address}"), orgbot_providers::Credential::Bearer("synthetic-test".into()),
    ));
    let dir = std::env::temp_dir().join(format!("orgbot-recovery-{}", orgbot_proto::RoomId::new()));
    resolved.state_path = Some(dir.join("state.json"));
    let (mut session, handle) = RoomSession::build(resolved).unwrap();
    let (tx, mut calls) = tokio::sync::mpsc::channel(8);
    session.attach_workspace(Workspace::new(
        WorkspaceSpec { label: "test".into(), root: "/work".into(), writable: true, commands: false }, tx,
    ));
    let mut events = handle.subscribe();
    let run = tokio::spawn(session.run());
    assert!(handle.try_send(Control::HumanMessage { files: vec![], author: "Alex".into(), body: "Write my research".into(), task: None, reply_to: None }));
    let call = tokio::time::timeout(Duration::from_secs(5), calls.recv()).await.unwrap().unwrap();
    assert_eq!(call.name, "workspace_write");
    call.reply.send(orgbot_tools::ToolOutcome { content: "write verified exactly once".into(), is_error: false }).unwrap();
    let mut retries = Vec::new();
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            match events.recv().await.unwrap() {
                ServerMessage::Stream { event: orgbot_providers::StreamEvent::Retry { attempt, max_retries }, .. } => {
                    assert_eq!(max_retries, 2); retries.push(attempt);
                }
                ServerMessage::Event { envelope, .. } => match &envelope.event {
                    Event::AgentErrored { error, .. } => panic!("recovery must not fault the bot: {error}"),
                    Event::MessagePosted { message } if message.body == "Recovered after reconnecting." => break,
                    _ => {},
                },
                _ => {},
            }
        }
    }).await.unwrap();
    assert_eq!(retries, [1, 1, 2]);
    assert!(calls.try_recv().is_err(), "do not replay the write or execute the interrupted proposal");
    let requests = network.await.unwrap();
    assert_eq!(requests[0], requests[1]);
    assert_eq!(requests[2], requests[3]);
    assert_eq!(requests[3], requests[4]);
    assert!(requests[4].to_string().contains("write verified exactly once"));
    let saved = std::fs::read_to_string(dir.join("state.json")).unwrap();
    assert!(saved.contains("Recovered after reconnecting."));
    assert!(!saved.contains("discard this partial answer"));
    run.abort(); let _ = run.await;
    std::fs::remove_dir_all(dir).unwrap();
}
