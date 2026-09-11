//! A real MCP server, over a real pipe.
//!
//! The unit tests beside the client check the protocol handling; this checks
//! that a server written by somebody else, registered the way the CLIs
//! register them, actually answers. It uses whichever stdio server this
//! machine has and skips when there is none, so it is a real check where it
//! can be and silent where it cannot.

use orgbot_runtime::mcp_client::{McpClient, McpServerSpec, McpTransport};

fn a_real_server() -> Option<McpServerSpec> {
    let script = std::env::var("ORGBOT_MCP_SCRIPT").ok()?;
    std::path::Path::new(&script).is_file().then(|| McpServerSpec {
        name: "history".into(),
        transport: McpTransport::Stdio {
            command: "python3".into(),
            args: vec![script],
            env: Default::default(),
        },
    })
}

#[tokio::test]
async fn a_real_stdio_server_answers_and_offers_its_tools() {
    let Some(spec) = a_real_server() else {
        eprintln!("MCP-LIVE skipped: set ORGBOT_MCP_SCRIPT to a stdio MCP server");
        return;
    };
    let client = McpClient::connect(&[spec]).await;
    assert_eq!(client.len(), 1, "the server should have answered");

    let tools = client.tools();
    assert!(!tools.is_empty(), "and offered something");
    eprintln!("MCP-LIVE connected: {} tools", tools.len());
    for tool in &tools {
        eprintln!("  {}", tool.name);
    }
    for tool in &tools {
        assert!(
            tool.name.starts_with("history__"),
            "every tool is named for its server: {}",
            tool.name
        );
        assert!(
            !tool.description.is_empty(),
            "a tool with no description is one a model will not use: {}",
            tool.name
        );
    }

    // And a call reaches it.
    let first = tools[0].name.clone();
    let outcome = client.call(&first, &serde_json::json!({})).await;
    assert!(
        !outcome.content.is_empty(),
        "`{first}` returned nothing at all"
    );
    eprintln!(
        "MCP-LIVE called {first} -> {}",
        outcome.content.chars().take(140).collect::<String>()
    );
}
