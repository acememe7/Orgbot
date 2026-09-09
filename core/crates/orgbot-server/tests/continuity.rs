use orgbot_engine::{Command, Engine};
use orgbot_proto::agent::ProviderRef;
use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
use orgbot_proto::{AgentSpec, Event, RoomPhase, Seq};
use orgbot_server::{Control, RoomConfig, RoomSession, ServerMessage};
use std::time::Duration;

const ROOM: &str = r#"
[room]
name = "continuity check"
goal = "remember the conversation"
completion = "manual"
[[bots]]
handle = "iris"
provider = { kind = "scripted", script = "I remember the room." }
"#;

#[test]
fn legacy_journal_keeps_messages_and_rejects_missing_events() {
    let mut original = Engine::new(RoomSpec {
        name: "saved".into(),
        goal: "remember".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
    });
    let agent = AgentSpec::new(
        "iris",
        "Iris",
        ProviderRef::Scripted {
            script: "reply".into(),
        },
    );
    original
        .apply(Command::JoinAgent {
            spec: Box::new(agent.clone()),
        })
        .unwrap();
    original.apply(Command::StartRoom).unwrap();
    original
        .apply(Command::PostMessage {
            message: Box::new(orgbot_proto::ChatMessage::say(
                original.room_id(),
                agent.id,
                "A decision to remember",
            )),
        })
        .unwrap();
    let replayed = Engine::from_archive(original.room().clone(), original.log().to_vec()).unwrap();
    assert_eq!(replayed.transcript(), original.transcript());
    assert_eq!(replayed.log(), original.log());
    assert_eq!(replayed.room_id(), original.room_id());
    let mut broken = original.log().to_vec();
    broken.remove(1);
    assert!(Engine::from_archive(original.room().clone(), broken).is_err());
}

