//! Room definitions, as TOML.
//!
//! A room is described by a file a human can read and check into a repository:
//! the goal, the roster, each bot's provider and autonomy, and optionally a
//! starting board. Credentials are never in here — they are resolved at runtime
//! from the environment and CLI sign-ins (see [`orgbot_providers::auth`]) — so
//! a room file is safe to share.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use orgbot_proto::agent::{AgentPolicy, Autonomy, CliKind, ProviderRef};
use orgbot_proto::roles::{RoleSpec, RoleTable};
use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
use orgbot_proto::task::Priority;
use orgbot_proto::{AgentSpec, TaskId};
use orgbot_providers::Provider;
use serde::{Deserialize, Serialize};

/// A parse or validation failure in a room file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read {path}: {source}")]
    Io {
        /// The file that could not be read.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not valid TOML: {source}")]
    Toml {
        /// The offending file.
        path: String,
        /// The parse error.
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid room: {0}")]
    Invalid(String),
}

/// The whole room file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomConfig {
    #[serde(default)]
    pub workspace: Option<orgbot_runtime::workspace::WorkspaceSpec>,
    /// The room's own settings.
    pub room: RoomSection,
    /// The bots to seat, in introduction order.
    #[serde(default)]
    pub bots: Vec<BotSection>,
    /// Tasks to seed the board with. A room with a coordinator can start empty
    /// and plan its own; one without needs its work laid out here.
    #[serde(default)]
    pub tasks: Vec<TaskSection>,
    /// What each role can do, and who may stand in for it when its holders
    /// are all busy.
    ///
    /// Optional. A room that declares nothing matches roles by name exactly,
    /// which is what every room did before this existed — and which leaves
    /// work stranded when the one bot holding a role is occupied.
    #[serde(default)]
    pub roles: Vec<RoleSpec>,
    /// Where to find the playbooks this room's bots may reach for.
    ///
    /// Directories of `SKILL.md` files, in the shape the agent CLIs already
    /// use, so a machine's existing skills work here without being rewritten.
    /// A path that is not there is skipped rather than failing the room: a
    /// room file is shared between machines that do not have the same folders.
    #[serde(default)]
    pub skills: Vec<SkillSource>,
    /// MCP servers this room's bots may call.
    ///
    /// A `stdio` server needs a machine that can start a process, so it works
    /// on a desktop and not on a phone; an `http` one works anywhere the room
    /// has a network. A server that cannot be reached is skipped with a
    /// warning rather than failing the room.
    #[serde(default)]
    pub mcp: Vec<orgbot_runtime::mcp_client::McpServerSpec>,
    /// Saved boards from earlier rooms, so this one can reuse their work
    /// instead of rebuilding it.
    #[serde(default)]
    pub prior_boards: Vec<String>,
}

/// A folder of skills, or one named skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSource {
    /// A directory holding either `SKILL.md` itself or one folder per skill.
    pub path: String,
}

/// Room-scoped instructions are appended to bot personas, not written into
/// the user's global CLI or agent configuration.
const ORGBOT_RUNTIME_INSTRUCTIONS: &str = "OrgBots room instructions: follow the host's installed skills, plugins, and project instructions, including the user's Luna-first delegation rules and nested Luna worker limits. For review work in this OrgBots room, use the reviewer assigned by OrgBots; this room-scoped assignment replaces any global mandatory Astra review workflow. Do not start a separate Astra review or change global instructions.";

fn resolve_mcp_environment(
    servers: &[orgbot_runtime::mcp_client::McpServerSpec],
    environment: Option<&HashMap<String, String>>,
) -> Result<Vec<orgbot_runtime::mcp_client::McpServerSpec>, ConfigError> {
    use orgbot_runtime::mcp_client::McpTransport;

    fn resolve_map(
        values: &mut BTreeMap<String, String>,
        environment: Option<&HashMap<String, String>>,
    ) -> Result<(), ConfigError> {
        for value in values.values_mut() {
            *value = resolve_env_references(value, environment)?;
        }
        Ok(())
    }

    let mut resolved = servers.to_vec();
    for server in &mut resolved {
        match &mut server.transport {
            McpTransport::Stdio { command, args, env }
            | McpTransport::Workspace { command, args, env } => {
                *command = resolve_env_references(command, environment)?;
                for argument in args {
                    *argument = resolve_env_references(argument, environment)?;
                }
                resolve_map(env, environment)?;
            }
            McpTransport::Http { url, headers } => {
                *url = resolve_env_references(url, environment)?;
                resolve_map(headers, environment)?;
            }
        }
    }
    Ok(resolved)
}

