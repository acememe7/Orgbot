//! Device-owned workspace tools. The Android host performs file/SSH I/O;
//! neither credentials nor an executable command runner enter the room model.
use orgbot_proto::AgentId;
use orgbot_providers::types::ToolDef;
use orgbot_tools::ToolOutcome;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSpec {
    pub label: String,
    pub root: String,
    #[serde(default)]
    pub writable: bool,
    #[serde(default)]
    pub commands: bool,
}

#[derive(Debug)]
pub struct WorkspaceCall {
    pub agent: AgentId,
    pub name: String,
    pub arguments: Value,
    pub reply: oneshot::Sender<ToolOutcome>,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub spec: WorkspaceSpec,
    pub tx: mpsc::Sender<WorkspaceCall>,
}

impl Workspace {
    pub fn definitions(&self) -> Vec<ToolDef> {
        let mut tools = vec![
            tool(
                "workspace_list",
                "List a directory in the selected computer's workspace (up to 500 entries).",
                json!({"path":{"type":"string"}}),
                json!(["path"]),
            ),
            tool(
                "workspace_read",
                "Read a UTF-8 text file, at most 1 MiB, in the selected workspace.",
                json!({"path":{"type":"string"}}),
                json!(["path"]),
            ),
        ];
        if self.spec.writable {
            tools.push(tool("workspace_import_attachment", "Copy a user-attached original file into the selected working directory. Pass its attachment id from the conversation. Returns a new unique relative path; never overwrites another file. Use this when extracted text/images are insufficient, or for other file formats.", json!({"id":{"type":"string"}}), json!(["id"])));
            tools.push(tool("workspace_write", "Atomically create or replace a UTF-8 file (up to 1 MiB). Parent directory must exist.", json!({"path":{"type":"string"},"content":{"type":"string"}}), json!(["path","content"])));
            tools.push(tool(
                "workspace_mkdir",
                "Create one directory in the workspace. Parent directory must exist.",
                json!({"path":{"type":"string"}}),
                json!(["path"]),
            ));
            if self.spec.commands {
                tools.push(tool("workspace_run", "Run a POSIX shell command on the selected SSH computer, starting in its workspace. Output is bounded to 64 KiB. Commands default to 300 seconds; set timeout_seconds from 1 to 1800 for longer builds or tests. A command timeout stops its channel, not SSH; inspect partial effects before retrying. Shell access has the SSH account's permissions, not a directory sandbox. Do not start persistent/background processes.", json!({"command":{"type":"string"},"timeout_seconds":{"type":"integer","minimum":1,"maximum":1800}}), json!(["command"])));
            }
        }
        tools
    }

    pub fn instruction(&self) -> String {
        format!(
            "\n\nCOMPUTER: {}. Working directory: {}. Use workspace_* tools for actual computer work. All paths must be relative to this root. File writes allowed: {}. Shell commands available: {}. You are an API model running in the Android app; no local CLI is installed. Do not claim files were changed or commands ran without successful tool results. Preserve existing user files; ask before destructive operations. Workspace reads reconnect automatically to the trusted computer. After an interrupted command or write, inspect its effects using workspace_read, workspace_list, or a read-only shell command before deciding what remains. Never blindly repeat a mutation or tell others the computer is unavailable without checking it.",
            self.spec.label,
            self.spec.root,
            self.spec.writable,
            self.spec.writable && self.spec.commands
        )
    }

    pub async fn call(&self, agent: AgentId, name: &str, arguments: &Value) -> ToolOutcome {
        if !self.definitions().iter().any(|tool| tool.name == name) {
            return failure("This workspace does not permit that tool.");
        }
        let command_seconds = if name == "workspace_run" {
            match command_timeout_seconds(arguments) {
                Ok(seconds) => seconds,
                Err(message) => return failure(message),
            }
        } else {
            30
        };
        let (reply, result) = oneshot::channel();
        let call = WorkspaceCall {
            agent,
            name: name.into(),
            arguments: arguments.clone(),
            reply,
        };
        match tokio::time::timeout(
            std::time::Duration::from_secs(command_seconds + 180),
            async {
                self.tx.send(call).await.map_err(|_| ())?;
                result.await.map_err(|_| ())
            },
        )
        .await
        {
            Ok(Ok(outcome)) => outcome,
            _ => failure(
                "Workspace operation interrupted or timed out; its effects may already have occurred. Inspect before retrying.",
            ),
        }
    }
}

