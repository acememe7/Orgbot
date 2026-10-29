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
use orgbot_proto::task::Task;
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
        "\nRule loading for this OrgBots task:\n\
         - Read applicable owner/project instruction entrypoints and keep core \
           engineering, quality, permission, credential, file-retention and review \
           requirements. Where available, engineering-protocol.md and master-taste.md \
           remain core. Unknown mandatory rules remain mandatory.\n\
         - Select domain manuals from the actual assigned work, affected files and \
           data risk, not your model name or job title: design-language.md and \
           frontend-craft.md for UI/interaction/game presentation; website-rules.md \
           for websites; unity-protocol.md for Unity; unreal-protocol.md for Unreal; \
           deadzone-networking.md for the specified peer-network architecture; \
           deadzone-matchmaking.md for its matchmaking. Load production-guardrails.md \
           for security-sensitive work, authentication, secrets, private data, \
           databases, billing, uploads, deployment or backup/restore.\n\
         - In OrgBots, this task scope replaces blanket startup reads of those \
           domain manuals. It does not waive their requirements when applicable \
           or an owner's explicit request to read one. Load additional relevant \
           rules before acting when the task changes; if applicability is uncertain, \
           inspect the relevant rule's scope. Never edit host rules or profiles to \
           save context.\n\
         - Use skill names/summaries to choose a relevant playbook, then load that \
           body with use_skill or its available file reader. Do not dump the whole \
           skill catalog or read every manual. Reuse unchanged rules already read \
           in this session; avoid recursively reloading cross-references. Record \
           selected domain rules and their reason in the task's verification \
           note, not repeated status messages.\n\
         - This room assigns specialists and independent reviewers. Use its tasks \
           and handoffs; do not create a second external review/delegation workflow \
           merely because a general CLI rule describes one. The room's permissions \
           and independent-approval gate still apply.\n\n",
    );

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
        "Verification before review:\n\
         - Workers run focused checks for their changes before submit_for_review. \
           Put the exact commands, exit results, tested files or revision, and \
           any checks that could not run in one short verification note per \
           submission. A resubmission adds a note of what changed, not a new full \
           report. Never infer a passing test from a planned command or missing \
           output.\n\
         - Use a QA/tester teammate for reproduction, broader coverage, or a \
           combined suite when changes interact. Reuse current evidence instead \
           of making every teammate repeat the same full audit.\n\
         - Reviewers inspect the implementation and verification artifacts, then \
           own the combined check of the current project state before approval. \
           Run the relevant checks when permitted; a read-only reviewer must keep \
           generated outputs in temporary storage. If a check needs writable \
           project files or unavailable tools, hand it to a QA/tester teammate \
           and inspect the returned command results. Do not approve work with \
           failed required checks or present unrun checks as verified.\n\
         - Every document costs the owner tokens to write and teammates tokens to \
           read. Do not write plan, checkpoint, status, audit, handover or summary \
           documents, as artifacts or as project files: status goes in one short \
           room message, and what a teammate needs goes in the verification note \
           or the handoff brief. Write a document, or add a Markdown file to the \
           project, only when the owner or the task asks for one.\n\n",
    );

    out.push_str(
        "Working with the others:\n\
         - Answer a human's new question or greeting directly, including when you have no task. \
           If it tags other bots only, let those bots answer.\n\
         - Read existing relevant artifacts with read_artifact before guessing the product or repeating research. \
           Include artifact IDs and concrete product context when delegating.\n\
         - Do not choose the owner's geography, audience, budget or personal identity for them. \
           Ask when it materially affects the work; label any provisional assumption.\n\
         - When you need an owner decision, use ask_owner with a concise question and \
           2-6 distinct options, even before any task is claimed. Custom answers are \
           supported. Include a task only when you own it. An answer never bypasses \
           configured tool or filesystem permissions. Continue from the answer instead \
           of asking the same question again.\n\
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
    if matches!(spec.provider, orgbot_proto::agent::ProviderRef::CliAgent { .. }) {
        out.push_str(
            "Images you make with your own image tool are posted to the room for \
             you, so the owner sees them; say in a sentence what you made. Do not \
             copy them into the project unless the owner or the task asks. To work \
             from an image someone replied to, import it with \
             workspace_import_attachment.\n\n",
        );
    }

    if !engine.archive().is_empty() {
        out.push_str(
            "\nEarlier rooms have already built things. Before you start a task, \
             call find_prior_work with what it is about. If it has been done, \
             read it with read_prior_artifact and reuse or adapt it — say what \
             you reused. Rebuilding something this organisation already has is \
             the most expensive mistake available to you.\n",
        );
    }

    out.push_str(
        "\nSkills and external MCP tools load on demand. Search find_capabilities \
         for relevant skills or server names; select a server to search its tool \
         summaries. Load only the needed playbook with use_skill, or selected \
         schema with load_mcp_tool, then invoke it through call_mcp_tool. \
         Use lifetime=once for a one-off; otherwise task leases end when your \
         assigned work leaves implementation (or this turn ends if no task is held). \
         Call unload_capability as soon as a capability is no longer needed. \
         You can discover and reload it later. A one-use MCP attempt releases its \
         lease even while awaiting approval; reload and retry the same arguments \
         after the owner allows it. Loading never grants execution permission. \
         Native CLI transcripts may retain previously read text; unloading does \
         not erase that history or reset your session. Save findings a teammate \
         will need in your verification note or handoff before releasing \
         context.\n",
    );

    out.push_str(
        "Style: you are talking to colleagues. Be brief and concrete. Say what you \
         did and what you need, not what you are about to do. Do not narrate your \
         tool calls.\n\
         \n\
         When you address the operator, open with one plain sentence a person \
         can read without knowing the project: what happened, and whether \
         anything is needed from them. No ids, no artifact hashes, no status \
         codes in that first line — put those underneath if they matter. \
         Someone who has been away for a day should be able to read your first \
         line and know where things stand. If the only honest first line is \
         `nothing needs you`, say that.\n",
    );

    #[cfg(target_os = "linux")]
    out.push_str(
        "\nFile retention: never permanently delete project files without explicit \
         owner permission. For recoverable removal use the installed shell command \
         `orgbot-trash move --project PROJECT -- PATH...`, with each argument safely \
         quoted. It moves files to the owner's configured trash by project. In your working copy it \
         needs no setup: do not configure trash storage or ask the owner to choose one. Do not use rm, unlink, \
         deletion patches, git clean, or other deletion shortcuts. If the command or \
         selected storage is unavailable, preserve the files and ask the owner. The independent \
         cleanup timer removes trash only after seven full days; never create an AI \
         task to clear it or run cleanup yourself. This policy does not grant extra \
         filesystem permissions.\n",
    );

    if spec.roles.iter().any(|role| role == "coordinator") {
        out.push_str(
            "\nCoordinate by writing one concrete plan and creating role-specific tasks \
             with acceptance criteria. Check the board before creating tasks; extend \
             the existing plan instead of duplicating another coordinator's work. \
             Route each task to the teammate whose specialty fits it. Consult the \
             other coordinator for a bounded decision when their expertise is needed; \
             do not require both leads to act on every message.\n",
        );
        if spec.autonomy.review_only {
            out.push_str(
                "You coordinate and review with read-only workspace access. Delegate \
                 implementation by creating tasks with required_role; do not claim \
                 implementation tasks or edit project files. Final delivery still \
                 requires independent approval.\n",
            );
        }
    }

    if spec.autonomy.review_only {
        out.push_str(
            "\nFor independent tests, use workspace_verify when available. It runs \
             offline in a read-only project copy; direct generated files to /tmp. \
             It cannot install dependencies or use host credentials. If a required \
             check cannot run there, report the limit instead of claiming it passed.\n",
        );
    }
    if !spec.autonomy.may_inspect {
        out.push_str("\nYou do not inspect work. That is the inspector's job.\n");
    }
    if !spec.autonomy.may_submit {
        out.push_str("You do not perform the final submit. That is the coordinator's job.\n");
    }
    out
}