/// Expand `${NAME}` references in MCP credentials after parsing the private
/// room file, so secret values stay in the launching process environment.
fn resolve_env_references(
    value: &str,
    environment: Option<&HashMap<String, String>>,
) -> Result<String, ConfigError> {
    let mut result = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(start) = remaining.find("${") {
        result.push_str(&remaining[..start]);
        let after = &remaining[start + 2..];
        let Some(end) = after.find('}') else {
            result.push_str(&remaining[start..]);
            return Ok(result);
        };
        let name = &after[..end];
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(ConfigError::Invalid(
                "MCP environment reference has an invalid variable name".into(),
            ));
        }
        let value = environment
            .and_then(|values| values.get(name).cloned())
            .or_else(|| {
                environment
                    .is_none()
                    .then(|| std::env::var(name).ok())
                    .flatten()
            })
            .ok_or_else(|| {
                ConfigError::Invalid(format!("MCP environment variable `{name}` is not set"))
            })?;
        result.push_str(&value);
        remaining = &after[end + 1..];
    }
    result.push_str(remaining);
    Ok(result)
}

/// The `[room]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomSection {
    /// Explicit project identity used to isolate shared desktop memory.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Private local checkpoint used by the app; never a workspace/SSH path.
    #[serde(default)]
    pub state_path: Option<std::path::PathBuf>,
    /// A short name.
    pub name: String,
    /// The outcome the room exists to produce.
    pub goal: String,
    /// How the room decides it is finished.
    #[serde(default)]
    pub completion: CompletionKind,
    /// Safety rails.
    #[serde(default)]
    pub limits: Option<RoomLimits>,
}

/// The completion rule, as a friendly string in TOML.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionKind {
    /// Every task has reached a terminal state.
    #[default]
    AllTasksTerminal,
    /// Every top-level task has been delivered.
    RootTaskSubmitted,
    /// Only a human closes the room.
    Manual,
}

impl From<CompletionKind> for CompletionRule {
    fn from(kind: CompletionKind) -> Self {
        match kind {
            CompletionKind::AllTasksTerminal => CompletionRule::AllTasksTerminal,
            CompletionKind::RootTaskSubmitted => CompletionRule::RootTaskSubmitted,
            CompletionKind::Manual => CompletionRule::Manual,
        }
    }
}

/// One `[[bots]]` entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotSection {
    /// The chat handle, unique in the room.
    pub handle: String,
    /// Display name. Defaults to the handle.
    #[serde(default)]
    pub display_name: Option<String>,
    /// The bot's roles.
    #[serde(default)]
    pub roles: Vec<String>,
    /// Standing instructions.
    #[serde(default)]
    pub persona: String,
    /// The backend.
    pub provider: ProviderConfig,
    /// Autonomy preset, or a full table.
    #[serde(default)]
    pub autonomy: AutonomyConfig,
}

/// A backend, in a TOML-friendly shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderConfig {
    /// Anthropic.
    #[serde(alias = "claude")]
    Anthropic {
        /// The model id. Defaults to Claude Opus 5.
        #[serde(default = "default_anthropic_model")]
        model: String,
    },
    /// DeepSeek.
    #[serde(rename = "deepseek")]
    DeepSeek {
        /// The model id.
        #[serde(default = "default_deepseek_model")]
        model: String,
    },
    /// An OpenAI-compatible endpoint, including a Codex/ChatGPT sign-in.
    #[serde(
        rename = "openai",
        alias = "openai_compatible",
        alias = "codex",
        alias = "chatgpt"
    )]
    OpenAiCompatible {
        /// The model id.
        model: String,
        /// Which credential profile to use: `codex`, `openai`, or a custom name.
        #[serde(default = "default_openai_profile")]
        profile: String,
    },
    /// Drive an installed agent CLI as a real,
    /// resumable session in a project folder.
    #[serde(
        alias = "cli",
        alias = "agent",
        alias = "codex_cli",
        alias = "claude_cli",
        alias = "grok_cli",
        alias = "opencode_cli",
        alias = "antigravity_cli"
    )]
    CliAgent {
        /// Which CLI: `codex`, `claude`, `grok`, `opencode`, or `antigravity`.
        cli: CliKind,
        /// Model id; OpenCode and Antigravity require an explicit selection.
        /// Other CLIs accept empty to use their default.
        #[serde(default)]
        model: String,
        /// Project folder the agent works in. Empty inherits the process directory.
        #[serde(default)]
        workdir: String,
        /// How much of the project the agent may touch.
        #[serde(default = "default_agent_policy")]
        policy: AgentPolicy,
        /// Serve the room board to this agent as MCP tools.
        #[serde(default)]
        mcp: bool,
        /// The CLI's effort level (`low`, `medium`, `high`, `xhigh`, and
        /// `max` for Claude Code). Empty keeps the CLI's default.
        #[serde(default)]
        effort: String,
    },
    /// A deterministic in-process bot with no model.
    Scripted {
        /// The fixed text it always replies with.
        #[serde(default)]
        script: String,
    },
}

