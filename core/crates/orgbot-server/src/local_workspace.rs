//! Bounded local file tools for an explicitly selected room workspace.
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use orgbot_runtime::workspace::{Workspace, WorkspaceCall, WorkspaceSpec};
use orgbot_tools::ToolOutcome;
use serde_json::{json, Value};
use tokio::sync::mpsc;

const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_LIST_ENTRIES: usize = 500;

/// A validated local workspace and its request channel.
#[derive(Debug)]
pub struct LocalWorkspaceHost {
    root: PathBuf,
    writable: bool,
    #[cfg(target_os = "linux")]
    root_fd: std::os::fd::OwnedFd,
}

impl LocalWorkspaceHost {
    /// Validate the requested root and attach a runtime workspace to a session.
    #[cfg(target_os = "linux")]
    pub async fn attach(
        spec: WorkspaceSpec,
        session: &mut crate::session::RoomSession,
    ) -> anyhow::Result<tokio::task::JoinHandle<()>> {
        anyhow::ensure!(
            !spec.commands,
            "Local workspace commands are unavailable; set workspace.commands = false"
        );
        let host = tokio::task::spawn_blocking({
            let root = spec.root.clone();
            let writable = spec.writable;
            move || Self::open_root(root, writable)
        })
        .await
        .map_err(|e| anyhow::anyhow!("local workspace setup failed: {e}"))??;
        let root = host.root.clone();
        let mut runtime_spec = spec;
        runtime_spec.root = root.display().to_string();
        runtime_spec.commands = false;
        let (tx, rx) = mpsc::channel::<WorkspaceCall>(8);
        session.attach_workspace(Workspace::new_without_attachment_import(runtime_spec, tx));
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
    fn open_root(root: String, writable: bool) -> anyhow::Result<Self> {
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
            root_fd,
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
            let name = call.name;
            let arguments = call.arguments;
            let outcome = tokio::task::spawn_blocking(move || host.handle(&name, &arguments))
                .await
                .unwrap_or_else(|e| ToolOutcome {
                    content: format!("Local workspace worker failed: {e}"),
                    is_error: true,
                });
            let _ = call.reply.send(outcome);
        }
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
            "workspace_write" | "workspace_mkdir" => Err("This workspace is read-only.".into()),
            "workspace_import_attachment" => Err(
                "Attachment import is unavailable because no attachment bridge is connected."
                    .into(),
            ),
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
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("orgbot-workspace-{nonce}"));
        fs::create_dir(&path).unwrap();
        path
    }

    async fn host(root: &Path, writable: bool) -> (Workspace, tokio::task::JoinHandle<()>) {
        let host =
            LocalWorkspaceHost::open_root(root.to_string_lossy().into_owned(), writable).unwrap();
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
        let task = tokio::spawn(Arc::new(host).serve(rx));
        (workspace, task)
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
        let host =
            LocalWorkspaceHost::open_root(root.to_string_lossy().into_owned(), true).unwrap();
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

    #[tokio::test]
    async fn fifo_read_is_rejected_without_blocking_and_large_reads_stay_bounded() {
        use rustix::fs::Mode;
        use std::time::Duration;

        let root = temp_dir();
        fs::write(root.join("large.txt"), vec![b'x'; MAX_FILE_BYTES + 1]).unwrap();
        let fixture =
            LocalWorkspaceHost::open_root(root.to_string_lossy().into_owned(), false).unwrap();
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
            LocalWorkspaceHost::open_root(root.to_string_lossy().into_owned(), true).unwrap(),
        );
        let task = tokio::spawn(host.serve(rx));
        tokio::task::yield_now().await;
        assert!(!root.join("cancelled.txt").exists());
        task.abort();
        let _ = fs::remove_dir_all(root);
    }
}
