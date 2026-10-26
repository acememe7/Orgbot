//! Bounded local file tools for an explicitly selected room workspace.
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use orgbot_runtime::workspace::{Workspace, WorkspaceCall, WorkspaceCapabilities, WorkspaceSpec};
use orgbot_tools::ToolOutcome;
use serde_json::{json, Value};
use tokio::sync::mpsc;

const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_LIST_ENTRIES: usize = 500;
const MAX_GIT_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;

async fn bounded_output(mut stream: impl tokio::io::AsyncRead + Unpin) -> Result<String, String> {
    use tokio::io::AsyncReadExt;
    let mut saved = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = stream.read(&mut buffer).await.map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        let keep = count.min(32768_usize.saturating_sub(saved.len()));
        saved.extend_from_slice(&buffer[..keep]);
    }
    Ok(String::from_utf8_lossy(&saved).into_owned())
}

/// A validated local workspace and its request channel.
#[derive(Debug)]
pub struct LocalWorkspaceHost {
    root: PathBuf,
    writable: bool,
    guard_state: Option<PathBuf>,
    commands: bool,
    #[cfg(target_os = "linux")]
    root_fd: std::os::fd::OwnedFd,
    #[cfg(target_os = "linux")]
    attachment_parent_fd: Option<std::os::fd::OwnedFd>,
    #[cfg(target_os = "linux")]
    committed_attachment_ids: Arc<RwLock<std::collections::HashSet<String>>>,
}

