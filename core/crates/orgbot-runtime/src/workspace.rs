//! Workspace tools are hosted by the device or local server; neither credentials
//! nor an executable command runner enter the room model.
use std::sync::Arc;

use orgbot_proto::AgentId;
use orgbot_providers::types::ToolDef;
use orgbot_tools::ToolOutcome;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
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
    /// What the owner has allowed this bot's sandbox for the work it is doing
    /// now (the bot server key, lent sign-ins). Set by the room, never from
    /// the model's arguments.
    pub grants: orgbot_providers::SandboxGrants,
    pub reply: oneshot::Sender<ToolOutcome>,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub spec: WorkspaceSpec,
    pub tx: mpsc::Sender<WorkspaceCall>,
    /// One workspace operation at a time, across every bot.
    ///
    /// There is one device at the other end of this holding one connection,
    /// and it refuses a second operation while one is running rather than
    /// interleaving them. With bots taking turns concurrently, two of them
    /// reaching for a file at the same moment is ordinary, so the wait belongs
    /// here: the second call queues for its turn instead of coming back as a
    /// failure the model has to interpret. Waiting is the honest answer — the
    /// work is genuinely serial.
    ///
    /// Every clone of a `Workspace` shares this, which is what makes it a
    /// room-wide queue rather than a per-bot one.
    pub gate: Arc<tokio::sync::Mutex<()>>,
    capabilities: WorkspaceCapabilities,
}

/// Operations a connected workspace host actually implements.
#[derive(Debug, Clone, Copy, Default)]
pub struct WorkspaceCapabilities {
    pub attachment_import: bool,
    pub git_read: bool,
    /// Run isolated, read-only checks in a protected Linux local workspace.
    pub verification: bool,
    /// Commands run in a protected copy whose home is private, so the
    /// computer's own home, and the CLI settings in it, are out of sight.
    pub private_home: bool,
}

impl Workspace {
    /// A workspace reachable through `tx`, with a fresh queue.
    pub fn new(spec: WorkspaceSpec, tx: mpsc::Sender<WorkspaceCall>) -> Self {
        Self {
            spec,
            tx,
            gate: Arc::new(tokio::sync::Mutex::new(())),
            capabilities: WorkspaceCapabilities {
                attachment_import: true,
                git_read: false,
                verification: false,
                private_home: false,
            },
        }
    }

    /// What the host behind this workspace implements.
    pub const fn capabilities(&self) -> WorkspaceCapabilities {
        self.capabilities
    }

    /// Construct a workspace with capabilities backed by its host.
    pub fn with_capabilities(
        spec: WorkspaceSpec,
        tx: mpsc::Sender<WorkspaceCall>,
        capabilities: WorkspaceCapabilities,
    ) -> Self {
        Self {
            spec,
            tx,
            gate: Arc::new(tokio::sync::Mutex::new(())),
            capabilities,
        }
    }

