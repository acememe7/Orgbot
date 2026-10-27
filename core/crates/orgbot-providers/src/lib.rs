//! # orgbot-providers
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
//! | OpenCode CLI session | [`agent_cli::AgentCliProvider`] with [`CliKind::OpenCode`] | Selected provider's OpenCode sign-in; protected Linux runtime |
//! | Antigravity CLI configuration | [`agent_cli::AgentCliProvider`] with [`CliKind::Antigravity`] | Execution blocked pending isolated authentication |
//! | None (deterministic) | [`scripted::ScriptedProvider`] | — |

#![forbid(unsafe_code)]
// See the note in orgbot-proto: `missing_docs` also fires on self-describing
// fields inside serde data-carrier variants, which are documented one level up.
#![warn(missing_debug_implementations, rust_2018_idioms, unreachable_pub)]

pub mod agent_cli;
pub mod anthropic;
pub mod auth;
pub mod codex;
pub mod error;
pub mod openai;
pub mod provider;
pub mod scripted;
pub mod sse;
pub mod types;
#[cfg(target_os = "linux")]
pub mod workspace_guard;

use std::sync::Arc;

use orgbot_proto::agent::ProviderRef;

pub use agent_cli::{locate, AgentCliProvider};
pub use anthropic::AnthropicProvider;
pub use auth::Credential;
pub use codex::CodexProvider;
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
            // A ChatGPT sign-in speaks the Responses dialect on ChatGPT's own
            // backend, not chat completions on api.openai.com.
            "codex" | "chatgpt" => Arc::new(CodexProvider::from_env()?),
            "openai" => Arc::new(OpenAiProvider::openai()?),
            "grok" | "xai" => Arc::new(OpenAiProvider::grok()?),
            other => {
                // A custom profile needs its endpoint supplied out of band.
                let base = std::env::var(format!(
                    "ORGBOT_{}_BASE_URL",
                    other.to_uppercase().replace('-', "_")
                ))
                .map_err(|_| {
                    ProviderError::Unsupported(format!(
                        "provider profile `{other}` needs ORGBOT_{}_BASE_URL set",
                        other.to_uppercase().replace('-', "_")
                    ))
                })?;
                let credential = auth::openai_compatible(
                    other,
                    &format!("ORGBOT_{}_API_KEY", other.to_uppercase().replace('-', "_")),
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

/// Build API bots from this room's credentials without mutating the process environment.
pub fn build_with_environment(
    reference: &ProviderRef,
    environment: &std::collections::HashMap<String, String>,
) -> Result<Arc<dyn Provider>> {
    let get = |key: &str| {
        environment
            .get(key)
            .filter(|v| !v.trim().is_empty())
            .cloned()
            .ok_or_else(|| ProviderError::MissingCredential {
                provider: key.into(),
                detail: "sign in on this device before starting the room".into(),
            })
    };
    Ok(match reference {
        ProviderRef::Anthropic { .. } => Arc::new(AnthropicProvider::with_credential(
            if let Ok(key) = get("ANTHROPIC_API_KEY") {
                Credential::ApiKey(key)
            } else {
                Credential::Bearer(get("ANTHROPIC_AUTH_TOKEN")?)
            },
        )),
        ProviderRef::DeepSeek { .. } => Arc::new(OpenAiProvider::custom(
            "deepseek",
            "https://api.deepseek.com",
            Credential::ApiKey(get("DEEPSEEK_API_KEY")?),
        )),
        ProviderRef::OpenAiCompatible { profile, .. } => match profile.as_str() {
            "codex" | "chatgpt" => Arc::new(CodexProvider::new(
                codex::DEFAULT_BASE_URL,
                Credential::Bearer(get("CODEX_AUTH_TOKEN")?),
            )),
            "grok" | "xai" => Arc::new(OpenAiProvider::grok_with_credential(
                if let Ok(token) = get("GROK_AUTH_TOKEN") {
                    Credential::Bearer(token)
                } else {
                    Credential::ApiKey(get("GROK_API_KEY")?)
                },
            )),
            "openai" => Arc::new(OpenAiProvider::custom(
                "openai",
                "https://api.openai.com",
                Credential::ApiKey(get("OPENAI_API_KEY")?),
            )),
            name => {
                let prefix = format!("ORGBOT_{}", name.to_uppercase().replace('-', "_"));
                let base = get(&format!("{prefix}_BASE_URL"))?;
                if !base.starts_with("https://") {
                    return Err(ProviderError::Unsupported(
                        "device API endpoints require HTTPS".into(),
                    ));
                }
                Arc::new(OpenAiProvider::custom(
                    name,
                    base,
                    Credential::ApiKey(get(&format!("{prefix}_API_KEY"))?),
                ))
            }
        },
        ProviderRef::Scripted { script } => Arc::new(ScriptedProvider::always_says(script.clone())),
        ProviderRef::CliAgent { .. } => {
            return Err(ProviderError::Unsupported(
                "device rooms use API models; system CLIs belong to the Linux app".into(),
            ))
        }
    })
}
