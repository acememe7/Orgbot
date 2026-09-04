//! Carry this machine's sign-ins to another device.
//!
//! A phone has no agent CLIs, so it cannot sign in the way the desktop does.
//! But the tokens those CLIs already hold are ordinary OAuth access tokens,
//! and the provider APIs accept them directly — so the phone does not need its
//! own OAuth client, it needs the sign-in you already have.
//!
//! This reads what is on disk and emits a bundle the app imports. Nothing is
//! sent anywhere: the bundle is printed, and moving it is the operator's
//! choice.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One provider's credential and the models it offers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedProvider {
    /// `anthropic`, `codex`, or `grok`.
    pub provider: String,
    /// The OAuth access token, or an API key.
    pub token: String,
    /// `bearer` for an OAuth token, `api_key` for a key.
    pub kind: String,
    /// The account the token belongs to, where one is recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// What the subscription is, for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Models this provider offers, carried over so a phone with no CLI can
    /// still show a real list rather than a guess.
    #[serde(default)]
    pub models: Vec<PairedModel>,
}

/// One model, as the provider names it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedModel {
    /// The id sent to the API.
    pub id: String,
    /// What a picker should show.
    pub label: String,
}

/// Everything being carried to the other device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingBundle {
    /// Bundle format version, so an older app can refuse politely.
    pub version: u32,
    /// When it was made, epoch millis.
    pub created_at: i64,
    /// The machine it came from, for display.
    pub source: String,
    /// The providers found.
    pub providers: Vec<PairedProvider>,
}

/// The bundle format this build writes.
pub const BUNDLE_VERSION: u32 = 1;

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn read_json(path: PathBuf) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Collect every sign-in this machine holds.
pub fn collect() -> PairingBundle {
    let mut providers = Vec::new();
    if let Some(entry) = anthropic() {
        providers.push(entry);
    }
    if let Some(entry) = codex() {
        providers.push(entry);
    }
    if let Some(entry) = grok() {
        providers.push(entry);
    }
    PairingBundle {
        version: BUNDLE_VERSION,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
        source: hostname(),
        providers,
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|name| name.trim().to_string())
        .unwrap_or_else(|_| "this machine".into())
}

/// The Claude subscription sign-in, which the Messages API accepts as a
/// bearer token alongside the oauth beta header.
fn anthropic() -> Option<PairedProvider> {
    let root = read_json(home()?.join(".claude/.credentials.json"))?;
    let oauth = root.get("claudeAiOauth")?;
    let token = oauth.get("accessToken")?.as_str()?.to_string();
    Some(PairedProvider {
        provider: "anthropic".into(),
        token,
        kind: "bearer".into(),
        account: None,
        plan: oauth
            .get("subscriptionType")
            .and_then(Value::as_str)
            .map(str::to_string),
        // Claude Code takes aliases, which follow whatever the plan serves.
        models: vec![
            PairedModel { id: "opus".into(), label: "Opus".into() },
            PairedModel { id: "sonnet".into(), label: "Sonnet".into() },
            PairedModel { id: "haiku".into(), label: "Haiku".into() },
        ],
    })
}

/// The ChatGPT sign-in the Codex CLI stores, plus its cached model list.
fn codex() -> Option<PairedProvider> {
    let root = read_json(home()?.join(".codex/auth.json"))?;
    let (token, kind) = match root.get("OPENAI_API_KEY").and_then(Value::as_str) {
        Some(key) if !key.trim().is_empty() => (key.to_string(), "api_key"),
        _ => (
            root.get("tokens")?
                .get("access_token")?
                .as_str()?
                .to_string(),
            "bearer",
        ),
    };
    Some(PairedProvider {
        provider: "codex".into(),
        token,
        kind: kind.into(),
        account: root
            .get("tokens")
            .and_then(|t| t.get("account_id"))
            .and_then(Value::as_str)
            .map(str::to_string),
        plan: root.get("auth_mode").and_then(Value::as_str).map(str::to_string),
        models: cached_models(home()?.join(".codex/models_cache.json")),
    })
}

/// The grok.com sign-in and its cached model list.
fn grok() -> Option<PairedProvider> {
    let root = read_json(home()?.join(".grok/auth.json"))?;
    // The file shape has moved around between versions; take the first
    // access token found rather than pinning one path.
    let token = find_token(&root)?;
    Some(PairedProvider {
        provider: "grok".into(),
        token,
        kind: "bearer".into(),
        account: find_str(&root, "email"),
        plan: find_str(&root, "auth_mode"),
        models: cached_models(home()?.join(".grok/models_cache.json")),
    })
}

/// Depth-first search for an access token, so a moved key still pairs.
fn find_token(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            // `key` is how the Grok CLI names its token, under an entry
            // keyed by the issuer URL. The others cover codex and claude.
            for key in ["access_token", "accessToken", "token", "key"] {
                if let Some(found) = map.get(key).and_then(Value::as_str) {
                    if !found.trim().is_empty() {
                        return Some(found.to_string());
                    }
                }
            }
            map.values().find_map(find_token)
        }
        _ => None,
    }
}

