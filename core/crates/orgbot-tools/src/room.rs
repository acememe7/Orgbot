//! The room tools.
//!
//! This is a bot's entire interface to the world: it talks, takes work, asks
//! for help, waits, inspects, and delivers through these calls and nothing
//! else. There is no shell and no computer use.
//!
//! Two things matter more here than anywhere else in the codebase.
//!
//! **The descriptions are the prompt.** A model decides whether to hand work
//! off or plough on alone based on what these say, so they describe *when* to
//! reach for a tool, not just what it does.
//!
//! **Errors are instructions.** Every rejection comes back as a readable
//! sentence saying what to do instead, because the model's next turn sees it
//! and can correct itself. A bare `InvalidArgument` wastes a turn.

use orgbot_engine::{Command, Engine, EngineError};
use orgbot_proto::graph::{Lineage, TaskKind, TaskOrigin};
use orgbot_proto::agent::AgentSpec;
use orgbot_proto::artifact::{ArtifactBody, PART_CHARS};
use orgbot_proto::handoff::{HandoffKind, Recipient};
use orgbot_proto::review::{Finding, Severity};
use orgbot_proto::task::{BlockReason, Priority};
use orgbot_proto::{
    AgentId, ArtifactId, Audience, ChatMessage, HandoffId, MessageIntent, TaskId, TaskState,
};
use orgbot_providers::ToolDef;
use serde_json::Value;

use crate::schema;

/// What a tool call produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutcome {
    /// The text the model sees.
    pub content: String,
    /// Whether the call failed. A failed call still returns a result — dropping
    /// it would leave the model with a dangling call.
    pub is_error: bool,
}

