//! The stdio relay that connects an agent CLI to the room's MCP socket.
//!
//! An agent CLI spawns an MCP server as a child process. That child is this
//! relay: it copies bytes between the CLI's stdio (where the MCP client
//! speaks newline-delimited JSON-RPC) and a unix socket where the room's
//! [crate::mcp] host is listening. Pure plumbing — no protocol logic here.

use std::path::Path;

use tokio::io::{stdin, stdout};
use tokio::net::UnixStream;

/// Bridge the CLI's stdio to the room's MCP unix socket until either side
/// closes. Returns when the connection is done.
pub async fn run(socket: &Path) -> Result<(), String> {
    let stream = UnixStream::connect(socket)
        .await
        .map_err(|error| format!("cannot reach room MCP socket {}: {error}", socket.display()))?;
    let (mut socket_read, mut socket_write) = stream.into_split();
    let mut stdin = stdin();
    let mut stdout = stdout();

    // Two one-way copies: the CLI's stdin reaches the socket, and whatever
    // the room answers is written back to the CLI's stdout. The relay exits
    // when either direction closes; a broken pipe is a normal ending.
    let up = tokio::spawn(async move { tokio::io::copy(&mut stdin, &mut socket_write).await });
    let down = tokio::spawn(async move { tokio::io::copy(&mut socket_read, &mut stdout).await });
    let _ = up.await;
    let _ = down.await;
    Ok(())
}