impl LocalWorkspaceHost {
    /// Validate the requested root and attach a runtime workspace to a session.
    #[cfg(target_os = "linux")]
    pub async fn attach(
        spec: WorkspaceSpec,
        session: &mut crate::session::RoomSession,
    ) -> anyhow::Result<tokio::task::JoinHandle<()>> {
        let state_parent = session
            .state_path()
            .and_then(|path| path.parent())
            .map(Path::to_path_buf);
        let committed_attachment_ids = session.committed_attachment_ids();
        let guard_state = state_parent.as_ref().map(|p| p.join("workspaces"));
        anyhow::ensure!(
            !spec.writable || guard_state.is_some(),
            "Writable local workspaces require a private room state_path"
        );
        let mut host = tokio::task::spawn_blocking({
            let root = spec.root.clone();
            let writable = spec.writable;
            move || {
                let attachment_parent_fd = match state_parent {
                    Some(path) => Some(open_dir_from_absolute(path)?),
                    None => None,
                };
                Self::open_root(
                    root,
                    writable,
                    attachment_parent_fd,
                    committed_attachment_ids,
                )
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("local workspace setup failed: {e}"))??;
        host.guard_state = guard_state;
        host.commands = spec.commands && spec.writable;
        let root = host.root.clone();
        let capabilities = WorkspaceCapabilities {
            attachment_import: spec.writable && host.attachment_parent_fd.is_some(),
            git_read: true,
        };
        let mut runtime_spec = spec;
        runtime_spec.root = root.display().to_string();
        let (tx, rx) = mpsc::channel::<WorkspaceCall>(8);
        session.attach_workspace(Workspace::with_capabilities(runtime_spec, tx, capabilities));
        Ok(tokio::spawn(Arc::new(host).serve(rx)))
    }

    #[cfg(not(target_os = "linux"))]
    pub async fn attach(
        _spec: WorkspaceSpec,
        _session: &mut crate::session::RoomSession,
    ) -> anyhow::Result<tokio::task::JoinHandle<()>> {
        anyhow::bail!("Local workspace file operations are supported on Linux only.")
    }

    #[cfg(target_os = "linux")]
    fn open_root(
        root: String,
        writable: bool,
        attachment_parent_fd: Option<std::os::fd::OwnedFd>,
        committed_attachment_ids: Arc<RwLock<std::collections::HashSet<String>>>,
    ) -> anyhow::Result<Self> {
        let path = Path::new(&root);
        anyhow::ensure!(path.is_absolute(), "local workspace root must be absolute");
        anyhow::ensure!(
            !path.components().any(|c| matches!(c, Component::ParentDir)),
            "local workspace root cannot contain parent components"
        );
        let relative = path.strip_prefix("/").expect("absolute path");
        let anchor = rustix::fs::open(
            "/",
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )?;
        let root_fd = rustix::fs::openat2(
            &anchor,
            if relative.as_os_str().is_empty() {
                "."
            } else {
                relative
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("local workspace root must be UTF-8"))?
            },
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
            rustix::fs::ResolveFlags::BENEATH | rustix::fs::ResolveFlags::NO_SYMLINKS,
        )?;
        Ok(Self {
            root: path.to_path_buf(),
            writable,
            guard_state: None,
            commands: false,
            root_fd,
            attachment_parent_fd,
            committed_attachment_ids,
        })
    }

    #[cfg(target_os = "linux")]
    async fn serve(self: Arc<Self>, mut rx: mpsc::Receiver<WorkspaceCall>) {
        while let Some(call) = rx.recv().await {
            // A caller may have timed out while this request was queued.
            if call.reply.is_closed() {
                continue;
            }
            let host = Arc::clone(&self);
            if call.name == "workspace_run" {
                let outcome = self.run(call.agent, &call.arguments).await;
                let _ = call.reply.send(outcome);
                continue;
            }
            let agent = call.agent;
            let name = call.name;
            let arguments = call.arguments;
            let outcome =
                tokio::task::spawn_blocking(move || host.handle_agent(agent, &name, &arguments))
                    .await
                    .unwrap_or_else(|e| ToolOutcome {
                        content: format!("Local workspace worker failed: {e}"),
                        is_error: true,
                    });
            let _ = call.reply.send(outcome);
        }
    }

    #[cfg(target_os = "linux")]
    fn guard(
        &self,
        agent: orgbot_proto::AgentId,
    ) -> Result<orgbot_providers::workspace_guard::WorkspaceGuard, String> {
        Ok(orgbot_providers::workspace_guard::WorkspaceGuard {
            project: self.root.clone(),
            state: self
                .guard_state
                .clone()
                .ok_or("Workspace protection is unavailable")?,
            agent: agent.to_string(),
            read_only: !self.writable,
        })
    }

    #[cfg(target_os = "linux")]
    fn handle_agent(&self, agent: orgbot_proto::AgentId, name: &str, args: &Value) -> ToolOutcome {
        let execute = || -> Result<ToolOutcome, String> {
            if self.guard_state.is_none() && !self.writable {
                return Ok(self.handle(name, args));
            }
            let guard = self.guard(agent)?;
            let prepared = guard.prepare()?;
            let attachment_fd = self
                .attachment_parent_fd
                .as_ref()
                .map(|fd| fd.try_clone())
                .transpose()
                .map_err(io_error)?;
            let host = Self::open_root(
                prepared.workspace.to_string_lossy().into_owned(),
                self.writable,
                attachment_fd,
                self.committed_attachment_ids.clone(),
            )
            .map_err(|e| e.to_string())?;
            if name == "workspace_write" {
                use rustix::fs::{Mode, OFlags};
                use sha2::{Digest, Sha256};
                let path = validate_relative(arg_string(args, "path")?)?;
                match rustix::fs::openat2(
                    &host.root_fd,
                    path_arg(&path)?,
                    OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
                    Mode::empty(),
                    no_symlink_resolution(),
                ) {
                    Ok(fd) => {
                        let current = read_open_file(fd)?;
                        let hash = format!("{:x}", Sha256::digest(current.as_bytes()));
                        if args.get("expected_sha256").and_then(Value::as_str)
                            != Some(hash.as_str())
                        {
                            return Err("File changed or expected_sha256 missing. Read this file again, then use its sha256 when writing.".into());
                        }
                    }
                    Err(rustix::io::Errno::NOENT) => {
                        if args.get("expected_sha256").is_some() {
                            return Err(
                                "File was removed after it was read; inspect before recreating it."
                                    .into(),
                            );
                        }
                    }
                    Err(error) => return Err(io_error(error)),
                }
            }
            if name == "workspace_read" {
                use sha2::{Digest, Sha256};
                let content = host.read(arg_string(args, "path"))?;
                let sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));
                return Ok(ToolOutcome {
                    content: serde_json::json!({"content":content,"sha256":sha256}).to_string(),
                    is_error: false,
                });
            }
            let mut outcome = host.handle(name, args);
            if !outcome.is_error && name == "workspace_mkdir" {
                outcome.content.push_str(" Staged in your working copy; original parent directories are created when a file is integrated.");
            }
            if !outcome.is_error
                && matches!(name, "workspace_write" | "workspace_import_attachment")
            {
                let status = guard.apply()?;
                if status
                    .get("deleted")
                    .and_then(Value::as_array)
                    .is_some_and(|v| !v.is_empty())
                {
                    outcome
                        .content
                        .push_str("\nOriginal file removals await owner approval.");
                }
            }
            Ok(outcome)
        };
        execute().unwrap_or_else(|content| ToolOutcome {
            content,
            is_error: true,
        })
    }

    #[cfg(target_os = "linux")]
    async fn run(&self, agent: orgbot_proto::AgentId, args: &Value) -> ToolOutcome {
        let execute = async {
            if !self.commands {
                return Err("Command execution is disabled for this workspace".to_owned());
            }
            let script = arg_string(args, "command")?;
            if script.len() > 64 * 1024 {
                return Err("Command exceeds 64 KiB".into());
            }
            let seconds = args
                .get("timeout_seconds")
                .map(|v| {
                    v.as_u64()
                        .filter(|n| (1..=1800).contains(n))
                        .ok_or("timeout_seconds must be 1–1800")
                })
                .transpose()?
                .unwrap_or(300);
            let guard = self.guard(agent)?;
            let setup_guard = guard.clone();
            tokio::task::spawn_blocking(move || setup_guard.prepare())
                .await
                .map_err(|e| e.to_string())??;
            let mut command = tokio::process::Command::from(guard.command("run")?);
            command
                .args(["--", "/bin/sh", "-c", script])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            for (name, _) in std::env::vars_os() {
                let key = name.to_string_lossy();
                if key.ends_with("_API_KEY")
                    || key.contains("TOKEN")
                    || key.starts_with("ORGBOT_MCP_")
                {
                    command.env_remove(name);
                }
            }
            let mut child = command.spawn().map_err(|e| e.to_string())?;
            let stdout = child.stdout.take().expect("piped stdout");
            let stderr = child.stderr.take().expect("piped stderr");
            let out = tokio::spawn(bounded_output(stdout));
            let err = tokio::spawn(bounded_output(stderr));
            let status = tokio::time::timeout(Duration::from_secs(seconds), child.wait()).await;
            let timed_out = status.is_err();
            if timed_out {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
            let output = out.await.map_err(|e| e.to_string())??;
            let error = err.await.map_err(|e| e.to_string())??;
            let success = matches!(status, Ok(Ok(code)) if code.success());
            let integration = if success {
                Some(
                    tokio::task::spawn_blocking(move || guard.apply())
                        .await
                        .map_err(|e| e.to_string())??,
                )
            } else {
                None
            };
            Ok(ToolOutcome {
                content: serde_json::json!({"stdout":output,"stderr":error,"timed_out":timed_out,
                "success":success,"integration":integration})
                .to_string(),
                is_error: !success,
            })
        };
        execute.await.unwrap_or_else(|content| ToolOutcome {
            content,
            is_error: true,
        })
    }

    #[cfg(target_os = "linux")]
    fn handle(&self, name: &str, args: &Value) -> ToolOutcome {
        let result = match name {
            "workspace_list" => self.list(arg_string(args, "path")),
            "workspace_read" => self.read(arg_string(args, "path")),
            "workspace_write" if self.writable => {
                self.write(arg_string(args, "path"), arg_string(args, "content"))
            }
            "workspace_mkdir" if self.writable => self.mkdir(arg_string(args, "path")),
            "workspace_git" => self.git(arg_string(args, "action")),
            "workspace_write" | "workspace_mkdir" => Err("This workspace is read-only.".into()),
            "workspace_import_attachment" if self.writable => {
                self.import_attachment(arg_string(args, "id"))
            }
            "workspace_import_attachment" => Err("This workspace is read-only.".into()),
            "workspace_run" => Err("Local workspace commands are unavailable.".into()),
            _ => Err("Unsupported local workspace operation.".into()),
        };
        match result {
            Ok(content) => ToolOutcome {
                content,
                is_error: false,
            },
            Err(message) => ToolOutcome {
                content: message,
                is_error: true,
            },
        }
    }

    #[cfg(target_os = "linux")]
    fn list(&self, raw: Result<&str, String>) -> Result<String, String> {
        use rustix::fd::AsFd;
        use rustix::fs::{AtFlags, Dir, FileType, OFlags};

        let relative = validate_relative(raw?)?;
        let dir_fd = rustix::fs::openat2(
            &self.root_fd,
            path_arg(&relative)?,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
            no_symlink_resolution(),
        )
        .map_err(io_error)?;
        if !FileType::from_raw_mode(rustix::fs::fstat(&dir_fd).map_err(io_error)?.st_mode).is_dir()
        {
            return Err("Workspace path is not a directory.".into());
        }
        let mut entries = Vec::new();
        let mut truncated = false;
        let mut iter = Dir::read_from(dir_fd.as_fd()).map_err(io_error)?;
        while let Some(entry) = iter.read() {
            let entry = entry.map_err(io_error)?;
            if entries.len() == MAX_LIST_ENTRIES {
                truncated = true;
                break;
            }
            let filename = entry.file_name();
            if filename.to_bytes() == b"." || filename.to_bytes() == b".." {
                continue;
            }
            let name = filename.to_string_lossy().into_owned();
            let metadata = rustix::fs::statat(dir_fd.as_fd(), filename, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(io_error)?;
            let kind = if FileType::from_raw_mode(metadata.st_mode).is_dir() {
                "directory"
            } else if FileType::from_raw_mode(metadata.st_mode).is_file() {
                "file"
            } else if FileType::from_raw_mode(metadata.st_mode).is_symlink() {
                "symlink"
            } else {
                "other"
            };
            entries.push(json!({
                "name": name,
                "kind": kind,
                "size": if FileType::from_raw_mode(metadata.st_mode).is_file() { Some(metadata.st_size) } else { None },
            }));
        }
        serde_json::to_string(&json!({"entries": entries, "truncated": truncated}))
            .map_err(|e| e.to_string())
    }

    #[cfg(target_os = "linux")]
    fn read(&self, raw: Result<&str, String>) -> Result<String, String> {
        use rustix::fs::OFlags;

        let relative = validate_relative(raw?)?;
        let fd = rustix::fs::openat2(
            &self.root_fd,
            path_arg(&relative)?,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            rustix::fs::Mode::empty(),
            no_symlink_resolution(),
        )
        .map_err(io_error)?;
        read_open_file(fd)
    }

    #[cfg(target_os = "linux")]
    fn write(
        &self,
        raw_path: Result<&str, String>,
        raw_content: Result<&str, String>,
    ) -> Result<String, String> {
        let relative = validate_relative(raw_path?)?;
        let content = raw_content?;
        if content.len() > MAX_FILE_BYTES {
            return Err("Workspace write exceeds the 1 MiB limit.".into());
        }
        let name = filename_arg(&relative)?;
        let parent = open_dir(&self.root_fd, relative.parent().unwrap_or(Path::new(".")))?;
        atomic_write_at(&parent, name, content)?;
        Ok(format!(
            "Wrote {} bytes to {}.",
            content.len(),
            relative.display()
        ))
    }

    #[cfg(target_os = "linux")]
    fn mkdir(&self, raw: Result<&str, String>) -> Result<String, String> {
        use rustix::fs::Mode;

        let relative = validate_relative(raw?)?;
        let name = filename_arg(&relative)?;
        let parent = open_dir(&self.root_fd, relative.parent().unwrap_or(Path::new(".")))?;
        rustix::fs::mkdirat(&parent, name, Mode::RWXU).map_err(io_error)?;
        Ok(format!("Created directory {}.", relative.display()))
    }

    #[cfg(target_os = "linux")]
    fn git(&self, action: Result<&str, String>) -> Result<String, String> {
        let action = action?;
        let args: &[&str] = match action {
            "status" => &[
                "status",
                "--short",
                "--branch",
                "--untracked-files=normal",
                "--no-renames",
            ],
            "diff" => &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--no-renames",
                "HEAD",
                "--",
                ".",
            ],
            "log" => &[
                "log",
                "-n",
                "20",
                "--oneline",
                "--decorate=no",
                "--no-color",
                "--no-show-signature",
            ],
            _ => return Err("Git action must be status, diff, or log.".into()),
        };
        let cwd = format!("/proc/self/fd/{}", self.root_fd.as_raw_fd());
        let mut root_command = git_command(&cwd);
        root_command
            .arg("rev-parse")
            .arg("--path-format=absolute")
            .arg("--show-toplevel");
        let root_result = run_bounded_git(&mut root_command, Duration::from_secs(5))?;
        if !root_result.success {
            return Err("Selected workspace is not inside a Git repository.".into());
        }
        let reported_root = String::from_utf8(root_result.stdout)
            .map_err(|_| "Git returned a non-UTF-8 repository path.")?;
        let reported_root = reported_root.trim_end_matches('\n');
        let expected_root = std::fs::canonicalize(&cwd).map_err(io_error)?;
        let actual_root = std::fs::canonicalize(reported_root)
            .map_err(|_| "Git repository root is unavailable.")?;
        if actual_root != expected_root {
            return Err(
                "Git access requires the selected workspace to be the repository root.".into(),
            );
        }
        let mut action_command = git_command(&cwd);
        action_command.args(args);
        let result = run_bounded_git(&mut action_command, Duration::from_secs(15))?;
        let output = String::from_utf8_lossy(&result.stdout).into_owned();
        if !result.success || !result.stderr.is_empty() {
            let detail = if result.stderr.is_empty() {
                "Git exited unsuccessfully.".into()
            } else {
                String::from_utf8_lossy(&result.stderr).trim().to_string()
            };
            return Err(format!("Git {action} failed: {detail}"));
        }
        Ok(output)
    }

    #[cfg(target_os = "linux")]
    fn import_attachment(&self, raw_id: Result<&str, String>) -> Result<String, String> {
        use rustix::fs::{AtFlags, Mode, OFlags};

        let id = raw_id?;
        if !valid_attachment_id(id) || !self.committed_attachment_ids.read().unwrap().contains(id) {
            return Err("Attachment id is not present on a committed message in this room.".into());
        }
        let attachment_parent = self
            .attachment_parent_fd
            .as_ref()
            .ok_or("Room attachment storage is unavailable.")?;
        let attachment_root = rustix::fs::openat2(
            attachment_parent,
            "attachments",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            no_symlink_resolution(),
        )
        .map_err(|_| "Room attachment storage is unavailable.")?;
        let attachment_dir = rustix::fs::openat2(
            &attachment_root,
            id,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            no_symlink_resolution(),
        )
        .map_err(|_| "Attachment original is unavailable.")?;
        let manifest_fd = rustix::fs::openat2(
            &attachment_dir,
            "manifest.json",
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
            no_symlink_resolution(),
        )
        .map_err(|_| "Attachment manifest is unavailable.")?;
        let manifest_bytes = read_fd_bounded(manifest_fd, 8192)?;
        let manifest: orgbot_proto::chat::ChatFile = serde_json::from_slice(&manifest_bytes)
            .map_err(|_| "Attachment manifest is invalid.")?;
        if manifest.id != id || manifest.bytes > MAX_ATTACHMENT_BYTES {
            return Err("Attachment manifest does not match the committed file.".into());
        }
        let mut original = open_validated_original(&attachment_dir, manifest.bytes)?;
        let import_dir = match rustix::fs::openat2(
            &self.root_fd,
            "imports",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            no_symlink_resolution(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => {
                rustix::fs::mkdirat(&self.root_fd, "imports", Mode::RWXU).map_err(io_error)?;
                rustix::fs::openat2(
                    &self.root_fd,
                    "imports",
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                    no_symlink_resolution(),
                )
                .map_err(io_error)?
            }
            Err(error) => return Err(io_error(error)),
        };
        let safe_name = safe_attachment_name(&manifest.name);
        for _ in 0..8 {
            let target_name = format!("{}-{safe_name}", uuid::Uuid::new_v4().simple());
            let target_fd = match rustix::fs::openat2(
                &import_dir,
                target_name.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::RUSR | Mode::WUSR,
                no_symlink_resolution(),
            ) {
                Ok(fd) => fd,
                Err(rustix::io::Errno::EXIST) => continue,
                Err(error) => return Err(io_error(error)),
            };
            let mut target = File::from(target_fd);
            let copied = match copy_validated_original(&mut original, &mut target, manifest.bytes) {
                Ok(copied) => copied,
                Err(error) => {
                    drop(target);
                    let _ =
                        rustix::fs::unlinkat(&import_dir, target_name.as_str(), AtFlags::empty());
                    return Err(error);
                }
            };
            return Ok(format!(
                "Imported {} bytes to imports/{target_name}.",
                copied
            ));
        }
        Err("Could not allocate a unique imported filename.".into())
    }
}

#[cfg(target_os = "linux")]
fn open_validated_original(
    attachment_dir: &std::os::fd::OwnedFd,
    expected_bytes: u64,
) -> Result<File, String> {
    use rustix::fs::{FileType, Mode, OFlags};

    let fd = rustix::fs::openat2(
        attachment_dir,
        "original",
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        no_symlink_resolution(),
    )
    .map_err(|_| "Attachment original is unavailable.")?;
    let metadata = rustix::fs::fstat(&fd).map_err(io_error)?;
    if !FileType::from_raw_mode(metadata.st_mode).is_file()
        || metadata.st_size < 0
        || metadata.st_size as u64 > MAX_ATTACHMENT_BYTES
        || metadata.st_size as u64 != expected_bytes
    {
        return Err("Attachment original is not a regular file within the 20 MiB limit.".into());
    }
    Ok(File::from(fd))
}

#[cfg(target_os = "linux")]
fn copy_validated_original(
    source: &mut File,
    target: &mut File,
    expected_bytes: u64,
) -> Result<u64, String> {
    let copied = std::io::copy(&mut (&mut *source).take(MAX_ATTACHMENT_BYTES + 1), target)
        .map_err(io_error)?;
    if copied != expected_bytes {
        return Err("Attachment size changed while it was being imported.".into());
    }
    target.sync_all().map_err(io_error)?;
    Ok(copied)
}

#[cfg(target_os = "linux")]
fn open_dir_from_absolute(path: PathBuf) -> anyhow::Result<std::os::fd::OwnedFd> {
    let path = std::fs::canonicalize(path)?;
    let path = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("room attachment directory must be UTF-8"))?;
    let path = Path::new(path);
    anyhow::ensure!(
        path.is_absolute(),
        "room attachment directory must be absolute"
    );
    let anchor = rustix::fs::open(
        "/",
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?;
    rustix::fs::openat2(
        &anchor,
        path.strip_prefix("/")
            .expect("absolute path")
            .to_str()
            .unwrap_or("."),
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
        rustix::fs::ResolveFlags::BENEATH | rustix::fs::ResolveFlags::NO_SYMLINKS,
    )
    .map_err(Into::into)
}

#[cfg(target_os = "linux")]
fn valid_attachment_id(id: &str) -> bool {
    id.strip_prefix("att_").is_some_and(|suffix| {
        suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

#[cfg(target_os = "linux")]
fn safe_attachment_name(name: &str) -> String {
    let basename = Path::new(name)
        .file_name()
        .and_then(|part| part.to_str())
        .unwrap_or("attachment");
    let mut clean = String::new();
    for character in basename.chars().take(96) {
        clean.push(
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            },
        );
    }
    while clean.starts_with('.') {
        clean.remove(0);
    }
    if clean.is_empty() {
        "attachment".into()
    } else {
        clean
    }
}

#[cfg(target_os = "linux")]
fn read_fd_bounded(fd: std::os::fd::OwnedFd, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    File::from(fd)
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > limit {
        return Err("Attachment manifest exceeds its size limit.".into());
    }
    Ok(bytes)
}

#[cfg(target_os = "linux")]
struct GitOutput {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[cfg(target_os = "linux")]
fn git_command(cwd: &str) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .arg("--no-pager")
        .arg("--no-optional-locks")
        .env_clear()
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_COUNT", "3")
        .env("GIT_CONFIG_KEY_0", "core.hooksPath")
        .env("GIT_CONFIG_VALUE_0", "/dev/null")
        .env("GIT_CONFIG_KEY_1", "core.fsmonitor")
        .env("GIT_CONFIG_VALUE_1", "false")
        .env("GIT_CONFIG_KEY_2", "diff.external")
        .env("GIT_CONFIG_VALUE_2", "")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}

#[cfg(target_os = "linux")]
fn run_bounded_git(command: &mut Command, timeout: Duration) -> Result<GitOutput, String> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "Git is unavailable or could not be started.".to_string())?;
    let overflow = Arc::new(AtomicBool::new(false));
    let stdout = read_pipe(
        child.stdout.take().expect("stdout piped"),
        MAX_GIT_OUTPUT_BYTES,
        Arc::clone(&overflow),
    );
    let stderr = read_pipe(
        child.stderr.take().expect("stderr piped"),
        8192,
        Arc::clone(&overflow),
    );
    let deadline = Instant::now() + timeout;
    let status = loop {
        if overflow.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout.join();
            let _ = stderr.join();
            return Err("Git output exceeded its limit.".into());
        }
        match child.try_wait() {
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout.join();
                let _ = stderr.join();
                return Err(io_error(error));
            }
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout.join();
                let _ = stderr.join();
                return Err("Git operation timed out.".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    let stdout = stdout
        .join()
        .map_err(|_| "Git output reader failed.")?
        .map_err(io_error)?;
    let stderr = stderr
        .join()
        .map_err(|_| "Git error reader failed.")?
        .map_err(io_error)?;
    if overflow.load(Ordering::Acquire) {
        return Err("Git output exceeded its limit.".into());
    }
    Ok(GitOutput {
        success: status.success(),
        stdout,
        stderr,
    })
}

#[cfg(target_os = "linux")]
fn read_pipe<R: Read + Send + 'static>(
    mut pipe: R,
    limit: usize,
    overflow: Arc<AtomicBool>,
) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.by_ref()
            .take((limit + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            overflow.store(true, Ordering::Release);
            bytes.truncate(limit);
        }
        Ok(bytes)
    })
}