impl ToolOutcome {
    /// A successful result.
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    /// A failure, phrased as a correction the model can act on.
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

/// The tools available to a given bot.
///
/// The list is filtered by the bot's autonomy, so a worker is never *offered*
/// `record_review`. Withholding the tool is better than refusing the call: the
/// model cannot be tempted by a capability it cannot see, and the refusal path
/// stays a backstop rather than the primary control.
pub fn definitions_for(spec: &AgentSpec) -> Vec<ToolDef> {
    let mut tools = vec![
        tool(
            "post_message",
            "Say something in the group chat. Everyone sees the transcript, but only \
             the bots you address are woken to respond. Use `to_agents` or `to_roles` \
             when you need a specific answer, and set `intent` to \"ask\" so they know \
             a reply is expected. Talking is not a substitute for a handoff: if you \
             need someone to *do* work, use offer_handoff.",
            schema::object(
                vec![
                    ("body", schema::string("What to say.")),
                    (
                        "intent",
                        schema::enumerated(
                            "\"ask\" expects an answer, \"answer\" replies to one, \
                             \"status\" reports progress, \"say\" is ordinary chatter.",
                            &["say", "ask", "answer", "status"],
                        ),
                    ),
                    (
                        "to_agents",
                        schema::string_array("Handles of the bots to address, e.g. [\"qa\"]."),
                    ),
                    (
                        "to_roles",
                        schema::string_array("Roles to address, e.g. [\"reviewer\"]."),
                    ),
                    ("task", schema::string("The task id this is about, if any.")),
                ],
                &["body"],
            ),
        ),
        tool(
            "room_status",
            "See who is in the room, what each bot is doing, and the state of every \
             task. Call this when you are unsure whether someone is free, or before \
             deciding who to hand work to.",
            schema::object(vec![], &[]),
        ),
        tool(
            "list_tasks",
            "List tasks on the board. With no filter you get everything; pass `mine` \
             for your own, or `claimable` for open work you are eligible to take.",
            schema::object(
                vec![(
                    "filter",
                    schema::enumerated(
                        "Which slice of the board to return.",
                        &["all", "mine", "claimable", "in_review", "blocked"],
                    ),
                )],
                &[],
            ),
        ),
        tool(
            "claim_task",
            "Take an open task. You must hold the task's required role, and every task \
             it depends on must already be submitted. Claiming does not start the \
             clock — call start_task when you actually begin.",
            schema::object(vec![("task", schema::string("The task id."))], &["task"]),
        ),
        tool(
            "start_task",
            "Begin work on a task you have claimed.",
            schema::object(vec![("task", schema::string("The task id."))], &["task"]),
        ),
        tool(
            "add_artifact",
            "Record a deliverable against a task: the text you wrote, the data you \
             gathered, the patch you produced. Artifacts are what an inspector reads \
             and what a handoff carries, so put the actual work here rather than in a \
             chat message.",
            schema::object(
                vec![
                    ("task", schema::string("The task id.")),
                    ("name", schema::string("A short label, e.g. \"notes.md\".")),
                    ("content", schema::string("The deliverable itself.")),
                    (
                        "kind",
                        schema::enumerated(
                            "How to interpret the content. Defaults to text.",
                            &["text", "json", "patch", "url"],
                        ),
                    ),
                ],
                &["task", "name", "content"],
            ),
        ),
        tool(
            "read_artifact",
            "Read a deliverable by id — yours or another bot's. Use this to pick up \
             the context attached to a handoff before starting work.\n\
             A long artifact arrives in numbered parts. If what you get says it is \
             part 1 of 3, you have not read the document: call this again with \
             `part` until you reach the last one. Never review or build on a \
             partial read.",
            schema::object(
                vec![
                    ("artifact", schema::string("The artifact id.")),
                    (
                        "part",
                        schema::string(
                            "Which part to read, from 1. Defaults to the first.",
                        ),
                    ),
                ],
                &["artifact"],
            ),
        ),
        tool(
            "submit_for_review",
            "Declare a task finished and send it for inspection. You cannot approve \
             your own work — an inspector decides, and may send it back. Attach the \
             artifacts that make up the deliverable.",
            schema::object(
                vec![
                    ("task", schema::string("The task id.")),
                    (
                        "artifacts",
                        schema::string_array("Artifact ids making up the deliverable."),
                    ),
                ],
                &["task"],
            ),
        ),
        tool(
            "offer_handoff",
            "Ask another bot to take on work. Choose the kind carefully:\n\
             - \"assist\": you need something back and will wait for it. Your task is \
             blocked automatically until they answer, so do not also call wait_for.\n\
             - \"delegate\": they take ownership of the task outright. You stop working \
             it and are not blocked.\n\
             - \"review\": you want their inspection.\n\
             - \"escalate\": push the decision to a supervisor.\n\
             Address one bot with `to_agent`, or a role with `to_role` so whoever is \
             free picks it up.",
            schema::object(
                vec![
                    ("task", schema::string("The task the handoff is about.")),
                    ("to_agent", schema::string("Handle of a specific bot.")),
                    ("to_role", schema::string("A role, if any holder will do.")),
                    (
                        "kind",
                        schema::enumerated(
                            "What you are asking for.",
                            &["assist", "delegate", "review", "escalate"],
                        ),
                    ),
                    ("brief", schema::string("What you need them to do.")),
                    (
                        "expects",
                        schema::string_array("What you need back before you can continue."),
                    ),
                    (
                        "context",
                        schema::string_array("Artifact ids they should read first."),
                    ),
                ],
                &["task", "kind", "brief"],
            ),
        ),
        tool(
            "list_handoffs",
            "See handoffs you could accept and the ones you are waiting on. Check this \
             before starting new work — someone may already be blocked on you.",
            schema::object(vec![], &[]),
        ),
        tool(
            "accept_handoff",
            "Take on a handoff addressed to you. Another bot is likely blocked waiting \
             for it, so accept only if you intend to do it now; otherwise reject it so \
             they can ask someone else.",
            schema::object(
                vec![("handoff", schema::string("The handoff id."))],
                &["handoff"],
            ),
        ),
        tool(
            "reject_handoff",
            "Decline a handoff. This immediately unblocks the sender, which is far \
             better for the room than leaving them waiting on work you will not do.",
            schema::object(
                vec![
                    ("handoff", schema::string("The handoff id.")),
                    ("reason", schema::string("Why, so they can route it elsewhere.")),
                ],
                &["handoff", "reason"],
            ),
        ),
        tool(
            "complete_handoff",
            "Deliver on a handoff you accepted. The sender is unblocked the moment you \
             do. Put the actual work in artifacts and summarise it here.",
            schema::object(
                vec![
                    ("handoff", schema::string("The handoff id.")),
                    ("summary", schema::string("What you produced, in prose.")),
                    ("artifacts", schema::string_array("Artifact ids you produced.")),
                ],
                &["handoff", "summary"],
            ),
        ),
        tool(
            "block_on",
            "Park your task because it cannot proceed. Prefer this over waiting in \
             silence: the engine tracks who is waiting on whom and will detect a \
             circular wait rather than letting the room hang. A blocking handoff \
             already parks you, so you do not need this after offer_handoff.",
            schema::object(
                vec![
                    ("task", schema::string("Your task id.")),
                    (
                        "waiting_for_task",
                        schema::string("A task id that must finish first."),
                    ),
                    (
                        "question_for_human",
                        schema::string("A question only a person can answer."),
                    ),
                ],
                &["task"],
            ),
        ),
        tool(
            "release_task",
            "Give a task back to the pool because you cannot make progress on it. \
             Better than holding work you are stuck on — another bot may be able to.",
            schema::object(
                vec![
                    ("task", schema::string("The task id.")),
                    ("reason", schema::string("Why you are handing it back.")),
                ],
                &["task", "reason"],
            ),
        ),
    ];

    if spec.autonomy.may_create_tasks {
        tools.push(tool(
            "create_task",
            "Add work to the board. Before you do, ask: is this a new, independently \
             owned outcome? If it is not — if it is a review, a fix for what a review \
             found, evidence for a claim, or restoring something the room already \
             produced — then set `kind` and point `of` at the task it belongs to. It \
             is still tracked and still real work; it just stops the board filling up \
             with cards that all describe one objective.\n\
             Split work along lines that let different bots work in parallel, and \
             write acceptance criteria concrete enough that an inspector can check \
             them without asking you. Use `depends_on` for work that genuinely cannot \
             start until something else has shipped.",
            schema::object(
                vec![
                    ("title", schema::string("A one-line summary.")),
                    ("brief", schema::string("The full instruction for whoever takes it.")),
                    (
                        "acceptance_criteria",
                        schema::string_array("Checkable conditions for approval."),
                    ),
                    (
                        "required_role",
                        schema::string("Restrict to bots holding this role."),
                    ),
                    (
                        "priority",
                        schema::enumerated(
                            "How urgently it should be picked up.",
                            &["low", "normal", "high", "urgent"],
                        ),
                    ),
                    (
                        "depends_on",
                        schema::string_array("Task ids that must be submitted first."),
                    ),
                    ("open", schema::boolean("Publish immediately. Defaults to true.")),
                    (
                        "anyway",
                        schema::string(
                            "Pass \"true\" to create this even though similar work \
                             exists. Say in the brief how it differs.",
                        ),
                    ),
                    (
                        "kind",
                        schema::enumerated(
                            "What this record is. \"objective\" is a top-level outcome; \
                             everything else attaches to one via `of`. Defaults to \
                             \"task\".",
                            &[
                                "objective",
                                "task",
                                "subtask",
                                "review",
                                "remediation",
                                "evidence",
                                "recovery",
                                "continuation",
                            ],
                        ),
                    ),
                    (
                        "of",
                        schema::string(
                            "The task this belongs to. Required for every kind except \
                             \"objective\" and \"task\" — a review with no `of` is just \
                             another top-level card.",
                        ),
                    ),
                ],
                &["title", "brief"],
            ),
        ));
    }

    if spec.autonomy.may_inspect {
        tools.push(tool(
            "record_review",
            "Inspect work that is awaiting review and record your findings. You do not \
             choose a verdict directly — it follows from what you report: a \"major\" \
             finding sends the work back for rework, a \"critical\" one fails the task, \
             and anything less approves it. So report what you actually found and let \
             the verdict follow. Check the task's acceptance criteria one by one.",
            schema::object(
                vec![
                    ("task", schema::string("The task under review.")),
                    ("summary", schema::string("Your overall assessment.")),
                    (
                        "findings",
                        schema::object_array(
                            "Specific problems. Omit or leave empty to approve.",
                            schema::object(
                                vec![
                                    (
                                        "severity",
                                        schema::enumerated(
                                            "note and minor do not block; major sends it \
                                             back; critical fails the task.",
                                            &["note", "minor", "major", "critical"],
                                        ),
                                    ),
                                    ("summary", schema::string("What is wrong.")),
                                    ("locus", schema::string("Where — an artifact or a criterion.")),
                                    ("suggestion", schema::string("What to do about it.")),
                                ],
                                &["severity", "summary"],
                            ),
                        ),
                    ),
                    (
                        "criteria_met",
                        schema::string_array("Acceptance criteria that passed."),
                    ),
                    (
                        "criteria_failed",
                        schema::string_array("Acceptance criteria that failed."),
                    ),
                ],
                &["task", "summary"],
            ),
        ));
    }

    if spec.autonomy.may_submit {
        tools.push(tool(
            "submit_task",
            "Deliver a task that has passed inspection. This is the final step and \
             cannot be undone. Only approved tasks can be submitted.",
            schema::object(vec![("task", schema::string("The task id."))], &["task"]),
        ));
    }

    tools
}

fn tool(name: &str, description: &str, input_schema: Value) -> ToolDef {
    ToolDef {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
    }
}

/// Run a tool call against the room.
///
/// Never panics and never returns `Err`: a bad call comes back as a
/// [`ToolOutcome`] flagged as an error, so the model reads the correction and
/// takes another turn.
pub fn dispatch(engine: &mut Engine, actor: AgentId, name: &str, input: &Value) -> ToolOutcome {
    match name {
        "post_message" => post_message(engine, actor, input),
        "room_status" => room_status(engine, actor),
        "list_tasks" => list_tasks(engine, actor, input),
        "claim_task" => simple(engine, input, "task", |task| Command::ClaimTask {
            task,
            by: actor,
        }),
        "start_task" => simple(engine, input, "task", |task| Command::StartTask {
            task,
            by: actor,
        }),
        "add_artifact" => add_artifact(engine, actor, input),
        "read_artifact" => read_artifact(engine, input),
        "submit_for_review" => submit_for_review(engine, actor, input),
        "offer_handoff" => offer_handoff(engine, actor, input),
        "list_handoffs" => list_handoffs(engine, actor),
        "accept_handoff" => accept_handoff(engine, actor, input),
        "reject_handoff" => reject_handoff(engine, actor, input),
        "complete_handoff" => complete_handoff(engine, actor, input),
        "block_on" => block_on(engine, actor, input),
        "release_task" => release_task(engine, actor, input),
        "create_task" => create_task(engine, actor, input),
        "record_review" => record_review(engine, actor, input),
        "submit_task" => simple(engine, input, "task", |task| Command::SubmitTask {
            task,
            by: actor,
        }),
        unknown => ToolOutcome::error(format!(
            "There is no tool called `{unknown}`. Use one of the tools listed for you."
        )),
    }
}

// ------------------------------------------------------------ argument access

fn required_str<'a>(input: &'a Value, field: &str) -> Result<&'a str, String> {
    input
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("`{field}` is required and must be a non-empty string."))
}

