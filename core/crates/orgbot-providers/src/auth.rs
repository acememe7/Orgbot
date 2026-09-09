//! Credential resolution.
//!
//! Credentials are read from the environment and from the local CLI credential
//! files that the user has already signed in with, so a room does not ask for
//! keys that are sitting on disk. Nothing here ever writes a credential to a
//! log: [`Credential`] has a hand-written `Debug` that prints the *kind* and
//! redacts the value, because these structs travel through `tracing` spans and
//! error messages.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::error::{ProviderError, Result};

/// How to authenticate to a backend.
#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    /// An API key sent in a provider-specific header.
    ApiKey(String),
    /// An OAuth access token sent as `Authorization: Bearer`.
    Bearer(String),
    /// A ChatGPT sign-in, as stored by the Codex CLI.
    ChatGptSession {
        /// The bearer token.
        access_token: String,
        /// The account the session belongs to.
        account_id: String,
    },
}

/// Redacts the secret. Credentials appear in tracing spans and error paths, so
/// the derived `Debug` would leak them into logs.
impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApiKey(_) => f.write_str("Credential::ApiKey(<redacted>)"),
            Self::Bearer(_) => f.write_str("Credential::Bearer(<redacted>)"),
            Self::ChatGptSession { account_id, .. } => write!(
                f,
                "Credential::ChatGptSession {{ account_id: {account_id:?}, access_token: <redacted> }}"
            ),
        }
    }
}

impl Credential {
    /// The secret value, for building a request header.
    pub fn secret(&self) -> &str {
        match self {
            Self::ApiKey(value) | Self::Bearer(value) => value,
            Self::ChatGptSession { access_token, .. } => access_token,
        }
    }

    /// Whether this is an OAuth-style token rather than a long-lived API key.
    ///
    /// Anthropic needs to know: an OAuth token goes on `Authorization: Bearer`
    /// with an extra beta header, not on `x-api-key`.
    pub const fn is_oauth(&self) -> bool {
        matches!(self, Self::Bearer(_) | Self::ChatGptSession { .. })
    }
}

/// Read a non-empty environment variable.
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Resolve an Anthropic credential.
///
/// Order matches the official SDKs: `ANTHROPIC_API_KEY`, then
/// `ANTHROPIC_AUTH_TOKEN`, then an OAuth profile written by `ant auth login`.
pub fn anthropic() -> Result<Credential> {
    if let Some(key) = env_var("ANTHROPIC_API_KEY") {
        return Ok(Credential::ApiKey(key));
    }
    if let Some(token) = env_var("ANTHROPIC_AUTH_TOKEN") {
        return Ok(Credential::Bearer(token));
    }
    if let Some(token) = anthropic_profile_token(&config_dir().join("anthropic")) {
        return Ok(Credential::Bearer(token));
    }
    Err(ProviderError::MissingCredential {
        provider: "anthropic".into(),
        detail: "set ANTHROPIC_API_KEY, or sign in with `ant auth login`".into(),
    })
}

/// Read an access token out of an `ant auth login` profile directory.
fn anthropic_profile_token(dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join("credentials.json")).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    // Either a flat token or one nested under the active profile.
    let direct = parsed.get("access_token").and_then(|v| v.as_str());
    let nested = parsed
        .get("profiles")
        .and_then(|p| p.as_object())
        .and_then(|profiles| {
            let active = parsed.get("active_profile").and_then(|v| v.as_str());
            active
                .and_then(|name| profiles.get(name))
                .or_else(|| profiles.values().next())
        })
        .and_then(|profile| profile.get("access_token"))
        .and_then(|v| v.as_str());
    direct.or(nested).map(str::to_string)
}

/// Resolve a DeepSeek credential from `DEEPSEEK_API_KEY`.
pub fn deepseek() -> Result<Credential> {
    env_var("DEEPSEEK_API_KEY")
        .map(Credential::ApiKey)
        .ok_or_else(|| ProviderError::MissingCredential {
            provider: "deepseek".into(),
            detail: "set DEEPSEEK_API_KEY".into(),
        })
}

