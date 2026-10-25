//! The client/server WebSocket protocol.
//!
//! A client — the Flutter app, a browser, the CLI's own viewer — connects,
//! receives a snapshot, then a live tail of the room's event log. It can inject
//! a few things a human is allowed to do: send a message into the chat, pause
//! and resume the room, and answer a question a bot is blocked on.
//!
//! Everything is JSON. The message types mirror the engine's own vocabulary so
//! a client can be a thin renderer over [`orgbot_proto`] without a translation
//! layer.

use orgbot_engine::digest::RoomDigest;
use orgbot_proto::event::{EventEnvelope, Seq};
use orgbot_proto::ids::{DecisionId, MessageId};
use orgbot_proto::room::RoomPhase;
use orgbot_proto::{AgentId, AgentStatus, Room, Task, TaskId};
use orgbot_providers::StreamEvent;
use serde::{Deserialize, Serialize};

/// Owner-only project-memory operation. This surface is available only after
/// the room socket handshake; model tools do not dispatch these controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAction {
    List,
    Search,
    SetShared,
    Delete,
    ShareToProject,
}

/// A bounded request for the owner-facing project memory panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRequest {
    pub request_id: String,
    pub action: MemoryAction,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub shared: Option<bool>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub target_project: Option<String>,
}

/// A memory search result, preserving a stable item DTO for Flutter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemorySearchItem {
    pub item: orgbot_engine::MemoryEntry,
    pub score: f32,
}

/// The protocol version a client must speak. Bumped on a breaking change so a
/// stale client is refused up front rather than failing on an unknown variant.
pub const WIRE_VERSION: u32 = 1;

/// A message from a client to the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Sent first. Declares the client's protocol version and where in the log
    /// it wants the tail to resume, so a reconnecting client does not re-read
    /// what it already has.
    Hello {
        /// The client's wire version.
        wire_version: u32,
        /// Resume the tail after this sequence number. `0` means from the start.
        #[serde(default)]
        since: Seq,
        /// The room's token, when it has one.
        ///
        /// Sent in the first frame rather than as a header or a query
        /// parameter: a header can be stripped by a proxy, and a URL ends up
        /// in logs and history. Absent for a loopback room with no token.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
    },
    /// Post a human message into the chat.
    SendMessage {
        #[serde(default)]
        files: Vec<String>,
        /// The message being replied to, when this is part of a thread.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<MessageId>,
        /// The person's display name.
        author: String,
        /// The message text.
        body: String,
        /// The task under discussion, if any.
        #[serde(default)]
        task: Option<TaskId>,
    },
    /// Answer a bot that is blocked awaiting a human.
    AnswerBot {
        /// The task that was waiting.
        task: TaskId,
        /// The answer.
        answer: String,
    },
    /// Answer a question the room put to the operator.
    ///
    /// Addressed to the decision rather than to a task, because one answer
    /// releases every task that was waiting on that same question.
    AnswerDecision {
        /// The question being answered.
        decision: DecisionId,
        /// The option chosen.
        chosen: String,
        /// Anything the person wants to add.
        #[serde(default)]
        note: String,
    },
    /// Pause the room.
    Pause {
        /// Why, for the transcript.
        #[serde(default)]
        reason: String,
    },
    /// Resume a paused room.
    Resume,
    /// Ask for a fresh snapshot, e.g. after a long gap.
    Resync,
    /// A liveness ping.
    Ping,
    /// Owner control for the local project memory panel.
    Memory { request: MemoryRequest },
}

