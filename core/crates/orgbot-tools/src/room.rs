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
use orgbot_proto::agent::AgentSpec;
use orgbot_proto::artifact::{ArtifactBody, PART_CHARS};
use orgbot_proto::graph::{Lineage, TaskKind, TaskOrigin};
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
             the bots you address are woken to respond. Use `@handle`, a unique `@display_name`, `to_agents`, or \
             `to_roles` to direct attention. Set `reply_to` to thread a reply; when no \
             recipient is explicit, it addresses the original author. Set `intent` to \
             \"ask\" so they know a reply is expected. For work that another bot needs \
             to perform, use offer_handoff.",
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
                    (
                        "reply_to",
                        schema::string(
                            "Message id this replies to; replies without explicit recipients address its author.",
                        ),
                    ),
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
                    (
                        "exact",
                        schema::boolean(
                            "Set true when the content carries authority that cannot \
                             be re-derived — keys, signatures, schemas, protocol \
                             constants, wire values. Nothing will then try to \
                             reconstruct it from memory, and only you can re-emit it.",
                        ),
                    ),
                ],
                &["task", "name", "content"],
            ),
        ),
        tool(
            "find_prior_work",
            "Search what earlier rooms already built, before building it again.              Call this at the start of a task: if the work exists, reuse it rather              than starting from scratch. Returns titles, which room, whether it              was delivered, and the deliverables it produced.",
            schema::object(
                vec![(
                    "about",
                    schema::string("What the work is about, in your own words."),
                )],
                &["about"],
            ),
        ),
        tool(
            "read_prior_artifact",
            "Read one deliverable an earlier room produced, in full. Get its id              from find_prior_work.",
            schema::object(
                vec![("artifact", schema::string("The artifact id."))],
                &["artifact"],
            ),
        ),
        tool(
            "search_project_memory",
            "Search durable notes shared by rooms in this explicitly configured project. This is a local semantic search and makes no model call. Use it when prior decisions or known constraints may affect the task.",
            schema::object(vec![("query", schema::string("What you need to recall, phrased naturally."))], &["query"]),
        ),
        tool(
            "save_project_memory",
            "Save a concise project decision or reusable outcome for later rooms. Keep it factual and scoped to this project; never store credentials or private user data.",
            schema::object(vec![
                ("title", schema::string("Short subject for the note.")),
                ("content", schema::string("Reusable decision, constraint, or result.")),
            ], &["title", "content"]),
        ),
        tool(
            "use_skill",
            "Load a playbook by name. The room lists what it has near the top of \
             your instructions, with one line each; this returns the whole thing. \
             Reach for one when its line matches the work in front of you — that \
             is what they are for, and following a proven approach beats working \
             one out again.",
            schema::object(
                vec![("skill", schema::string("The skill's name."))],
                &["skill"],
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
                        schema::integer(
                            "Which part to read, from 1. Defaults to the first. \
                             Asking past the last part is an error, not the last part.",
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
             - \"delegate\": they take ownership of the task outright when they accept. \
             You are not blocked, but keep holding the task until then: releasing it \
             withdraws the offer.\n\
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
            "fail_handoff",
            "Give up a handoff you accepted but cannot finish, saying why, so the sender \
             is released instead of waiting for it to run out.",
            schema::object(
                vec![
                    ("handoff", schema::string("The handoff id.")),
                    ("reason", schema::string("Why you cannot finish it.")),
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
            "ask_owner",
            "Ask the owner for information or a choice when the room cannot safely proceed. \
             Offer 2–6 distinct options; the owner can also type a custom answer. Include a task \
             only when you own active work that must wait. An answer supplies information; it does \
             not bypass tool, filesystem, or other configured permissions.",
            schema::object(
                vec![
                    ("question", schema::string("The concise question to show the owner.")),
                    ("options", schema::string_array("2–6 distinct suggested answers.")),
                    ("task", schema::string("Your active task to block while waiting, if any.")),
                ],
                &["question", "options"],
            ),
        ),
        tool(
            "request_server_access",
            "Ask the owner to let you use the bot server key (SSH) for a task you hold, for \
             example to deploy, read server logs or restart a service. Your sandbox has no \
             server key otherwise. Your task waits for the answer. Once allowed, the key is \
             in ~/.ssh only while you work that task: not while it is blocked, waiting on \
             unfinished work, on hold, in review or done.",
            schema::object(
                vec![
                    ("task", schema::string("Your task that needs a server.")),
                    ("reason", schema::string("What you will do on which server, in one or two sentences.")),
                ],
                &["task", "reason"],
            ),
        ),
        tool(
            "request_access",
            "Ask to use what the owner lends for a task you hold: \"server\" for the bot \
             server key (SSH), or one of the owner's sign-ins listed in your situation, such \
             as Wrangler for Cloudflare. Depending on the room's access mode and how risky the \
             request is, it is approved at once or waits for the owner, and your task waits \
             with it. Once allowed, it is in your sandbox only while you work that task: not \
             while it is blocked, waiting on unfinished work, on hold, in review or done.",
            schema::object(
                vec![
                    ("task", schema::string("Your task that needs it.")),
                    ("items", schema::string_array("What to use: \"server\" and/or names of lent sign-ins.")),
                    (
                        "action",
                        schema::enumerated(
                            "What you will do: inspect (read or check), change (build, test or easily undone changes), release (production or a live service), destroy (delete data or change credentials), spend (money). Say it honestly; it decides who approves.",
                            &["inspect", "change", "release", "destroy", "spend"],
                        ),
                    ),
                    ("reason", schema::string("What you will do with it, in one or two sentences.")),
                ],
                &["task", "items", "action", "reason"],
            ),
        ),
        tool(
            "propose_retire_task",
            "Coordinators: propose to the owner that a task nobody should work on be \
             retired, for example one a later owner decision or another task superseded. \
             The owner answers with one tap; until then the task stays as it is. Use this \
             instead of asking the owner to archive a task in the app, which has no such \
             button.",
            schema::object(
                vec![
                    ("task", schema::string("The task to retire.")),
                    ("reason", schema::string("Why it is obsolete, in one or two sentences.")),
                ],
                &["task", "reason"],
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
                        schema::string("A question for the owner. It creates a durable decision and releases your task when answered."),
                    ),
                    ("options", schema::string_array("2–6 distinct suggested answers for question_for_human; omitted means free-form.")),
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
                    (
                        "brief",
                        schema::string("The full instruction for whoever takes it."),
                    ),
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
                    (
                        "open",
                        schema::boolean("Publish immediately. Defaults to true."),
                    ),
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
                                    (
                                        "locus",
                                        schema::string("Where — an artifact or a criterion."),
                                    ),
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

    if !spec.roles.iter().any(|role| role == "coordinator") {
        tools.retain(|tool| tool.name != "propose_retire_task");
    }
    if spec.autonomy.review_only {
        tools.retain(|tool| review_only_tool_allowed(spec, &tool.name));
    }
    tools
}

fn review_only_tool_allowed(spec: &AgentSpec, name: &str) -> bool {
    let coordinates = spec.roles.iter().any(|role| role == "coordinator");
    (coordinates
        && match name {
            "create_task" => spec.autonomy.may_create_tasks,
            "submit_task" => spec.autonomy.may_submit,
            "propose_retire_task" => true,
            _ => false,
        })
        || matches!(
            name,
            "post_message"
                | "room_status"
                | "list_tasks"
                | "claim_task"
                | "start_task"
                | "find_prior_work"
                | "read_prior_artifact"
                | "search_project_memory"
                | "use_skill"
                | "read_artifact"
                | "release_task"
                | "record_review"
                | "add_artifact"
                | "submit_for_review"
        )
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
    if engine
        .agent(actor)
        .is_some_and(|spec| spec.autonomy.review_only && !review_only_tool_allowed(spec, name))
    {
        return ToolOutcome::error(format!(
            "Review-only bots cannot use `{name}`. Use the tools allowed by your review and coordination permissions."
        ));
    }
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
        "use_skill" => use_skill(engine, input),
        "find_prior_work" => find_prior_work(engine, input),
        "read_prior_artifact" => read_prior_artifact(engine, input),
        "search_project_memory" => search_project_memory(engine, input),
        "save_project_memory" => save_project_memory(engine, input),
        "read_artifact" => read_artifact(engine, input),
        "submit_for_review" => submit_for_review(engine, actor, input),
        "offer_handoff" => offer_handoff(engine, actor, input),
        "list_handoffs" => list_handoffs(engine, actor),
        "accept_handoff" => accept_handoff(engine, actor, input),
        "reject_handoff" => reject_handoff(engine, actor, input),
        "complete_handoff" => complete_handoff(engine, actor, input),
        "ask_owner" => ask_owner(engine, actor, input),
        "request_server_access" => request_server_access(engine, actor, input),
        "request_access" => request_access(engine, actor, input),
        "fail_handoff" => fail_handoff(engine, actor, input),
        "propose_retire_task" => propose_retire_task(engine, actor, input),
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

fn optional_string_list(input: &Value, field: &str) -> Result<Vec<String>, String> {
    let Some(value) = input.get(field) else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        return Err(format!("`{field}` must be an array of strings."));
    };
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("Every `{field}` item must be a string."))
        })
        .collect()
}

/// Parse an id, explaining the expected shape on failure.
fn parse_id<T: std::str::FromStr>(raw: &str, field: &str, prefix: &str) -> Result<T, String> {
    raw.parse().map_err(|_| {
        format!(
            "`{field}` is not a valid id. Expected something like `{prefix}_1a2b3c…`, got `{raw}`."
        )
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

    let parent_author = match optional_str(input, "reply_to") {
        Some(raw) => {
            let reply_to = match parse_id::<orgbot_proto::MessageId>(raw, "reply_to", "message") {
                Ok(id) => id,
                Err(message) => return ToolOutcome::error(message),
            };
            let Some(parent) = engine
                .transcript()
                .iter()
                .find(|message| message.id == reply_to)
            else {
                return ToolOutcome::error("There is no message with that id in this room.");
            };
            message.reply_to = Some(reply_to);
            Some(parent.author.clone())
        }
        None => None,
    };

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
                    ));
                }
            }
        }
        message.audience = Audience::Agents { agents: targets };
    } else if !roles.is_empty() {
        message.audience = Audience::Roles { roles };
    } else {
        let mentions = message_handles(&message.body);
        if let Some(mentions) = mentions {
            if mentions.is_empty() {
                message.audience = Audience::Everyone;
            } else {
                let mut targets = Vec::new();
                for handle in mentions {
                    let agent = match engine.agent_by_handle(&handle) {
                        Some(agent) => agent,
                        None => match resolve_display_name(engine, &handle) {
                            Ok(Some(agent)) => agent,
                            Ok(None) => {
                                return ToolOutcome::error(format!(
                                    "There is no bot named `{handle}`. Call room_status to see who is here."
                                ));
                            }
                            Err(()) => {
                                return ToolOutcome::error(format!(
                                    "The display name `{handle}` is ambiguous. Use the bot's @handle from room_status."
                                ));
                            }
                        },
                    };
                    targets.push(agent.id);
                }
                message.audience = Audience::Agents { agents: targets };
            }
        } else if let Some(author) = parent_author {
            message.audience = match author {
                orgbot_proto::Author::Agent { agent } => Audience::Agents {
                    agents: vec![agent],
                },
                orgbot_proto::Author::Human { .. } | orgbot_proto::Author::System => {
                    Audience::Human
                }
            };
        }
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

fn resolve_display_name<'a>(
    engine: &'a Engine,
    name: &str,
) -> Result<Option<&'a orgbot_proto::AgentSpec>, ()> {
    let mut matches = engine
        .agents()
        .filter(|agent| agent.display_name.eq_ignore_ascii_case(name));
    let first = matches.next();
    if matches.next().is_some() {
        Err(())
    } else {
        Ok(first)
    }
}