#[cfg(target_os = "linux")]
fn no_symlink_resolution() -> rustix::fs::ResolveFlags {
    rustix::fs::ResolveFlags::BENEATH | rustix::fs::ResolveFlags::NO_SYMLINKS
}

#[cfg(target_os = "linux")]
fn path_arg(path: &Path) -> Result<&str, String> {
    if path.as_os_str().is_empty() {
        Ok(".")
    } else {
        path.to_str()
            .ok_or_else(|| "Workspace paths must be valid UTF-8.".into())
    }
}

#[cfg(target_os = "linux")]
fn filename_arg(path: &Path) -> Result<&str, String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| *name != "." && *name != "..")
        .ok_or_else(|| "Workspace path must name a file or directory.".into())
}

#[cfg(target_os = "linux")]
fn open_dir(root: &std::os::fd::OwnedFd, path: &Path) -> Result<std::os::fd::OwnedFd, String> {
    rustix::fs::openat2(
        root,
        path_arg(path)?,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
        no_symlink_resolution(),
    )
    .map_err(io_error)
}

#[cfg(target_os = "linux")]
fn read_open_file(fd: std::os::fd::OwnedFd) -> Result<String, String> {
    use rustix::fs::FileType;

    let metadata = rustix::fs::fstat(&fd).map_err(io_error)?;
    if !FileType::from_raw_mode(metadata.st_mode).is_file() {
        return Err("Workspace path is not a regular file.".into());
    }
    if metadata.st_size > MAX_FILE_BYTES as i64 {
        return Err("Workspace file exceeds the 1 MiB read limit.".into());
    }
    let mut bytes = Vec::with_capacity(metadata.st_size.max(0) as usize);
    File::from(fd)
        .take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err("Workspace file exceeds the 1 MiB read limit.".into());
    }
    String::from_utf8(bytes).map_err(|_| "Workspace file is not valid UTF-8.".into())
}