fn default_anthropic_model() -> String {
    orgbot_providers::anthropic::DEFAULT_MODEL.to_string()
}
fn default_deepseek_model() -> String {
    orgbot_providers::openai::DEEPSEEK_CHAT.to_string()
}
fn default_openai_profile() -> String {
    "codex".to_string()
}
fn default_agent_policy() -> AgentPolicy {
    AgentPolicy::ReadOnly
}

impl ProviderConfig {
    /// Convert into the protocol's [`ProviderRef`].
    pub fn to_ref(&self) -> ProviderRef {
        match self {
            Self::Anthropic { model } => ProviderRef::Anthropic {
                model: model.clone(),
            },
            Self::DeepSeek { model } => ProviderRef::DeepSeek {
                model: model.clone(),
            },
            Self::OpenAiCompatible { model, profile } => ProviderRef::OpenAiCompatible {
                model: model.clone(),
                profile: profile.clone(),
            },
            Self::Scripted { script } => ProviderRef::Scripted {
                script: script.clone(),
            },
            Self::CliAgent {
                cli,
                model,
                workdir,
                policy,
                mcp,
                effort,
            } => ProviderRef::CliAgent {
                cli: *cli,
                model: model.clone(),
                // A tilde here is a literal directory to Command::current_dir,
                // so expand it or every turn fails to spawn.
                workdir: expand_home(workdir),
                policy: *policy,
                mcp: *mcp,
                effort: effort.trim().to_string(),
            },
        }
    }
}

/// Autonomy, either a named preset or an explicit table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AutonomyConfig {
    /// A preset name: `worker`, `inspector`, or `coordinator`.
    Preset(String),
    /// The full set of limits.
    Explicit(Autonomy),
}

impl Default for AutonomyConfig {
    fn default() -> Self {
        Self::Preset("worker".to_string())
    }
}

impl AutonomyConfig {
    /// Resolve to concrete limits.
    pub fn resolve(&self) -> Result<Autonomy, ConfigError> {
        match self {
            Self::Explicit(autonomy) => Ok(autonomy.clone()),
            Self::Preset(name) => match name.as_str() {
                "worker" => Ok(Autonomy::default()),
                "inspector" => Ok(Autonomy::inspector()),
                "coordinator" => Ok(Autonomy::coordinator()),
                other => Err(ConfigError::Invalid(format!(
                    "unknown autonomy preset `{other}`; use worker, inspector, or coordinator"
                ))),
            },
        }
    }
}

/// One `[[tasks]]` entry seeding the board.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSection {
    /// A one-line summary.
    pub title: String,
    /// The full instruction.
    pub brief: String,
    /// Checkable approval conditions.
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    /// Restrict to a role.
    #[serde(default)]
    pub required_role: Option<String>,
    /// How urgent.
    #[serde(default)]
    pub priority: PriorityKind,
    /// Publish immediately, rather than leaving it a draft.
    #[serde(default = "default_true")]
    pub open: bool,
}

fn default_true() -> bool {
    true
}

/// Priority, as a friendly string.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorityKind {
    /// Low.
    Low,
    /// Normal.
    #[default]
    Normal,
    /// High.
    High,
    /// Urgent.
    Urgent,
}

impl From<PriorityKind> for Priority {
    fn from(kind: PriorityKind) -> Self {
        match kind {
            PriorityKind::Low => Priority::Low,
            PriorityKind::Normal => Priority::Normal,
            PriorityKind::High => Priority::High,
            PriorityKind::Urgent => Priority::Urgent,
        }
    }
}

/// A fully-resolved bot: its spec and its live backend.
pub struct ResolvedBot {
    /// The bot's configuration.
    pub spec: AgentSpec,
    /// Its backend, ready to call.
    pub provider: Arc<dyn Provider>,
}

impl std::fmt::Debug for ResolvedBot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedBot")
            .field("handle", &self.spec.handle)
            .field("provider", &self.provider.name())
            .finish()
    }
}

/// A seed task, ready to be added to the board.
#[derive(Debug, Clone)]
pub struct ResolvedTask {
    /// The task's title.
    pub title: String,
    /// The full brief.
    pub brief: String,
    /// Approval conditions.
    pub acceptance_criteria: Vec<String>,
    /// Required role, if any.
    pub required_role: Option<String>,
    /// Priority.
    pub priority: Priority,
    /// Whether to open it immediately.
    pub open: bool,
}