/// Resolve a Grok credential.
///
/// API keys use the public API; OAuth sessions use the CLI chat proxy.
pub fn grok() -> Result<Credential> {
    if let Some(token) = env_var("GROK_AUTH_TOKEN") {
        return Ok(Credential::Bearer(token));
    }
    if let Some(key) = env_var("GROK_API_KEY").or_else(|| env_var("XAI_API_KEY")) {
        return Ok(Credential::ApiKey(key));
    }
    if let Some(token) = home_dir().and_then(|h| grok_token_from(&h.join(".grok/auth.json"))) {
        return Ok(Credential::Bearer(token));
    }
    Err(ProviderError::MissingCredential {
        provider: "grok".into(),
        detail: "set GROK_API_KEY, or sign in with `grok`".into(),
    })
}

/// Read the access token the Grok CLI stored.
///
/// The file is a map keyed by issuer, and the token is named `key`. That
/// layout has already moved once between versions, so this takes the first
/// entry carrying a token rather than pinning one path.
pub fn grok_token_from(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    parsed.as_object()?.values().find_map(|entry| {
        entry
            .get("key")
            .and_then(|v| v.as_str())
            .filter(|k| !k.trim().is_empty())
            .map(str::to_string)
    })
}

/// Resolve a credential for an OpenAI-compatible endpoint.
///
/// `env` names the variable to read first (e.g. `OPENAI_API_KEY`); the Codex
/// CLI's stored sign-in is the fallback, so a user who has run `codex login`
/// does not have to mint an API key as well.
pub fn openai_compatible(profile: &str, env: &str) -> Result<Credential> {
    if let Some(key) = env_var(env) {
        return Ok(Credential::ApiKey(key));
    }
    if let Ok(session) = codex_cli() {
        return Ok(session);
    }
    Err(ProviderError::MissingCredential {
        provider: profile.to_string(),
        detail: format!("set {env}, or sign in with `codex login`"),
    })
}

/// Read the credential the Codex CLI stored at `~/.codex/auth.json`.
///
/// The file holds either an API key (`OPENAI_API_KEY`) or a ChatGPT sign-in
/// under `tokens`. Both are supported; the API key is preferred when present
/// because it does not expire.
pub fn codex_cli() -> Result<Credential> {
    let path = home_dir()
        .ok_or_else(|| ProviderError::MissingCredential {
            provider: "codex".into(),
            detail: "no home directory".into(),
        })?
        .join(".codex/auth.json");
    codex_credential_from(&path)
}

/// Parse a Codex credential file. Split out so it is testable without a home
/// directory.
pub fn codex_credential_from(path: &Path) -> Result<Credential> {
    let raw = std::fs::read_to_string(path).map_err(|e| ProviderError::MissingCredential {
        provider: "codex".into(),
        detail: format!("could not read {}: {e}", path.display()),
    })?;
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| ProviderError::Decode {
            provider: "codex".into(),
            detail: format!("{} is not valid JSON: {e}", path.display()),
        })?;

    if let Some(key) = parsed
        .get("OPENAI_API_KEY")
        .and_then(|v| v.as_str())
        .filter(|k| !k.trim().is_empty())
    {
        return Ok(Credential::ApiKey(key.to_string()));
    }

    let tokens = parsed
        .get("tokens")
        .ok_or_else(|| ProviderError::MissingCredential {
            provider: "codex".into(),
            detail: format!("{} has neither OPENAI_API_KEY nor tokens", path.display()),
        })?;
    let access_token = tokens
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| ProviderError::MissingCredential {
            provider: "codex".into(),
            detail: "no access_token in the Codex sign-in; run `codex login`".into(),
        })?;
    let account_id = tokens
        .get("account_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    Ok(Credential::ChatGptSession {
        access_token: access_token.to_string(),
        account_id: account_id.to_string(),
    })
}