#[cfg(target_os = "linux")]
fn atomic_write_at(parent: &std::os::fd::OwnedFd, name: &str, content: &str) -> Result<(), String> {
    use rustix::fs::{AtFlags, FileType, OFlags};

    let mode = match rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(meta) if FileType::from_raw_mode(meta.st_mode).is_symlink() => {
            return Err("Refusing to overwrite a symlink target.".into());
        }
        Ok(meta) if !FileType::from_raw_mode(meta.st_mode).is_file() => {
            return Err("Workspace target is not a regular file.".into());
        }
        Ok(meta) => Some(rustix::fs::Mode::from_raw_mode(meta.st_mode & 0o777)),
        Err(rustix::io::Errno::NOENT) => None,
        Err(e) => return Err(io_error(e)),
    }
    .unwrap_or(rustix::fs::Mode::from_raw_mode(0o600));

    let mut temp_name = None;
    let mut file = None;
    for _ in 0..4 {
        let candidate = format!(".orgbot-{}.tmp", uuid::Uuid::new_v4().simple());
        match rustix::fs::openat2(
            parent,
            candidate.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            no_symlink_resolution(),
        ) {
            Ok(fd) => {
                if let Err(error) = rustix::fs::fchmod(&fd, mode) {
                    let _ = rustix::fs::unlinkat(parent, candidate.as_str(), AtFlags::empty());
                    return Err(io_error(error));
                }
                temp_name = Some(candidate);
                file = Some(File::from(fd));
                break;
            }
            Err(rustix::io::Errno::EXIST) => continue,
            Err(e) => return Err(io_error(e)),
        }
    }
    let temp_name = temp_name.ok_or("Could not create a temporary workspace file.")?;
    let mut file = file.expect("temporary file name and descriptor are set together");
    let result = (|| {
        file.write_all(content.as_bytes()).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        drop(file);
        rustix::fs::renameat(parent, temp_name.as_str(), parent, name).map_err(io_error)
    })();
    if result.is_err() {
        let _ = rustix::fs::unlinkat(parent, temp_name.as_str(), AtFlags::empty());
    }
    result
}