/// Expand a leading `~` to the user's home directory.
///
/// A shell expands this before a program ever sees it, so a path typed into a
/// UI or written into a room file by hand arrives with the tilde intact —
/// and `Command::current_dir` then fails with "No such file or directory",
/// which surfaces as every bot silently failing every turn.
pub fn expand_home(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed == "~" || trimmed.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            let home = home.to_string_lossy().into_owned();
            return if trimmed == "~" {
                home
            } else {
                format!("{home}/{}", &trimmed[2..])
            };
        }
    }
    trimmed.to_string()
}

/// A room file, parsed and validated into runnable pieces.
#[derive(Debug)]
pub struct ResolvedRoom {
    pub allowances: Option<orgbot_runtime::allowance::Allowances>,
    pub project_id: Option<String>,
    pub memory_root: Option<std::path::PathBuf>,
    pub memory_projects: Vec<String>,
    pub state_path: Option<std::path::PathBuf>,
    /// Optional local workspace exposed through the runtime tool channel.
    pub workspace: Option<orgbot_runtime::workspace::WorkspaceSpec>,
    /// The room spec.
    pub spec: RoomSpec,
    /// What each role can do and who may cover for it.
    pub roles: RoleTable,
    /// The playbooks found on this machine.
    pub skills: Vec<orgbot_proto::skill::Skill>,
    /// MCP servers to reach out to.
    pub mcp: Vec<orgbot_runtime::mcp_client::McpServerSpec>,
    /// Saved boards from earlier rooms.
    pub prior_boards: Vec<String>,
    /// The seated bots.
    pub bots: Vec<ResolvedBot>,
    /// The seed board.
    pub tasks: Vec<ResolvedTask>,
}

