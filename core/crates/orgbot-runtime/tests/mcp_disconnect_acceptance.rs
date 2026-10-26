//! Controlled transport acceptance for a room MCP connection interrupted mid-call.

use std::sync::Arc;

use async_trait::async_trait;
use orgbot_proto::agent::{AgentPolicy, Autonomy, CliKind, ProviderRef};
use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
use orgbot_providers::{
    CompletionRequest, CompletionResponse, ContentBlock, Provider, ProviderError, StopReason,
    StreamEvent, Usage,
};
use orgbot_runtime::Orchestrator;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    sync::mpsc,
};

#[derive(Debug)]
struct DisconnectThenReconnect;

async fn exchange(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    reader: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    request: &str,
) -> Result<Value, ProviderError> {
    writer
        .write_all(format!("{request}\n").as_bytes())
        .await
        .map_err(|error| {
            ProviderError::Unsupported(format!("MCP fixture write failed: {error}"))
        })?;
    let line = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let line = reader
                .next_line()
                .await
                .map_err(|error| {
                    ProviderError::Unsupported(format!("MCP fixture read failed: {error}"))
                })?
                .ok_or_else(|| ProviderError::Unsupported("MCP fixture socket closed".into()))?;
            if !line.is_empty() {
                return serde_json::from_str(&line).map_err(|error| {
                    ProviderError::Unsupported(format!("MCP fixture response was invalid: {error}"))
                });
            }
        }
    })
    .await
    .map_err(|_| ProviderError::Unsupported("MCP fixture response timed out".into()))??;
    Ok(line)
}

async fn connect_and_initialize(
    socket: &str,
) -> Result<
    (
        tokio::net::unix::OwnedWriteHalf,
        tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    ),
    ProviderError,
> {
    let stream = UnixStream::connect(socket).await.map_err(|error| {
        ProviderError::Unsupported(format!("MCP fixture connect failed: {error}"))
    })?;
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read).lines();
    exchange(
        &mut write,
        &mut reader,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"disconnect-fixture","version":"1"}}}"#,
    ).await?;
    write
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await
        .map_err(|error| {
            ProviderError::Unsupported(format!(
                "MCP fixture initialize notification failed: {error}"
            ))
        })?;
    Ok((write, reader))
}

#[async_trait]
impl Provider for DisconnectThenReconnect {
    fn name(&self) -> &str {
        "disconnect-then-reconnect-fixture"
    }

    async fn complete(
        &self,
        request: CompletionRequest,
        _sink: Option<mpsc::Sender<StreamEvent>>,
    ) -> Result<CompletionResponse, ProviderError> {
        let socket = request
            .mcp_socket
            .ok_or_else(|| ProviderError::Unsupported("room MCP socket was not supplied".into()))?;

        // The first connection completes MCP initialization, sends a real room
        // tool request, then disappears before reading the response.
        let (mut writer, _reader) = connect_and_initialize(&socket).await?;
        writer.write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"room_status\",\"arguments\":{}}}\n",
        ).await.map_err(|error| ProviderError::Unsupported(format!("MCP fixture call failed: {error}")))?;
        writer.flush().await.map_err(|error| {
            ProviderError::Unsupported(format!("MCP fixture flush failed: {error}"))
        })?;
        drop(writer);
        drop(_reader);

        // A fresh connection during the same provider turn must still receive
        // a complete tool response from the room bridge.
        let (mut writer, mut reader) = connect_and_initialize(&socket).await?;
        let reply = exchange(
            &mut writer,
            &mut reader,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"room_status","arguments":{}}}"#,
        ).await?;
        if reply.pointer("/result/isError").and_then(Value::as_bool) == Some(true) {
            return Err(ProviderError::Unsupported(format!(
                "reconnected MCP call failed: {reply}"
            )));
        }
        Ok(CompletionResponse {
            content: vec![ContentBlock::text(
                "reconnected after interrupted MCP exchange",
            )],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            model: "fixture".into(),
        })
    }
}

#[tokio::test]
async fn an_mcp_disconnect_during_exchange_allows_reconnect() {
    let mut room = Orchestrator::new(RoomSpec {
        name: "mcp-disconnect-fixture".into(),
        goal: "exercise a local bridge disconnect".into(),
        roster: vec![],
        completion: CompletionRule::Manual,
        limits: RoomLimits::default(),
    });
    let spec = orgbot_proto::AgentSpec::new(
        "coordinator",
        "Coordinator",
        ProviderRef::CliAgent {
            cli: CliKind::Codex,
            model: "synthetic-test-model".into(),
            workdir: ".".into(),
            policy: AgentPolicy::ReadOnly,
            mcp: true,
        },
    )
    .with_roles(["coordinator"])
    .with_autonomy(Autonomy::coordinator());
    room.seat(spec, Arc::new(DisconnectThenReconnect)).unwrap();
    room.start().unwrap();

    let report = room
        .step(None)
        .await
        .expect("the coordinator should receive a turn");
    assert_eq!(
        report.said.as_deref(),
        Some("reconnected after interrupted MCP exchange")
    );
    assert_eq!(room.engine().room().turns_used, 1);
}
