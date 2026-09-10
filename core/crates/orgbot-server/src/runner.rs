//! The headless runner: drive a room to completion and print what happened.
//!
//! This is the fastest way to see the whole system work — no client, no
//! sockets. It subscribes to the session's broadcast and renders each event to
//! the terminal as it arrives, so a run reads like a chat transcript.

use orgbot_proto::event::Event;
use orgbot_runtime::RunOutcome;

use crate::config::ResolvedRoom;
use crate::protocol::ServerMessage;
use crate::session::RoomSession;

/// Run a resolved room to completion, printing a live transcript.
pub async fn run_headless(resolved: ResolvedRoom, quiet: bool) -> anyhow::Result<RunOutcome> {
    let (session, handle) = RoomSession::build(resolved)?;
    let mut events = handle.subscribe();

    // Render events on a background task so printing never backs up the room.
    let printer = tokio::spawn(async move {
        while let Ok(message) = events.recv().await {
            match &message {
                ServerMessage::Event { envelope, .. } => {
                    if let Some(line) = render(&envelope.event) {
                        if !quiet {
                            println!("{line}");
                        }
                    }
                }
                ServerMessage::Closed { reason } => {
                    if !quiet {
                        println!("\n── room closed: {reason} ──");
                    }
                    break;
                }
                _ => {}
            }
        }
    });

    let outcome = session.run_batch().await;
    let _ = printer.await;
    Ok(outcome)
}

/// Render one event as a transcript line, or `None` for internal bookkeeping
/// that would only add noise.
fn render(event: &Event) -> Option<String> {
    match event {
        Event::RoomOpened { name, goal, .. } => Some(format!("╭─ room `{name}` — {goal}")),
        Event::MessagePosted { message } => {
            let who = match &message.author {
                orgbot_proto::Author::Agent { agent } => format!("@{}", agent.short()),
                orgbot_proto::Author::Human { name } => name.clone(),
                orgbot_proto::Author::System => "system".into(),
            };
            Some(format!("  {who}: {}", message.body))
        }
        Event::TaskCreated { task } => {
            Some(format!("  + task {} — {}", task.id.short(), task.title))
        }
        Event::TaskTransitioned { task, from, to, .. } => Some(format!(
            "  · {} {} → {}",
            task.short(),
            from.as_str(),
            to.as_str()
        )),
        Event::HandoffOffered { handoff } => Some(format!(
            "  ⇢ handoff {} ({:?}): {}",
            handoff.id.short(),
            handoff.kind,
            handoff.brief
        )),
        Event::HandoffTransitioned { handoff, to, .. } => {
            Some(format!("  ⇠ handoff {} {}", handoff.short(), to.as_str()))
        }
        Event::ReviewRecorded { review } => Some(format!(
            "  ⊙ review [{:?}]: {}",
            review.verdict, review.summary
        )),
        Event::DeadlockDetected { resolution, .. } => {
            Some(format!("  ⚠ deadlock broken: {resolution:?}"))
        }
        Event::AgentErrored {
            agent,
            error,
            attempt,
            ..
        } => Some(format!(
            "  ✗ {} errored (attempt {attempt}): {error}",
            agent.short()
        )),
        Event::RoomClosed { reason, .. } => Some(format!("╰─ closed: {reason}")),
        // Status flips and artifact adds are visible through their effects.
        _ => None,
    }
}