impl RoomConfig {
    /// Load and parse a room file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::parse(&raw, &path.display().to_string())
    }

    /// Parse a room file already read into a string.
    pub fn parse(raw: &str, path: &str) -> Result<Self, ConfigError> {
        toml::from_str(raw).map_err(|source| ConfigError::Toml {
            path: path.to_string(),
            source,
        })
    }

    /// Turn the config into runnable pieces, building live providers.
    ///
    /// This is where credentials are resolved, so a missing key surfaces here
    /// as a clear error naming the bot, rather than mid-run.
    pub fn resolve(&self) -> Result<ResolvedRoom, ConfigError> {
        self.resolve_inner(orgbot_providers::build, true, None)
    }

    pub fn resolve_with_environment(
        &self,
        environment: &HashMap<String, String>,
    ) -> Result<ResolvedRoom, ConfigError> {
        self.resolve_inner(
            |reference| orgbot_providers::build_with_environment(reference, environment),
            true,
            Some(environment),
        )
    }

    #[cfg(test)]
    fn resolve_using(
        &self,
        build: impl Fn(&ProviderRef) -> orgbot_providers::Result<Arc<dyn Provider>>,
    ) -> Result<ResolvedRoom, ConfigError> {
        self.resolve_inner(build, false, None)
    }

    fn resolve_inner(
        &self,
        build: impl Fn(&ProviderRef) -> orgbot_providers::Result<Arc<dyn Provider>>,
        persist_cli_sessions: bool,
        environment: Option<&HashMap<String, String>>,
    ) -> Result<ResolvedRoom, ConfigError> {
        if self.bots.is_empty() {
            return Err(ConfigError::Invalid("a room needs at least one bot".into()));
        }
        let mut seen = HashMap::new();
        let mut bots = Vec::new();
        for section in &self.bots {
            if seen.insert(section.handle.clone(), ()).is_some() {
                return Err(ConfigError::Invalid(format!(
                    "duplicate bot handle `{}`",
                    section.handle
                )));
            }
            let autonomy = section.autonomy.resolve()?;
            let mut reference = section.provider.to_ref();
            if autonomy.review_only {
                if let ProviderRef::CliAgent { policy, .. } = &mut reference {
                    *policy = AgentPolicy::ReadOnly;
                }
            }
            let provider = if persist_cli_sessions {
                match &reference {
                    ProviderRef::CliAgent {
                        cli,
                        model,
                        workdir,
                        policy,
                        mcp,
                        effort,
                    } => {
                        let session_store = self.room.state_path.as_ref().and_then(|path| {
                            path.parent().map(|parent| {
                                parent
                                    .join("cli-sessions")
                                    .join(format!("{}.json", session_file_key(&section.handle)))
                            })
                        });
                        let provider = orgbot_providers::AgentCliProvider::new(
                            *cli,
                            model.clone(),
                            workdir.clone(),
                            *policy,
                            *mcp,
                        )
                        .and_then(|provider| provider.with_effort(effort))
                        .map_err(|error| {
                            ConfigError::Invalid(format!("bot `{}`: {error}", section.handle))
                        })?;
                        let provider = match session_store {
                            Some(path) => provider.with_session_store(path),
                            None => provider,
                        };
                        #[cfg(target_os = "linux")]
                        let provider = {
                            let state = self.room.state_path.as_ref()
                                .and_then(|path| path.parent().map(|p| p.join("workspaces")))
                                .ok_or_else(|| ConfigError::Invalid("Linux CLI rooms need a private state_path for workspace protection".into()))?;
                            let agent = session_file_key(&section.handle);
                            let project = if workdir.trim().is_empty() {
                                orgbot_providers::workspace_guard::WorkspaceGuard::empty_project(&state, &agent)
                                    .map_err(|error| ConfigError::Invalid(format!("bot `{}`: {error}", section.handle)))?
                            } else {
                                std::path::PathBuf::from(workdir)
                            };
                            provider.with_workspace_guard_project(state, agent, project)
                        };
                        Ok(Arc::new(provider) as Arc<dyn Provider>)
                    }
                    _ => build(&reference),
                }
            } else {
                build(&reference)
            }
            .map_err(|error| ConfigError::Invalid(format!("bot `{}`: {error}", section.handle)))?;
            let persona = if section.persona.trim().is_empty() {
                ORGBOT_RUNTIME_INSTRUCTIONS.to_string()
            } else {
                format!(
                    "{}\n\n{}",
                    section.persona.trim(),
                    ORGBOT_RUNTIME_INSTRUCTIONS
                )
            };
            let spec = AgentSpec {
                display_name: section
                    .display_name
                    .clone()
                    .unwrap_or_else(|| section.handle.clone()),
                roles: section.roles.clone(),
                persona,
                tools: Vec::new(),
                autonomy,
                ..AgentSpec::new(section.handle.clone(), section.handle.clone(), reference)
            };
            bots.push(ResolvedBot { spec, provider });
        }

        let tasks = self
            .tasks
            .iter()
            .map(|section| ResolvedTask {
                title: section.title.clone(),
                brief: section.brief.clone(),
                acceptance_criteria: section.acceptance_criteria.clone(),
                required_role: section.required_role.clone(),
                priority: section.priority.into(),
                open: section.open,
            })
            .collect();

        let spec = RoomSpec {
            name: self.room.name.clone(),
            goal: self.room.goal.clone(),
            roster: bots.iter().map(|b| b.spec.id).collect(),
            completion: self.room.completion.into(),
            limits: self.room.limits.clone().unwrap_or_default(),
        };

        Ok(ResolvedRoom {
            allowances: orgbot_runtime::allowance::Allowances::from_environment(environment)
                .map_err(ConfigError::Invalid)?,
            memory_projects: {
                let raw = environment
                    .and_then(|env| env.get("ORGBOT_MEMORY_PROJECTS").cloned())
                    .or_else(|| std::env::var("ORGBOT_MEMORY_PROJECTS").ok());
                match raw {
                    Some(raw) if raw.len() <= 65536 => serde_json::from_str::<Vec<String>>(&raw)
                        .map_err(|_| {
                            ConfigError::Invalid("Memory project list is invalid".into())
                        })?,
                    Some(_) => {
                        return Err(ConfigError::Invalid(
                            "Memory project list is too large".into(),
                        ))
                    }
                    None => Vec::new(),
                }
            },
            project_id: self.room.project_id.clone(),
            memory_root: environment
                .and_then(|env| env.get("ORGBOT_MEMORY_ROOT").map(std::path::PathBuf::from))
                .or_else(|| std::env::var_os("ORGBOT_MEMORY_ROOT").map(std::path::PathBuf::from)),
            roles: RoleTable::new(self.roles.clone()),
            skills: self.load_skills(),
            mcp: resolve_mcp_environment(&self.mcp, environment)?,
            prior_boards: self.prior_boards.clone(),
            spec,
            bots,
            tasks,
            state_path: self.room.state_path.clone(),
            workspace: self.workspace.clone(),
        })
    }
}

fn session_file_key(handle: &str) -> String {
    handle.bytes().map(|byte| format!("{byte:02x}")).collect()
}