fn message_handles(body: &str) -> Option<Vec<String>> {
    let mut handles = Vec::new();
    let mut found = false;
    for word in body.split(|c: char| !(c.is_alphanumeric() || matches!(c, '@' | '_' | '-'))) {
        let Some(handle) = word.strip_prefix('@').filter(|handle| !handle.is_empty()) else {
            continue;
        };
        found = true;
        if orgbot_proto::is_broadcast_tag(handle) {
            return Some(Vec::new());
        }
        if !handles
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(handle))
        {
            handles.push(handle.to_string());
        }
    }
    found.then_some(handles)
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
    // The board, compacted the same way it is for a person. A room with
    // ninety-six records is as unreadable to a model as to anyone else, and
    // pasting all of them costs a turn's worth of context to say very little.
    let view = engine.digest();
    out.push_str(&format!("\n{}\n", view.summary("Board")));

    if view.health.coordination_expansion {
        out.push_str(&format!(
            "\nCoordination expansion: {:.1} records per objective, {} of them \
             reviews and follow-ups. Attach supporting work to the objective it \
             serves rather than raising it top-level.\n",
            view.health.inflation_ratio, view.health.supporting_records,
        ));
    }
    if !view.critical_path.is_empty() {
        let chain: Vec<String> = view
            .critical_path
            .iter()
            .filter_map(|id| engine.task(*id))
            .map(|task| task.title.clone())
            .collect();
        out.push_str(&format!(
            "\nThe chain deciding when this room finishes: {}\n",
            chain.join(" → ")
        ));
    }

    // Then only what this bot can act on. Everything else is one call away.
    let mine = engine.active_tasks_of(actor);
    if !mine.is_empty() {
        out.push_str("\nYours:\n");
        out.push_str(&render_tasks(engine, mine));
    }
    let claimable = engine.claimable_by(actor);
    if !claimable.is_empty() {
        out.push_str("\nYou could take:\n");
        out.push_str(&render_tasks(
            engine,
            claimable.into_iter().take(10).collect(),
        ));
    }
    out.push_str("\nThat is the board compacted. Call list_tasks for the full set.\n");
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
            task.id,
            task.state.as_str(),
            owner,
            task.title
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
            ));
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
            );
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
            return ToolOutcome::error(message);
        }
    };
    let body = match optional_str(input, "kind").unwrap_or("text") {
        "json" => match serde_json::from_str(&content) {
            Ok(value) => ArtifactBody::Json { value },
            Err(error) => {
                return ToolOutcome::error(format!(
                    "`content` is not valid JSON ({error}). Either fix it or set kind to \"text\"."
                ));
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
        authority_bearing: input
            .get("exact")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
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

/// Which part the caller asked for.
///
/// The schema said `part` was a string and this parser only accepted a JSON
/// number, so a model following the schema sent `"2"`, `as_u64()` gave `None`,
/// and the fallback quietly served part 1. Every request for a later part
/// returned the first one — with the document's own header still reading "PART
/// 1/2" — which made the tail of every multi-part artifact unreachable and
/// looked, from the outside, like the document simply ended there.
///
/// So: take a number or a string of digits, and refuse anything else out
/// loud. What must never happen again is a supplied argument being ignored.
fn requested_part(input: &Value) -> Result<u32, String> {
    let Some(raw) = input.get("part") else {
        return Ok(1);
    };
    if raw.is_null() {
        return Ok(1);
    }
    let parsed = match raw {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    };
    match parsed {
        Some(part) if part >= 1 && part <= u64::from(u32::MAX) => Ok(part as u32),
        _ => Err(format!(
            "PART_INVALID requested={raw} — `part` must be a whole number from 1. \
             The part you asked for was not read."
        )),
    }
}

/// What earlier rooms already built on this subject.
///
/// The point is reuse. A room that cannot see what came before rebuilds it,
/// re-argues the decision, and earns the same reviewer finding a second time.
/// Summaries only — a body is fetched by the bot that decided it wanted one.
fn find_prior_work(engine: &Engine, input: &Value) -> ToolOutcome {
    let query = match required_str(input, "about") {
        Ok(query) => query,
        Err(message) => return ToolOutcome::error(message),
    };
    let hits = engine.archive().search(query);
    if hits.is_empty() {
        return ToolOutcome::ok(
            "No earlier room did anything like this. It is new work; carry on.".to_string(),
        );
    }
    let mut out = format!(
        "{} earlier piece(s) of work look related. Read one before rebuilding it:\n",
        hits.len()
    );
    for hit in hits {
        out.push_str(&format!(
            "\n  {} — in room `{}`{}\n    {}\n",
            hit.task.title,
            hit.room,
            if hit.task.delivered {
                ", delivered"
            } else {
                ", attempted but not delivered"
            },
            hit.task.brief.chars().take(200).collect::<String>(),
        ));
        for artifact in &hit.task.artifacts {
            out.push_str(&format!(
                "    produced {} ({}) — read_prior_artifact to see it\n",
                artifact.name, artifact.id
            ));
        }
    }
    ToolOutcome::ok(out)
}

/// One deliverable an earlier room produced, in full.
fn read_prior_artifact(engine: &Engine, input: &Value) -> ToolOutcome {
    let id = match required_str(input, "artifact") {
        Ok(id) => id,
        Err(message) => return ToolOutcome::error(message),
    };
    match engine.archive().artifact(id) {
        Some((name, body)) if !body.is_empty() => ToolOutcome::ok(format!(
            "{name} (from an earlier room)\nBEGIN\n{body}\nEND\n\nThis is prior \
             work, not this room's. Reuse it, adapt it, or say why it does not fit."
        )),
        Some((name, _)) => ToolOutcome::error(format!(
            "`{name}` has no readable text; it may be a file reference rather than a document."
        )),
        None => ToolOutcome::error(format!(
            "No earlier room holds artifact `{id}`. Use find_prior_work to get an id."
        )),
    }
}

fn search_project_memory(engine: &Engine, input: &Value) -> ToolOutcome {
    let query = match required_str(input, "query") {
        Ok(query) => query,
        Err(message) => return ToolOutcome::error(message),
    };
    if !engine.archive().has_memory() {
        return ToolOutcome::error(
            "Project memory is unavailable because this room has no explicitly configured project scope.",
        );
    }
    match engine.archive().search_memory(query, 5) {
        Ok(hits) if hits.is_empty() => ToolOutcome::ok(
            "No related shared project notes were found. This project has no matching memory yet; proceed from the current room evidence.",
        ),
        Ok(hits) => {
            let mut out = String::from("Related project notes, most relevant first:\n");
            for hit in hits {
                out.push_str(&format!(
                    "\n{} (relevance {:.2})\n{}\n",
                    hit.entry.title, hit.score, hit.entry.content
                ));
            }
            ToolOutcome::ok(out)
        }
        Err(error) => ToolOutcome::error(format!("Project memory could not be read: {error}")),
    }
}

fn save_project_memory(engine: &Engine, input: &Value) -> ToolOutcome {
    let (title, content) = match (required_str(input, "title"), required_str(input, "content")) {
        (Ok(title), Ok(content)) => (title, content),
        (Err(message), _) | (_, Err(message)) => return ToolOutcome::error(message),
    };
    match engine.archive().remember(title, content, None, None) {
        Ok(entry) => ToolOutcome::ok(format!("Saved project note `{}`.", entry.id)),
        Err(error) => ToolOutcome::error(format!("Project memory could not be saved: {error}")),
    }
}

/// Hand over one playbook in full.
///
/// The index a bot carries is one line per skill; this is the rest. Kept as a
/// call rather than folded into the prompt so a room of twenty skills costs a
/// few hundred tokens to advertise and nothing until one is actually used.
fn use_skill(engine: &Engine, input: &Value) -> ToolOutcome {
    let name = match required_str(input, "skill") {
        Ok(name) => name,
        Err(message) => return ToolOutcome::error(message),
    };
    match engine.skill(name) {
        Some(skill) => ToolOutcome::ok(format!(
            "SKILL {}\n{}\n\nFollow this for the work in front of you. If it does \
             not fit, say so rather than forcing it.",
            skill.name, skill.body
        )),
        None => {
            let known: Vec<&str> = engine.skills().iter().map(|s| s.name.as_str()).collect();
            ToolOutcome::error(if known.is_empty() {
                "This room has no skills loaded.".to_string()
            } else {
                format!(
                    "There is no skill called `{name}`. This room has: {}.",
                    known.join(", ")
                )
            })
        }
    }
}

fn read_artifact(engine: &Engine, input: &Value) -> ToolOutcome {
    let id = match required_str(input, "artifact").and_then(|raw| parse_id(raw, "artifact", "art"))
    {
        Ok(id) => id,
        Err(message) => return ToolOutcome::error(message),
    };
    let part = match requested_part(input) {
        Ok(part) => part,
        Err(message) => return ToolOutcome::error(message),
    };

    let Some(artifact) = engine.artifact(id) else {
        return ToolOutcome::error(format!("There is no artifact {id}."));
    };
    let heading = format!(
        "{} ({}), by {}:",
        artifact.name,
        artifact.body.kind(),
        artifact.author.short(),
    );
    // Never a bare preview. A reviewer who reads two thirds of a document with
    // no sign that anything is missing, and approves it, has approved
    // something nobody read.
    match artifact.body.excerpt(part, PART_CHARS) {
        Some(excerpt) => {
            let mut out = excerpt.render(&heading);
            // The manifest on the first read, so a reader knows up front what
            // the complete set is and can check each part off against a digest
            // the room produced rather than one the document claims.
            if !excerpt.is_whole() && excerpt.part == 1 {
                out.push_str("\n\n");
                out.push_str(&artifact.manifest(PART_CHARS).render());
            }
            ToolOutcome::ok(out)
        }
        None => ToolOutcome::error(format!(
            "PART_OUT_OF_RANGE requested={part} total_parts={} artifact={id} — \
             there is no such part. Nothing was returned; do not treat any \
             earlier part as the end of the document.",
            artifact.body.parts(PART_CHARS)
        )),
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
                ));
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
    match engine.apply(Command::AcceptHandoff { handoff, by: actor }) {
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

fn fail_handoff(engine: &mut Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let (handoff, reason) = match (handoff_arg(input), required_str(input, "reason")) {
        (Ok(handoff), Ok(reason)) => (handoff, reason.to_string()),
        (Err(message), _) | (_, Err(message)) => return ToolOutcome::error(message),
    };
    match engine.apply(Command::FailHandoff {
        handoff,
        by: actor,
        reason,
    }) {
        Ok(_) => ToolOutcome::ok("Given up. The sender has been released."),
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

fn ask_owner(engine: &mut Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let question = match required_str(input, "question") {
        Ok(value) => value.to_string(),
        Err(message) => return ToolOutcome::error(message),
    };
    let options = match optional_string_list(input, "options") {
        Ok(options) => options,
        Err(message) => return ToolOutcome::error(message),
    };
    if !(2..=6).contains(&options.len()) {
        return ToolOutcome::error("Offer between 2 and 6 distinct options. The owner can still type a custom answer.");
    }
    let task = match input.get("task") {
        Some(Value::Null) | None => None,
        Some(_) => match task_arg(input, "task") {
            Ok(task) => Some(task),
            Err(message) => return ToolOutcome::error(message),
        },
    };
    match engine.apply(Command::AskOwner {
        by: actor,
        task,
        question,
        options,
    }) {
        Ok(events) => {
            let decision = events.iter().find_map(|event| match &event.event {
                orgbot_proto::Event::DecisionAsked { decision } => Some(decision.id),
                _ => None,
            });
            match decision {
                Some(id) => ToolOutcome::ok(format!(
                    "Owner question {id} is waiting for an answer. Continue with other work."
                )),
                None => events
                    .iter()
                    .find_map(|event| match &event.event {
                        orgbot_proto::Event::MessagePosted { message } => {
                            Some(ToolOutcome::ok(message.body.clone()))
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| {
                        ToolOutcome::ok(
                            "That question is already waiting for an answer. Continue with other work.",
                        )
                    }),
            }
        }
        Err(error) => explain(error),
    }
}

fn propose_retire_task(engine: &mut Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let task = match task_arg(input, "task") {
        Ok(task) => task,
        Err(message) => return ToolOutcome::error(message),
    };
    let reason = match required_str(input, "reason") {
        Ok(reason) => reason.to_string(),
        Err(message) => return ToolOutcome::error(message),
    };
    match engine.apply(Command::ProposeRetire {
        by: actor,
        task,
        reason,
    }) {
        Ok(events) => {
            for event in &events {
                match &event.event {
                    orgbot_proto::Event::DecisionAsked { decision } => {
                        return ToolOutcome::ok(format!(
                            "Retirement proposal {} is waiting for the owner. Leave the task \
                             alone until then.",
                            decision.id
                        ));
                    }
                    orgbot_proto::Event::MessagePosted { message } => {
                        return ToolOutcome::ok(message.body.clone());
                    }
                    _ => {}
                }
            }
            ToolOutcome::ok("That proposal is already waiting for the owner.")
        }
        Err(error) => explain(error),
    }
}

fn request_server_access(engine: &mut Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let task = match task_arg(input, "task") {
        Ok(task) => task,
        Err(message) => return ToolOutcome::error(message),
    };
    let reason = match required_str(input, "reason") {
        Ok(reason) => reason.to_string(),
        Err(message) => return ToolOutcome::error(message),
    };
    let outcome = engine.apply(Command::RequestServerAccess {
        by: actor,
        task,
        reason,
    });
    access_outcome(engine, actor, outcome)
}

fn request_access(engine: &mut Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let task = match task_arg(input, "task") {
        Ok(task) => task,
        Err(message) => return ToolOutcome::error(message),
    };
    let reason = match required_str(input, "reason") {
        Ok(reason) => reason.to_string(),
        Err(message) => return ToolOutcome::error(message),
    };
    let items = string_list(input, "items");
    let action = match required_str(input, "action")
        .ok()
        .and_then(orgbot_proto::decision::AccessAction::parse)
    {
        Some(action) => action,
        None => {
            return ToolOutcome::error(
                "action must be one of inspect, change, release, destroy, spend",
            )
        }
    };
    let outcome = engine.apply(Command::RequestAccess {
        by: actor,
        task,
        items,
        action,
        reason,
    });
    access_outcome(engine, actor, outcome)
}

/// What a bot is told after asking for access, approved on the spot or not.
fn access_outcome(
    engine: &Engine,
    actor: AgentId,
    outcome: Result<Vec<orgbot_proto::EventEnvelope>, orgbot_engine::EngineError>,
) -> ToolOutcome {
    let events = match outcome {
        Ok(events) => events,
        Err(error) => return explain(error),
    };
    let approved = events.iter().find_map(|event| match &event.event {
        orgbot_proto::Event::DecisionAnswered { answer, .. } => Some(answer.note.clone()),
        _ => None,
    });
    if let Some(note) = approved {
        return ToolOutcome::ok(format!(
            "{note} It is in your sandbox from your next command or turn, while the task is under way."
        ));
    }
    for event in &events {
        match &event.event {
            orgbot_proto::Event::DecisionAsked { decision } => {
                return ToolOutcome::ok(format!(
                    "Access request {} is waiting for the owner. Your task waits for the \
                     answer; continue with other work.",
                    decision.id
                ));
            }
            orgbot_proto::Event::MessagePosted { message } => {
                return ToolOutcome::ok(message.body.clone());
            }
            _ => {}
        }
    }
    if events.is_empty() && (engine.server_access_for(actor) || !engine.lent_to(actor).is_empty()) {
        ToolOutcome::ok("You already have that for this task.")
    } else {
        ToolOutcome::ok(
            "That request is already waiting for the owner, or the owner has allowed it \
             and the task is not under way yet. Continue with other work.",
        )
    }
}

fn block_on(engine: &mut Engine, actor: AgentId, input: &Value) -> ToolOutcome {
    let task = match task_arg(input, "task") {
        Ok(task) => task,
        Err(message) => return ToolOutcome::error(message),
    };
    let waiting_for_task = optional_str(input, "waiting_for_task");
    let question_for_human = optional_str(input, "question_for_human");
    if waiting_for_task.is_some() && question_for_human.is_some() {
        return ToolOutcome::error("Choose one blocker: `waiting_for_task` or `question_for_human`, not both.");
    }
    if let Some(prompt) = question_for_human {
        let options = match optional_string_list(input, "options") {
            Ok(options) => options,
            Err(message) => return ToolOutcome::error(message),
        };
        return match engine.apply(Command::AskOwner {
            by: actor,
            task: Some(task),
            question: prompt.to_string(),
            options,
        }) {
            Ok(events) => {
                if let Some(reply) = events.iter().find_map(|event| match &event.event {
                    orgbot_proto::Event::MessagePosted { message } => Some(message.body.clone()),
                    _ => None,
                }) {
                    ToolOutcome::ok(reply)
                } else {
                    ToolOutcome::ok("Blocked on the owner’s answer. The task will resume when the answer arrives.")
                }
            }
            Err(error) => explain(error),
        };
    }
    let reason = if let Some(raw) = waiting_for_task {
        match parse_id::<TaskId>(raw, "waiting_for_task", "task") {
            Ok(other) => BlockReason::AwaitingTask { task: other },
            Err(message) => return ToolOutcome::error(message),
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
            let broke_a_deadlock = events
                .iter()
                .any(|e| matches!(e.event, orgbot_proto::Event::DeadlockDetected { .. }));
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
        let severity = match raw
            .get("severity")
            .and_then(Value::as_str)
            .unwrap_or("note")
        {
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