fn optional_str<'a>(input: &'a Value, field: &str) -> Option<&'a str> {
    input
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn string_list(input: &Value, field: &str) -> Vec<String> {
    input
        .get(field)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Parse an id, explaining the expected shape on failure.
fn parse_id<T: std::str::FromStr>(raw: &str, field: &str, prefix: &str) -> Result<T, String> {
    raw.parse().map_err(|_| {
        format!("`{field}` is not a valid id. Expected something like `{prefix}_1a2b3c…`, got `{raw}`.")
    })
}

fn task_arg(input: &Value, field: &str) -> Result<TaskId, String> {
    parse_id(required_str(input, field)?, field, "task")
}

fn artifact_ids(input: &Value, field: &str) -> Result<Vec<ArtifactId>, String> {
    string_list(input, field)
        .iter()
        .map(|raw| parse_id(raw, field, "art"))
        .collect()
}

/// Render an engine rejection as advice rather than a bare error.
fn explain(error: EngineError) -> ToolOutcome {
    let advice = match &error {
        EngineError::NotAssignee { .. } => {
            Some("Claim the task first, or ask its current owner for a handoff.")
        }
        EngineError::RoleMismatch { .. } => {
            Some("You do not hold the role this task requires. Leave it for a bot that does.")
        }
        EngineError::NotPermitted { .. } => {
            Some("That is another role's job in this room. Hand it off instead.")
        }
        EngineError::SelfInspection { .. } => {
            Some("Submit it for review and let a different bot inspect it.")
        }
        EngineError::DependencyNotMet { .. } => {
            Some("Wait for the dependency to be submitted, or work on something else.")
        }
        EngineError::TooManyOpenHandoffs { .. } => {
            Some("Wait for one of your outstanding requests to settle before asking for more.")
        }
        EngineError::Transition(_) => {
            Some("The task is not in a state that allows this. Call list_tasks to see where it is.")
        }
        EngineError::OutOfRevisions { .. } => {
            Some("This task has used its rework budget and cannot go round again.")
        }
        _ => None,
    };
    match advice {
        Some(advice) => ToolOutcome::error(format!("{error}. {advice}")),
        None => ToolOutcome::error(error.to_string()),
    }
}

/// Run a command taking a single task id.
fn simple(
    engine: &mut Engine,
    input: &Value,
    field: &str,
    build: impl FnOnce(TaskId) -> Command,
) -> ToolOutcome {
    let task = match task_arg(input, field) {
        Ok(task) => task,
        Err(message) => return ToolOutcome::error(message),
    };
    match engine.apply(build(task)) {
        Ok(_) => ToolOutcome::ok(match engine.task(task) {
            Some(record) => format!(
                "{task} ({}) is now {}.",
                record.title,
                record.state.as_str()
            ),
            None => format!("{task}: done."),
        }),
        Err(error) => explain(error),
    }
}

// ------------------------------------------------------------------- the tools

fn post_message(engine: &mut Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let body = match required_str(input, "body") {
        Ok(body) => body,
        Err(message) => return ToolOutcome::error(message),
    };
    let intent = match optional_str(input, "intent").unwrap_or("say") {
        "ask" => MessageIntent::Ask,
        "answer" => MessageIntent::Answer,
        "status" => MessageIntent::Status,
        _ => MessageIntent::Say,
    };

    let room = engine.room_id();
    let mut message = ChatMessage::say(room, actor, body).with_intent(intent);

    let handles = string_list(input, "to_agents");
    let roles = string_list(input, "to_roles");
    if !handles.is_empty() {
        let mut targets = Vec::new();
        for handle in &handles {
            // Accept a handle or a raw id, since a model will produce either.
            match engine
                .agent_by_handle(handle)
                .map(|spec| spec.id)
                .or_else(|| handle.parse().ok())
            {
                Some(id) => targets.push(id),
                None => {
                    return ToolOutcome::error(format!(
                        "There is no bot with handle `{handle}`. Call room_status to see who is here."
                    ))
                }
            }
        }
        message.audience = Audience::Agents { agents: targets };
    } else if !roles.is_empty() {
        message.audience = Audience::Roles { roles };
    }

    if let Some(raw) = optional_str(input, "task") {
        match parse_id::<TaskId>(raw, "task", "task") {
            Ok(task) => message.task = Some(task),
            Err(message) => return ToolOutcome::error(message),
        }
    }

    match engine.apply(Command::PostMessage {
        message: Box::new(message),
    }) {
        Ok(_) => ToolOutcome::ok("Posted."),
        Err(error) => explain(error),
    }
}

fn room_status(engine: &Engine, actor: AgentId) -> ToolOutcome {
    let mut out = String::new();
    out.push_str(&format!(
        "Goal: {}\nRoom is {:?}, {} of {} turns used.\n\nBots:\n",
        engine.room().spec.goal,
        engine.room().phase,
        engine.room().turns_used,
        engine.room().spec.limits.max_total_turns,
    ));
    for spec in engine.agents() {
        let held = engine.active_tasks_of(spec.id);
        out.push_str(&format!(
            "  @{} ({}) — {:?}{}{}\n",
            spec.handle,
            spec.roles.join("/"),
            engine.status(spec.id),
            if spec.id == actor { " ← you" } else { "" },
            if held.is_empty() {
                String::new()
            } else {
                format!(", holding {} task(s)", held.len())
            },
        ));
    }
    out.push_str("\nBoard:\n");
    out.push_str(&render_tasks(engine, engine.tasks().map(|t| t.id).collect()));
    ToolOutcome::ok(out)
}

fn render_tasks(engine: &Engine, ids: Vec<TaskId>) -> String {
    if ids.is_empty() {
        return "  (nothing)\n".to_string();
    }
    let mut out = String::new();
    for id in ids {
        let Some(task) = engine.task(id) else {
            continue;
        };
        let owner = task
            .assignee
            .and_then(|a| engine.agent(a))
            .map(|spec| format!(" @{}", spec.handle))
            .unwrap_or_else(|| " unassigned".to_string());
        out.push_str(&format!(
            "  {} [{}]{} — {}",
            task.id, task.state.as_str(), owner, task.title
        ));
        if !task.blocked_on.is_empty() {
            out.push_str(&format!(" (blocked on {} thing(s))", task.blocked_on.len()));
        }
        out.push('\n');
    }
    out
}

fn list_tasks(engine: &Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let filter = optional_str(input, "filter").unwrap_or("all");
    let ids: Vec<TaskId> = match filter {
        "mine" => engine
            .tasks()
            .filter(|t| t.assignee == Some(actor))
            .map(|t| t.id)
            .collect(),
        "claimable" => engine.claimable_by(actor),
        "in_review" => engine
            .tasks()
            .filter(|t| t.state == TaskState::InReview)
            .map(|t| t.id)
            .collect(),
        "blocked" => engine
            .tasks()
            .filter(|t| t.state == TaskState::Blocked)
            .map(|t| t.id)
            .collect(),
        _ => engine.tasks().map(|t| t.id).collect(),
    };
    ToolOutcome::ok(format!("Tasks ({filter}):\n{}", render_tasks(engine, ids)))
}

/// Read the `kind` and `of` arguments into a [`TaskOrigin`].
///
/// A supporting record with no subject is refused rather than quietly created
/// as a top-level card, because that silent fallback is exactly how a board
/// ends up with ninety-six peers of one another. The error says what to do.
fn task_origin(input: &Value) -> Result<TaskOrigin, String> {
    let kind = match optional_str(input, "kind").unwrap_or("task") {
        "objective" => TaskKind::Objective,
        "task" => TaskKind::Task,
        "subtask" => TaskKind::Subtask,
        "review" => TaskKind::Review,
        "remediation" => TaskKind::Remediation,
        "evidence" => TaskKind::Evidence,
        "recovery" => TaskKind::Recovery,
        "continuation" => TaskKind::Continuation,
        other => {
            return Err(format!(
                "`{other}` is not a kind of work. Use one of: objective, task, \
                 subtask, review, remediation, evidence, recovery, continuation."
            ))
        }
    };

    let of = match optional_str(input, "of") {
        Some(raw) => Some(parse_id::<TaskId>(raw, "of", "task")?),
        None => None,
    };

    let Some(subject) = of else {
        if matches!(kind, TaskKind::Objective | TaskKind::Task) {
            return Ok(TaskOrigin {
                kind,
                lineage: Lineage::default(),
            });
        }
        return Err(format!(
            "a `{}` has to say what it is about. Pass `of` with the task id it \
             belongs to, or create it as a plain task if it really is a new, \
             independently owned outcome.",
            kind.as_str()
        ));
    };

    let mut lineage = Lineage {
        parent: Some(subject),
        ..Default::default()
    };
    match kind {
        TaskKind::Review => lineage.review_of = Some(subject),
        TaskKind::Remediation => lineage.remediation_of = Some(subject),
        TaskKind::Recovery => lineage.recovery_for = Some(subject),
        TaskKind::Continuation => lineage.continuation_of = Some(subject),
        // A subtask or evidence record hangs beneath its parent without
        // being *about* it in the review sense.
        TaskKind::Subtask | TaskKind::Evidence | TaskKind::Task => {}
        TaskKind::Objective => {
            return Err(
                "an objective is a top-level outcome, so it cannot belong to \
                 another task. Drop `of`, or pick a different kind."
                    .into(),
            )
        }
    }
    Ok(TaskOrigin { kind, lineage })
}

fn create_task(engine: &mut Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let (title, brief) = match (required_str(input, "title"), required_str(input, "brief")) {
        (Ok(title), Ok(brief)) => (title.to_string(), brief.to_string()),
        (Err(message), _) | (_, Err(message)) => return ToolOutcome::error(message),
    };
    let priority = match optional_str(input, "priority").unwrap_or("normal") {
        "low" => Priority::Low,
        "high" => Priority::High,
        "urgent" => Priority::Urgent,
        _ => Priority::Normal,
    };
    let depends_on = match string_list(input, "depends_on")
        .iter()
        .map(|raw| parse_id::<TaskId>(raw, "depends_on", "task"))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(ids) => ids,
        Err(message) => return ToolOutcome::error(message),
    };

    let origin = match task_origin(input) {
        Ok(origin) => origin,
        Err(message) => return ToolOutcome::error(message),
    };

    // Several bots independently noticing the same gap and each raising it is
    // how a board grows fastest, and the records are near-impossible to
    // reconcile afterwards because nothing links them. Say so now, while the
    // caller can still attach to the existing one.
    let existing = engine.work_like(&title);
    if !existing.is_empty() && optional_str(input, "anyway") != Some("true") {
        let named: Vec<String> = existing
            .iter()
            .filter_map(|id| engine.task(*id))
            .map(|task| format!("{} — \"{}\"", task.id, task.title))
            .collect();
        return ToolOutcome::error(format!(
            "This looks like work that already exists:\n{}\n\nUse the existing \
             task — claim it, add a subtask under it, or supersede it if yours \
             replaces it. If it really is separate work, call this again with \
             `anyway: \"true\"` and say in the brief how it differs.",
            named.join("\n")
        ));
    }

    let events = match engine.apply(Command::CreateTask {
        title: title.clone(),
        brief,
        by: Some(actor),
        required_role: optional_str(input, "required_role").map(str::to_string),
        priority,
        acceptance_criteria: string_list(input, "acceptance_criteria"),
        depends_on,
        origin,
    }) {
        Ok(events) => events,
        Err(error) => return explain(error),
    };

    let Some(task) = events.iter().find_map(|e| e.event.task()) else {
        return ToolOutcome::ok("Created.");
    };
    // Publishing immediately is the common case; a draft nobody can claim is
    // rarely what a coordinator means.
    if input.get("open").and_then(Value::as_bool).unwrap_or(true) {
        if let Err(error) = engine.apply(Command::OpenTask { task }) {
            return explain(error);
        }
        return ToolOutcome::ok(format!("Created and opened {task} — {title}"));
    }
    ToolOutcome::ok(format!("Created {task} as a draft — {title}"))
}