/// Opens every board snapshot, so a later turn can recognise an earlier one.
///
/// A situation is the board as it stood at the start of one turn. It is the
/// largest thing a bot is sent and the shortest-lived thing it is told: by the
/// next turn it describes a board that has moved. Keeping them all meant a bot
/// carried thirty-odd snapshots and re-sent every one on every step — which is
/// where almost all of a room's token spend was going, and why bots would
/// quote task states that had not been true for hours.
pub const SITUATION_MARKER: &str = "--- the board, as it stands now ---";

/// What replaces a snapshot once a newer one exists.
pub const SUPERSEDED_SITUATION: &str =
    "(an earlier view of the board was here; it is out of date and has been \
     dropped. The current board is further down.)";

/// Build the per-turn situation digest.
///
/// This answers the only question a bot has at the start of a turn: what
/// changed, and what should I do about it?
pub fn situation(engine: &Engine, agent: AgentId, since: Seq) -> String {
    let mut out = String::from(SITUATION_MARKER);
    out.push('\n');

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
    //
    // This is a reading taken now, not a reservation. Several bots take their
    // turns at once and each is looking at the same board, so by the time this
    // one acts, one of these may be gone. Saying so is what stops a bot
    // treating a lost race as the room contradicting itself.
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
        out.push_str(
            "  (the board as it stands now; another bot may take one first. A \
             claim refused as ALREADY_CLAIMED means somebody got there ahead of \
             you — claim a different one rather than stopping.)\n",
        );
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
    if let Some(name) = latest_human_name(engine, since) {
        out.push_str(&owner_name_note(&name));
    }

    // The closing instruction, last thing read and so the most influential.
    //
    // This used to end with "if there is genuinely nothing to do, say so and
    // take no action" for every bot, every turn — including bots holding
    // nothing with claimable work sitting in front of them. A room then
    // accumulates a transcript of "nothing actionable for me", which is a
    // pattern the next model continues: one real room spent eight hours
    // talking with four tasks free and the bot that could take them idle.
    //
    // So the close now names the bot's actual situation, and only offers the
    // do-nothing option when doing nothing is genuinely the answer.
    if asked_by_everyone(engine, agent, since) {
        out.push_str(EVERYONE_ASK_INSTRUCTION);
    }
    out.push_str(&closing_instruction(engine, agent, &mine));
    out
}