impl RoomConfig {
    /// Read every skill the declared sources actually hold.
    ///
    /// A missing path is skipped rather than refused. Room files travel between
    /// machines that do not have the same folders, and a room that will not
    /// open because a skills directory is absent is worse than one that opens
    /// with fewer playbooks — the second is obvious from the index, the first
    /// looks like the room is broken.
    fn load_skills(&self) -> Vec<orgbot_proto::skill::Skill> {
        use orgbot_proto::skill::Skill;

        let mut found: Vec<Skill> = Vec::new();
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for source in &self.skills {
            let root = std::path::Path::new(&source.path);
            // Either the folder is a skill, or it holds one folder per skill.
            let candidates: Vec<std::path::PathBuf> = if root.join("SKILL.md").is_file() {
                vec![root.to_path_buf()]
            } else {
                let Ok(entries) = std::fs::read_dir(root) else {
                    continue;
                };
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| path.join("SKILL.md").is_file())
                    .collect()
            };
            for dir in candidates {
                let Ok(text) = std::fs::read_to_string(dir.join("SKILL.md")) else {
                    continue;
                };
                let fallback = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let Some(skill) = Skill::parse(&text, &fallback) else {
                    continue;
                };
                // First source wins, so a room can put its own ahead of a
                // machine-wide set without having to remove anything.
                if seen.insert(skill.name.to_ascii_lowercase()) {
                    found.push(skill);
                }
            }
        }
        found.sort_by(|a, b| a.name.cmp(&b.name));
        found
    }
}