fn add_artifact(engine: &mut Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let (task, name, content) = match (
        task_arg(input, "task"),
        required_str(input, "name"),
        required_str(input, "content"),
    ) {
        (Ok(task), Ok(name), Ok(content)) => (task, name.to_string(), content.to_string()),
        (Err(message), _, _) | (_, Err(message), _) | (_, _, Err(message)) => {
            return ToolOutcome::error(message)
        }
    };
    let body = match optional_str(input, "kind").unwrap_or("text") {
        "json" => match serde_json::from_str(&content) {
            Ok(value) => ArtifactBody::Json { value },
            Err(error) => {
                return ToolOutcome::error(format!(
                    "`content` is not valid JSON ({error}). Either fix it or set kind to \"text\"."
                ))
            }
        },
        "patch" => ArtifactBody::Patch { diff: content },
        "url" => ArtifactBody::Url { url: content },
        _ => ArtifactBody::Text { text: content },
    };

    match engine.apply(Command::AddArtifact {
        task,
        author: actor,
        name: name.clone(),
        body,
    }) {
        Ok(events) => {
            let id = events
                .iter()
                .find_map(|e| match &e.event {
                    orgbot_proto::Event::ArtifactAdded { artifact } => Some(artifact.id),
                    _ => None,
                })
                .map(|id| id.to_string())
                .unwrap_or_default();
            ToolOutcome::ok(format!("Recorded `{name}` as {id}."))
        }
        Err(error) => explain(error),
    }
}