fn arg_string<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Missing string argument `{key}`."))
}

fn validate_relative(raw: &str) -> Result<PathBuf, String> {
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err("Workspace paths must be relative to the selected root.".into());
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir if raw == "." || raw.is_empty() => {}
            Component::CurDir => normalized.push("."),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("Workspace paths cannot contain absolute or parent components.".into());
            }
        }
    }
    Ok(normalized)
}

fn io_error(error: impl std::fmt::Display) -> String {
    format!("Workspace file operation failed: {error}")
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use orgbot_proto::AgentId;
    use std::fs;
    fn temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!("orgbot-workspace-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        path
    }

    // File-boundary unit tests below deliberately call the pinned-fd handler.
    // Protected provider integration is covered separately with real bubblewrap.
    async fn serve_files(host: LocalWorkspaceHost, mut rx: mpsc::Receiver<WorkspaceCall>) {
        while let Some(call) = rx.recv().await {
            let _ = call.reply.send(host.handle(&call.name, &call.arguments));
        }
    }

    async fn host(root: &Path, writable: bool) -> (Workspace, tokio::task::JoinHandle<()>) {
        let host = LocalWorkspaceHost::open_root(
            root.to_string_lossy().into_owned(),
            writable,
            None,
            Arc::new(RwLock::new(std::collections::HashSet::new())),
        )
        .unwrap();
        let root = host.root.clone();
        let (tx, rx) = mpsc::channel(8);
        let workspace = Workspace::new_without_attachment_import(
            WorkspaceSpec {
                label: "test".into(),
                root: root.to_string_lossy().into_owned(),
                writable,
                commands: false,
            },
            tx,
        );
        let task = tokio::spawn(serve_files(host, rx));
        (workspace, task)
    }

    async fn capable_host(
        root: &Path,
        writable: bool,
        capabilities: WorkspaceCapabilities,
    ) -> (Workspace, tokio::task::JoinHandle<()>) {
        let host = LocalWorkspaceHost::open_root(
            root.to_string_lossy().into_owned(),
            writable,
            None,
            Arc::new(RwLock::new(std::collections::HashSet::new())),
        )
        .unwrap();
        let (tx, rx) = mpsc::channel(8);
        let workspace = Workspace::with_capabilities(
            WorkspaceSpec {
                label: "test".into(),
                root: host.root.to_string_lossy().into_owned(),
                writable,
                commands: false,
            },
            tx,
            capabilities,
        );
        let task = tokio::spawn(serve_files(host, rx));
        (workspace, task)
    }

    fn protected_host(root: &Path, state: &Path, commands: bool) -> LocalWorkspaceHost {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(state, fs::Permissions::from_mode(0o700)).unwrap();
        let mut host = LocalWorkspaceHost::open_root(
            root.to_string_lossy().into_owned(),
            true,
            None,
            Arc::new(RwLock::new(std::collections::HashSet::new())),
        )
        .unwrap();
        host.guard_state = Some(state.to_path_buf());
        host.commands = commands;
        host
    }

    #[tokio::test]
    async fn protected_api_commands_preserve_originals_and_bound_runtime_and_output() {
        let root = temp_dir();
        let state = temp_dir();
        fs::write(root.join("keep.txt"), "original").unwrap();
        let host = protected_host(&root, &state, true);
        let agent = AgentId::new();
        let command = format!(
            "printf added > added.txt; rm keep.txt; if rm '{}'; then exit 9; fi; printf done",
            root.join("keep.txt").display()
        );
        let result = host.run(agent, &json!({"command":command})).await;
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(
            fs::read_to_string(root.join("keep.txt")).unwrap(),
            "original"
        );
        assert_eq!(fs::read_to_string(root.join("added.txt")).unwrap(), "added");
        let result: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(result["integration"]["pending_delete"], json!(["keep.txt"]));
        let timeout = host
            .run(agent, &json!({"command":"sleep 20", "timeout_seconds":1}))
            .await;
        assert!(timeout.is_error);
        assert_eq!(
            serde_json::from_str::<Value>(&timeout.content).unwrap()["timed_out"],
            true
        );
        let output = host
            .run(
                agent,
                &json!({"command":"head -c 100000 /dev/zero | tr '\\0' x"}),
            )
            .await;
        assert!(!output.is_error, "{}", output.content);
        let output: Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(output["stdout"].as_str().unwrap().len(), 32768);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(state).unwrap();
    }

    #[tokio::test]
    async fn protected_api_write_rejects_stale_reads_and_unreadable_existing_files() {
        let root = temp_dir();
        let state = temp_dir();
        fs::write(root.join("shared.txt"), "first").unwrap();
        fs::write(root.join("large.txt"), "a".repeat(MAX_FILE_BYTES + 1)).unwrap();
        let host = protected_host(&root, &state, false);
        let agent = AgentId::new();
        let read = host.handle_agent(agent, "workspace_read", &json!({"path":"shared.txt"}));
        assert!(!read.is_error, "{}", read.content);
        let value: Value = serde_json::from_str(&read.content).unwrap();
        fs::write(root.join("shared.txt"), "another agent").unwrap();
        let stale = host.handle_agent(agent, "workspace_write", &json!({"path":"shared.txt", "content":"stale edit", "expected_sha256":value["sha256"]}));
        assert!(stale.is_error, "{}", stale.content);
        assert_eq!(
            fs::read_to_string(root.join("shared.txt")).unwrap(),
            "another agent"
        );
        let large = host.handle_agent(
            agent,
            "workspace_write",
            &json!({"path":"large.txt", "content":"truncated"}),
        );
        assert!(large.is_error, "{}", large.content);
        assert_eq!(
            fs::metadata(root.join("large.txt")).unwrap().len(),
            (MAX_FILE_BYTES + 1) as u64
        );
        let added = host.handle_agent(
            agent,
            "workspace_write",
            &json!({"path":"fresh.txt", "content":"new file"}),
        );
        assert!(!added.is_error, "{}", added.content);
        assert_eq!(
            fs::read_to_string(root.join("fresh.txt")).unwrap(),
            "new file"
        );
        assert!(host.run(agent, &json!({"command":"true"})).await.is_error);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(state).unwrap();
    }

    #[tokio::test]
    async fn local_workspace_reads_writes_makes_directories_and_enforces_bounds() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir();
        fs::write(root.join("hello.txt"), "hello").unwrap();
        fs::set_permissions(root.join("hello.txt"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(root.join("too-large.txt"), vec![b'x'; MAX_FILE_BYTES + 1]).unwrap();
        for index in 0..=MAX_LIST_ENTRIES {
            fs::write(root.join(format!("entry-{index}")), "").unwrap();
        }
        let (workspace, host_task) = host(&root, true).await;
        let agent = AgentId::new();
        let read = workspace
            .call(agent, "workspace_read", &json!({"path":"hello.txt"}))
            .await;
        assert!(!read.is_error);
        assert_eq!(read.content, "hello");
        assert!(
            workspace
                .call(agent, "workspace_read", &json!({"path":"too-large.txt"}))
                .await
                .is_error
        );
        let listing = workspace
            .call(agent, "workspace_list", &json!({"path":""}))
            .await;
        let listing: Value = serde_json::from_str(&listing.content).unwrap();
        assert_eq!(
            listing["entries"].as_array().unwrap().len(),
            MAX_LIST_ENTRIES
        );
        assert_eq!(listing["truncated"], true);
        let write = workspace
            .call(
                agent,
                "workspace_write",
                &json!({"path":"nested.txt","content":"updated"}),
            )
            .await;
        assert!(!write.is_error);
        assert_eq!(
            fs::read_to_string(root.join("nested.txt")).unwrap(),
            "updated"
        );
        assert_eq!(
            fs::metadata(root.join("nested.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let update = workspace
            .call(
                agent,
                "workspace_write",
                &json!({"path":"hello.txt","content":"updated hello"}),
            )
            .await;
        assert!(!update.is_error);
        assert_eq!(
            fs::metadata(root.join("hello.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert!(
            workspace
                .call(
                    agent,
                    "workspace_write",
                    &json!({"path":"big","content":"x".repeat(MAX_FILE_BYTES + 1)})
                )
                .await
                .is_error
        );
        let mkdir = workspace
            .call(agent, "workspace_mkdir", &json!({"path":"new-dir"}))
            .await;
        assert!(!mkdir.is_error);
        assert!(root.join("new-dir").is_dir());
        host_task.abort();
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn local_workspace_rejects_traversal_absolute_paths_and_read_only_mutations() {
        let root = temp_dir();
        let (workspace, host_task) = host(&root, false).await;
        let agent = AgentId::new();
        for path in ["../escape", "/tmp/escape"] {
            assert!(
                workspace
                    .call(agent, "workspace_read", &json!({"path":path}))
                    .await
                    .is_error
            );
        }
        assert!(
            workspace
                .call(agent, "workspace_write", &json!({"path":"x","content":"y"}))
                .await
                .is_error
        );
        assert!(
            workspace
                .call(agent, "workspace_mkdir", &json!({"path":"x"}))
                .await
                .is_error
        );
        assert!(!workspace
            .definitions()
            .iter()
            .any(|tool| tool.name == "workspace_import_attachment"));
        host_task.abort();
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn escaping_symlinks_are_rejected_and_write_does_not_follow_target_symlink() {
        use std::os::unix::fs::symlink;
        let root = temp_dir();
        let outside = temp_dir();
        fs::write(outside.join("secret"), "private").unwrap();
        symlink(&outside, root.join("outside")).unwrap();
        symlink(outside.join("secret"), root.join("target-link")).unwrap();
        let (workspace, host_task) = host(&root, true).await;
        let agent = AgentId::new();
        assert!(
            workspace
                .call(agent, "workspace_read", &json!({"path":"outside/secret"}))
                .await
                .is_error
        );
        assert!(
            workspace
                .call(
                    agent,
                    "workspace_write",
                    &json!({"path":"target-link","content":"changed"})
                )
                .await
                .is_error
        );
        assert_eq!(
            fs::read_to_string(outside.join("secret")).unwrap(),
            "private"
        );
        host_task.abort();
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pinned_parent_and_open_file_survive_ancestor_symlink_swap() {
        use rustix::fs::{OFlags, ResolveFlags};
        use std::os::unix::fs::symlink;

        let root = temp_dir();
        let outside = temp_dir();
        let parent_path = root.join("parent");
        fs::create_dir(&parent_path).unwrap();
        fs::write(parent_path.join("secret.txt"), "from pinned directory").unwrap();
        fs::write(outside.join("secret.txt"), "outside file").unwrap();
        let host = LocalWorkspaceHost::open_root(
            root.to_string_lossy().into_owned(),
            true,
            None,
            Arc::new(RwLock::new(std::collections::HashSet::new())),
        )
        .unwrap();
        let parent = open_dir(&host.root_fd, Path::new("parent")).unwrap();
        let opened = rustix::fs::openat2(
            &host.root_fd,
            "parent/secret.txt",
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            rustix::fs::Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        )
        .unwrap();

        fs::rename(&parent_path, root.join("pinned")).unwrap();
        symlink(&outside, &parent_path).unwrap();

        assert_eq!(read_open_file(opened).unwrap(), "from pinned directory");
        atomic_write_at(&parent, "result.txt", "still in pinned directory").unwrap();
        assert_eq!(
            fs::read_to_string(root.join("pinned/result.txt")).unwrap(),
            "still in pinned directory"
        );
        assert!(!outside.join("result.txt").exists());
        assert_eq!(
            fs::read_to_string(outside.join("secret.txt")).unwrap(),
            "outside file"
        );
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn git_status_diff_and_log_are_available_without_shell_access() {
        let root = temp_dir();
        let init = Command::new("git").arg("init").arg(&root).output().unwrap();
        assert!(init.status.success());
        for (key, value) in [
            ("user.name", "Workspace test"),
            ("user.email", "test@example.invalid"),
        ] {
            assert!(Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["config", key, value])
                .status()
                .unwrap()
                .success());
        }
        fs::write(root.join("tracked.txt"), "before\n").unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["add", "tracked.txt"])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["commit", "--quiet", "-m", "initial"])
            .status()
            .unwrap()
            .success());
        fs::write(root.join("tracked.txt"), "after\n").unwrap();
        let (workspace, host_task) = capable_host(
            &root,
            false,
            WorkspaceCapabilities {
                git_read: true,
                attachment_import: false,
            },
        )
        .await;
        let agent = AgentId::new();
        for (action, expected) in [
            ("status", "tracked.txt"),
            ("diff", "+after"),
            ("log", "initial"),
        ] {
            let result = workspace
                .call(agent, "workspace_git", &json!({"action":action}))
                .await;
            assert!(!result.is_error, "{action} failed: {}", result.content);
            assert!(
                result.content.contains(expected),
                "{action} omitted {expected}: {}",
                result.content
            );
            assert!(result.content.len() <= MAX_GIT_OUTPUT_BYTES);
        }
        assert!(
            workspace
                .call(agent, "workspace_git", &json!({"action":"config"}))
                .await
                .is_error
        );
        assert!(!workspace
            .definitions()
            .iter()
            .any(|tool| tool.name == "workspace_run"));
        host_task.abort();
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn attachment_import_requires_committed_room_id_and_never_overwrites() {
        let state_parent = temp_dir();
        let workspace_root = temp_dir();
        let attachment_id = "att_0123456789abcdef0123456789abcdef";
        let attachment_dir = state_parent.join("attachments").join(attachment_id);
        fs::create_dir_all(&attachment_dir).unwrap();
        fs::write(attachment_dir.join("original"), b"room-file").unwrap();
        fs::write(
            attachment_dir.join("manifest.json"),
            serde_json::to_vec(&orgbot_proto::chat::ChatFile {
                id: attachment_id.into(),
                name: "../../safe file.txt".into(),
                mime_type: "text/plain".into(),
                bytes: 9,
                has_text: false,
                images: 0,
                note: String::new(),
            })
            .unwrap(),
        )
        .unwrap();
        let authorized = Arc::new(RwLock::new(
            [attachment_id.to_string()].into_iter().collect(),
        ));
        let state_fd = open_dir_from_absolute(state_parent.clone()).unwrap();
        let host = LocalWorkspaceHost::open_root(
            workspace_root.to_string_lossy().into_owned(),
            true,
            Some(state_fd),
            authorized,
        )
        .unwrap();
        let (tx, rx) = mpsc::channel(8);
        let workspace = Workspace::with_capabilities(
            WorkspaceSpec {
                label: "test".into(),
                root: workspace_root.to_string_lossy().into_owned(),
                writable: true,
                commands: false,
            },
            tx,
            WorkspaceCapabilities {
                attachment_import: true,
                git_read: true,
            },
        );
        let host_task = tokio::spawn(serve_files(host, rx));
        let agent = AgentId::new();
        let guessed = workspace
            .call(
                agent,
                "workspace_import_attachment",
                &json!({"id":"att_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}),
            )
            .await;
        assert!(guessed.is_error);
        let first = workspace
            .call(
                agent,
                "workspace_import_attachment",
                &json!({"id":attachment_id}),
            )
            .await;
        assert!(!first.is_error, "{}", first.content);
        let path = first
            .content
            .strip_prefix("Imported 9 bytes to ")
            .unwrap()
            .trim_end_matches('.');
        assert_eq!(fs::read(workspace_root.join(path)).unwrap(), b"room-file");
        let second = workspace
            .call(
                agent,
                "workspace_import_attachment",
                &json!({"id":attachment_id}),
            )
            .await;
        assert!(!second.is_error, "{}", second.content);
        assert_ne!(first.content, second.content);
        let _ = fs::remove_dir_all(workspace_root.join("imports"));
        host_task.abort();
        let _ = fs::remove_dir_all(workspace_root);
        let _ = fs::remove_dir_all(state_parent);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn validated_attachment_descriptor_survives_original_path_replacement() {
        use rustix::fs::Mode;

        let attachment_dir = temp_dir();
        let original_path = attachment_dir.join("original");
        fs::write(&original_path, b"room-file").unwrap();
        let attachment_fd = open_dir_from_absolute(attachment_dir.clone()).unwrap();
        let mut source = open_validated_original(&attachment_fd, 9).unwrap();

        fs::rename(&original_path, attachment_dir.join("saved-original")).unwrap();
        rustix::fs::mkfifoat(&attachment_fd, "original", Mode::RUSR | Mode::WUSR).unwrap();
        assert!(open_validated_original(&attachment_fd, 9).is_err());

        let imported_path = attachment_dir.join("imported");
        let mut imported = File::create(&imported_path).unwrap();
        assert_eq!(
            copy_validated_original(&mut source, &mut imported, 9).unwrap(),
            9
        );
        assert_eq!(fs::read(imported_path).unwrap(), b"room-file");
        let _ = fs::remove_dir_all(attachment_dir);
    }

    #[tokio::test]
    async fn fifo_read_is_rejected_without_blocking_and_large_reads_stay_bounded() {
        use rustix::fs::Mode;
        use std::time::Duration;

        let root = temp_dir();
        fs::write(root.join("large.txt"), vec![b'x'; MAX_FILE_BYTES + 1]).unwrap();
        let fixture = LocalWorkspaceHost::open_root(
            root.to_string_lossy().into_owned(),
            false,
            None,
            Arc::new(RwLock::new(std::collections::HashSet::new())),
        )
        .unwrap();
        rustix::fs::mkfifoat(&fixture.root_fd, "pipe", Mode::RUSR | Mode::WUSR).unwrap();
        let (workspace, host_task) = host(&root, false).await;
        let agent = AgentId::new();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            workspace.call(agent, "workspace_read", &json!({"path":"pipe"})),
        )
        .await
        .expect("FIFO reads must not block");
        assert!(result.is_error);
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            workspace.call(agent, "workspace_read", &json!({"path":"large.txt"})),
        )
        .await
        .expect("oversized file reads must be rejected promptly");
        assert!(result.is_error);
        host_task.abort();
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn stale_and_cancelled_receivers_fail_cleanly() {
        let root = temp_dir();
        let (workspace, host_task) = host(&root, true).await;
        host_task.abort();
        let result = workspace
            .call(AgentId::new(), "workspace_list", &json!({"path":""}))
            .await;
        assert!(result.is_error);

        let (tx, rx) = mpsc::channel(1);
        let (reply, response) = tokio::sync::oneshot::channel();
        tx.try_send(WorkspaceCall {
            agent: AgentId::new(),
            name: "workspace_write".into(),
            arguments: json!({"path":"cancelled.txt", "content":"must not be written"}),
            reply,
        })
        .unwrap();
        // A canceled queued mutation must not have a side effect.
        drop(tx);
        drop(response);
        let host = Arc::new(
            LocalWorkspaceHost::open_root(
                root.to_string_lossy().into_owned(),
                true,
                None,
                Arc::new(RwLock::new(std::collections::HashSet::new())),
            )
            .unwrap(),
        );
        let task = tokio::spawn(host.serve(rx));
        tokio::task::yield_now().await;
        assert!(!root.join("cancelled.txt").exists());
        task.abort();
        let _ = fs::remove_dir_all(root);
    }
}