    /// Construct a workspace whose host cannot import conversation attachments.
    pub fn new_without_attachment_import(
        spec: WorkspaceSpec,
        tx: mpsc::Sender<WorkspaceCall>,
    ) -> Self {
        let mut workspace = Self::new(spec, tx);
        workspace.capabilities.attachment_import = false;
        workspace
    }

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
            if self.capabilities.attachment_import {
                tools.push(tool("workspace_import_attachment", "Copy a user-attached original file into the selected working directory. Pass its attachment id from the conversation. Returns a new unique relative path; never overwrites another file. Use this when extracted text/images are insufficient, or for other file formats.", json!({"id":{"type":"string"}}), json!(["id"])));
            }
            tools.push(tool("workspace_write", "Atomically create or replace a UTF-8 file (up to 1 MiB). Parent directory must exist. On Linux, overwrites require expected_sha256 from workspace_read; reread after a conflict.", json!({"path":{"type":"string"},"content":{"type":"string"},"expected_sha256":{"type":"string"}}), json!(["path","content"])));
            tools.push(tool(
                "workspace_mkdir",
                "Create one directory in the workspace. Parent directory must exist.",
                json!({"path":{"type":"string"}}),
                json!(["path"]),
            ));
            if self.spec.commands {
                tools.push(tool("workspace_run", "Run a POSIX shell command in the selected workspace. Linux local commands use an isolated working copy: original project files are read-only, edits integrate after success, and removals await owner approval. A file that changed in the project since your copy was made is not overwritten: your version is kept under ~/orgbot-conflicts/ and your copy takes the project's, so redo the change on top if it is still needed. SSH commands retain the SSH account's permissions. Output is bounded to 64 KiB. Timeout defaults to 300 seconds (1–1800 allowed). Inspect partial effects after interruption; do not start background processes.", json!({"command":{"type":"string"},"timeout_seconds":{"type":"integer","minimum":1,"maximum":1800}}), json!(["command"])));
            }
        }
        if self.capabilities.verification {
            tools.push(tool("workspace_verify", "Run a test or verification command in a read-only, network-isolated Linux workspace copy. No results are applied to the original workspace. Output is bounded to 64 KiB. Timeout defaults to 300 seconds (1–1800 allowed).", json!({"command":{"type":"string"},"timeout_seconds":{"type":"integer","minimum":1,"maximum":1800}}), json!(["command"])));
        }
        if self.capabilities.git_read && self.spec.writable && self.spec.commands {
            tools.push(tool(
                "workspace_land_commits",
                "Land commits you made in your copy onto a branch of the owner's real repository. \
                 Your copy's git history never reaches the project on its own. Bundle the commits \
                 inside the project (`git bundle create <path> <base>..<branch>`, where <base> is \
                 the commit the branch is on in the project now), then call this with the bundle's \
                 path, the branch, the task you hold and why. The owner approves once; call again \
                 with the same bundle and branch after they do. The branch only moves forward: \
                 nothing is pushed or merged, no history is rewritten and no project file changes, \
                 so every file the commits change must already be in the project as they leave it. \
                 Run the project's checks (typecheck, lint, tests) on these exact commits first, \
                 record the results as an artifact on the task and pass its id as checks: the \
                 owner's card says which checks, if any, were recorded.",
                json!({"bundle":{"type":"string"},"branch":{"type":"string"},"task":{"type":"string"},"reason":{"type":"string"},"checks":{"type":"string"}}),
                json!(["bundle","branch","task","reason"]),
            ));
        }
        if self.capabilities.git_read && self.spec.writable && self.spec.commands {
            tools.push(tool(
                "workspace_push_commits",
                "Push a branch of the owner's repository to one of its remotes, such as origin, \
                 once its commits have landed there. The owner approves each push on a card, \
                 whatever the room's access mode; call again with the same remote and branch \
                 after they do. It is a normal push from the owner's own git setup: it never \
                 forces, never creates a branch, and is refused if the remote has commits the \
                 project lacks.",
                json!({"remote":{"type":"string"},"branch":{"type":"string"},"task":{"type":"string"},"reason":{"type":"string"}}),
                json!(["branch","task","reason"]),
            ));
        }
        if self.capabilities.git_read {
            tools.push(tool(
                "workspace_git",
                "Read repository status, a bounded diff, or recent commit history for the selected Linux workspace. This is read-only.",
                json!({"action":{"type":"string","enum":["status","diff","log"]}}),
                json!(["action"]),
            ));
        }
        tools
    }

    pub fn instruction(&self) -> String {
        let landing = if self.capabilities.git_read && self.spec.writable && self.spec.commands {
            " Commits you make in your copy stay in your copy: to put them on the owner's branch, bundle them in the project and use workspace_land_commits, which the owner approves once. Never ask the owner to run git commands or scripts for you."
        } else {
            ""
        };
        let text = format!(
            "\n\nCOMPUTER: {}. Working directory: {}. Use workspace_* tools for actual computer work. All paths must be relative to this root. File writes allowed: {}. Shell commands available: {}. You are an API model; do not claim files were changed or commands ran without successful tool results. Preserve existing user files; ask before destructive operations. After an interrupted write, inspect its effects using workspace_read or workspace_list before deciding what remains. Never blindly repeat a mutation or tell others the computer is unavailable without checking it. This workspace may already carry playbooks written for it — look in .agents/skills, .claude/skills, .codex/skills or ~/.codex/skills with workspace_list, and read a SKILL.md with workspace_read when its name matches your work. Prefer one written for this machine over a general approach; it knows things about this setup that you do not.",
            self.spec.label,
            self.spec.root,
            self.spec.writable,
            self.spec.writable && self.spec.commands
        );
        text + landing
    }

    pub async fn call(&self, agent: AgentId, name: &str, arguments: &Value) -> ToolOutcome {
        self.call_as(agent, name, arguments, false).await
    }

    /// [`Self::call`] for a bot the room has, or has not, allowed the bot
    /// server key for its current work.
    pub async fn call_as(
        &self,
        agent: AgentId,
        name: &str,
        arguments: &Value,
        grants: impl Into<orgbot_providers::SandboxGrants>,
    ) -> ToolOutcome {
        let grants = grants.into();
        if !self.definitions().iter().any(|tool| tool.name == name) {
            return failure("This workspace does not permit that tool.");
        }
        let command_seconds = if matches!(name, "workspace_run" | "workspace_verify") {
            match command_timeout_seconds(arguments) {
                Ok(seconds) => seconds,
                Err(message) => return failure(message),
            }
        } else {
            30
        };
        // Wait for the device to be free before starting the clock, so a bot
        // queued behind a long build still gets its own full allowance rather
        // than spending it in the queue.
        let _turn = self.gate.lock().await;

        let (reply, result) = oneshot::channel();
        let call = WorkspaceCall {
            agent,
            name: name.into(),
            arguments: arguments.clone(),
            grants,
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

    #[tokio::test]
    async fn workspace_tools_follow_host_capabilities_and_write_policy() {
        let (tx, _rx) = mpsc::channel(1);
        let workspace = Workspace::with_capabilities(
            WorkspaceSpec {
                label: "read-only Linux".into(),
                root: "/tmp/project".into(),
                writable: false,
                commands: false,
            },
            tx,
            WorkspaceCapabilities {
                attachment_import: true,
                git_read: true,
                verification: false,
                private_home: false,
            },
        );
        let names: Vec<_> = workspace
            .definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        assert!(names.contains(&"workspace_git".into()));
        assert!(!names.contains(&"workspace_import_attachment".into()));
        assert!(!names.contains(&"workspace_write".into()));
        assert!(!names.contains(&"workspace_mkdir".into()));

        let (tx, _rx) = mpsc::channel(1);
        let unsupported = Workspace::with_capabilities(
            WorkspaceSpec {
                label: "Android".into(),
                root: "/project".into(),
                writable: true,
                commands: false,
            },
            tx,
            WorkspaceCapabilities::default(),
        );
        let names: Vec<_> = unsupported
            .definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        assert!(!names.contains(&"workspace_git".into()));
        assert!(!names.contains(&"workspace_import_attachment".into()));
    }

    #[tokio::test]
    async fn verification_tool_is_independent_of_write_and_command_capabilities() {
        let (tx, _rx) = mpsc::channel(1);
        let workspace = Workspace::with_capabilities(
            WorkspaceSpec {
                label: "review workspace".into(),
                root: "/tmp/project".into(),
                writable: false,
                commands: false,
            },
            tx,
            WorkspaceCapabilities {
                attachment_import: false,
                git_read: false,
                verification: true,
                private_home: false,
            },
        );
        let names: Vec<_> = workspace
            .definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        assert!(names.contains(&"workspace_verify".into()));
        assert!(!names.contains(&"workspace_run".into()));
        assert!(!names.contains(&"workspace_write".into()));
    }
    use super::*;
    use crate::Orchestrator;
    use orgbot_proto::{
        agent::ProviderRef,
        room::{CompletionRule, RoomLimits, RoomSpec},
        AgentSpec,
    };
    use orgbot_providers::{
        types::{CompletionResponse, ContentBlock, StopReason, Usage},
        ScriptedProvider,
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
            access: Default::default(),
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
        let workspace = Workspace::new(
            WorkspaceSpec {
                label: "SSH test computer".into(),
                root: "/work".into(),
                writable: false,
                commands: true,
            },
            tx,
        );
        assert!(!workspace
            .definitions()
            .iter()
            .any(|tool| tool.name == "workspace_run" || tool.name == "workspace_write"));
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
        let spec = room.engine().room().spec.clone();
        let mut engine = orgbot_engine::Engine::new(spec);
        engine
            .apply(orgbot_engine::Command::JoinAgent {
                spec: Box::new(bot.spec.clone()),
            })
            .unwrap();
        engine.apply(orgbot_engine::Command::StartRoom).unwrap();
        let engine = crate::SharedEngine::new(engine);
        let run = tokio::spawn(async move { bot.take_turn(&engine, None).await });
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
        assert!(serde_json::to_string(&requests[1].messages)
            .unwrap()
            .contains("actual file from the selected server"));
    }
}