fn read_artifact(engine: &Engine, input: &Value) -> ToolOutcome {
    let id = match required_str(input, "artifact").and_then(|raw| parse_id(raw, "artifact", "art")) {
        Ok(id) => id,
        Err(message) => return ToolOutcome::error(message),
    };
    let part = input
        .get("part")
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
        .clamp(1, u64::from(u32::MAX)) as u32;

    match engine.artifact(id) {
        Some(artifact) => {
            let heading = format!(
                "{} ({}), by {}:",
                artifact.name,
                artifact.body.kind(),
                artifact.author.short(),
            );
            // Never a bare preview. A reviewer who reads two thirds of a
            // document with no sign that anything is missing, and approves it,
            // has approved something nobody read.
            ToolOutcome::ok(artifact.body.excerpt(part, PART_CHARS).render(&heading))
        }
        None => ToolOutcome::error(format!("There is no artifact {id}.")),
    }
}

fn submit_for_review(engine: &mut Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let task = match task_arg(input, "task") {
        Ok(task) => task,
        Err(message) => return ToolOutcome::error(message),
    };
    let artifacts = match artifact_ids(input, "artifacts") {
        Ok(ids) => ids,
        Err(message) => return ToolOutcome::error(message),
    };
    match engine.apply(Command::SubmitForReview {
        task,
        by: actor,
        artifacts,
    }) {
        Ok(_) => ToolOutcome::ok(
            "Sent for inspection. Wait for the verdict — you cannot approve this yourself.",
        ),
        Err(error) => explain(error),
    }
}

