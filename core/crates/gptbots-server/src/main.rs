//! The `gptbots` command.

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use gptbots_server::config::RoomConfig;
use gptbots_server::runner::run_headless;
use tracing_subscriber::EnvFilter;

/// Run and host multi-bot group-chat rooms.
#[derive(Parser)]
#[command(name = "gptbots", version, about)]
struct Cli {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand)]
enum CliCommand {
    /// Run a room to completion in the terminal, printing a live transcript.
    Run {
        /// Path to the room's TOML definition.
        config: PathBuf,
        /// Print only the final outcome, not the transcript.
        #[arg(long)]
        quiet: bool,
    },
    /// Host a room over WebSocket for clients to attach to.
    Serve {
        /// Path to the room's TOML definition.
        config: PathBuf,
        /// Address to bind.
        #[arg(long, default_value = "127.0.0.1:8787")]
        addr: SocketAddr,
    },
    /// Parse and validate a room file, printing what it resolves to.
    Check {
        /// Path to the room's TOML definition.
        config: PathBuf,
    },
    /// Write a sample room file to get started.
    Init {
        /// Where to write it.
        #[arg(default_value = "room.toml")]
        path: PathBuf,
    },
    /// Bridge an agent CLI's stdio to a room's MCP socket. Spawned by the
    /// agent CLI itself from its MCP config; you normally never run this.
    RoomMcp {
        /// The room MCP unix socket to connect to.
        #[arg(long)]
        socket: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,gptbots=debug")),
        )
        .with_target(false)
        .init();

    match Cli::parse().command {
        CliCommand::Run { config, quiet } => {
            let resolved = RoomConfig::load(&config)?.resolve()?;
            let outcome = run_headless(resolved, quiet).await?;
            println!("outcome: {outcome:?}");
            Ok(())
        }
        CliCommand::Serve { config, addr } => serve(config, addr).await,
        CliCommand::Check { config } => check(config),
        CliCommand::Init { path } => init(path),
        CliCommand::RoomMcp { socket } => {
            gptbots_runtime::relay::run(&socket)
                .await
                .map_err(anyhow::Error::msg)?;
            Ok(())
        }
    }
}

/// Host a room over WebSocket.
async fn serve(config: PathBuf, addr: SocketAddr) -> anyhow::Result<()> {
    gptbots_server::serve_room(&config, addr).await
}

/// Validate a room file and describe what it resolves to.
fn check(config: PathBuf) -> anyhow::Result<()> {
    let resolved = RoomConfig::load(&config)?.resolve()?;
    println!("room `{}` — {}", resolved.spec.name, resolved.spec.goal);
    println!("completion: {:?}", resolved.spec.completion);
    println!("\nbots ({}):", resolved.bots.len());
    for bot in &resolved.bots {
        println!(
            "  @{} [{}] via {} ({})",
            bot.spec.handle,
            bot.spec.roles.join("/"),
            bot.provider.name(),
            bot.spec.provider.model(),
        );
    }
    if !resolved.tasks.is_empty() {
        println!("\nseed tasks ({}):", resolved.tasks.len());
        for task in &resolved.tasks {
            println!("  · {} — {}", task.title, task.brief);
        }
    }
    println!("\nlooks valid.");
    Ok(())
}

/// Write a sample room file.
fn init(path: PathBuf) -> anyhow::Result<()> {
    if path.exists() {
        anyhow::bail!(
            "{} already exists; refusing to overwrite it",
            path.display()
        );
    }
    std::fs::write(&path, SAMPLE_ROOM)?;
    println!("wrote {}", path.display());
    println!("edit it, then run:  gptbots run {}", path.display());
    Ok(())
}

/// A starter room: a coordinator, two writers, and an inspector.
const SAMPLE_ROOM: &str = r#"# A starter room. Bots resolve their credentials from your environment
# and CLI sign-ins (ANTHROPIC_API_KEY, `codex login`, DEEPSEEK_API_KEY),
# so there are no secrets in this file.

[room]
name = "release-notes"
goal = "Write clear, accurate release notes for version 1.3 covering every change since 1.2."
completion = "all_tasks_terminal"

[room.limits]
max_total_turns = 200
handoff_timeout_ms = 300000

# The coordinator plans the work and delivers it, but does not inspect.
[[bots]]
handle = "lead"
display_name = "Coordinator"
roles = ["coordinator"]
autonomy = "coordinator"
persona = "You break work into clear, parallelisable tasks and keep the room moving."
provider = { kind = "anthropic", model = "claude-opus-5" }

# Two writers do the work. Swap providers freely — mix and match.
[[bots]]
handle = "writer-a"
roles = ["writer"]
persona = "You write concise, accurate technical prose."
provider = { kind = "anthropic", model = "claude-sonnet-5" }

[[bots]]
handle = "writer-b"
roles = ["writer"]
persona = "You are thorough and check details."
provider = { kind = "deepseek", model = "deepseek-chat" }

# A dedicated inspector. It can approve or send work back, but never submits.
[[bots]]
handle = "qa"
roles = ["reviewer"]
autonomy = "inspector"
persona = "You check work against its acceptance criteria, one by one."
provider = { kind = "anthropic", model = "claude-opus-5" }

# Optional: seed the board. Remove this and the coordinator will plan instead.
# [[tasks]]
# title = "Draft the 1.3 notes"
# brief = "Summarise every user-facing change since 1.2 in a changelog."
# acceptance_criteria = ["Covers every merged PR", "No internal jargon"]
# required_role = "writer"
# priority = "high"
"#;
