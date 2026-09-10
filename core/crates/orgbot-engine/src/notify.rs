//! What is worth interrupting someone for.
//!
//! A room generates a great deal of activity and almost none of it is news. A
//! task becoming blocked on another task is the system working; a review
//! rejecting something is the system working; a bot handing work across is the
//! system working. Notifying on any of it trains a person to ignore the
//! notifications, and then the one that mattered is ignored too.
//!
//! So this is a deliberately short list. An event reaches a person when they
//! have to decide something, when something material happened to their
//! security posture, when money or infrastructure is about to be committed, or
//! when the room reached the end of something. Everything else is available to
//! look at and never pushed.

use orgbot_proto::event::Event;
use serde::{Deserialize, Serialize};

/// Why a person is being told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// A decision only they can make.
    DecisionNeeded,
    /// Something material happened to the room's security posture.
    SecurityIncident,
    /// A milestone: an objective delivered.
    MilestoneReached,
    /// The room finished.
    RoomFinished,
    /// The room stopped and cannot continue on its own.
    RoomStuck,
    /// A bot asked you something directly.
    Mentioned,
    /// The room hit its turn budget and is waiting to be resumed.
    ///
    /// Its own reason because it is not a fault and not a finish: the room is
    /// fine and simply out of the allowance it was given. Left unsaid it
    /// becomes a silent stall — one room sat like this for six hours.
    BudgetSpent,
}

impl Reason {
    /// A short line for a push notification.
    pub const fn headline(self) -> &'static str {
        match self {
            Self::DecisionNeeded => "A decision needs you",
            Self::SecurityIncident => "A security issue needs your judgement",
            Self::MilestoneReached => "An objective is done",
            Self::RoomFinished => "The room finished",
            Self::RoomStuck => "The room cannot continue",
            Self::Mentioned => "A bot is asking you something",
            Self::BudgetSpent => "The room is out of turns",
        }
    }

    /// Whether this should interrupt rather than wait to be found.
    ///
    /// A milestone is good news and can wait for someone to look; being asked
    /// for something, or being stuck, cannot.
    pub const fn is_urgent(self) -> bool {
        matches!(
            self,
            Self::DecisionNeeded
                | Self::SecurityIncident
                | Self::RoomStuck
                | Self::Mentioned
                | Self::BudgetSpent
        )
    }
}

/// Whether `event` is worth telling a person about, and why.
///
/// Note what is missing: task transitions, handoffs, reviews, blocks,
/// artifacts. All of those are the room working, and a person who is told
/// about them learns to stop reading.
pub fn reason_for(event: &Event) -> Option<Reason> {
    match event {
        // A question was raised that a standing rule did not already answer.
        Event::DecisionAsked { .. } => Some(Reason::DecisionNeeded),
        Event::RoomClosed { .. } => Some(Reason::RoomFinished),
        // A bot asking a question, or the room reporting it has stopped. Both
        // are things a person is waiting to be told rather than expected to
        // discover by opening the app.
        Event::MessagePosted { message } => {
            if message.intent.expects_reply() && message.author.agent().is_some() {
                Some(Reason::Mentioned)
            } else if message.body.contains("turn budget exhausted") {
                Some(Reason::BudgetSpent)
            } else {
                None
            }
        }
        // A deadlock the room broke by itself is not news; one it reports and
        // cannot resolve is.
        Event::DeadlockDetected { resolution, .. } => {
            matches!(
                resolution,
                orgbot_proto::event::DeadlockResolution::EscalatedToHuman
            )
            .then_some(Reason::RoomStuck)
        }
        // Delivering an objective is the milestone; delivering a subtask is
        // not, and the difference is why tasks carry a kind.
        Event::TaskTransitioned { to, kind, .. } => {
            (*to == orgbot_proto::task::TaskState::Submitted
                && kind.is_headline())
            .then_some(Reason::MilestoneReached)
        }
        _ => None,
    }
}

/// One thing a person should be told, ready to render or push.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    /// Why they are being told.
    pub reason: Reason,
    /// The headline.
    pub title: String,
    /// The detail, in a sentence.
    pub body: String,
    /// Whether it should interrupt.
    pub urgent: bool,
}