fn offer_handoff(engine: &mut Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let (task, brief) = match (task_arg(input, "task"), required_str(input, "brief")) {
        (Ok(task), Ok(brief)) => (task, brief.to_string()),
        (Err(message), _) | (_, Err(message)) => return ToolOutcome::error(message),
    };
    let kind = match optional_str(input, "kind").unwrap_or("assist") {
        "delegate" => HandoffKind::Delegate,
        "review" => HandoffKind::Review,
        "escalate" => HandoffKind::Escalate,
        _ => HandoffKind::Assist,
    };

    let to = if let Some(handle) = optional_str(input, "to_agent") {
        match engine
            .agent_by_handle(handle)
            .map(|spec| spec.id)
            .or_else(|| handle.parse().ok())
        {
            Some(agent) => Recipient::Agent { agent },
            None => {
                return ToolOutcome::error(format!(
                    "There is no bot with handle `{handle}`. Call room_status to see who is here."
                ))
            }
        }
    } else if let Some(role) = optional_str(input, "to_role") {
        Recipient::Role {
            role: role.to_string(),
        }
    } else {
        return ToolOutcome::error(
            "Give either `to_agent` (a specific bot) or `to_role` (anyone holding a role).",
        );
    };

    let context = match artifact_ids(input, "context") {
        Ok(ids) => ids,
        Err(message) => return ToolOutcome::error(message),
    };

    match engine.apply(Command::OfferHandoff {
        task,
        from: actor,
        to,
        kind,
        brief,
        expects: string_list(input, "expects"),
        context,
        blocks_sender: None,
        timeout_ms: None,
    }) {
        Ok(events) => {
            let id = events
                .iter()
                .find_map(|e| match &e.event {
                    orgbot_proto::Event::HandoffOffered { handoff } => Some(handoff.id),
                    _ => None,
                })
                .map(|id| id.to_string())
                .unwrap_or_default();
            if kind.blocks_sender_by_default() {
                ToolOutcome::ok(format!(
                    "Offered {id}. Your task is now blocked waiting on it — you will be \
                     woken when it settles, so do not poll."
                ))
            } else {
                ToolOutcome::ok(format!("Offered {id}. You are not blocked on it."))
            }
        }
        Err(error) => explain(error),
    }
}