/// A message from the server to a client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Sent in response to `Hello`: the current state of the room in one shot.
    /// Everything after arrives as `Event`s.
    Snapshot {
        /// The server's wire version.
        wire_version: u32,
        /// The room record.
        room: Box<Room>,
        /// Everyone in the room, with their current availability.
        roster: Vec<AgentView>,
        /// The whole board.
        tasks: Vec<Task>,
        /// The most recent log position, so the client knows where the tail
        /// begins.
        head: Seq,
        /// The board as a person should see it: objectives, progress, and the
        /// decisions only they can make.
        ///
        /// Computed on the server rather than in each client. It is derived
        /// from the same scheduling rules the engine claims work by, and a
        /// second implementation in the UI would be a second set of answers.
        digest: Box<RoomDigest>,
    },
    /// The board changed, so the person-facing view of it did too.
    ///
    /// Sent whenever a task moves. Clients replace their copy wholesale rather
    /// than patching it, because the digest is a fold over every task and a
    /// partial update cannot be applied correctly.
    Digest {
        /// The refreshed view.
        digest: Box<RoomDigest>,
    },
    /// One durable event from the room's log.
    Event {
        /// The event and its position.
        envelope: Box<EventEnvelope>,
        /// Set when this event is one a person should be told about.
        ///
        /// Decided here rather than in each client: the judgement about what
        /// is worth interrupting someone for belongs beside the events, and
        /// two implementations of it would drift — the one that drifted being
        /// the one that woke you at three in the morning.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notification: Option<orgbot_engine::Notification>,
    },
    /// A live model delta, for showing a bot typing. Not part of the durable
    /// log — a reconnecting client never replays these.
    Stream {
        /// Which bot is producing it.
        agent: AgentId,
        /// The incremental update.
        event: StreamEvent,
    },
    /// The room's phase changed. A convenience so a client does not have to
    /// derive it from the event stream.
    Phase {
        /// The new phase.
        phase: RoomPhase,
    },
    /// A reply to `Ping`.
    Pong,
    /// The client's request was refused.
    Error {
        /// What went wrong.
        message: String,
    },
    /// Private reply to the authenticated owner's memory request.
    MemoryResult {
        request_id: String,
        action: MemoryAction,
        ok: bool,
        #[serde(default)]
        entries: Vec<orgbot_engine::MemoryEntry>,
        #[serde(default)]
        hits: Vec<MemorySearchItem>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// The room has finished; the socket will close after this.
    Closed {
        /// Why.
        reason: String,
    },
}

/// A bot plus its live status, for the snapshot roster.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentView {
    /// The bot's id.
    pub id: AgentId,
    /// Its chat handle.
    pub handle: String,
    /// Its display name.
    pub display_name: String,
    /// Its roles.
    pub roles: Vec<String>,
    /// Its current availability.
    pub status: AgentStatus,
    /// Current consecutive failures; old journal errors may already be resolved.
    #[serde(default)]
    pub error_streak: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hello_defaults_to_the_start_of_the_log() {
        let hello: ClientMessage =
            serde_json::from_str(r#"{"type":"hello","wire_version":1}"#).unwrap();
        assert_eq!(
            hello,
            ClientMessage::Hello {
                wire_version: 1,
                since: Seq::ZERO,
                token: None,
            }
        );
    }

    #[test]
    fn client_messages_round_trip() {
        let messages = [
            ClientMessage::SendMessage {
                files: vec![],
                reply_to: None,
                author: "Alex".into(),
                body: "how's it going?".into(),
                task: None,
            },
            ClientMessage::Pause {
                reason: "lunch".into(),
            },
            ClientMessage::Resume,
            ClientMessage::Ping,
        ];
        for message in messages {
            let json = serde_json::to_string(&message).unwrap();
            assert_eq!(
                serde_json::from_str::<ClientMessage>(&json).unwrap(),
                message
            );
        }
    }

    #[test]
    fn memory_controls_carry_owner_action_and_correlation_id() {
        let message = ClientMessage::Memory {
            request: MemoryRequest {
                request_id: "room-1".into(),
                action: MemoryAction::ShareToProject,
                id: Some("entry".into()),
                shared: None,
                query: None,
                target_project: Some("project:/workspace".into()),
            },
        };
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(
            serde_json::from_str::<ClientMessage>(&json).unwrap(),
            message
        );
    }

    #[test]
    fn older_human_message_payloads_default_reply_to() {
        let message: ClientMessage =
            serde_json::from_str(r#"{"type":"send_message","author":"Alex","body":"hello"}"#)
                .unwrap();
        assert!(matches!(
            message,
            ClientMessage::SendMessage { reply_to: None, .. }
        ));
    }

    #[test]
    fn send_message_round_trips_a_reply_target() {
        let message = ClientMessage::SendMessage {
            files: vec![],
            reply_to: Some(MessageId::new()),
            author: "Alex".into(),
            body: "Following up".into(),
            task: None,
        };
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(
            serde_json::from_str::<ClientMessage>(&json).unwrap(),
            message
        );
    }

    #[test]
    fn server_error_serializes_with_a_type_tag() {
        let json = serde_json::to_string(&ServerMessage::Error {
            message: "nope".into(),
        })
        .unwrap();
        assert!(json.contains(r#""type":"error""#));
        assert!(json.contains("nope"));
    }
}
