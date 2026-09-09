//! What a bot is told.
//!
//! Split in two on purpose. The **system prompt** is everything that does not
//! change during a room — persona, goal, roster, the rules of the workflow — so
//! it is byte-identical on every turn and caches. The **situation digest** is
//! everything that does change, and is rebuilt fresh each turn as a user
//! message.
//!
//! Keeping the volatile half out of the system prompt is what makes prompt
//! caching work at all: a single changing timestamp in the cached prefix
//! invalidates the whole thing on every request.

use orgbot_engine::Engine;
use orgbot_proto::{AgentId, AgentSpec, Seq, TaskState};

/// Build the stable system prompt for a bot.
pub fn system_prompt(engine: &Engine, spec: &AgentSpec) -> String {
    let mut out = String::new();

    if !spec.persona.is_empty() {
        out.push_str(&spec.persona);
        out.push_str("\n\n");
    }

    out.push_str(&format!(
        "You are @{} in a group chat of AI bots working together.\n\
         Your roles: {}.\n\n\
         The room's goal: {}\n\n",
        spec.handle,
        if spec.roles.is_empty() {
            "none".to_string()
        } else {
            spec.roles.join(", ")
        },
        engine.room().spec.goal,
    ));

    out.push_str("Who else is here:\n");
    for other in engine.agents() {
        if other.id == spec.id {
            continue;
        }
        out.push_str(&format!(
            "  @{} — {}\n",
            other.handle,
            if other.roles.is_empty() {
                other.display_name.clone()
            } else {
                other.roles.join("/")
            },
        ));
    }

    out.push_str(
        "\nHow work moves through this room:\n\
         1. Take an open task with claim_task, then start_task.\n\
         2. Do the work. Record what you produce with add_artifact — the actual \
            deliverable goes there, not in a chat message.\n\
         3. Call submit_for_review when it is done. You cannot approve your own \
            work; an inspector decides and may send it back.\n\
         4. Once approved, the coordinator delivers it.\n\n",
    );

    out.push_str(
        "Working with the others:\n\
         - Answer a human's new question or greeting directly, including when you have no task. \
           If it tags other bots only, let those bots answer.\n\
         - Read existing relevant artifacts with read_artifact before guessing the product or repeating research. \
           Include artifact IDs and concrete product context when delegating.\n\
         - Do not choose the owner's geography, audience, budget or personal identity for them. \
           Ask when it materially affects the work; label any provisional assumption.\n\
         - Avoid repeating another bot's status or asking it to confirm a requirement already stated. \
           Contribute new evidence, a decision, or completed work.\n\
         - If you need someone to *do* something, use offer_handoff, not a chat \
           message. A message is a conversation; a handoff is tracked, routed, and \
           chased up if it goes unanswered.\n\
         - An \"assist\" handoff parks your task until they answer. You will be woken \
           when it settles — do not poll, and do not sit in a loop waiting.\n\
         - If you cannot make progress, say so: block_on if you are waiting for \
           something specific, release_task if someone else would do better. \
           Holding work you are stuck on is the one thing that actually stalls the \
           room.\n\
         - If a handoff reaches you and you will not do it, reject it immediately. \
           Someone is blocked on your answer.\n\n",
    );

    // An agent CLI reaches the board through MCP, and every MCP client
    // namespaces a server's tools by its name. The prompt above calls them
    // `claim_task` and so on; the agent will only find `orgbot_room__claim_task`.
    // Without this it hunts for the bare names, concludes it has no room tools,
    // and does nothing — which is exactly a room where the board stays empty.
    if let orgbot_proto::agent::ProviderRef::CliAgent { mcp: true, .. } = spec.provider {
        out.push_str(
            "Your room tools arrive over MCP and are prefixed with the server \
             name. Every tool named above is called as `orgbot_room__<name>` — \
             `orgbot_room__claim_task`, `orgbot_room__create_task`, \
             `orgbot_room__add_artifact`, and so on. Use those exact names; the \
             unprefixed ones do not exist.\n\n",
        );
    }

    out.push_str(
        "Style: you are talking to colleagues. Be brief and concrete. Say what you \
         did and what you need, not what you are about to do. Do not narrate your \
         tool calls.\n",
    );

    if !spec.autonomy.may_inspect {
        out.push_str("\nYou do not inspect work. That is the inspector's job.\n");
    }
    if !spec.autonomy.may_submit {
        out.push_str("You do not perform the final submit. That is the coordinator's job.\n");
    }
    out
}