/// Said when a message to the whole room asks every bot for something.
///
/// Without it a bot with nothing on the board reads "do the most useful thing"
/// and stays silent, so an `@all` roll call is answered by one bot only.
pub const EVERYONE_ASK_INSTRUCTION: &str =
    "\nA message to the whole room (marked \"to everyone, including you: reply\" \
     above) asks every bot to respond. Answer it yourself in the chat, in your \
     own words and briefly, even if others have already answered. Then carry \
     on as below.\n";

/// Whether an unseen message addressed to everyone asks this bot to respond.
fn asked_by_everyone(engine: &Engine, agent: AgentId, since: Seq) -> bool {
    let roles = engine
        .agent(agent)
        .map(|spec| spec.roles.clone())
        .unwrap_or_default();
    engine.events_since(since).any(|envelope| {
        matches!(
            &envelope.event,
            orgbot_proto::Event::MessagePosted { message }
                if matches!(message.audience, orgbot_proto::Audience::Everyone)
                    && message.wakes(agent, &roles)
        )
    })
}

/// What to tell a bot to do, given what is actually in front of it.
fn closing_instruction(engine: &Engine, agent: AgentId, mine: &[&Task]) -> String {
    let live: Vec<&&Task> = mine.iter().filter(|t| !t.state.is_waiting()).collect();
    if !live.is_empty() {
        return "\nYou have work in hand. Continue it, and stop when you have \
                made a step worth recording.\n"
            .to_string();
    }

    let queue = engine.review_queue_for(agent);
    if !queue.is_empty() {
        return format!(
            "\nYou have {} thing(s) waiting on your inspection. Review one now, \
             reporting every blocking finding you can see in a single pass.\n",
            queue.len()
        );
    }

    let claimable = engine.claimable_by(agent);
    if !claimable.is_empty() {
        let named = claimable
            .iter()
            .take(3)
            .filter_map(|id| engine.task(*id))
            .map(|task| format!("`{}`", task.title))
            .collect::<Vec<_>>()
            .join(", ");
        return format!(
            "\nYou are holding nothing and {} task(s) are open to you, including \
             {named}. Claim one and begin it. Do not reply that there is nothing \
             to do — there is, and it is listed above. If a claim is refused as \
             ALREADY_CLAIMED, somebody simply got there first: claim another. If \
             every one of them is genuinely blocked by something not on the \
             board, say which task and what blocks it.\n",
            claimable.len(),
        );
    }

    if !mine.is_empty() {
        return "\nEverything you hold is waiting on someone else, and there is \
                nothing open to you. Say briefly what you are waiting for, then \
                stop.\n"
            .to_string();
    }

    "\nDo the most useful thing you can right now, then stop. If there is \
     genuinely nothing to do, say so briefly and take no action.\n"
        .to_string()
}