/// The unused-import guard: [`TaskId`] is part of the public seed-task story
/// even though this module does not mint one itself.
#[allow(dead_code)]
type SeedTaskId = TaskId;

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        [room]
        name = "release"
        goal = "ship the release notes"
        completion = "all_tasks_terminal"

        [[bots]]
        handle = "lead"
        roles = ["coordinator"]
        autonomy = "coordinator"
        provider = { kind = "scripted", script = "planning" }

        [[bots]]
        handle = "w1"
        display_name = "Worker One"
        roles = ["writer"]
        provider = { kind = "scripted", script = "working" }

        [[bots]]
        handle = "qa"
        roles = ["reviewer"]
        autonomy = "inspector"
        provider = { kind = "scripted", script = "checking" }

        [[tasks]]
        title = "write the notes"
        brief = "summarise since v1.2"
        acceptance_criteria = ["covers every PR"]
        required_role = "writer"
        priority = "high"
    "#;

    #[test]
    fn a_sample_room_parses_and_resolves() {
        let config = RoomConfig::parse(SAMPLE, "sample.toml").unwrap();
        let resolved = config.resolve().unwrap();

        assert_eq!(resolved.spec.name, "release");
        assert_eq!(resolved.bots.len(), 3);
        assert_eq!(resolved.spec.roster.len(), 3);

        let lead = &resolved.bots[0];
        assert_eq!(lead.spec.handle, "lead");
        assert!(lead.spec.autonomy.may_create_tasks);
        assert_eq!(resolved.bots[1].spec.display_name, "Worker One");
        assert!(resolved.bots[2].spec.autonomy.may_inspect);

        assert_eq!(resolved.tasks.len(), 1);
        assert_eq!(resolved.tasks[0].priority, Priority::High);
        assert!(resolved.tasks[0].open);
    }

    #[test]
    fn provider_defaults_fill_in_a_model() {
        let config = RoomConfig::parse(
            r#"
            [room]
            name = "r"
            goal = "g"
            [[bots]]
            handle = "a"
            provider = { kind = "anthropic" }
            "#,
            "t.toml",
        )
        .unwrap();
        match &config.bots[0].provider {
            ProviderConfig::Anthropic { model } => {
                assert_eq!(model, orgbot_providers::anthropic::DEFAULT_MODEL)
            }
            other => panic!("expected anthropic, got {other:?}"),
        }
    }

    #[test]
    fn a_duplicate_handle_is_rejected() {
        let config = RoomConfig::parse(
            r#"
            [room]
            name = "r"
            goal = "g"
            [[bots]]
            handle = "a"
            provider = { kind = "scripted" }
            [[bots]]
            handle = "a"
            provider = { kind = "scripted" }
            "#,
            "t.toml",
        )
        .unwrap();
        assert!(matches!(config.resolve(), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn a_room_with_no_bots_is_rejected() {
        let config = RoomConfig::parse(
            r#"
            [room]
            name = "r"
            goal = "g"
            "#,
            "t.toml",
        )
        .unwrap();
        assert!(matches!(config.resolve(), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn an_unknown_autonomy_preset_names_the_valid_ones() {
        let config = RoomConfig::parse(
            r#"
            [room]
            name = "r"
            goal = "g"
            [[bots]]
            handle = "a"
            autonomy = "wizard"
            provider = { kind = "scripted" }
            "#,
            "t.toml",
        )
        .unwrap();
        let error = config.resolve().unwrap_err();
        assert!(error.to_string().contains("coordinator"));
    }

    #[test]
    fn autonomy_can_be_an_explicit_table() {
        let config = RoomConfig::parse(
            r#"
            [room]
            name = "r"
            goal = "g"
            [[bots]]
            handle = "a"
            provider = { kind = "scripted" }
            autonomy = { max_turns_per_task = 8, max_open_handoffs = 2, may_inspect = true, may_submit = false, may_create_tasks = false }
            "#,
            "t.toml",
        )
        .unwrap();
        let resolved = config.resolve().unwrap();
        assert_eq!(resolved.bots[0].spec.autonomy.max_turns_per_task, 8);
        assert!(resolved.bots[0].spec.autonomy.may_inspect);
    }

    #[test]
    fn opencode_and_antigravity_room_configs_preserve_provider_identity() {
        for (name, cli, backend) in [
            ("opencode", CliKind::OpenCode, "cli:opencode"),
            ("antigravity", CliKind::Antigravity, "cli:agy"),
            ("agy", CliKind::Antigravity, "cli:agy"),
        ] {
            let config = RoomConfig::parse(
                &format!(
                    r#"
                    [room]
                    name = "CLI fixture"
                    goal = "Check provider routing"
                    [[bots]]
                    handle = "worker"
                    provider = {{ kind = "cli_agent", cli = "{name}", model = "", workdir = ".", mcp = true, policy = "workspace_write" }}
                    "#
                ),
                "cli-fixture.toml",
            )
            .unwrap();
            let reference = config.bots[0].provider.to_ref();
            assert_eq!(reference.backend(), backend);
            assert!(matches!(reference, ProviderRef::CliAgent {
                cli: parsed, mcp: true, policy: AgentPolicy::WorkspaceWrite, ..
            } if parsed == cli));
        }
    }

    #[test]
    fn a_cli_agent_provider_parses_and_maps_to_a_cli_agent_ref() {
        let config = RoomConfig::parse(
            r#"
            [room]
            name = "r"
            goal = "g"
            [[bots]]
            handle = "coder"
            provider = { kind = "cli", cli = "codex", model = "gpt-5.2-codex", workdir = ".", policy = "workspace_write" }
            "#,
            "t.toml",
        )
        .unwrap();
        match &config.bots[0].provider {
            ProviderConfig::CliAgent {
                cli,
                model,
                workdir,
                policy,
                mcp,
                effort,
            } => {
                assert_eq!(*cli, CliKind::Codex);
                assert_eq!(model, "gpt-5.2-codex");
                assert_eq!(workdir, ".");
                assert_eq!(*policy, AgentPolicy::WorkspaceWrite);
                assert!(!mcp);
                assert!(effort.is_empty(), "no effort keeps the CLI default");
            }
            other => panic!("expected cli_agent, got {other:?}"),
        }
        let reference = config.bots[0].provider.to_ref();
        match reference {
            ProviderRef::CliAgent {
                cli, policy, mcp, ..
            } => {
                assert_eq!(cli, CliKind::Codex);
                assert_eq!(policy, AgentPolicy::WorkspaceWrite);
                assert!(!mcp);
            }
            other => panic!("expected CliAgent ref, got {other:?}"),
        }
    }

    #[test]
    fn a_cli_bot_effort_level_is_read_and_checked_against_the_cli() {
        let room = |cli: &str, effort: &str| {
            RoomConfig::parse(
                &format!(
                    r#"
                    [room]
                    name = "r"
                    goal = "g"
                    [[bots]]
                    handle = "worker"
                    provider = {{ kind = "cli", cli = "{cli}", effort = "{effort}" }}
                    "#
                ),
                "t.toml",
            )
            .unwrap()
        };
        let config = room("claude", "max");
        assert!(matches!(
            config.bots[0].provider.to_ref(),
            ProviderRef::CliAgent { ref effort, .. } if effort == "max"
        ));
        // Codex offers no `max`. Seating is skipped when the CLI is not
        // installed here, so only check the refusal where it would run.
        if orgbot_providers::AgentCliProvider::available(CliKind::Codex) {
            let error = room("codex", "max").resolve().unwrap_err().to_string();
            assert!(error.contains("does not offer effort `max`"), "{error}");
        }
    }

    #[test]
    fn mcp_credentials_expand_only_from_the_supplied_room_environment() {
        let config = RoomConfig::parse(
            r#"
            [room]
            name = "private mcp"
            goal = "use a local tool"
            [[bots]]
            handle = "worker"
            persona = "Keep working."
            provider = { kind = "scripted" }
            [[mcp]]
            name = "private_tool"
            transport = "stdio"
            command = "${ORGBOT_TEST_MCP_COMMAND}"
            args = ["${ORGBOT_TEST_MCP_ARGUMENT}"]
            [mcp.env]
            TOKEN = "${ORGBOT_TEST_ROOM_MCP_TOKEN}"
            "#,
            "private-mcp.toml",
        )
        .unwrap();
        let environment = HashMap::from([
            (
                "ORGBOT_TEST_ROOM_MCP_TOKEN".to_string(),
                "private-token-value".to_string(),
            ),
            (
                "ORGBOT_TEST_MCP_COMMAND".to_string(),
                "private-command-value".to_string(),
            ),
            (
                "ORGBOT_TEST_MCP_ARGUMENT".to_string(),
                "private-argument-value".to_string(),
            ),
        ]);
        let resolved = config.resolve_with_environment(&environment).unwrap();
        let orgbot_runtime::mcp_client::McpTransport::Stdio { command, args, env } =
            &resolved.mcp[0].transport
        else {
            panic!("expected local stdio transport");
        };
        assert_eq!(
            env.get("TOKEN").map(String::as_str),
            Some("private-token-value")
        );
        assert_eq!(command, "private-command-value");
        assert_eq!(args, &["private-argument-value"]);
        assert!(resolved.bots[0]
            .spec
            .persona
            .contains("reviewer assigned by OrgBots"));
        assert!(resolved.bots[0]
            .spec
            .persona
            .contains("nested Luna worker limits"));
    }

    #[test]
    fn missing_room_mcp_environment_values_fail_without_echoing_values() {
        let config = RoomConfig::parse(
            r#"
            [room]
            name = "private mcp"
            goal = "use a local tool"
            [[bots]]
            handle = "worker"
            provider = { kind = "scripted" }
            [[mcp]]
            name = "private_tool"
            transport = "http"
            url = "https://example.invalid/mcp"
            [mcp.headers]
            Authorization = "Bearer ${ORGBOT_MISSING_MCP_TOKEN}"
            "#,
            "private-mcp.toml",
        )
        .unwrap();
        let error = config
            .resolve_with_environment(&HashMap::new())
            .unwrap_err()
            .to_string();
        assert!(error.contains("ORGBOT_MISSING_MCP_TOKEN"));
        assert!(!error.contains("Bearer "));
    }

    #[test]
    fn review_only_forces_read_only_before_building_cli_provider() {
        for policy in ["workspace_write", "full_access"] {
            let config = RoomConfig::parse(
                &format!(
                    r#"
                [room]
                name = "review"
                goal = "inspect"
                [[bots]]
                handle = "astra"
                autonomy = {{ review_only = true, may_inspect = true }}
                provider = {{ kind = "cli_agent", cli = "codex", policy = "{policy}" }}
                "#
                ),
                "test.toml",
            )
            .unwrap();
            let resolved = config
                .resolve_using(|reference| {
                    assert!(matches!(
                        reference,
                        ProviderRef::CliAgent {
                            policy: AgentPolicy::ReadOnly,
                            ..
                        }
                    ));
                    orgbot_providers::build(&ProviderRef::Scripted {
                        script: String::new(),
                    })
                })
                .unwrap();
            assert!(matches!(
                resolved.bots[0].spec.provider,
                ProviderRef::CliAgent {
                    policy: AgentPolicy::ReadOnly,
                    ..
                }
            ));
        }
    }
}

#[cfg(test)]
mod home_tests {
    use super::*;

    /// The failure this guards: a room whose project folder was typed with a
    /// tilde spawned every CLI turn against a directory that does not exist,
    /// so ten bots sat "ready" and nothing ever happened.
    #[test]
    fn a_leading_tilde_is_expanded() {
        let home = std::env::var("HOME").expect("a home directory");
        assert_eq!(expand_home("~"), home);
        assert_eq!(
            expand_home("~/projects/High"),
            format!("{home}/projects/High")
        );
    }

    #[test]
    fn an_absolute_or_relative_path_is_left_alone() {
        assert_eq!(expand_home("/tmp/work"), "/tmp/work");
        assert_eq!(expand_home("."), ".");
        assert_eq!(expand_home(""), "");
    }

    /// A tilde inside a path is part of the name, not a home reference.
    #[test]
    fn a_tilde_that_is_not_leading_is_untouched() {
        assert_eq!(expand_home("/tmp/~backup"), "/tmp/~backup");
        assert_eq!(expand_home("./~odd"), "./~odd");
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        let home = std::env::var("HOME").expect("a home directory");
        assert_eq!(expand_home("  ~/work  "), format!("{home}/work"));
    }
}