fn command_timeout_seconds(arguments: &Value) -> Result<u64, &'static str> {
    match arguments.get("timeout_seconds") {
        None => Ok(300),
        Some(value) => value
            .as_u64()
            .filter(|s| (1..=1800).contains(s))
            .ok_or("timeout_seconds must be an integer from 1 to 1800."),
    }
}

fn tool(name: &str, description: &str, properties: Value, required: Value) -> ToolDef {
    ToolDef {
        name: name.into(),
        description: description.into(),
        input_schema: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
    }
}
fn failure(message: &str) -> ToolOutcome {
    ToolOutcome {
        content: message.into(),
        is_error: true,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shell_timeout_contract_accepts_long_builds_but_rejects_unbounded_values() {
        assert_eq!(
            super::command_timeout_seconds(&serde_json::json!({})),
            Ok(300)
        );
        assert_eq!(
            super::command_timeout_seconds(&serde_json::json!({"timeout_seconds":1800})),
            Ok(1800)
        );
        for value in [
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!(1801),
            serde_json::json!(1.5),
            serde_json::json!("60"),
            serde_json::json!(null),
        ] {
            assert!(
                super::command_timeout_seconds(&serde_json::json!({"timeout_seconds":value}))
                    .is_err()
            );
        }
    }
    use super::*;
    use crate::Orchestrator;
    use orgbot_proto::{
        AgentSpec,
        agent::ProviderRef,
        room::{CompletionRule, RoomLimits, RoomSpec},
    };
    use orgbot_providers::{
        ScriptedProvider,
        types::{CompletionResponse, ContentBlock, StopReason, Usage},
    };
    use std::sync::Arc;

    #[tokio::test]
    async fn api_bot_receives_computer_tools_and_the_hosts_actual_result() {
        let provider = Arc::new(ScriptedProvider::new(vec![CompletionResponse {
            model: "test".into(),
            content: vec![ContentBlock::ToolUse {
                id: "read1".into(),
                name: "workspace_read".into(),
                input: json!({"path":"README.md"}),
            }],
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
        }]));
        let mut room = Orchestrator::new(RoomSpec {
            name: "device".into(),
            goal: "read a file".into(),
            roster: vec![],
            completion: CompletionRule::Manual,
            limits: RoomLimits::default(),
        });
        let spec = AgentSpec::new(
            "worker",
            "Worker",
            ProviderRef::Scripted {
                script: "test".into(),
            },
        );
        let agent = room.seat(spec, provider.clone()).unwrap();
        let (tx, mut rx) = mpsc::channel(2);
        let workspace = Workspace {
            spec: WorkspaceSpec {
                label: "SSH test computer".into(),
                root: "/work".into(),
                writable: false,
                commands: true,
            },
            tx,
        };
        assert!(
            !workspace
                .definitions()
                .iter()
                .any(|tool| tool.name == "workspace_run" || tool.name == "workspace_write")
        );
        room.attach_workspace(workspace.clone());
        room.start().unwrap();
        let _ = agent;
        // Drive the bot directly to avoid task scheduling obscuring the bridge.
        let mut bot = crate::Bot::new(
            AgentSpec::new(
                "reader",
                "Reader",
                ProviderRef::Scripted {
                    script: "test".into(),
                },
            ),
            provider.clone(),
        );
        bot.attach_workspace(workspace);
        let mut engine = orgbot_engine::Engine::new(room.engine().room().spec.clone());
        engine
            .apply(orgbot_engine::Command::JoinAgent {
                spec: Box::new(bot.spec.clone()),
            })
            .unwrap();
        engine.apply(orgbot_engine::Command::StartRoom).unwrap();
        let run = tokio::spawn(async move { bot.take_turn(&mut engine, None).await });
        let call = rx.recv().await.unwrap();
        assert_eq!(call.name, "workspace_read");
        assert_eq!(call.arguments["path"], "README.md");
        call.reply
            .send(ToolOutcome {
                content: "actual file from the selected server".into(),
                is_error: false,
            })
            .unwrap();
        let report = run.await.unwrap().unwrap();
        assert_eq!(report.tool_errors, 0);
        let requests = provider.requests();
        assert!(requests[0].system.contains("SSH test computer"));
        assert!(
            serde_json::to_string(&requests[1].messages)
                .unwrap()
                .contains("actual file from the selected server")
        );
    }
}
