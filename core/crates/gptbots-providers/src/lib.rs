//! # gptbots-providers
//!
//! Model backends, normalised.
//!
//! Every backend is presented through one [`Provider`] trait speaking one
//! vocabulary ([`types`]), so the agent loop above never branches on which
//! model a bot happens to be configured with. Mixing an expensive planner, a
//! cheap inspector, and a scripted rule-based role in the same room is a
//! configuration choice, not a code path.
//!
//! Supported backends:
//!
//! | Backend | Adapter | Credential |
//! |---|---|---|
//! | Anthropic Messages API | [`anthropic::AnthropicProvider`] | `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, or an `ant auth login` profile |
//! | DeepSeek | [`openai::OpenAiProvider::deepseek`] | `DEEPSEEK_API_KEY` |
//! | OpenAI / Codex / local | [`openai::OpenAiProvider`] | `OPENAI_API_KEY` or `~/.codex/auth.json` |
//! | Codex CLI session | [`agent_cli::AgentCliProvider`] with [`CliKind::Codex`] | `codex login` on this machine |
//! | Claude Code session | [`agent_cli::AgentCliProvider`] with [`CliKind::Claude`] | Claude Code sign-in |
//! | Grok CLI session | [`agent_cli::AgentCliProvider`] with [`CliKind::Grok`] | Grok CLI sign-in |
//! | None (deterministic) | [`scripted::ScriptedProvider`] | — |

#![forbid(unsafe_code)]
// See the note in gptbots-proto: `missing_docs` also fires on self-describing
// fields inside serde data-carrier variants, which are documented one level up.
#![warn(missing_debug_implementations, rust_2018_idioms, unreachable_pub)]

pub mod agent_cli;
pub mod anthropic;
pub mod auth;
pub mod error;
pub mod openai;
pub mod provider;
pub mod scripted;
pub mod sse;
pub mod types;

use std::sync::Arc;

use gptbots_proto::agent::ProviderRef;

pub use agent_cli::{locate, AgentCliProvider};
pub use anthropic::AnthropicProvider;
pub use auth::Credential;
pub use error::{ErrorReport, ProviderError, Result};
pub use openai::OpenAiProvider;
pub use provider::Provider;
pub use scripted::ScriptedProvider;
pub use types::{
    CompletionRequest, CompletionResponse, ContentBlock, Effort, Role, StopReason, StreamEvent,
    ToolDef, Turn, Usage,
};

/// Build a live provider for a bot's configured backend.
///
/// `ProviderRef` names the backend and model but carries no credentials; those
/// are resolved here from the environment and the local CLI sign-ins, so a room
/// definition can be checked into a repository without secrets in it.
pub fn build(reference: &ProviderRef) -> Result<Arc<dyn Provider>> {
    Ok(match reference {
        ProviderRef::Anthropic { .. } => Arc::new(AnthropicProvider::from_env()?),
        ProviderRef::DeepSeek { .. } => Arc::new(OpenAiProvider::deepseek()?),
        ProviderRef::OpenAiCompatible { profile, .. } => match profile.as_str() {
            "codex" | "chatgpt" => Arc::new(OpenAiProvider::codex()?),
            "openai" => Arc::new(OpenAiProvider::openai()?),
            other => {
                // A custom profile needs its endpoint supplied out of band.
                let base = std::env::var(format!(
                    "GPTBOTS_{}_BASE_URL",
                    other.to_uppercase().replace('-', "_")
                ))
                .map_err(|_| {
                    ProviderError::Unsupported(format!(
                        "provider profile `{other}` needs GPTBOTS_{}_BASE_URL set",
                        other.to_uppercase().replace('-', "_")
                    ))
                })?;
                let credential = auth::openai_compatible(
                    other,
                    &format!("GPTBOTS_{}_API_KEY", other.to_uppercase().replace('-', "_")),
                )?;
                Arc::new(OpenAiProvider::custom(other, base, credential))
            }
        },
        ProviderRef::Scripted { script } => Arc::new(ScriptedProvider::always_says(script.clone())),
        ProviderRef::CliAgent {
            cli,
            model,
            workdir,
            policy,
            mcp,
        } => Arc::new(AgentCliProvider::new(
            *cli,
            model.clone(),
            workdir.clone(),
            *policy,
            *mcp,
        )?),
    })
}

/// The model a bot uses when none is configured.
pub const DEFAULT_MODEL: &str = anthropic::DEFAULT_MODEL;