/// Turn an event into a notification, or nothing.
pub fn notification_for(event: &Event) -> Option<Notification> {
    let reason = reason_for(event)?;
    let body = match event {
        Event::DecisionAsked { decision } => {
            format!("Decision required because: {}", decision.because)
        }
        Event::RoomClosed { reason, .. } => reason.clone(),
        Event::DeadlockDetected { .. } => {
            "Work is waiting in a circle and the room could not break it.".into()
        }
        Event::TaskTransitioned { title, .. } => format!("\"{title}\" is delivered."),
        Event::MessagePosted { message } => {
            if reason == Reason::BudgetSpent {
                "It has used the turns it was given. Resume it to grant more, \
                 or raise its budget."
                    .into()
            } else {
                message.body.chars().take(160).collect()
            }
        }
        _ => String::new(),
    };
    Some(Notification {
        reason,
        title: reason.headline().to_string(),
        body,
        urgent: reason.is_urgent(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orgbot_proto::decision::Decision;
    use orgbot_proto::event::DeadlockResolution;
    use orgbot_proto::graph::TaskKind;
    use orgbot_proto::task::{TaskState, TaskTransition};
    use orgbot_proto::{AgentId, RoomId, TaskId};

    fn transitioned(to: TaskState, kind: TaskKind) -> Event {
        Event::TaskTransitioned {
            task: TaskId::new(),
            title: "repair the harness".into(),
            from: TaskState::Approved,
            to,
            transition: Box::new(TaskTransition::Submit { by: AgentId::new() }),
            actor: None,
            kind,
        }
    }

    /// The whole point: the ordinary working of the room is not news.
    #[test]
    fn the_room_working_normally_tells_nobody() {
        for quiet in [
            transitioned(TaskState::InProgress, TaskKind::Task),
            transitioned(TaskState::Blocked, TaskKind::Task),
            transitioned(TaskState::InReview, TaskKind::Task),
            // Even a delivered subtask: an objective is the milestone.
            transitioned(TaskState::Submitted, TaskKind::Subtask),
            transitioned(TaskState::Submitted, TaskKind::Review),
        ] {
            assert_eq!(reason_for(&quiet), None, "{quiet:?} should stay quiet");
        }
    }

    #[test]
    fn a_question_reaches_the_person() {
        let event = Event::DecisionAsked {
            decision: Box::new(Decision::ask(
                RoomId::new(),
                "it provisions paid infrastructure",
            )),
        };
        let notification = notification_for(&event).expect("worth telling");
        assert_eq!(notification.reason, Reason::DecisionNeeded);
        assert!(notification.urgent);
        assert!(notification.body.contains("paid infrastructure"));
    }

    #[test]
    fn a_delivered_objective_is_a_milestone_but_not_an_interruption() {
        let notification =
            notification_for(&transitioned(TaskState::Submitted, TaskKind::Objective))
                .expect("worth telling");
        assert_eq!(notification.reason, Reason::MilestoneReached);
        assert!(!notification.urgent, "good news can wait to be found");
        assert!(notification.body.contains("repair the harness"));
    }

    /// A deadlock the room broke itself is not news.
    #[test]
    fn only_an_unresolved_deadlock_is_reported() {
        let broke_it = Event::DeadlockDetected {
            cycle: vec![],
            agents: vec![],
            resolution: DeadlockResolution::ReleasedTask { task: TaskId::new() },
        };
        assert_eq!(reason_for(&broke_it), None);

        let stuck = Event::DeadlockDetected {
            cycle: vec![],
            agents: vec![],
            resolution: DeadlockResolution::EscalatedToHuman,
        };
        assert_eq!(reason_for(&stuck), Some(Reason::RoomStuck));
        assert!(Reason::RoomStuck.is_urgent());
    }

    #[test]
    fn the_room_finishing_is_reported() {
        let event = Event::RoomClosed {
            room: RoomId::new(),
            reason: "every objective is delivered".into(),
        };
        let notification = notification_for(&event).unwrap();
        assert_eq!(notification.reason, Reason::RoomFinished);
        assert_eq!(notification.body, "every objective is delivered");
    }

    #[test]
    fn every_reason_has_a_headline() {
        for reason in [
            Reason::DecisionNeeded,
            Reason::SecurityIncident,
            Reason::MilestoneReached,
            Reason::RoomFinished,
            Reason::RoomStuck,
        ] {
            assert!(!reason.headline().is_empty());
        }
    }
}

#[cfg(test)]
mod attention_tests {
    use super::*;
    use orgbot_proto::chat::{Audience, Author, ChatMessage, MessageIntent};
    use orgbot_proto::{AgentId, MessageId, RoomId, Timestamp};

    fn said(body: &str, intent: MessageIntent, from_bot: bool) -> Event {
        Event::MessagePosted {
            message: Box::new(ChatMessage {
                id: MessageId::new(),
                room: RoomId::new(),
                author: if from_bot {
                    Author::Agent { agent: AgentId::new() }
                } else {
                    Author::System
                },
                audience: Audience::Everyone,
                intent,
                body: body.into(),
                task: None,
                handoff: None,
                reply_to: None,
                attachments: vec![],
                files: vec![],
                sent_at: Timestamp::now(),
            }),
        }
    }

    /// A bot asking a question is the clearest case of "this needs you".
    #[test]
    fn a_bot_asking_something_reaches_you() {
        let event = said("Which registry should I use?", MessageIntent::Ask, true);
        let notification = notification_for(&event).expect("worth telling");
        assert_eq!(notification.reason, Reason::Mentioned);
        assert!(notification.urgent);
        assert!(notification.body.contains("Which registry"));
    }

    /// Bots talking among themselves is not.
    #[test]
    fn bots_reporting_progress_stay_quiet() {
        for chatter in [
            said("Claimed it, starting now.", MessageIntent::Say, true),
            said("Nothing actionable for me.", MessageIntent::Say, true),
            said("Done, submitting for review.", MessageIntent::Status, true),
        ] {
            assert_eq!(reason_for(&chatter), None, "{chatter:?} should stay quiet");
        }
    }

    /// A room out of turns is stopped and waiting, which is not a fault and
    /// not a finish — and left unsaid it is a silent stall.
    #[test]
    fn running_out_of_turns_is_reported_as_its_own_thing() {
        let event = said("room paused: turn budget exhausted", MessageIntent::SystemNotice, false);
        let notification = notification_for(&event).expect("worth telling");
        assert_eq!(notification.reason, Reason::BudgetSpent);
        assert!(notification.urgent, "it is waiting on you to act");
        assert!(notification.body.contains("Resume it"));
    }

    #[test]
    fn ordinary_phase_notices_stay_quiet() {
        assert_eq!(
            reason_for(&said("room running: resumed", MessageIntent::SystemNotice, false)),
            None
        );
    }
}
