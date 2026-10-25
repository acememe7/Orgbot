//! Live tests that call the subscription APIs directly, with no CLI involved.
//!
//! This is the path a phone takes: the sign-in is an OAuth token and the
//! engine speaks to the provider itself. These are gated behind
//! ORGBOT_TEST_LIVE_APIS=1 because they spend real tokens. Run them with:
//!
//!   ORGBOT_TEST_LIVE_APIS=1 cargo test -p orgbot-providers --test live_api -- --nocapture
//!
//! A provider with no sign-in on this machine skips rather than failing.

use orgbot_providers::types::{CompletionRequest, Turn};
use orgbot_providers::{CodexProvider, OpenAiProvider, Provider};

fn live_enabled() -> bool {
    std::env::var_os("ORGBOT_TEST_LIVE_APIS").is_some()
}

fn request(model: &str) -> CompletionRequest {
    let mut request = CompletionRequest::new(
        model.to_string(),
        "You are a terse assistant in a test harness.".to_string(),
        vec![Turn::user("Reply with the single word: ready")],
    );
    request.max_tokens = 2048;
    request
}

/// A ChatGPT subscription answering on the Responses dialect.
#[tokio::test]
async fn a_chatgpt_subscription_completes_a_turn() {
    if !live_enabled() {
        eprintln!("skipped (set ORGBOT_TEST_LIVE_APIS=1)");
        return;
    }
    let Ok(provider) = CodexProvider::from_env() else {
        eprintln!("skipped (no ChatGPT sign-in on this machine)");
        return;
    };
    let model = std::env::var("ORGBOT_TEST_CODEX_MODEL").unwrap_or_else(|_| "gpt-5.6-sol".into());
    let response = provider
        .complete(request(&model), None)
        .await
        .expect("the ChatGPT backend should answer a signed-in session");
    let text = response.text().to_lowercase();
    assert!(text.contains("ready"), "got {text:?}");
}

/// A Grok subscription answering on the xAI API.
#[tokio::test]
async fn a_grok_subscription_completes_a_turn() {
    if !live_enabled() {
        eprintln!("skipped (set ORGBOT_TEST_LIVE_APIS=1)");
        return;
    }
    let Ok(provider) = OpenAiProvider::grok() else {
        eprintln!("skipped (no Grok sign-in on this machine)");
        return;
    };
    let model = std::env::var("ORGBOT_TEST_GROK_MODEL").unwrap_or_else(|_| "grok-4.6".into());
    let response = provider
        .complete(request(&model), None)
        .await
        .expect("the xAI API should answer a signed-in session");
    let text = response.text().to_lowercase();
    assert!(text.contains("ready"), "got {text:?}");
}

/// A saved API key through the same streaming adapter used by desktop rooms.
#[tokio::test]
async fn deepseek_api_completes_a_bounded_turn() {
    if !live_enabled() { return; }
    let provider = OpenAiProvider::deepseek().expect("DeepSeek key must be supplied for this selected live check");
    let model = std::env::var("ORGBOT_TEST_DEEPSEEK_MODEL").expect("select a model returned by the provider catalog");
    let mut req = request(&model);
    req.max_tokens = 512;
    let response = tokio::time::timeout(std::time::Duration::from_secs(60), provider.complete(req, None))
        .await.expect("bounded live request timed out").expect("DeepSeek request failed");
    assert!(response.text().to_lowercase().contains("ready"));
    assert!(!response.usage.is_empty(), "usage must be reported");
    println!("DeepSeek live adapter: model={}, input={}, output={}", response.model, response.usage.input_tokens, response.usage.output_tokens);
}
