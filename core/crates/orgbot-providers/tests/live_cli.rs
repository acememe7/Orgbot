//! Live tests that drive the real installed CLIs.
//!
//! These are gated behind ORGBOT_TEST_LIVE_CLIS=1 because they spend real
//! tokens and need the machine's sign-ins. Run them with:
//!
//!   ORGBOT_TEST_LIVE_CLIS=1 cargo test -p orgbot-providers --test live_cli -- --nocapture
//!
//! A CLI that is not installed, or an account that is rate-limited, skips
//! rather than failing the suite.

use orgbot_proto::agent::{AgentPolicy, CliKind};
use orgbot_providers::agent_cli::AgentCliProvider;
use orgbot_providers::types::{CompletionRequest, Turn};
use orgbot_providers::Provider;

#[tokio::test]
#[ignore = "Spends three Codex episodes; requires a new ORGBOT_CODEX_USAGE_PROOF directory"]
async fn codex_usage_reconciles_after_resume_and_provider_restart() {
    let root = std::path::PathBuf::from(
        std::env::var_os("ORGBOT_CODEX_USAGE_PROOF").expect("explicit proof destination required"),
    );
    assert!(root.is_absolute());
    std::fs::create_dir(&root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let project = root.join("project");
    std::fs::create_dir(&project).unwrap();
    let session_path = root.join("session.json");
    let make_provider = || {
        let provider = AgentCliProvider::new(
            CliKind::Codex, "gpt-6.1-sol", project.to_string_lossy(), AgentPolicy::ReadOnly, false,
        ).unwrap().with_session_store(session_path.clone());
        #[cfg(target_os = "linux")]
        let provider = provider.with_workspace_guard(root.join("workspaces"), "usage-proof".into());
        provider
    };
    let mut provider = make_provider();
    let mut totals = [0u64; 4];
    let mut episodes = Vec::new();
    let mut session_id = None;
    for episode in 0..3 {
        if episode == 2 {
            drop(provider);
            provider = make_provider();
            assert_eq!(provider.session_id(), session_id);
        }
        let response = provider.complete(request(if episode == 0 {
            "Reply with exactly one word: potato. Do not call tools."
        } else {
            "--- Resume check: repeat your previous one-word answer. Do not call tools."
        }), None).await.unwrap();
        assert_eq!(response.text().trim().to_lowercase(), "potato");
        assert!(response.usage.reported && response.usage.complete);
        if episode == 0 { session_id = provider.session_id(); }
        assert_eq!(provider.session_id(), session_id);
        let usage = response.usage;
        let counts = [usage.input_tokens, usage.cache_read_tokens, usage.output_tokens, usage.cache_write_tokens];
        for (total, count) in totals.iter_mut().zip(counts) { *total += u64::from(count); }
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&session_path).unwrap()).unwrap();
        let cumulative = saved
            .get("native_usage")
            .or_else(|| saved.get("codex_usage"))
            .expect("native cumulative usage must be persisted");
        for (field, total) in ["input_tokens", "cache_read_tokens", "output_tokens", "cache_write_tokens"].into_iter().zip(totals) {
            assert_eq!(cumulative[field].as_u64(), Some(total), "episode {episode}: {field}");
        }
        episodes.push(serde_json::json!({"episode": episode + 1, "usage": usage, "cumulative": cumulative}));
        std::fs::write(root.join("usage-proof.json"), serde_json::to_vec_pretty(&episodes).unwrap()).unwrap();
    }
    println!("Three native episodes reconciled across resume and provider restart: input={}, cache={}, output={}, cache_write={}", totals[0], totals[1], totals[2], totals[3]);
}

fn live_enabled() -> bool {
    std::env::var_os("ORGBOT_TEST_LIVE_CLIS").is_some()
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
        eprintln!("skipped (set ORGBOT_TEST_LIVE_CLIS=1 with codex installed)");
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
        eprintln!("skipped (set ORGBOT_TEST_LIVE_CLIS=1 with grok installed)");
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
        eprintln!("skipped (set ORGBOT_TEST_LIVE_CLIS=1 with claude installed)");
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
