//! The group chat itself: who said what, to whom, about which task.
//!
//! Every bot sees the room transcript, but not every message is *for* every
//! bot. [`Audience`] separates the two so a bot's context window carries the
//! whole conversation while its attention is directed at the messages that
//! actually address it.

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, ArtifactId, HandoffId, MessageId, RoomId, TaskId};
use crate::time::Timestamp;

/// Who produced a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Author {
    /// A bot in the room.
    Agent { agent: AgentId },
    /// The person operating the room.
    Human { name: String },
    /// The engine itself: state changes, deadlock reports, expiries.
    System,
}

impl Author {
    /// The bot behind this message, if any.
    pub const fn agent(&self) -> Option<AgentId> {
        match self {
            Self::Agent { agent } => Some(*agent),
            _ => None,
        }
    }

    /// Whether this message came from the engine rather than a participant.
    pub const fn is_system(&self) -> bool {
        matches!(self, Self::System)
    }
}

/// Who a message is addressed to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Audience {
    /// Everyone in the room.
    Everyone,
    /// The named bots. Others still see it in the transcript.
    Agents { agents: Vec<AgentId> },
    /// Every bot holding one of these roles.
    Roles { roles: Vec<String> },
    /// The human operator.
    Human,
}

impl Audience {
    /// Whether `agent`, holding `roles`, is directly addressed.
    pub fn addresses(&self, agent: AgentId, roles: &[String]) -> bool {
        match self {
            Self::Everyone => true,
            Self::Agents { agents } => agents.contains(&agent),
            Self::Roles { roles: targets } => {
                targets.iter().any(|t| roles.iter().any(|held| held == t))
            }
            Self::Human => false,
        }
    }
}

/// What a message is doing, beyond carrying prose.
///
/// The engine reads this to decide whether a message needs to wake a bot up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageIntent {
    /// Ordinary conversation.
    Say,
    /// A question that expects an answer before the asker continues.
    Ask,
    /// An answer to an earlier `Ask`.
    Answer,
    /// A status report on a task.
    Status,
    /// Narration attached to a handoff.
    Handoff,
    /// Inspection feedback.
    Review,
    /// The engine reporting a state change.
    SystemNotice,
}

impl MessageIntent {
    /// Whether the addressee is expected to respond.
    pub const fn expects_reply(self) -> bool {
        matches!(self, Self::Ask | Self::Handoff)
    }
}

/// A single message in a room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: MessageId,
    pub room: RoomId,
    pub author: Author,
    pub audience: Audience,
    pub intent: MessageIntent,
    /// The message text.
    pub body: String,
    /// The task under discussion, if any. Lets the UI filter the transcript
    /// down to one thread of work.
    pub task: Option<TaskId>,
    /// The handoff this message narrates, if any.
    pub handoff: Option<HandoffId>,
    /// The message being replied to.
    pub reply_to: Option<MessageId>,
    /// Artifacts referenced by the message.
    pub attachments: Vec<ArtifactId>,
    /// User files saved beside the room checkpoint, never inline binary data.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<ChatFile>,
    pub sent_at: Timestamp,
}

/// A user attachment's durable manifest. Paths are derived from its id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatFile {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub bytes: u64,
    #[serde(default)]
    pub has_text: bool,
    #[serde(default)]
    pub images: u32,
    #[serde(default)]
    pub note: String,
}

