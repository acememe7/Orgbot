use std::collections::{HashMap, HashSet};

use orgbot_proto::task::TaskState;
use orgbot_proto::{AgentId, TaskId};
use orgbot_providers::{ContentBlock, ToolDef, Turn};
use orgbot_tools::room::ToolOutcome;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{SharedEngine, mcp_client::McpClient};

const PAGE_SIZE: usize = 10;
const TOOL_NAMES: &[&str] = &[
    "find_capabilities",
    "use_skill",
    "load_mcp_tool",
    "call_mcp_tool",
    "unload_capability",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Skill,
    Mcp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Lifetime {
    Once,
    Task,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Lease {
    kind: Kind,
    name: String,
    #[serde(default)]
    server: Option<String>,
    lifetime: Lifetime,
    #[serde(default)]
    task_ids: Vec<TaskId>,
    #[serde(default)]
    turn_scoped: bool,
    #[serde(default)]
    consumed: bool,
    #[serde(default)]
    expired: bool,
}

/// Capabilities explicitly loaded by one bot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Capabilities {
    #[serde(default)]
    leases: Vec<Lease>,
    #[serde(default)]
    pending_once_skills: Vec<String>,
    #[serde(default)]
    pending_discovery: bool,
}

pub(crate) fn handles(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

pub(crate) fn definitions(review_only: bool) -> Vec<ToolDef> {
    let mut definitions = vec![
        tool(
            "find_capabilities",
            "Find a skill by name or topic. For MCP, first list configured server names, then inspect one selected server's tools without loading schemas.",
            json!({"type":"object","properties":{
                "kind":{"type":"string","enum":["skill","mcp"]},
                "query":{"type":"string"},"server":{"type":"string"},
                "offset":{"type":"integer","minimum":0}
            },"required":["kind"],"additionalProperties":false}),
        ),
        tool(
            "use_skill",
            "Load the full instructions for one relevant skill. It is available for one successful response or for the current task.",
            json!({"type":"object","properties":{
                "skill":{"type":"string"},
                "lifetime":{"type":"string","enum":["once","task"],"default":"task"}
            },"required":["skill"],"additionalProperties":false}),
        ),
    ];
    definitions.push(tool(
        "unload_capability",
        "Release a loaded skill or MCP tool before its scope ends.",
        json!({"type":"object","properties":{
            "kind":{"type":"string","enum":if review_only { vec!["skill"] } else { vec!["skill","mcp"] }},"name":{"type":"string"}
        },"required":["kind","name"],"additionalProperties":false}),
    ));
    if !review_only {
        definitions.extend([
            tool(
                "load_mcp_tool",
                "Load one selected MCP tool's schema before calling it. Connections are scoped to you and the selected server.",
                json!({"type":"object","properties":{
                    "server":{"type":"string"},"tool":{"type":"string"},
                    "lifetime":{"type":"string","enum":["once","task"],"default":"task"}
                },"required":["server","tool"],"additionalProperties":false}),
            ),
            tool(
                "call_mcp_tool",
                "Call an MCP tool you loaded. Owner approval may be required; a one-shot lease is spent after the attempt.",
                json!({"type":"object","properties":{
                    "tool":{"type":"string"},"arguments":{"type":"object"}
                },"required":["tool","arguments"],"additionalProperties":false}),
            ),
        ]);
    }
    definitions
}

fn tool(name: &str, description: &str, input_schema: Value) -> ToolDef {
    ToolDef {
        name: name.into(),
        description: description.into(),
        input_schema,
    }
}

impl Capabilities {
    pub(crate) async fn dispatch(
        &mut self,
        engine: &SharedEngine,
        actor: AgentId,
        review_only: bool,
        servers: Option<&McpClient>,
        name: &str,
        input: &Value,
    ) -> ToolOutcome {
        self.refresh(engine, actor, servers).await;
        let Some(args) = input.as_object() else {
            return ToolOutcome::error("Capability arguments must be an object.");
        };
        match name {
            "find_capabilities" => {
                if review_only && args.get("kind").and_then(Value::as_str) == Some("mcp") {
                    ToolOutcome::error("MCP capabilities are unavailable to review-only agents.")
                } else {
                    let result = self.find(engine, actor, servers, args).await;
                    if !result.is_error {
                        self.pending_discovery = true;
                    }
                    result
                }
            }
            "use_skill" => self.use_skill(engine, actor, args),
            "load_mcp_tool" | "call_mcp_tool" if review_only => {
                ToolOutcome::error("MCP capabilities are unavailable to review-only agents.")
            }
            "load_mcp_tool" => self.load_mcp(engine, actor, servers, args).await,
            "call_mcp_tool" => self.call_mcp(engine, actor, servers, args).await,
            "unload_capability"
                if review_only && args.get("kind").and_then(Value::as_str) == Some("mcp") =>
            {
                ToolOutcome::error("MCP capabilities are unavailable to review-only agents.")
            }
            "unload_capability" => self.unload(actor, servers, args).await,
            _ => ToolOutcome::error("Unknown capability operation."),
        }
    }

    fn active_tasks(engine: &SharedEngine, actor: AgentId) -> Vec<TaskId> {
        engine.with(|engine| {
            engine
                .tasks()
                .filter(|task| {
                    task.assignee == Some(actor)
                        && matches!(
                            task.state,
                            TaskState::Claimed | TaskState::InProgress | TaskState::Blocked
                        )
                })
                .map(|task| task.id)
                .collect()
        })
    }

    fn scope(
        &self,
        engine: &SharedEngine,
        actor: AgentId,
        lifetime: Lifetime,
    ) -> (Vec<TaskId>, bool) {
        if lifetime == Lifetime::Once {
            return (Vec::new(), false);
        }
        let tasks = Self::active_tasks(engine, actor);
        let turn_scoped = tasks.is_empty();
        (tasks, turn_scoped)
    }

    async fn find(
        &mut self,
        engine: &SharedEngine,
        actor: AgentId,
        servers: Option<&McpClient>,
        args: &serde_json::Map<String, Value>,
    ) -> ToolOutcome {
        let Some(kind) = args.get("kind").and_then(Value::as_str) else {
            return ToolOutcome::error("`kind` must be `skill` or `mcp`.");
        };
        if args.get("query").is_some_and(|value| !value.is_string()) {
            return ToolOutcome::error("`query` must be a string.");
        }
        if args.get("server").is_some_and(|value| !value.is_string()) {
            return ToolOutcome::error("`server` must be a string.");
        }
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        let offset = match args.get("offset") {
            None => 0,
            Some(value) => match value.as_u64() {
                Some(value) => value.min(usize::MAX as u64) as usize,
                None => return ToolOutcome::error("`offset` must be a non-negative integer."),
            },
        };
        match kind {
            "skill" => {
                let entries = engine.with(|e| {
                    e.skills()
                        .iter()
                        .filter(|s| {
                            query.is_empty()
                                || s.name.to_lowercase().contains(&query)
                                || s.description.to_lowercase().contains(&query)
                        })
                        .map(|s| json!({"name":s.name,"description":s.summary()}))
                        .collect::<Vec<_>>()
                });
                ToolOutcome::ok(page(entries, offset))
            }
            "mcp" => {
                let Some(servers) = servers else {
                    return ToolOutcome::error("MCP is not configured.");
                };
                let Some(server) = args.get("server").and_then(Value::as_str) else {
                    let names = servers
                        .server_names()
                        .into_iter()
                        .filter(|n| query.is_empty() || n.to_lowercase().contains(&query))
                        .map(|name| json!({"server":name}))
                        .collect();
                    return ToolOutcome::ok(page(names, offset));
                };
                if !servers
                    .server_names()
                    .iter()
                    .any(|configured| configured == server)
                {
                    return ToolOutcome::error("That MCP server is not configured.");
                }
                let tools = match servers.load_for(actor, server).await {
                    Ok(tools) => tools,
                    Err(_) => return ToolOutcome::error("Could not inspect that MCP server."),
                };
                let entries = tools
                    .into_iter()
                    .filter(|t| {
                        query.is_empty()
                            || t.name.to_lowercase().contains(&query)
                            || t.description.to_lowercase().contains(&query)
                    })
                    .map(|t| json!({"name":t.name,"description":short(&t.description)}))
                    .collect::<Vec<_>>();
                if !self.has_server(server) {
                    if self.release(actor, servers, server).await.is_err() {
                        return ToolOutcome::error(
                            "Could not release the temporary MCP connection.",
                        );
                    }
                }
                ToolOutcome::ok(page(entries, offset))
            }
            _ => ToolOutcome::error("`kind` must be `skill` or `mcp`."),
        }
    }

    fn use_skill(
        &mut self,
        engine: &SharedEngine,
        actor: AgentId,
        args: &serde_json::Map<String, Value>,
    ) -> ToolOutcome {
        let Some(name) = args
            .get("skill")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        else {
            return ToolOutcome::error("`skill` must name an available skill.");
        };
        let lifetime = match lifetime(args) {
            Ok(v) => v,
            Err(e) => return ToolOutcome::error(e),
        };
        let Some(skill) = engine.with(|e| e.skill(name).cloned()) else {
            return ToolOutcome::error(
                "That skill is not available. Use `find_capabilities` to search.",
            );
        };
        let (task_ids, turn_scoped) = self.scope(engine, actor, lifetime);
        let lease = Lease {
            kind: Kind::Skill,
            name: skill.name.clone(),
            server: None,
            lifetime,
            task_ids,
            turn_scoped,
            consumed: false,
            expired: false,
        };
        if let Some(existing) = self.leases.iter_mut().find(|l| {
            l.kind == Kind::Skill && l.name.eq_ignore_ascii_case(&skill.name) && !l.expired
        }) {
            *existing = lease;
        } else {
            self.leases.push(lease);
        }
        if lifetime == Lifetime::Once {
            self.pending_once_skills.push(skill.name.clone());
        }
        ToolOutcome::ok(format!("SKILL {}\n{}", skill.name, skill.body))
    }

    async fn load_mcp(
        &mut self,
        engine: &SharedEngine,
        actor: AgentId,
        servers: Option<&McpClient>,
        args: &serde_json::Map<String, Value>,
    ) -> ToolOutcome {
        let (Some(client), Some(server), Some(name)) = (
            servers,
            args.get("server").and_then(Value::as_str),
            args.get("tool").and_then(Value::as_str),
        ) else {
            return ToolOutcome::error(
                "`server` and `tool` are required, and MCP must be configured.",
            );
        };
        let lifetime = match lifetime(args) {
            Ok(v) => v,
            Err(e) => return ToolOutcome::error(e),
        };
        if !client
            .server_names()
            .iter()
            .any(|configured| configured == server)
        {
            return ToolOutcome::error("That MCP server is not configured.");
        }
        let tools = match client.load_for(actor, server).await {
            Ok(tools) => tools,
            Err(_) => return ToolOutcome::error("Could not load that MCP server."),
        };
        let Some(definition) = tools.into_iter().find(|tool| tool.name == name) else {
            if !self.has_server(server) {
                if self.release(actor, client, server).await.is_err() {
                    return ToolOutcome::error("Could not release the temporary MCP connection.");
                }
            }
            return ToolOutcome::error(
                "That exact tool is not offered by the selected server. Search it first.",
            );
        };
        let (task_ids, turn_scoped) = self.scope(engine, actor, lifetime);
        let lease = Lease {
            kind: Kind::Mcp,
            name: definition.name.clone(),
            server: Some(server.into()),
            lifetime,
            task_ids,
            turn_scoped,
            consumed: false,
            expired: false,
        };
        if let Some(existing) = self.leases.iter_mut().find(|l| {
            l.kind == Kind::Mcp
                && l.name == definition.name
                && l.server.as_deref() == Some(server)
                && !l.expired
        }) {
            *existing = lease;
        } else {
            self.leases.push(lease);
        }
        ToolOutcome::ok(
            serde_json::to_string(&definition)
                .unwrap_or_else(|_| "Loaded tool schema could not be rendered.".into()),
        )
    }

    async fn call_mcp(
        &mut self,
        engine: &SharedEngine,
        actor: AgentId,
        servers: Option<&McpClient>,
        args: &serde_json::Map<String, Value>,
    ) -> ToolOutcome {
        let (Some(client), Some(name), Some(arguments)) = (
            servers,
            args.get("tool").and_then(Value::as_str),
            args.get("arguments"),
        ) else {
            return ToolOutcome::error(
                "`tool` and object `arguments` are required, and MCP must be configured.",
            );
        };
        if !arguments.is_object() {
            return ToolOutcome::error("MCP `arguments` must be an object.");
        }
        let Some(index) = self
            .leases
            .iter()
            .position(|l| l.kind == Kind::Mcp && l.name == name && !l.expired)
        else {
            return ToolOutcome::error("That MCP tool is not loaded. Load its exact schema first.");
        };
        let once = self.leases[index].lifetime == Lifetime::Once;
        let Some(server) = self.leases[index].server.clone() else {
            self.leases[index].expired = true;
            self.prune_expired();
            return ToolOutcome::error("The saved MCP lease is incomplete. Load the tool again.");
        };
        let mut outcome = if client.load_for(actor, &server).await.is_err() {
            ToolOutcome::error("Could not reconnect the loaded MCP tool.")
        } else {
            client.call_loaded_for(engine, actor, name, arguments).await
        };
        if once {
            self.leases[index].expired = true;
            self.prune_expired();
            if let Err(error) = self.release_if_unused(actor, client, &server).await {
                tracing::warn!(server, "could not release one-shot MCP connection: {error}");
                outcome.content.push_str(
                    "\nThe one-shot capability expired, but its connection could not be released.",
                );
            }
        }
        outcome
    }

    async fn unload(
        &mut self,
        actor: AgentId,
        servers: Option<&McpClient>,
        args: &serde_json::Map<String, Value>,
    ) -> ToolOutcome {
        let (Some(kind), Some(name)) = (
            args.get("kind").and_then(Value::as_str),
            args.get("name").and_then(Value::as_str),
        ) else {
            return ToolOutcome::error("`kind` and `name` are required.");
        };
        let kind = match kind {
            "skill" => Kind::Skill,
            "mcp" => Kind::Mcp,
            _ => return ToolOutcome::error("`kind` must be `skill` or `mcp`."),
        };
        let Some(index) = self.leases.iter().position(|l| {
            l.kind == kind
                && (if kind == Kind::Skill {
                    l.name.eq_ignore_ascii_case(name)
                } else {
                    l.name == name || l.server.as_deref() == Some(name)
                })
                && !l.expired
        }) else {
            return ToolOutcome::error("That capability is not loaded.");
        };
        let server = self.leases[index].server.clone();
        self.leases[index].expired = true;
        self.prune_expired();
        if let (Some(client), Some(server)) = (servers, server) {
            if self
                .release_if_unused(actor, client, &server)
                .await
                .is_err()
            {
                return ToolOutcome::error(
                    "Capability was unloaded, but its MCP connection could not be released.",
                );
            }
        }
        ToolOutcome::ok("Capability unloaded.")
    }

    fn has_server(&self, server: &str) -> bool {
        self.leases
            .iter()
            .any(|l| !l.expired && l.kind == Kind::Mcp && l.server.as_deref() == Some(server))
    }

    async fn release_if_unused(
        &self,
        actor: AgentId,
        client: &McpClient,
        server: &str,
    ) -> Result<(), String> {
        if !self.has_server(server) {
            self.release(actor, client, server).await?;
        }
        Ok(())
    }

    async fn release(
        &self,
        actor: AgentId,
        client: &McpClient,
        server: &str,
    ) -> Result<(), String> {
        client.release_for(actor, server).await.map_err(|_| {
            tracing::warn!(server, "could not release actor MCP connection");
            "MCP connection release failed".to_string()
        })
    }

    pub(crate) async fn refresh(
        &mut self,
        engine: &SharedEngine,
        actor: AgentId,
        servers: Option<&McpClient>,
    ) {
        let active: HashSet<_> = Self::active_tasks(engine, actor).into_iter().collect();
        let to_expire: Vec<_> = self
            .leases
            .iter_mut()
            .filter_map(|lease| {
                if lease.lifetime == Lifetime::Task
                    && !lease.turn_scoped
                    && lease.task_ids.iter().all(|id| !active.contains(id))
                {
                    lease.expired = true;
                    lease.server.clone()
                } else {
                    None
                }
            })
            .collect();
        self.prune_expired();
        if let Some(client) = servers {
            for server in to_expire {
                if self
                    .release_if_unused(actor, client, &server)
                    .await
                    .is_err()
                {
                    tracing::warn!(server, "expired MCP lease could not release its connection");
                }
            }
        }
    }

    pub(crate) async fn end_turn(&mut self, actor: AgentId, servers: Option<&McpClient>) {
        let to_expire: Vec<_> = self
            .leases
            .iter_mut()
            .filter_map(|lease| {
                let end = lease.turn_scoped && lease.lifetime == Lifetime::Task
                    || lease.kind == Kind::Mcp && lease.lifetime == Lifetime::Once;
                if end && !lease.expired {
                    lease.expired = true;
                    lease.server.clone()
                } else {
                    None
                }
            })
            .collect();
        self.prune_expired();
        if let Some(client) = servers {
            for server in to_expire {
                if self
                    .release_if_unused(actor, client, &server)
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        server,
                        "end-of-turn MCP lease could not release its connection"
                    );
                }
            }
        }
    }

    pub(crate) fn after_response(&mut self, history: &mut [Turn]) {
        for name in self.pending_once_skills.drain(..) {
            if let Some(lease) = self.leases.iter_mut().find(|l| {
                l.kind == Kind::Skill
                    && l.lifetime == Lifetime::Once
                    && l.name == name
                    && !l.expired
            }) {
                lease.consumed = true;
                lease.expired = true;
            }
        }
        self.pending_discovery = false;
        self.prune_expired();
        self.prune_history(history);
    }

    pub(crate) fn prune_history(&self, history: &mut [Turn]) {
        let loaded_skills: HashSet<_> = self
            .leases
            .iter()
            .filter(|l| l.kind == Kind::Skill && !l.expired)
            .map(|l| l.name.to_lowercase())
            .collect();
        let loaded_tools: HashSet<_> = self
            .leases
            .iter()
            .filter(|l| l.kind == Kind::Mcp && !l.expired)
            .filter_map(|l| {
                l.server
                    .as_ref()
                    .map(|server| format!("{server}/{}", l.name))
            })
            .collect();
        let mut calls = HashMap::new();
        for turn in history.iter() {
            for block in &turn.content {
                if let ContentBlock::ToolUse { id, name, input } = block {
                    calls.insert(id.clone(), (name.clone(), input.clone()));
                }
            }
        }
        let mut keep_ids = HashSet::new();
        let mut seen = HashSet::new();
        for turn in history.iter().rev() {
            for block in turn.content.iter().rev() {
                let ContentBlock::ToolResult {
                    tool_use_id,
                    is_error: false,
                    content,
                    ..
                } = block
                else {
                    continue;
                };
                if content.starts_with('[') {
                    continue;
                }
                let Some((tool, input)) = calls.get(tool_use_id) else {
                    continue;
                };
                if tool == "find_capabilities" && self.pending_discovery {
                    keep_ids.insert(tool_use_id.clone());
                    continue;
                }
                let key = match tool.as_str() {
                    "use_skill" => input
                        .get("skill")
                        .and_then(Value::as_str)
                        .map(str::to_lowercase)
                        .filter(|skill| loaded_skills.contains(skill))
                        .map(|skill| format!("skill/{skill}")),
                    "load_mcp_tool" => {
                        let server = input.get("server").and_then(Value::as_str).unwrap_or("");
                        let name = input.get("tool").and_then(Value::as_str).unwrap_or("");
                        let key = format!("{server}/{name}");
                        loaded_tools.contains(&key).then(|| format!("mcp/{key}"))
                    }
                    _ => None,
                };
                if let Some(key) = key {
                    if seen.insert(key) {
                        keep_ids.insert(tool_use_id.clone());
                    }
                }
            }
        }
        for turn in history.iter_mut() {
            for block in &mut turn.content {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    ..
                } = block
                {
                    if *is_error || keep_ids.contains(tool_use_id) {
                        continue;
                    }
                    match calls.get(tool_use_id).map(|(name, _)| name.as_str()) {
                        Some("use_skill") if content.starts_with("SKILL ") => {
                            *content = "[skill instructions released]".into()
                        }
                        Some("load_mcp_tool") => *content = "[MCP tool schema released]".into(),
                        Some("find_capabilities") => {
                            *content = "[capability search result consumed]".into()
                        }
                        _ if content.starts_with("SKILL ") => {
                            *content = "[untracked skill instructions released]".into()
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn prune_expired(&mut self) {
        self.leases.retain(|lease| !lease.expired);
    }
}

fn lifetime(args: &serde_json::Map<String, Value>) -> Result<Lifetime, String> {
    if args.get("lifetime").is_some_and(|value| !value.is_string()) {
        return Err("`lifetime` must be `once` or `task`.".into());
    }
    match args
        .get("lifetime")
        .and_then(Value::as_str)
        .unwrap_or("task")
    {
        "once" => Ok(Lifetime::Once),
        "task" => Ok(Lifetime::Task),
        _ => Err("`lifetime` must be `once` or `task`.".into()),
    }
}

fn short(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.chars().take(180).collect()
}

fn page(entries: Vec<Value>, offset: usize) -> String {
    let total = entries.len();
    let page = entries
        .into_iter()
        .skip(offset)
        .take(PAGE_SIZE)
        .collect::<Vec<_>>();
    json!({"total":total,"offset":offset,"limit":PAGE_SIZE,"entries":page}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
    use orgbot_proto::skill::Skill;
    use orgbot_providers::Role;

    fn engine_with_skills() -> SharedEngine {
        let mut engine = orgbot_engine::Engine::new(RoomSpec {
            name: "capabilities-test".into(),
            goal: "test capability lifecycle".into(),
            roster: vec![],
            completion: CompletionRule::AllTasksTerminal,
            limits: RoomLimits::default(),
        });
        engine.set_skills(vec![
            Skill {
                name: "alpha".into(),
                description: "first".into(),
                body: "alpha body".into(),
            },
            Skill {
                name: "beta".into(),
                description: "second".into(),
                body: "beta body".into(),
            },
        ]);
        SharedEngine::new(engine)
    }

    fn capability_result(
        id: &str,
        name: &str,
        input: Value,
        content: &str,
        is_error: bool,
    ) -> Vec<ContentBlock> {
        vec![
            ContentBlock::ToolUse {
                id: id.into(),
                name: name.into(),
                input,
            },
            ContentBlock::ToolResult {
                tool_use_id: id.into(),
                content: content.into(),
                is_error,
            },
        ]
    }

    #[test]
    fn definitions_keep_skill_unload_for_reviewers_but_withhold_mcp() {
        let tools = definitions(true);
        let names: HashSet<_> = tools.iter().map(|tool| tool.name.as_str()).collect();
        assert!(names.contains("find_capabilities"));
        assert!(names.contains("use_skill"));
        assert!(names.contains("unload_capability"));
        assert!(!names.contains("load_mcp_tool"));
        assert!(!names.contains("call_mcp_tool"));
        assert!(handles("call_mcp_tool"));
        assert!(!handles("random"));
    }

    #[tokio::test]
    async fn once_skill_waits_until_consumed_then_redacts_without_orphaning_pair() {
        let engine = engine_with_skills();
        let actor = AgentId::new();
        let mut capabilities = Capabilities::default();
        let loaded = capabilities
            .dispatch(
                &engine,
                actor,
                false,
                None,
                "use_skill",
                &json!({"skill":"alpha","lifetime":"once"}),
            )
            .await;
        assert!(!loaded.is_error);
        assert!(loaded.content.contains("alpha body"));
        assert_eq!(capabilities.leases.len(), 1);

        let mut history = vec![
            Turn {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "skill-call".into(),
                    name: "use_skill".into(),
                    input: json!({"skill":"alpha","lifetime":"once"}),
                }],
            },
            Turn::tool_results(vec![ContentBlock::ToolResult {
                tool_use_id: "skill-call".into(),
                content: loaded.content,
                is_error: false,
            }]),
        ];
        capabilities.prune_history(&mut history);
        assert!(
            matches!(&history[1].content[0], ContentBlock::ToolResult { content, .. } if content.contains("alpha body"))
        );
        capabilities.after_response(&mut history);
        assert!(capabilities.leases.is_empty());
        assert!(
            matches!(&history[1].content[0], ContentBlock::ToolResult { tool_use_id, content, .. } if tool_use_id == "skill-call" && content == "[skill instructions released]")
        );
    }

    #[tokio::test]
    async fn task_skill_refresh_replaces_body_and_expiry_allows_reload() {
        let engine = engine_with_skills();
        let actor = AgentId::new();
        let mut capabilities = Capabilities::default();
        let first = capabilities
            .dispatch(
                &engine,
                actor,
                false,
                None,
                "use_skill",
                &json!({"skill":"alpha"}),
            )
            .await;
        engine.with(|engine| {
            engine.set_skills(vec![Skill {
                name: "alpha".into(),
                description: "updated".into(),
                body: "updated alpha body".into(),
            }]);
        });
        let refreshed = capabilities
            .dispatch(
                &engine,
                actor,
                false,
                None,
                "use_skill",
                &json!({"skill":"alpha"}),
            )
            .await;
        assert!(!first.is_error);
        assert!(!refreshed.is_error);
        assert!(refreshed.content.contains("updated alpha body"));
        assert_eq!(capabilities.leases.len(), 1);
        assert!(capabilities.leases[0].turn_scoped);

        capabilities.end_turn(actor, None).await;
        assert!(capabilities.leases.is_empty());
        let reloaded = capabilities
            .dispatch(
                &engine,
                actor,
                false,
                None,
                "use_skill",
                &json!({"skill":"alpha"}),
            )
            .await;
        assert!(!reloaded.is_error);
        let unloaded = capabilities
            .dispatch(
                &engine,
                actor,
                false,
                None,
                "unload_capability",
                &json!({"kind":"skill","name":"ALPHA"}),
            )
            .await;
        assert!(!unloaded.is_error);
        assert!(capabilities.leases.is_empty());
    }

    #[tokio::test]
    async fn invalid_lifetime_and_missing_skill_do_not_create_leases() {
        let engine = engine_with_skills();
        let mut capabilities = Capabilities::default();
        let invalid = capabilities
            .dispatch(
                &engine,
                AgentId::new(),
                false,
                None,
                "use_skill",
                &json!({"skill":"alpha","lifetime":false}),
            )
            .await;
        let missing = capabilities
            .dispatch(
                &engine,
                AgentId::new(),
                false,
                None,
                "use_skill",
                &json!({"skill":"absent"}),
            )
            .await;
        let malformed = capabilities
            .dispatch(
                &engine,
                AgentId::new(),
                false,
                None,
                "use_skill",
                &json!("not an object"),
            )
            .await;
        assert!(invalid.is_error && missing.is_error && malformed.is_error);
        assert!(capabilities.leases.is_empty());
    }

    #[tokio::test]
    async fn failed_mcp_load_and_duplicate_tool_load_do_not_add_leases() {
        let engine = engine_with_skills();
        let actor = AgentId::new();
        let client = McpClient::default();
        let mut capabilities = Capabilities::default();
        let failed = capabilities
            .dispatch(
                &engine,
                actor,
                false,
                Some(&client),
                "load_mcp_tool",
                &json!({"server":"missing","tool":"missing__tool"}),
            )
            .await;
        assert!(failed.is_error);
        assert!(capabilities.leases.is_empty());

        capabilities.leases.push(Lease {
            kind: Kind::Mcp,
            name: "fixture__lookup".into(),
            server: Some("fixture".into()),
            lifetime: Lifetime::Task,
            task_ids: vec![],
            turn_scoped: true,
            consumed: false,
            expired: false,
        });
        let duplicate = capabilities
            .dispatch(
                &engine,
                actor,
                false,
                Some(&client),
                "load_mcp_tool",
                &json!({"server":"fixture","tool":"fixture__lookup"}),
            )
            .await;
        assert!(duplicate.is_error);
        assert_eq!(capabilities.leases.len(), 1);
        assert_eq!(capabilities.leases[0].name, "fixture__lookup");
    }

    #[tokio::test]
    async fn successful_capability_discovery_is_pruned_after_the_next_response() {
        let engine = engine_with_skills();
        let mut capabilities = Capabilities::default();
        let found = capabilities
            .dispatch(
                &engine,
                AgentId::new(),
                false,
                None,
                "find_capabilities",
                &json!({"kind":"skill"}),
            )
            .await;
        assert!(!found.is_error);
        let mut history = vec![
            Turn {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "find".into(),
                    name: "find_capabilities".into(),
                    input: json!({"kind":"skill"}),
                }],
            },
            Turn::tool_results(vec![ContentBlock::ToolResult {
                tool_use_id: "find".into(),
                content: found.content,
                is_error: false,
            }]),
        ];
        capabilities.prune_history(&mut history);
        assert!(
            matches!(&history[1].content[0], ContentBlock::ToolResult { content, .. } if content.contains("alpha"))
        );
        capabilities.after_response(&mut history);
        assert!(
            matches!(&history[1].content[0], ContentBlock::ToolResult { tool_use_id, content, .. } if tool_use_id == "find" && content == "[capability search result consumed]")
        );
    }

    #[test]
    fn parallel_discovery_results_are_all_kept_until_consumed() {
        let capabilities = Capabilities {
            pending_discovery: true,
            ..Capabilities::default()
        };
        let mut history = vec![
            Turn {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        id: "find-skill".into(),
                        name: "find_capabilities".into(),
                        input: json!({"kind":"skill"}),
                    },
                    ContentBlock::ToolUse {
                        id: "find-mcp".into(),
                        name: "find_capabilities".into(),
                        input: json!({"kind":"mcp"}),
                    },
                ],
            },
            Turn::tool_results(vec![
                ContentBlock::ToolResult {
                    tool_use_id: "find-skill".into(),
                    content: "skill results".into(),
                    is_error: false,
                },
                ContentBlock::ToolResult {
                    tool_use_id: "find-mcp".into(),
                    content: "MCP results".into(),
                    is_error: false,
                },
            ]),
        ];
        capabilities.prune_history(&mut history);
        assert!(
            matches!(&history[1].content[0], ContentBlock::ToolResult { content, .. } if content == "skill results")
        );
        assert!(
            matches!(&history[1].content[1], ContentBlock::ToolResult { content, .. } if content == "MCP results")
        );
    }

    #[tokio::test]
    async fn task_lease_expires_when_its_captured_task_is_no_longer_active() {
        let engine = engine_with_skills();
        let actor = AgentId::new();
        let task_id = TaskId::new();
        let mut capabilities = Capabilities::default();
        capabilities.leases.push(Lease {
            kind: Kind::Skill,
            name: "alpha".into(),
            server: None,
            lifetime: Lifetime::Task,
            task_ids: vec![task_id],
            turn_scoped: false,
            consumed: false,
            expired: false,
        });
        let mut history = vec![
            Turn {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "skill".into(),
                    name: "use_skill".into(),
                    input: json!({"skill":"alpha"}),
                }],
            },
            Turn::tool_results(vec![ContentBlock::ToolResult {
                tool_use_id: "skill".into(),
                content: "SKILL alpha\nbody".into(),
                is_error: false,
            }]),
        ];
        capabilities.refresh(&engine, actor, None).await;
        capabilities.prune_history(&mut history);
        assert!(capabilities.leases.is_empty());
        assert!(
            matches!(&history[1].content[0], ContentBlock::ToolResult { content, .. } if content == "[skill instructions released]")
        );
    }

    #[tokio::test]
    async fn dispatch_refreshes_task_scope_before_executing_a_tool() {
        let engine = engine_with_skills();
        let actor = AgentId::new();
        let mut capabilities = Capabilities::default();
        capabilities.leases.push(Lease {
            kind: Kind::Skill,
            name: "alpha".into(),
            server: None,
            lifetime: Lifetime::Task,
            task_ids: vec![TaskId::new()],
            turn_scoped: false,
            consumed: false,
            expired: false,
        });

        let unloaded = capabilities
            .dispatch(
                &engine,
                actor,
                false,
                None,
                "unload_capability",
                &json!({"kind":"skill","name":"alpha"}),
            )
            .await;
        assert!(unloaded.is_error);
        assert!(capabilities.leases.is_empty());
    }

    #[test]
    fn history_keeps_each_active_skill_and_releases_only_failed_or_old_duplicates() {
        let mut capabilities = Capabilities::default();
        for name in ["alpha", "beta"] {
            capabilities.leases.push(Lease {
                kind: Kind::Skill,
                name: name.into(),
                server: None,
                lifetime: Lifetime::Task,
                task_ids: vec![],
                turn_scoped: true,
                consumed: false,
                expired: false,
            });
        }
        let old = capability_result(
            "old",
            "use_skill",
            json!({"skill":"alpha"}),
            "SKILL alpha\nold body",
            false,
        );
        let failed_duplicate = capability_result(
            "failed",
            "use_skill",
            json!({"skill":"alpha"}),
            "duplicate",
            true,
        );
        let latest = capability_result(
            "new",
            "use_skill",
            json!({"skill":"alpha"}),
            "SKILL alpha\nnew body",
            false,
        );
        let other = capability_result(
            "beta",
            "use_skill",
            json!({"skill":"beta"}),
            "SKILL beta\nbeta body",
            false,
        );
        let mut history = vec![
            Turn {
                role: Role::Assistant,
                content: old,
            },
            Turn::tool_results(failed_duplicate),
            Turn {
                role: Role::Assistant,
                content: latest,
            },
            Turn::tool_results(other),
        ];
        capabilities.prune_history(&mut history);
        assert!(
            matches!(&history[0].content[1], ContentBlock::ToolResult { content, .. } if content == "[skill instructions released]")
        );
        assert!(
            matches!(&history[1].content[1], ContentBlock::ToolResult { content, is_error: true, .. } if content == "duplicate")
        );
        assert!(
            matches!(&history[2].content[1], ContentBlock::ToolResult { content, .. } if content == "SKILL alpha\nnew body")
        );
        assert!(
            matches!(&history[3].content[1], ContentBlock::ToolResult { content, .. } if content == "SKILL beta\nbeta body")
        );
    }

    #[test]
    fn released_mcp_schema_is_redacted_even_when_short() {
        let mut capabilities = Capabilities::default();
        capabilities.leases.push(Lease {
            kind: Kind::Mcp,
            name: "fixture__lookup".into(),
            server: Some("fixture".into()),
            lifetime: Lifetime::Once,
            task_ids: vec![],
            turn_scoped: false,
            consumed: false,
            expired: true,
        });
        let mut history = vec![
            Turn {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "schema".into(),
                    name: "load_mcp_tool".into(),
                    input: json!({"server":"fixture","tool":"fixture__lookup"}),
                }],
            },
            Turn::tool_results(vec![ContentBlock::ToolResult {
                tool_use_id: "schema".into(),
                content: "{}".into(),
                is_error: false,
            }]),
        ];
        capabilities.prune_history(&mut history);
        assert!(
            matches!(&history[1].content[0], ContentBlock::ToolResult { tool_use_id, content, .. } if tool_use_id == "schema" && content == "[MCP tool schema released]")
        );
    }
}