fn list_handoffs(engine: &Engine, actor: AgentId) -> ToolOutcome {
    let mut out = String::new();

    out.push_str("Addressed to you (accept or reject these):\n");
    let offers = engine.offers_for(actor);
    if offers.is_empty() {
        out.push_str("  (nothing)\n");
    }
    for id in offers {
        let Some(handoff) = engine.handoff(id) else {
            continue;
        };
        let from = engine
            .agent(handoff.from)
            .map(|spec| spec.handle.clone())
            .unwrap_or_else(|| handoff.from.short());
        out.push_str(&format!(
            "  {} from @{} ({:?}): {}\n",
            handoff.id, from, handoff.kind, handoff.brief
        ));
    }

    out.push_str("\nYours, still outstanding:\n");
    let mine: Vec<_> = engine
        .handoffs()
        .filter(|h| h.from == actor && h.state.is_pending())
        .collect();
    if mine.is_empty() {
        out.push_str("  (nothing)\n");
    }
    for handoff in mine {
        out.push_str(&format!(
            "  {} [{}]: {}\n",
            handoff.id,
            handoff.state.as_str(),
            handoff.brief
        ));
    }
    ToolOutcome::ok(out)
}

fn handoff_arg(input: &Value) -> Result<HandoffId, String> {
    parse_id(required_str(input, "handoff")?, "handoff", "ho")
}

fn accept_handoff(engine: &mut Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let handoff = match handoff_arg(input) {
        Ok(handoff) => handoff,
        Err(message) => return ToolOutcome::error(message),
    };
    match engine.apply(Command::AcceptHandoff {
        handoff,
        by: actor,
    }) {
        Ok(_) => {
            let brief = engine
                .handoff(handoff)
                .map(|h| h.brief.clone())
                .unwrap_or_default();
            ToolOutcome::ok(format!(
                "Accepted {handoff}. Someone is waiting on this, so do it now: {brief}\n\
                 Call complete_handoff with that id when you have the answer."
            ))
        }
        Err(error) => explain(error),
    }
}