impl ChatMessage {
    /// A broadcast message from a bot.
    pub fn say(room: RoomId, agent: AgentId, body: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            room,
            author: Author::Agent { agent },
            audience: Audience::Everyone,
            intent: MessageIntent::Say,
            body: body.into(),
            task: None,
            handoff: None,
            reply_to: None,
            attachments: Vec::new(),
            files: Vec::new(),
            sent_at: Timestamp::now(),
        }
    }

    /// A message from the engine.
    pub fn system(room: RoomId, body: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            room,
            author: Author::System,
            audience: Audience::Everyone,
            intent: MessageIntent::SystemNotice,
            body: body.into(),
            task: None,
            handoff: None,
            reply_to: None,
            attachments: Vec::new(),
            files: Vec::new(),
            sent_at: Timestamp::now(),
        }
    }

    /// Builder: direct the message at specific bots.
    pub fn to_agents(mut self, agents: Vec<AgentId>) -> Self {
        self.audience = Audience::Agents { agents };
        self
    }

    /// Builder: attach the message to a task.
    pub fn about_task(mut self, task: TaskId) -> Self {
        self.task = Some(task);
        self
    }

    /// Builder: set the intent.
    pub fn with_intent(mut self, intent: MessageIntent) -> Self {
        self.intent = intent;
        self
    }

    /// Whether this message should wake `agent` (holding `roles`) for a turn.
    ///
    /// A bot is woken by messages that address it and expect something back,
    /// never by its own messages — otherwise a bot that says anything
    /// immediately re-triggers itself and the room spins.
    pub fn wakes(&self, agent: AgentId, roles: &[String]) -> bool {
        if self.author.agent() == Some(agent) {
            return false;
        }
        if !self.audience.addresses(agent, roles) {
            return false;
        }
        match self.audience {
            // A broadcast only wakes bots when it explicitly asks for something.
            Audience::Everyone => self.intent.expects_reply(),
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room() -> RoomId {
        RoomId::new()
    }

    #[test]
    fn a_bot_is_never_woken_by_its_own_message() {
        let (room, bot) = (room(), AgentId::new());
        let msg = ChatMessage::say(room, bot, "thinking out loud").with_intent(MessageIntent::Ask);
        assert!(!msg.wakes(bot, &[]));
    }

    #[test]
    fn a_broadcast_statement_does_not_wake_the_room() {
        let (room, speaker, listener) = (room(), AgentId::new(), AgentId::new());
        let msg = ChatMessage::say(room, speaker, "status: still working");
        assert!(
            !msg.wakes(listener, &[]),
            "ordinary chatter must not trigger a turn for every bot"
        );
    }

    #[test]
    fn a_broadcast_question_wakes_the_room() {
        let (room, speaker, listener) = (room(), AgentId::new(), AgentId::new());
        let msg = ChatMessage::say(room, speaker, "anyone know the schema?")
            .with_intent(MessageIntent::Ask);
        assert!(msg.wakes(listener, &[]));
    }

    #[test]
    fn a_direct_message_wakes_its_target_whatever_the_intent() {
        let (room, speaker, target, bystander) =
            (room(), AgentId::new(), AgentId::new(), AgentId::new());
        let msg = ChatMessage::say(room, speaker, "here is the file").to_agents(vec![target]);
        assert!(msg.wakes(target, &[]));
        assert!(!msg.wakes(bystander, &[]));
    }

    #[test]
    fn a_direct_reply_wakes_only_the_replied_to_bot() {
        let (room, speaker, target, bystander) =
            (room(), AgentId::new(), AgentId::new(), AgentId::new());
        let mut msg = ChatMessage::say(room, speaker, "here is the answer")
            .to_agents(vec![target]);
        msg.reply_to = Some(crate::MessageId::new());
        assert!(msg.wakes(target, &[]));
        assert!(!msg.wakes(bystander, &[]));
        assert!(!msg.wakes(speaker, &[]));
    }

    #[test]
    fn role_addressed_messages_reach_every_holder() {
        let msg = ChatMessage::say(room(), AgentId::new(), "need a review");
        let audience = Audience::Roles {
            roles: vec!["reviewer".into()],
        };
        let msg = ChatMessage { audience, ..msg };
        let bot = AgentId::new();
        assert!(msg.wakes(bot, &["reviewer".into()]));
        assert!(!msg.wakes(bot, &["writer".into()]));
    }

    #[test]
    fn system_messages_wake_nobody_by_default() {
        let msg = ChatMessage::system(room(), "task task_x moved to blocked");
        assert!(
            !msg.wakes(AgentId::new(), &[]),
            "state-change notices are transcript context, not a trigger"
        );
    }

    #[test]
    fn human_addressed_messages_do_not_wake_bots() {
        let mut msg = ChatMessage::say(room(), AgentId::new(), "should I proceed?")
            .with_intent(MessageIntent::Ask);
        msg.audience = Audience::Human;
        assert!(!msg.wakes(AgentId::new(), &["anything".into()]));
    }

    #[test]
    fn only_asks_and_handoffs_expect_a_reply() {
        assert!(MessageIntent::Ask.expects_reply());
        assert!(MessageIntent::Handoff.expects_reply());
        for quiet in [
            MessageIntent::Say,
            MessageIntent::Answer,
            MessageIntent::Status,
            MessageIntent::Review,
            MessageIntent::SystemNotice,
        ] {
            assert!(!quiet.expects_reply(), "{quiet:?}");
        }
    }
}