#[tokio::test]
async fn restart_restores_identity_chat_and_bot_memory_without_running_again() {
    let dir =
        std::env::temp_dir().join(format!("orgbot-continuity-{}", orgbot_proto::RoomId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("state.json");
    let mut config = RoomConfig::parse(ROOM, "test.toml").unwrap();
    config.room.state_path = Some(path.clone());
    let (session, handle) = RoomSession::build(config.resolve().unwrap()).unwrap();
    let mut events = handle.subscribe();
    let running = tokio::spawn(session.run());
    handle
        .send(Control::HumanMessage {
            files: vec![],
            author: "Alex".into(),
            body: "Keep this decision".into(),
            task: None,
        })
        .await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let ServerMessage::Event { envelope } = events.recv().await.unwrap() {
                if let Event::MessagePosted { message } = envelope.event {
                    if message.body == "I remember the room." {
                        break;
                    }
                }
            }
        }
    })
    .await
    .unwrap();
    let before = handle.backlog(Seq::ZERO).await;
    let memory: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(memory["bots"]
        .as_object()
        .unwrap()
        .values()
        .any(|b| !b["history"].as_array().unwrap().is_empty()));
    running.abort();
    let _ = running.await;
    config.room.limits = Some(RoomLimits { max_duration_ms: 0, ..RoomLimits::default() });
    let (restored, again) = RoomSession::build(config.resolve().unwrap()).unwrap();
    let ServerMessage::Snapshot { room, roster, .. } = restored.snapshot() else {
        panic!("snapshot")
    };
    assert_eq!(room.phase, RoomPhase::Paused);
    assert_eq!(room.spec.limits.max_duration_ms, 0, "updated Android policy applies to existing rooms");
    assert_eq!(room.id, before[0].room);
    assert_eq!(again.backlog(Seq::ZERO).await, before);
    let saved_id = memory["bots"].as_object().unwrap().keys().next().unwrap();
    assert_eq!(roster[0].id.to_string(), *saved_id);
    let after: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(after["bots"], memory["bots"]);
    drop(restored);
    // Unknown/corrupt versions are preserved instead of silently starting over.
    std::fs::write(&path, br#"{"version":999}"#).unwrap();
    assert!(RoomSession::build(config.resolve().unwrap()).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"version\":999}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "Run with ORGBOT_LEGACY_JOURNAL to validate an existing private app journal"]
fn imports_existing_private_journal_without_losing_artifacts_or_reviews() {
    let path = std::env::var("ORGBOT_LEGACY_JOURNAL").unwrap();
    let mut room = None;
    let mut events = std::collections::BTreeMap::new();
    for line in std::fs::read_to_string(path).unwrap().lines() {
        let Ok(frame) = serde_json::from_str::<serde_json::Value>(line) else {
            break;
        };
        if frame["type"] == "snapshot" {
            room =
                Some(serde_json::from_value::<orgbot_proto::Room>(frame["room"].clone()).unwrap());
        }
        if frame["type"] == "event" {
            let event: orgbot_proto::EventEnvelope =
                serde_json::from_value(frame["envelope"].clone()).unwrap();
            events.insert(event.seq, event);
        }
    }
    let events: Vec<_> = events.into_values().collect();
    let engine = Engine::from_archive(room.unwrap(), events.clone()).unwrap();
    assert_eq!(engine.log(), events);
    let mut messages = 0;
    let mut reviews = 0;
    for envelope in &events {
        match &envelope.event {
            Event::MessagePosted { message }
                if !matches!(message.author, orgbot_proto::chat::Author::System) =>
            {
                messages += 1
            }
            Event::ReviewRecorded { .. } => reviews += 1,
            Event::ArtifactAdded { artifact } => {
                assert_eq!(engine.artifact(artifact.id), Some(artifact.as_ref()))
            }
            _ => {}
        }
    }
    assert_eq!(engine.transcript().len(), messages);
    assert_eq!(engine.reviews().len(), reviews);
}

#[tokio::test]
async fn attachments_reach_bot_memory_and_survive_restart_with_room_boundaries() {
    use orgbot_runtime::attachments;
    let dir = std::env::temp_dir().join(format!(
        "orgbot-attachments-{}",
        orgbot_proto::RoomId::new()
    ));
    let root = dir.join("attachments");
    let id = "att_0123456789abcdef0123456789abcdef";
    let folder = root.join(id);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("original"), "original").unwrap();
    std::fs::write(
        folder.join("context.txt"),
        "The study's verified sample size was 73.",
    )
    .unwrap();
    std::fs::write(folder.join("image-0.jpg"), [1u8, 2, 3]).unwrap();
    let manifest = serde_json::json!({"id":id,"name":"study.pdf","mime_type":"application/pdf","bytes":8,"has_text":true,"images":1});
    std::fs::write(folder.join("manifest.json"), manifest.to_string()).unwrap();
    assert!(attachments::manifests(Some(&root), &["../../outside".into()]).is_err());
    assert!(attachments::manifests(Some(&root), &vec![id.into(); 6]).is_err());
    #[cfg(unix)]
    {
        let other = "att_ffffffffffffffffffffffffffffffff";
        std::os::unix::fs::symlink(&dir, root.join(other)).unwrap();
        std::fs::write(dir.join("outside"), "private").unwrap();
        assert!(attachments::file_path(&root, other, "outside").is_err());
    }
    let mut config = RoomConfig::parse(ROOM, "test.toml").unwrap();
    let path = dir.join("state.json");
    config.room.state_path = Some(path.clone());
    let (session, handle) = RoomSession::build(config.resolve().unwrap()).unwrap();
    let mut events = handle.subscribe();
    let running = tokio::spawn(session.run());
    handle
        .send(Control::HumanMessage {
            author: "Alex".into(),
            body: "Inspect my study".into(),
            task: None,
            files: vec![id.into()],
        })
        .await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let ServerMessage::Event { envelope } = events.recv().await.unwrap() {
                if let Event::MessagePosted { message } = envelope.event {
                    if message.body == "I remember the room." {
                        break;
                    }
                }
            }
        }
    })
    .await
    .unwrap();
    let checkpoint = std::fs::read_to_string(&path).unwrap();
    assert!(checkpoint.contains("verified sample size was 73"));
    assert!(checkpoint.contains("image-0.jpg"));
    assert!(!checkpoint.contains("base64"));
    running.abort();
    let _ = running.await;
    let (restored, again) = RoomSession::build(config.resolve().unwrap()).unwrap();
    let messages = again.backlog(Seq::ZERO).await;
    assert!(messages.iter().any(|e| matches!(&e.event, Event::MessagePosted { message } if message.files.first().is_some_and(|f| f.id == id))));
    drop(restored);
    std::fs::write(folder.join("original"), "changed length").unwrap();
    assert!(attachments::manifests(Some(&root), &[id.into()]).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}