/// The name the human most recently wrote under, among unseen messages.
///
/// A bot's persona names the owner as it was when the room started, and an
/// agent CLI's resumed session remembers that too. The name on the message
/// is current, so it wins.
fn latest_human_name(engine: &Engine, since: Seq) -> Option<String> {
    engine
        .events_since(since)
        .filter_map(|envelope| match &envelope.event {
            orgbot_proto::Event::MessagePosted { message } => match &message.author {
                // A message the room wrote for the owner carries a stand-in
                // name, not the one they go by.
                orgbot_proto::Author::Human { name }
                    if !name.trim().is_empty() && name.trim() != orgbot_proto::OWNER_STAND_IN =>
                {
                    Some(name.trim().to_string())
                }
                _ => None,
            },
            _ => None,
        })
        .last()
}

fn owner_name_note(name: &str) -> String {
    format!(
        "\nThe human writing to the room is {name}. Address them as {name}, \
         even if earlier instructions used another name.\n"
    )
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
                let asks_everyone = matches!(message.audience, orgbot_proto::Audience::Everyone)
                    && message.wakes(agent, roles);
                out.push_str(&format!(
                    "  {who}{}: {}\n",
                    if to_you {
                        " (to you)"
                    } else if asks_everyone {
                        " (to everyone, including you: reply)"
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

#[cfg(test)]
mod tests {
    use super::*;
    use orgbot_proto::agent::ProviderRef;
    use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};

    #[test]
    fn bots_are_told_not_to_write_documents_nobody_asked_for() {
        // A live room wrote sixteen Markdown reports in three hours: a
        // verification file beside every patch, plus plans, checkpoints and
        // custody notes. Each one is output tokens, then input for every
        // teammate who reads it.
        let engine = Engine::new(RoomSpec {
            name: "docs".into(),
            goal: "ship".into(),
            roster: vec![],
            completion: CompletionRule::Manual,
            limits: RoomLimits::default(),
        });
        let spec = AgentSpec::new(
            "dev",
            "Dev",
            ProviderRef::Scripted {
                script: "test".into(),
            },
        );
        let prompt = system_prompt(&engine, &spec);
        assert!(prompt.contains("one short verification note per submission"));
        assert!(prompt.contains("Do not write plan, checkpoint, status, audit, handover or summary"));
        assert!(prompt.contains("only when the owner or the task asks for one"));
    }
}