fn reject_handoff(engine: &mut Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let (handoff, reason) = match (handoff_arg(input), required_str(input, "reason")) {
        (Ok(handoff), Ok(reason)) => (handoff, reason.to_string()),
        (Err(message), _) | (_, Err(message)) => return ToolOutcome::error(message),
    };
    match engine.apply(Command::RejectHandoff {
        handoff,
        by: actor,
        reason,
    }) {
        Ok(_) => ToolOutcome::ok("Declined. The sender has been unblocked."),
        Err(error) => explain(error),
    }
}

fn complete_handoff(engine: &mut Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let (handoff, summary) = match (handoff_arg(input), required_str(input, "summary")) {
        (Ok(handoff), Ok(summary)) => (handoff, summary.to_string()),
        (Err(message), _) | (_, Err(message)) => return ToolOutcome::error(message),
    };
    let artifacts = match artifact_ids(input, "artifacts") {
        Ok(ids) => ids,
        Err(message) => return ToolOutcome::error(message),
    };
    match engine.apply(Command::CompleteHandoff {
        handoff,
        by: actor,
        artifacts,
        summary,
    }) {
        Ok(_) => ToolOutcome::ok("Delivered. The sender has been unblocked."),
        Err(error) => explain(error),
    }
}

fn block_on(engine: &mut Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let task = match task_arg(input, "task") {
        Ok(task) => task,
        Err(message) => return ToolOutcome::error(message),
    };
    let reason = if let Some(raw) = optional_str(input, "waiting_for_task") {
        match parse_id::<TaskId>(raw, "waiting_for_task", "task") {
            Ok(other) => BlockReason::AwaitingTask { task: other },
            Err(message) => return ToolOutcome::error(message),
        }
    } else if let Some(prompt) = optional_str(input, "question_for_human") {
        BlockReason::AwaitingHuman {
            prompt: prompt.to_string(),
        }
    } else {
        return ToolOutcome::error(
            "Say what you are waiting for: `waiting_for_task` or `question_for_human`.",
        );
    };

    match engine.apply(Command::BlockTask {
        task,
        by: actor,
        reason,
    }) {
        Ok(events) => {
            // A circular wait is resolved on the spot; tell the bot, because it
            // changes what it should do next.
            let broke_a_deadlock = events.iter().any(|e| {
                matches!(e.event, orgbot_proto::Event::DeadlockDetected { .. })
            });
            if broke_a_deadlock {
                ToolOutcome::ok(
                    "Blocked — but this created a circular wait, which the room has just \
                     broken. Check list_handoffs and room_status: something you were \
                     waiting on has changed.",
                )
            } else {
                ToolOutcome::ok("Blocked. You will be woken when it clears.")
            }
        }
        Err(error) => explain(error),
    }
}

fn release_task(engine: &mut Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let (task, reason) = match (task_arg(input, "task"), required_str(input, "reason")) {
        (Ok(task), Ok(reason)) => (task, reason.to_string()),
        (Err(message), _) | (_, Err(message)) => return ToolOutcome::error(message),
    };
    match engine.apply(Command::ReleaseTask {
        task,
        by: actor,
        reason,
    }) {
        Ok(_) => ToolOutcome::ok("Handed back to the pool."),
        Err(error) => explain(error),
    }
}

fn record_review(engine: &mut Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let (task, summary) = match (task_arg(input, "task"), required_str(input, "summary")) {
        (Ok(task), Ok(summary)) => (task, summary.to_string()),
        (Err(message), _) | (_, Err(message)) => return ToolOutcome::error(message),
    };

    let mut findings = Vec::new();
    for raw in input
        .get("findings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let severity = match raw.get("severity").and_then(Value::as_str).unwrap_or("note") {
            "minor" => Severity::Minor,
            "major" => Severity::Major,
            "critical" => Severity::Critical,
            _ => Severity::Note,
        };
        let Some(summary) = raw.get("summary").and_then(Value::as_str) else {
            return ToolOutcome::error("Every finding needs a `summary` saying what is wrong.");
        };
        findings.push(Finding {
            severity,
            summary: summary.to_string(),
            locus: raw.get("locus").and_then(Value::as_str).map(str::to_string),
            suggestion: raw
                .get("suggestion")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }

    match engine.apply(Command::RecordReview {
        task,
        inspector: actor,
        findings,
        summary,
        criteria_met: string_list(input, "criteria_met"),
        criteria_failed: string_list(input, "criteria_failed"),
    }) {
        Ok(_) => {
            let state = engine
                .task(task)
                .map(|t| t.state.as_str())
                .unwrap_or("unknown");
            ToolOutcome::ok(format!(
                "Review recorded. Your findings put the task in `{state}`."
            ))
        }
        Err(error) => explain(error),
    }
}
