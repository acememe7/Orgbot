//! Bot definitions: who is in the room and what they are allowed to do.

use serde::{Deserialize, Serialize};

use crate::ids::AgentId;
use crate::time::Timestamp;

/// Which backend answers for a bot.
///
/// Providers are per-bot rather than per-room so a single group chat can mix
/// an expensive planner with cheap workers and a cheap inspector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ProviderRef {
    /// Anthropic Messages API.
    Anthropic { model: String },
    /// DeepSeek's OpenAI-compatible endpoint.
    DeepSeek { model: String },
    /// Any OpenAI-compatible `/v1/chat/completions` endpoint, including a
    /// ChatGPT/Codex session and local servers.
    OpenAiCompatible {
        model: String,
        /// Names a `[providers.<profile>]` block in the engine config, which
        /// carries the base URL and credentials.
        profile: String,
    },
    /// A deterministic in-process bot with no model behind it. Used for
    /// scripted tests and for rule-based roles such as a schema validator.
    Scripted { script: String },
}

impl ProviderRef {
    /// The model identifier, for display and logging.
    pub fn model(&self) -> &str {
        match self {
            Self::Anthropic { model }
            | Self::DeepSeek { model }
            | Self::OpenAiCompatible { model, .. } => model,
            Self::Scripted { script } => script,
        }
    }
}

/// Limits on what a bot may do without a human in the loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Autonomy {
    /// Model turns a bot may take on one task before it must report back.
    pub max_turns_per_task: u32,
    /// Handoffs a bot may have in flight at once. Caps fan-out so one bot
    /// cannot flood the room.
    pub max_open_handoffs: u32,
    /// Whether the bot may approve another bot's work. Off for worker roles so
    /// that inspection is always performed by a dedicated inspector.
    pub may_inspect: bool,
    /// Whether the bot may perform the final submit.
    pub may_submit: bool,
    /// Whether the bot may create new tasks on the board.
    pub may_create_tasks: bool,
}

impl Default for Autonomy {
    fn default() -> Self {
        Self {
            max_turns_per_task: 24,
            max_open_handoffs: 4,
            may_inspect: false,
            may_submit: false,
            may_create_tasks: false,
        }
    }
}

impl Autonomy {
    /// Permissions for a bot whose job is inspecting other bots' work.
    pub fn inspector() -> Self {
        Self {
            may_inspect: true,
            ..Default::default()
        }
    }

    /// Permissions for a coordinator: it plans, splits, and delivers, but does
    /// not sign off on its own work.
    pub fn coordinator() -> Self {
        Self {
            max_open_handoffs: 16,
            may_create_tasks: true,
            may_submit: true,
            may_inspect: false,
            ..Default::default()
        }
    }
}

/// A bot's configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSpec {
    pub id: AgentId,
    /// Short handle used for `@mentions` in chat. Unique within a room.
    pub handle: String,
    /// Human-readable name shown in the UI.
    pub display_name: String,
    /// Roles this bot holds. Task routing and role-addressed handoffs match
    /// against these.
    pub roles: Vec<String>,
    /// The bot's standing instructions, prepended to every request.
    pub persona: String,
    pub provider: ProviderRef,
    /// Names of tools this bot may call. An empty list means no tools.
    pub tools: Vec<String>,
    pub autonomy: Autonomy,
    pub created_at: Timestamp,
}

impl AgentSpec {
    /// A bot with default autonomy and no tools.
    pub fn new(
        handle: impl Into<String>,
        display_name: impl Into<String>,
        provider: ProviderRef,
    ) -> Self {
        Self {
            id: AgentId::new(),
            handle: handle.into(),
            display_name: display_name.into(),
            roles: Vec::new(),
            persona: String::new(),
            provider,
            tools: Vec::new(),
            autonomy: Autonomy::default(),
            created_at: Timestamp::now(),
        }
    }

    /// Whether this bot holds `role`.
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|held| held == role)
    }

    /// Whether this bot may claim a task requiring `required_role`.
    pub fn can_take(&self, required_role: Option<&str>) -> bool {
        match required_role {
            None => true,
            Some(role) => self.has_role(role),
        }
    }

    /// Builder: give the bot a set of roles.
    pub fn with_roles<I, S>(mut self, roles: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.roles = roles.into_iter().map(Into::into).collect();
        self
    }

    /// Builder: set the standing instructions.
    pub fn with_persona(mut self, persona: impl Into<String>) -> Self {
        self.persona = persona.into();
        self
    }

    /// Builder: set the tool allowlist.
    pub fn with_tools<I, S>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.tools = tools.into_iter().map(Into::into).collect();
        self
    }

    /// Builder: set autonomy limits.
    pub fn with_autonomy(mut self, autonomy: Autonomy) -> Self {
        self.autonomy = autonomy;
        self
    }
}

/// Whether a bot is currently able to take work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Connected and free.
    Idle,
    /// Running a model turn or a tool call.
    Working,
    /// Holding a task that is blocked on someone else.
    Waiting,
    /// Not accepting work.
    Offline,
    /// Halted after repeated failures; needs a human to resume it.
    Faulted,
}

impl AgentStatus {
    /// Whether the scheduler may assign new work to a bot in this state.
    pub const fn accepts_work(self) -> bool {
        matches!(self, Self::Idle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> AgentSpec {
        AgentSpec::new(
            "coder",
            "Coder",
            ProviderRef::Anthropic {
                model: "claude-opus-5".into(),
            },
        )
        .with_roles(["engineer", "writer"])
    }

    #[test]
    fn role_matching_gates_task_claims() {
        let bot = spec();
        assert!(bot.can_take(None));
        assert!(bot.can_take(Some("engineer")));
        assert!(!bot.can_take(Some("reviewer")));
    }

    #[test]
    fn workers_cannot_inspect_or_submit_by_default() {
        let a = Autonomy::default();
        assert!(!a.may_inspect);
        assert!(!a.may_submit);
    }

    #[test]
    fn inspectors_may_inspect_but_not_submit() {
        let a = Autonomy::inspector();
        assert!(a.may_inspect);
        assert!(!a.may_submit, "inspection and delivery stay separate duties");
    }

    #[test]
    fn coordinators_may_submit_but_not_inspect() {
        let a = Autonomy::coordinator();
        assert!(a.may_submit);
        assert!(a.may_create_tasks);
        assert!(!a.may_inspect, "a coordinator must not sign off on its own plan");
    }

    #[test]
    fn only_idle_bots_accept_work() {
        assert!(AgentStatus::Idle.accepts_work());
        for busy in [
            AgentStatus::Working,
            AgentStatus::Waiting,
            AgentStatus::Offline,
            AgentStatus::Faulted,
        ] {
            assert!(!busy.accepts_work(), "{busy:?}");
        }
    }

    #[test]
    fn provider_refs_survive_a_json_round_trip() {
        let provider = ProviderRef::OpenAiCompatible {
            model: "gpt-5-codex".into(),
            profile: "codex".into(),
        };
        let json = serde_json::to_string(&provider).unwrap();
        assert_eq!(serde_json::from_str::<ProviderRef>(&json).unwrap(), provider);
        assert_eq!(provider.model(), "gpt-5-codex");
    }
}