fn home_dir() -> Option<PathBuf> {
    env_var("HOME").map(PathBuf::from)
}

fn config_dir() -> PathBuf {
    env_var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The base URL for a backend, honouring the usual override variables.
pub fn base_url(provider: &str, default: &str) -> String {
    let override_var = match provider {
        "anthropic" => "ANTHROPIC_BASE_URL",
        "deepseek" => "DEEPSEEK_BASE_URL",
        "grok" | "xai" => "GROK_BASE_URL",
        "codex" | "chatgpt" => "CODEX_BASE_URL",
        _ => "OPENAI_BASE_URL",
    };
    env_var(override_var)
        .unwrap_or_else(|| default.to_string())
        .trim_end_matches('/')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(name: &str, contents: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("orgbot-auth-test-{}-{name}", std::process::id()));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        path
    }

    #[test]
    fn debug_never_prints_the_secret() {
        let rendered = format!("{:?}", Credential::ApiKey("sk-super-secret".into()));
        assert!(!rendered.contains("sk-super-secret"), "got {rendered}");
        assert!(rendered.contains("redacted"));

        let session = Credential::ChatGptSession {
            access_token: "tok-secret".into(),
            account_id: "acct_123".into(),
        };
        let rendered = format!("{session:?}");
        assert!(!rendered.contains("tok-secret"), "got {rendered}");
        assert!(
            rendered.contains("acct_123"),
            "the non-secret account id stays visible for debugging"
        );
    }

    #[test]
    fn a_codex_api_key_is_preferred_over_a_session() {
        let path = temp_file(
            "apikey.json",
            r#"{"OPENAI_API_KEY":"sk-abc","tokens":{"access_token":"tok","account_id":"a"}}"#,
        );
        assert_eq!(
            codex_credential_from(&path).unwrap(),
            Credential::ApiKey("sk-abc".into())
        );
        let _ = std::fs::remove_file(path);
    }

    /// Matches the shape the Codex CLI actually writes: a null API key with the
    /// real credential under `tokens`.
    #[test]
    fn a_chatgpt_sign_in_is_read_from_the_tokens_block() {
        let path = temp_file(
            "session.json",
            r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,
                "tokens":{"id_token":"i","access_token":"tok-xyz","refresh_token":"r","account_id":"acct_9"},
                "last_refresh":"2026-09-02T23:05:00Z"}"#,
        );
        let credential = codex_credential_from(&path).unwrap();
        assert_eq!(
            credential,
            Credential::ChatGptSession {
                access_token: "tok-xyz".into(),
                account_id: "acct_9".into(),
            }
        );
        assert!(credential.is_oauth());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_empty_api_key_falls_through_to_the_session() {
        let path = temp_file(
            "empty.json",
            r#"{"OPENAI_API_KEY":"   ","tokens":{"access_token":"tok","account_id":"a"}}"#,
        );
        assert!(matches!(
            codex_credential_from(&path).unwrap(),
            Credential::ChatGptSession { .. }
        ));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_missing_file_is_a_credential_error_not_a_panic() {
        let err = codex_credential_from(Path::new("/nonexistent/auth.json")).unwrap_err();
        assert!(matches!(err, ProviderError::MissingCredential { .. }));
    }

    #[test]
    fn malformed_json_is_reported_as_a_decode_error() {
        let path = temp_file("bad.json", "{not json");
        assert!(matches!(
            codex_credential_from(&path).unwrap_err(),
            ProviderError::Decode { .. }
        ));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn api_keys_are_not_treated_as_oauth() {
        assert!(!Credential::ApiKey("k".into()).is_oauth());
        assert!(Credential::Bearer("t".into()).is_oauth());
    }

    #[test]
    fn base_urls_have_their_trailing_slash_trimmed() {
        assert_eq!(
            base_url("unknown-provider", "https://example.test/v1/"),
            "https://example.test/v1"
        );
    }
}