/// Build the per-turn situation digest.
///
/// This answers the only question a bot has at the start of a turn: what
/// changed, and what should I do about it?
pub fn situation(engine: &Engine, agent: AgentId, since: Seq) -> String {
    let mut out = String::new();

    // Anything addressed to this bot comes first: someone may be blocked on it.
    let offers = engine.offers_for(agent);
    if !offers.is_empty() {
        out.push_str("Handoffs waiting on your answer — accept or reject them now:\n");
        for id in offers {
            let Some(handoff) = engine.handoff(id) else {
                continue;
            };
            let from = engine
                .agent(handoff.from)
                .map(|s| format!("@{}", s.handle))
                .unwrap_or_else(|| handoff.from.short());
            out.push_str(&format!(
                "  {} from {} ({:?}): {}\n",
                handoff.id, from, handoff.kind, handoff.brief
            ));
            if !handoff.expects.is_empty() {
                out.push_str(&format!("    wants back: {}\n", handoff.expects.join("; ")));
            }
            if !handoff.context.is_empty() {
                out.push_str(&format!(
                    "    read first: {}\n",
                    handoff
                        .context
                        .iter()
                        .map(|a| a.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        out.push('\n');
    }

    // Then this bot's own work.
    let mine: Vec<_> = engine
        .tasks()
        .filter(|t| t.assignee == Some(agent) && t.state.is_assigned())
        .collect();
    if mine.is_empty() {
        out.push_str("You are holding no tasks.\n");
    } else {
        out.push_str("Your tasks:\n");
        for task in &mine {
            out.push_str(&format!(
                "  {} [{}] {}\n",
                task.id,
                task.state.as_str(),
                task.title
            ));
            out.push_str(&format!("    brief: {}\n", task.brief));
            if !task.acceptance_criteria.is_empty() {
                out.push_str(&format!(
                    "    must satisfy: {}\n",
                    task.acceptance_criteria.join("; ")
                ));
            }
            if task.revision > 0 {
                out.push_str(&format!(
                    "    this is revision {} — it came back from inspection\n",
                    task.revision
                ));
            }
            if !task.blocked_on.is_empty() {
                out.push_str(&format!(
                    "    blocked on {} thing(s)\n",
                    task.blocked_on.len()
                ));
            }
        }
    }

    // Work this bot could pick up.
    let claimable = engine.claimable_by(agent);
    if !claimable.is_empty() && mine.iter().all(|t| t.state.is_waiting()) {
        out.push_str("\nOpen tasks you could claim:\n");
        for id in claimable.iter().take(8) {
            if let Some(task) = engine.task(*id) {
                out.push_str(&format!(
                    "  {} [{:?}] {}\n",
                    task.id, task.priority, task.title
                ));
            }
        }
    }

    // An inspector's queue.
    let queue = engine.review_queue_for(agent);
    if !queue.is_empty() {
        out.push_str("\nAwaiting your inspection:\n");
        for id in queue {
            if let Some(task) = engine.task(id) {
                out.push_str(&format!("  {} {}\n", task.id, task.title));
                if !task.acceptance_criteria.is_empty() {
                    out.push_str(&format!(
                        "    check: {}\n",
                        task.acceptance_criteria.join("; ")
                    ));
                }
                if !task.artifacts.is_empty() {
                    out.push_str(&format!(
                        "    artifacts: {}\n",
                        task.artifacts
                            .iter()
                            .map(|a| a.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
        }
    }

    // An empty board, for whoever is expected to fill it.
    if let Some(spec) = engine.agent(agent) {
        if spec.autonomy.may_create_tasks && engine.tasks().next().is_none() {
            out.push_str(
                "\nThe board is empty. Break the room's goal into tasks the others can \
                 work in parallel, with acceptance criteria concrete enough to inspect \
                 against.\n",
            );
        }
    }

    // Approved work waiting on delivery. Only shown to bots that can act on
    // it, since for anyone else it is noise they cannot do anything about.
    if let Some(spec) = engine.agent(agent) {
        if spec.autonomy.may_submit {
            let ready = engine.awaiting_delivery();
            if !ready.is_empty() {
                out.push_str("\nApproved and waiting for you to deliver:\n");
                for id in ready {
                    if let Some(task) = engine.task(id) {
                        out.push_str(&format!("  {} {}\n", task.id, task.title));
                    }
                }
            }
        }
    }

    // What has been said since this bot last looked.
    let transcript = recent_messages(engine, agent, since);
    if !transcript.is_empty() {
        out.push_str("\nSince your last turn:\n");
        out.push_str(&transcript);
    }

    out.push_str("\nDo the most useful thing you can right now, then stop. ");
    out.push_str("If there is genuinely nothing to do, say so briefly and take no action.\n");
    out
}

/// Render chat and state changes a bot has not seen yet.
fn recent_messages(engine: &Engine, agent: AgentId, since: Seq) -> String {
    use orgbot_proto::Event;

    let mut out = String::new();
    for envelope in engine.events_since(since) {
        match &envelope.event {
            Event::MessagePosted { message } => {
                // A bot's own messages are not news to it.
                if message.author.agent() == Some(agent) {
                    continue;
                }
                let who = match &message.author {
                    orgbot_proto::Author::Agent { agent } => engine
                        .agent(*agent)
                        .map(|s| format!("@{}", s.handle))
                        .unwrap_or_else(|| agent.short()),
                    orgbot_proto::Author::Human { name } => name.clone(),
                    orgbot_proto::Author::System => "system".to_string(),
                };
                let directed = matches!(
                    message.audience,
                    orgbot_proto::Audience::Agents { .. } | orgbot_proto::Audience::Roles { .. }
                );
                let roles = engine
                    .agent(agent)
                    .map(|spec| spec.roles.as_slice())
                    .unwrap_or(&[]);
                let to_you = directed && message.audience.addresses(agent, roles);
                out.push_str(&format!(
                    "  {who}{}: {}\n",
                    if to_you {
                        " (to you)"
                    } else if directed {
                        " (to other bots)"
                    } else {
                        ""
                    },
                    message.body
                ));
            }
            Event::ArtifactAdded { artifact } => {
                out.push_str(&format!("  [artifact available] {}: {} (task {}). Read it with read_artifact if relevant.\n",
                    artifact.id, artifact.name, artifact.task));
            }
            Event::TaskTransitioned { task, to, .. } if *to == TaskState::Submitted => {
                if let Some(record) = engine.task(*task) {
                    out.push_str(&format!("  [delivered] {}\n", record.title));
                }
            }
            Event::ReviewRecorded { review } => {
                // Feedback on this bot's own work is the most important thing
                // it can read, so it is spelled out rather than summarised.
                let is_mine = engine
                    .task(review.task)
                    .is_some_and(|t| t.assignee == Some(agent));
                if !is_mine {
                    continue;
                }
                out.push_str(&format!("  [inspection] {}\n", review.summary));
                for finding in &review.findings {
                    out.push_str(&format!(
                        "    {:?}: {}{}\n",
                        finding.severity,
                        finding.summary,
                        finding
                            .suggestion
                            .as_ref()
                            .map(|s| format!(" — {s}"))
                            .unwrap_or_default()
                    ));
                }
            }
            Event::HandoffTransitioned { handoff, to, .. } => {
                let Some(record) = engine.handoff(*handoff) else {
                    continue;
                };
                if record.from != agent {
                    continue;
                }
                let detail = record
                    .outcome
                    .as_ref()
                    .map(|o| format!(": {}", o.summary))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "  [your request {} was {}]{}\n",
                    record.id.short(),
                    to.as_str(),
                    detail
                ));
            }
            _ => {}
        }
    }
    out
}