/// Depth-first search for a named string field.
fn find_str(value: &Value, name: &str) -> Option<String> {
    match value {
        Value::Object(map) => map
            .get(name)
            .and_then(Value::as_str)
            .filter(|found| !found.trim().is_empty())
            .map(str::to_string)
            .or_else(|| map.values().find_map(|child| find_str(child, name))),
        _ => None,
    }
}

/// Read a CLI's cached model list, if it keeps one.
fn cached_models(path: PathBuf) -> Vec<PairedModel> {
    let Some(root) = read_json(path) else {
        return Vec::new();
    };
    // codex keeps a list of model entries; grok keeps a map keyed by model id.
    // Folding the key in as `slug` lets both shapes read the same below.
    let entries: Vec<Value> = match root.get("models") {
        Some(Value::Array(list)) => list.clone(),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(id, entry)| match entry.clone() {
                Value::Object(mut fields) => {
                    fields
                        .entry("slug".to_string())
                        .or_insert_with(|| Value::String(id.clone()));
                    Value::Object(fields)
                }
                _ => serde_json::json!({ "slug": id }),
            })
            .collect(),
        _ => return Vec::new(),
    };
    entries
        .iter()
        .filter(|entry| entry.get("visibility").and_then(Value::as_str) != Some("hidden"))
        .filter_map(|entry| {
            let id = entry
                .get("slug")
                .or_else(|| entry.get("id"))
                .or_else(|| entry.get("name"))
                .and_then(Value::as_str)?;
            let label = entry
                .get("display_name")
                .and_then(Value::as_str)
                .unwrap_or(id);
            Some(PairedModel {
                id: id.to_string(),
                label: label.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_found_wherever_it_sits() {
        let nested = serde_json::json!({"tokens": {"access_token": "abc"}});
        assert_eq!(find_token(&nested).as_deref(), Some("abc"));

        let flat = serde_json::json!({"accessToken": "xyz"});
        assert_eq!(find_token(&flat).as_deref(), Some("xyz"));

        let deep = serde_json::json!({"a": {"b": {"c": {"token": "deep"}}}});
        assert_eq!(find_token(&deep).as_deref(), Some("deep"));
    }

    /// The Grok CLI names its token `key`, under an issuer-keyed entry.
    #[test]
    fn the_grok_shape_is_recognised() {
        let grok = serde_json::json!({
            "https://auth.x.ai::abc": {"key": "grok-token", "auth_mode": "sso"}
        });
        assert_eq!(find_token(&grok).as_deref(), Some("grok-token"));
        assert_eq!(find_str(&grok, "auth_mode").as_deref(), Some("sso"));
    }

    #[test]
    fn an_empty_token_is_not_accepted() {
        let blank = serde_json::json!({"access_token": "   "});
        assert_eq!(find_token(&blank), None);
    }

    #[test]
    fn a_file_with_no_token_yields_nothing() {
        assert_eq!(find_token(&serde_json::json!({"unrelated": 1})), None);
    }

    #[test]
    fn hidden_models_are_left_out_of_the_bundle() {
        let dir = std::env::temp_dir().join(format!("orgbot-pair-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("models.json");
        std::fs::write(
            &path,
            r#"{"models":[
                {"slug":"good","display_name":"Good"},
                {"slug":"secret","visibility":"hidden"}
            ]}"#,
        )
        .unwrap();
        let models = cached_models(path);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "good");
        assert_eq!(models[0].label, "Good");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// grok keys its cache by model id rather than listing entries.
    #[test]
    fn a_map_shaped_model_cache_is_read() {
        let dir =
            std::env::temp_dir().join(format!("orgbot-pair-map-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("models.json");
        std::fs::write(
            &path,
            r#"{"models":{"grok-4.6":{"display_name":"Grok 4.6"},"grok-4.5":{}}}"#,
        )
        .unwrap();
        let mut models = cached_models(path);
        models.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "grok-4.5");
        assert_eq!(models[1].label, "Grok 4.6");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A bundle carries secrets, so its shape must stay explicit.
    #[test]
    fn a_bundle_round_trips() {
        let bundle = PairingBundle {
            version: BUNDLE_VERSION,
            created_at: 1,
            source: "host".into(),
            providers: vec![PairedProvider {
                provider: "anthropic".into(),
                token: "t".into(),
                kind: "bearer".into(),
                account: None,
                plan: Some("max".into()),
                models: vec![PairedModel { id: "opus".into(), label: "Opus".into() }],
            }],
        };
        let json = serde_json::to_string(&bundle).unwrap();
        let back: PairingBundle = serde_json::from_str(&json).unwrap();
        assert_eq!(back.providers[0].provider, "anthropic");
        assert_eq!(back.providers[0].models[0].id, "opus");
    }
}
