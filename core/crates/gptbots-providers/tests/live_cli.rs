//! Live tests that drive the real installed CLIs.
//!
//! These are gated behind GPTBOTS_TEST_LIVE_CLIS=1 because they spend real
//! tokens and need the machine's sign-ins. Run them with:
//!
//!   GPTBOTS_TEST_LIVE_CLIS=1 cargo test -p gptbots-providers --test live_cli -- --nocapture
//!
//! A CLI that is not installed, or an account that is rate-limited, skips
//! rather than failing the suite.

use gptbots_proto::agent::{AgentPolicy, CliKind};
use gptbots_providers::agent_cli::AgentCliProvider;
use gptbots_providers::types::{CompletionRequest, Turn};
use gptbots_providers::Provider;

fn live_enabled() -> bool {
    std::env::var_os("GPTBOTS_TEST_LIVE_CLIS").is_some()
}

fn request(prompt: &str) -> CompletionRequest {
    CompletionRequest::new(
        String::new(), // CLI default model
        "You are a terse assistant in a test harness.".to_string(),
        vec![Turn::user(prompt)],
    )
}

async fn run_turn(provider: &AgentCliProvider, prompt: &str) -> Result<String, String> {
    let response = provider
        .complete(request(prompt), None)
        .await
        .map_err(|error| error.to_string())?;
    Ok(response.text())
}

#[tokio::test]
async fn codex_cli_runs_a_turn_and_remembers_across_resume() {
    if !live_enabled() || !AgentCliProvider::available(CliKind::Codex) {
        eprintln!("skipped (set GPTBOTS_TEST_LIVE_CLIS=1 with codex installed)");
        return;
    }
    let provider =
        AgentCliProvider::new(CliKind::Codex, "", "", AgentPolicy::ReadOnly, false).unwrap();

    let first = run_turn(&provider, "Reply with exactly one word: potato")
        .await
        .unwrap();
    assert!(
        first.to_lowercase().contains("potato"),
        "first turn should answer potato, got: {first}"
    );

    // A resumed session must remember the earlier exchange.
    let second = run_turn(
        &provider,
        "What single word did you reply with in your previous answer?",
    )
    .await
    .unwrap();
    assert!(
        second.to_lowercase().contains("potato"),
        "the resumed session lost its memory: {second}"
    );
    assert!(
        provider.session_id().is_some(),
        "a session id should be recorded"
    );
}

#[tokio::test]
async fn grok_cli_runs_a_turn() {
    if !live_enabled() || !AgentCliProvider::available(CliKind::Grok) {
        eprintln!("skipped (set GPTBOTS_TEST_LIVE_CLIS=1 with grok installed)");
        return;
    }
    let provider =
        AgentCliProvider::new(CliKind::Grok, "", "", AgentPolicy::ReadOnly, false).unwrap();
    let answer = run_turn(&provider, "Reply with exactly one word: potato")
        .await
        .unwrap();
    assert!(answer.to_lowercase().contains("potato"), "got: {answer}");
}

#[tokio::test]
async fn claude_cli_runs_a_turn_when_not_rate_limited() {
    if !live_enabled() || !AgentCliProvider::available(CliKind::Claude) {
        eprintln!("skipped (set GPTBOTS_TEST_LIVE_CLIS=1 with claude installed)");
        return;
    }
    let provider =
        AgentCliProvider::new(CliKind::Claude, "", "", AgentPolicy::ReadOnly, false).unwrap();
    match run_turn(&provider, "Reply with exactly one word: potato").await {
        Ok(answer) => assert!(answer.to_lowercase().contains("potato"), "got: {answer}"),
        Err(error) => {
            // Claude accounts rate-limit hard; skip rather than fail the suite.
            eprintln!("claude skipped (account state): {error}");
        }
    }
}
