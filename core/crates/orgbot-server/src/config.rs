//! Room definitions, as TOML.
//!
//! A room is described by a file a human can read and check into a repository:
//! the goal, the roster, each bot's provider and autonomy, and optionally a
//! starting board. Credentials are never in here — they are resolved at runtime
//! from the environment and CLI sign-ins (see [`orgbot_providers::auth`]) — so
//! a room file is safe to share.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use orgbot_proto::agent::{AgentPolicy, Autonomy, CliKind, ProviderRef};
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
}

/// The `[room]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomSection {
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
    /// Drive an installed agent CLI (codex, claude, or grok) as a real,
    /// resumable session in a project folder.
    #[serde(
        alias = "cli",
        alias = "agent",
        alias = "codex_cli",
        alias = "claude_cli",
        alias = "grok_cli"
    )]
    CliAgent {
        /// Which CLI: `codex`, `claude`, or `grok`.
        cli: CliKind,
        /// Model id, or empty for the CLI's default.
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
            } => ProviderRef::CliAgent {
                cli: *cli,
                model: model.clone(),
                // A tilde here is a literal directory to Command::current_dir,
                // so expand it or every turn fails to spawn.
                workdir: expand_home(workdir),
                policy: *policy,
                mcp: *mcp,
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
    pub state_path: Option<std::path::PathBuf>,
    /// The room spec.
    pub spec: RoomSpec,
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
        self.resolve_using(orgbot_providers::build)
    }

    pub fn resolve_with_environment(
        &self,
        environment: &HashMap<String, String>,
    ) -> Result<ResolvedRoom, ConfigError> {
        self.resolve_using(|reference| {
            orgbot_providers::build_with_environment(reference, environment)
        })
    }

    fn resolve_using(
        &self,
        build: impl Fn(&ProviderRef) -> orgbot_providers::Result<Arc<dyn Provider>>,
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
            let reference = section.provider.to_ref();
            let provider = build(&reference).map_err(|error| {
                ConfigError::Invalid(format!("bot `{}`: {error}", section.handle))
            })?;
            let spec = AgentSpec {
                display_name: section
                    .display_name
                    .clone()
                    .unwrap_or_else(|| section.handle.clone()),
                roles: section.roles.clone(),
                persona: section.persona.clone(),
                tools: Vec::new(),
                autonomy: section.autonomy.resolve()?,
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
            spec,
            bots,
            tasks,
            state_path: self.room.state_path.clone(),
        })
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
            } => {
                assert_eq!(*cli, CliKind::Codex);
                assert_eq!(model, "gpt-5.2-codex");
                assert_eq!(workdir, ".");
                assert_eq!(*policy, AgentPolicy::WorkspaceWrite);
                assert!(!mcp);
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
