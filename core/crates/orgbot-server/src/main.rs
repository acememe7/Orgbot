//! The `orgbot` command.

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use orgbot_server::config::RoomConfig;
use orgbot_server::Access;
use orgbot_server::runner::run_headless;
use tracing_subscriber::EnvFilter;

/// Run and host multi-bot group-chat rooms.
#[derive(Parser)]
#[command(name = "orgbot", version, about)]
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
        /// The token clients must present. Generated and printed when a
        /// reachable room is served without one. Also read from
        /// ORGBOT_ROOM_TOKEN, so it need not appear in a process list.
        #[arg(long)]
        token: Option<String>,
        /// Serve plain HTTP on a reachable address because TLS is terminated
        /// in front of this process — a Cloudflare tunnel, nginx, Caddy.
        ///
        /// Without this, binding anything but loopback is refused, because
        /// the token would otherwise cross the network in the clear.
        #[arg(long)]
        behind_proxy: bool,
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
    /// Carry this machine's sign-ins to another device, such as a phone.
    ///
    /// A phone has no agent CLIs, but the tokens those CLIs hold are ordinary
    /// OAuth tokens the provider APIs accept — so the phone does not need its
    /// own sign-in, it needs this one.
    Pair {
        /// Write the bundle here instead of printing it.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
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
                .unwrap_or_else(|_| EnvFilter::new("info,orgbot=debug")),
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
        CliCommand::Serve {
            config,
            addr,
            token,
            behind_proxy,
        } => serve(config, addr, token, behind_proxy).await,
        CliCommand::Check { config } => check(config),
        CliCommand::Init { path } => init(path),
        CliCommand::Pair { out } => pair(out),
        CliCommand::RoomMcp { socket } => {
            orgbot_runtime::relay::run(&socket)
                .await
                .map_err(anyhow::Error::msg)?;
            Ok(())
        }
    }
}

/// Host a room over WebSocket.
async fn serve(
    config: PathBuf,
    addr: SocketAddr,
    token: Option<String>,
    behind_proxy: bool,
) -> anyhow::Result<()> {
    // A token on the command line is visible to every process on the box, so
    // the environment is the better place for it; the flag stays for
    // convenience when trying things out.
    let token = token.or_else(|| {
        std::env::var("ORGBOT_ROOM_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty())
    });
    let access = access_for(addr, token, behind_proxy)?;
    orgbot_server::serve_room(&config, addr, access).await
}

/// Decide how a room is protected, and refuse to serve one that is not.
///
/// Reaching the port is reaching the room: a client receives the whole
/// transcript and can post as a person and drive the bots. Loopback is fine
/// because the operating system is the boundary. Anything else needs a token,
/// and needs TLS somewhere in front of it.
fn access_for(
    addr: SocketAddr,
    token: Option<String>,
    behind_proxy: bool,
) -> anyhow::Result<Access> {
    if addr.ip().is_loopback() {
        return Ok(match token {
            Some(token) => Access::with_token(token),
            None => Access::open(),
        });
    }

    if !behind_proxy {
        anyhow::bail!(
            concat!(
                "refusing to serve {} without TLS.\n\n",
                "This address is reachable from other machines, and plain ",
                "HTTP would put the room's token on the wire in the clear.",
                "\n\nPut TLS in front of it - a Cloudflare tunnel, nginx ",
                "or Caddy - then pass --behind-proxy.\n",
                "To try it on this machine only, bind 127.0.0.1 instead.",
            ),
            addr
        );
    }

    let token = match token {
        Some(token) if token.trim().len() >= MIN_TOKEN_LEN => token,
        Some(_) => anyhow::bail!(
            concat!(
                "that token is too short to be worth having; use at least ",
                "{} characters, or omit --token and one will ",
                "be generated",
            ),
            MIN_TOKEN_LEN
        ),
        None => {
            let generated = generate_token();
            // Printed rather than written anywhere: it is the room's key, and
            // the operator decides where it goes.
            println!("Room token (needed to connect): {generated}");
            // Block-buffered when stdout is a pipe: without this the one
            // thing the operator needs can be lost on shutdown.
            use std::io::Write;
            let _ = std::io::stdout().flush();
            generated
        }
    };
    Ok(Access::with_token(token))
}

/// The shortest token worth accepting. Long enough that guessing is hopeless.
const MIN_TOKEN_LEN: usize = 20;

/// A fresh random token.
fn generate_token() -> String {
    // Two v4 UUIDs: 244 bits of randomness from the OS, no new dependency.
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
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
    println!("edit it, then run:  orgbot run {}", path.display());
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

/// Collect this machine's sign-ins into a bundle for another device.
fn pair(out: Option<std::path::PathBuf>) -> anyhow::Result<()> {
    let bundle = orgbot_server::pair::collect();
    if bundle.providers.is_empty() {
        anyhow::bail!(
            "no sign-ins found. Sign in with `claude`, `codex` or `grok` first."
        );
    }
    let json = serde_json::to_string(&bundle)?;

    match out {
        Some(path) => {
            std::fs::write(&path, &json)?;
            // The bundle carries live access tokens; keep it off other users'
            // eyes even on a shared machine.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    &path,
                    std::fs::Permissions::from_mode(0o600),
                );
            }
            eprintln!("wrote {} ({} provider(s))", path.display(), bundle.providers.len());
            for provider in &bundle.providers {
                eprintln!(
                    "  {} — {} model(s){}",
                    provider.provider,
                    provider.models.len(),
                    provider
                        .plan
                        .as_deref()
                        .map(|plan| format!(" [{plan}]"))
                        .unwrap_or_default()
                );
            }
            eprintln!();
            eprintln!("This file contains live access tokens. Move it to the device");
            eprintln!("and delete it afterwards.");
        }
        // Printed on stdout so it can be piped; the summary goes to stderr.
        None => println!("{json}"),
    }
    Ok(())
}
