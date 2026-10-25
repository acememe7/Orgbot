//! Per-user Linux supervision for persistent OrgBots rooms.
//!
//! Room TOML and the connection descriptor live in a private data directory.
//! Provider credentials are stored in the user's Secret Service collection and
//! are only materialized in the service process memory. When systemd user
//! services are unavailable, a detached local supervisor restarts the room
//! process until an explicit stop marker is written.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use fs2::FileExt as LockExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const SECRET_APP: &str = "orgbots";
const SECRET_PURPOSE: &str = "room-environment";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomServiceDescriptor {
    pub room_id: String,
    pub url: String,
    pub token: String,
    pub unit: String,
    pub systemd: bool,
    pub has_environment: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoomServiceStatus {
    pub running: bool,
    pub descriptor: Option<RoomServiceDescriptor>,
}

fn room_id(directory: &Path) -> anyhow::Result<String> {
    let name = directory
        .file_name()
        .and_then(|part| part.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid room service directory"))?;
    anyhow::ensure!(
        !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "invalid room service identifier"
    );
    Ok(name.to_owned())
}

fn paths(directory: &Path) -> anyhow::Result<(PathBuf, String)> {
    let directory = directory.canonicalize()?;
    let _ = room_id(&directory)?;
    let hash = Sha256::digest(directory.as_os_str().as_encoded_bytes());
    let unit = format!("orgbots-room-{}.service", hex_prefix(&hash, 16));
    Ok((directory, unit))
}

fn lock_file(directory: &Path, name: &str) -> anyhow::Result<std::fs::File> {
    let path = directory.join(name);
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        anyhow::ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "invalid room service lock"
        );
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

fn acquire_lock(directory: &Path, name: &str) -> anyhow::Result<std::fs::File> {
    let file = lock_file(directory, name)?;
    file.lock_exclusive()?;
    Ok(file)
}

fn try_acquire_lock(directory: &Path, name: &str) -> anyhow::Result<Option<std::fs::File>> {
    let file = lock_file(directory, name)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn wait_for_lock(directory: &Path, name: &str, timeout: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let file = lock_file(directory, name)?;
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    anyhow::bail!("room service did not stop in time");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn stop_fallback(directory: &Path) -> anyhow::Result<()> {
    write_private(&directory.join("stop"), b"stop")?;
    wait_for_lock(directory, "monitor.lock", Duration::from_secs(12))?;
    let _ = fs::remove_file(directory.join("supervisor.pid"));
    Ok(())
}

fn clear_stop_marker(directory: &Path) -> anyhow::Result<()> {
    match fs::remove_file(directory.join("stop")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn secret_id(directory: &Path) -> String {
    let digest = Sha256::digest(directory.as_os_str().as_encoded_bytes());
    hex_prefix(&digest, digest.len())
}

fn hex_prefix(bytes: &[u8], count: usize) -> String {
    bytes
        .iter()
        .take(count)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn ensure_private_directory(directory: &Path) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(directory)?;
    let metadata = fs::symlink_metadata(directory)?;
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "room service directory must be a real directory"
    );
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    Ok(directory.canonicalize()?)
}

fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)?;
    Ok(())
}

fn read_descriptor(directory: &Path) -> anyhow::Result<RoomServiceDescriptor> {
    let path = directory.join("service.json");
    let metadata = fs::symlink_metadata(&path)?;
    anyhow::ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "invalid service descriptor"
    );
    anyhow::ensure!(
        metadata.permissions().mode() & 0o077 == 0,
        "service descriptor permissions are too broad"
    );
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn systemctl(args: &[&str]) -> anyhow::Result<std::process::Output> {
    let output = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()?;
    Ok(output)
}

fn systemd_user_available() -> bool {
    if std::env::var_os("ORGBOT_DISABLE_SYSTEMD").is_some() {
        return false;
    }
    systemctl(&["show-environment"])
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn user_unit_directory() -> anyhow::Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| anyhow::anyhow!("home directory is not available"))?;
    let directory = base.join("systemd/user");
    fs::create_dir_all(&directory)?;
    Ok(directory)
}

fn user_autostart_directory() -> anyhow::Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| anyhow::anyhow!("home directory is not available"))?;
    let directory = base.join("autostart");
    fs::create_dir_all(&directory)?;
    Ok(directory)
}

fn desktop_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
}

fn install_autostart(directory: &Path) -> anyhow::Result<PathBuf> {
    let executable = std::env::current_exe()?.canonicalize()?;
    let path = user_autostart_directory()?.join(format!(
        "orgbots-room-{}.desktop",
        hex_prefix(
            &Sha256::digest(directory.as_os_str().as_encoded_bytes()),
            16
        )
    ));
    let exec = format!(
        "\"{}\" room-service-supervise --directory \"{}\"",
        desktop_escape(&executable.to_string_lossy()),
        desktop_escape(&directory.to_string_lossy())
    );
    let body = format!(
        "[Desktop Entry]\nType=Application\nName=OrgBots room service\nExec={exec}\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n"
    );
    write_private(&path, body.as_bytes())?;
    Ok(path)
}

fn remove_autostart(directory: &Path) {
    if let Ok(path) = user_autostart_directory() {
        let file = path.join(format!(
            "orgbots-room-{}.desktop",
            hex_prefix(
                &Sha256::digest(directory.as_os_str().as_encoded_bytes()),
                16
            )
        ));
        let _ = fs::remove_file(file);
    }
}

fn systemd_quote(argument: &str) -> String {
    let mut quoted = String::from("\"");
    for ch in argument.chars() {
        match ch {
            '\\' | '"' => {
                quoted.push('\\');
                quoted.push(ch);
            }
            '%' => quoted.push_str("%%"),
            '\n' | '\r' => quoted.push(' '),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

fn install_unit(directory: &Path, unit: &str) -> anyhow::Result<PathBuf> {
    let executable = std::env::current_exe()?.canonicalize()?;
    let unit_dir = user_unit_directory()?;
    let path = unit_dir.join(unit);
    let service_args = [
        executable.to_string_lossy().to_string(),
        "room-service-run".to_owned(),
        "--directory".to_owned(),
        directory.to_string_lossy().to_string(),
    ];
    let exec = service_args
        .iter()
        .map(|arg| systemd_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let body = format!(
        "[Unit]\nDescription=OrgBots persistent room {unit}\nAfter=default.target\n\n[Service]\nType=simple\nExecStart={exec}\nRestart=on-failure\nRestartSec=3\nTimeoutStopSec=20\nEnvironment=RUST_LOG=warn\n\n[Install]\nWantedBy=default.target\n"
    );
    write_private(&path, body.as_bytes())?;
    Ok(path)
}

fn secret_store(id: &str, environment: &HashMap<String, String>) -> anyhow::Result<()> {
    let payload = serde_json::to_vec(environment)?;
    let mut child = Command::new("secret-tool")
        .args([
            "store",
            "--label=OrgBots room credentials",
            "application",
            SECRET_APP,
            "room",
            id,
            "purpose",
            SECRET_PURPOSE,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| anyhow::anyhow!("Linux Secret Service is unavailable; install libsecret-tools and unlock the keyring"))?;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(&payload)?;
    let result = child.wait()?;
    anyhow::ensure!(
        result.success(),
        "could not save room credentials to the unlocked Linux keyring"
    );
    Ok(())
}

fn secret_lookup(id: &str) -> anyhow::Result<HashMap<String, String>> {
    let output = Command::new("secret-tool")
        .args([
            "lookup",
            "application",
            SECRET_APP,
            "room",
            id,
            "purpose",
            SECRET_PURPOSE,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| {
            anyhow::anyhow!("Linux Secret Service is unavailable; install libsecret-tools")
        })?;
    anyhow::ensure!(
        output.status.success(),
        "room credentials are unavailable; unlock the Linux keyring and reopen the room"
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn secret_clear(id: &str) -> anyhow::Result<()> {
    // `secret-tool clear` returns the same failure status when no matching
    // item exists and when Secret Service is unavailable. Query first so an
    // already-cleared room is idempotent while an inaccessible keyring still
    // surfaces as a stop/start failure.
    let search = Command::new("secret-tool")
        .args([
            "search",
            "application",
            SECRET_APP,
            "room",
            id,
            "purpose",
            SECRET_PURPOSE,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| {
            anyhow::anyhow!("Linux Secret Service is unavailable; install libsecret-tools")
        })?;
    anyhow::ensure!(
        search.status.success(),
        "Linux Secret Service is unavailable; unlock the keyring and retry"
    );
    let room_attribute = format!("attribute.room = {id}");
    if !String::from_utf8_lossy(&search.stdout).contains(&room_attribute) {
        return Ok(());
    }

    let output = Command::new("secret-tool")
        .args([
            "clear",
            "application",
            SECRET_APP,
            "room",
            id,
            "purpose",
            SECRET_PURPOSE,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| {
            anyhow::anyhow!("Linux Secret Service is unavailable; install libsecret-tools")
        })?;
    anyhow::ensure!(
        output.status.success(),
        "could not clear room credentials from the Linux keyring"
    );
    Ok(())
}

fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn active_systemd(unit: &str) -> bool {
    systemctl(&["is-active", "--quiet", unit])
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn should_reuse_service(running: bool, service_systemd: bool, manager_available: bool) -> bool {
    running && service_systemd == manager_available
}

fn health_check(address: &str) -> bool {
    use std::io::{Read, Write};
    let Ok(address) = address.parse::<SocketAddr>() else {
        return false;
    };
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&address, Duration::from_millis(300))
    else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(400)));
    let request = format!("GET /health HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = [0u8; 128];
    let Ok(count) = stream.read(&mut response) else {
        return false;
    };
    response[..count].starts_with(b"HTTP/1.1 200") || response[..count].starts_with(b"HTTP/1.0 200")
}

fn descriptor_address(url: &str) -> Option<&str> {
    url.strip_prefix("ws://")?.strip_suffix("/ws")
}

fn descriptor_status(directory: &Path) -> anyhow::Result<RoomServiceStatus> {
    let descriptor = read_descriptor(directory)?;
    let running = descriptor_address(&descriptor.url).is_some_and(health_check);
    Ok(RoomServiceStatus {
        running,
        descriptor: Some(descriptor),
    })
}

/// Install and start the persistent service. The caller sends the current
/// provider environment as JSON over stdin; it is stored directly in Secret
/// Service and never written to a file or argument list.
pub fn start(
    directory: &Path,
    address: SocketAddr,
    mut environment_input: impl Read,
) -> anyhow::Result<RoomServiceStatus> {
    let directory = ensure_private_directory(directory)?;
    let (directory, unit) = paths(&directory)?;
    let _lifecycle = acquire_lock(&directory, "lifecycle.lock")?;
    anyhow::ensure!(
        directory.join("room.toml").is_file(),
        "room configuration is missing"
    );

    let mut bytes = Vec::new();
    environment_input.read_to_end(&mut bytes)?;
    let environment: HashMap<String, String> = if bytes.is_empty() {
        HashMap::new()
    } else {
        serde_json::from_slice(&bytes)?
    };

    let manager_available = systemd_user_available();
    let previous = match read_descriptor(&directory) {
        Ok(descriptor) => Some(descriptor),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    if let Some(previous) = &previous {
        let running = descriptor_address(&previous.url).is_some_and(health_check);
        if should_reuse_service(running, previous.systemd, manager_available) {
            return Ok(RoomServiceStatus {
                running: true,
                descriptor: Some(previous.clone()),
            });
        }
        if previous.systemd && !manager_available {
            anyhow::bail!("the room's Linux user service manager is unavailable; log in to the desktop session and retry");
        }
    }
    let systemd = manager_available;

    if previous
        .as_ref()
        .is_none_or(|descriptor| !descriptor.systemd)
    {
        stop_fallback(&directory)?;
    }

    let id = room_id(&directory)?;
    let credential_key = secret_id(&directory);
    if !environment.is_empty() {
        if previous
            .as_ref()
            .is_some_and(|descriptor| descriptor.has_environment)
        {
            secret_clear(&credential_key)?;
        }
        secret_store(&credential_key, &environment)?;
    } else if previous
        .as_ref()
        .is_some_and(|descriptor| descriptor.has_environment)
    {
        secret_clear(&credential_key)?;
    }

    let url = format!("ws://{address}/ws");
    let descriptor = RoomServiceDescriptor {
        room_id: id,
        url,
        token: random_token(),
        unit: unit.clone(),
        systemd,
        has_environment: !environment.is_empty(),
    };
    write_private(
        &directory.join("service.json"),
        &serde_json::to_vec(&descriptor)?,
    )?;
    // A stopped room can be relaunched only by this explicit owner action.
    clear_stop_marker(&directory)?;
    if systemd {
        let was_active = active_systemd(&unit);
        install_unit(&directory, &unit)?;
        let reload = systemctl(&["daemon-reload"])?;
        anyhow::ensure!(
            reload.status.success(),
            "could not reload the Linux user service manager"
        );
        let start = if was_active {
            systemctl(&["restart", &unit])?
        } else {
            systemctl(&["reset-failed", &unit])?;
            systemctl(&["enable", "--now", &unit])?
        };
        anyhow::ensure!(
            start.status.success(),
            "could not start the Linux room service"
        );
    } else {
        install_autostart(&directory)?;
        let executable = std::env::current_exe()?;
        let mut child = Command::new(executable)
            .args(["room-service-supervise", "--directory"])
            .arg(&directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|_| anyhow::anyhow!("could not start the detached room supervisor"))?;
        write_private(
            &directory.join("supervisor.pid"),
            child.id().to_string().as_bytes(),
        )?;
        // Detach ownership; the process remains alive after this command exits.
        child.stdin.take();
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if health_check(&address.to_string()) {
            return Ok(RoomServiceStatus {
                running: true,
                descriptor: Some(descriptor),
            });
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    if systemd {
        let stopped = systemctl(&["disable", "--now", &unit])?;
        anyhow::ensure!(
            stopped.status.success(),
            "the Linux room service manager could not stop this room"
        );
        anyhow::ensure!(
            !active_systemd(&unit),
            "the Linux room service is still running"
        );
        wait_for_lock(&directory, "lifetime.lock", Duration::from_secs(12))?;
    } else {
        stop_fallback(&directory)?;
        wait_for_lock(&directory, "lifetime.lock", Duration::from_secs(12))?;
    }
    anyhow::bail!("room service did not become ready; configuration and checkpoint were preserved")
}

/// Report whether the supervised local room endpoint responds.
pub fn status(directory: &Path) -> anyhow::Result<RoomServiceStatus> {
    let directory = directory.canonicalize()?;
    let _ = paths(&directory)?;
    descriptor_status(&directory)
}

/// Stop only the background service. The room checkpoint and transcript stay
/// in place and are available to the next explicit start.
pub fn stop(directory: &Path) -> anyhow::Result<()> {
    let directory = directory.canonicalize()?;
    let _lifecycle = acquire_lock(&directory, "lifecycle.lock")?;
    let descriptor = read_descriptor(&directory)?;
    let (_, unit) = paths(&directory)?;
    if descriptor.systemd {
        anyhow::ensure!(systemd_user_available(), "the Linux room service manager is unavailable; log in to the desktop session to stop the room");
        let stopped = systemctl(&["disable", "--now", &unit])?;
        anyhow::ensure!(
            stopped.status.success(),
            "the Linux room service manager could not stop this room"
        );
        anyhow::ensure!(
            !active_systemd(&unit),
            "the Linux room service is still running"
        );
        wait_for_lock(&directory, "lifetime.lock", Duration::from_secs(12))?;
        if let Ok(unit_path) = user_unit_directory().map(|path| path.join(&unit)) {
            let _ = fs::remove_file(unit_path);
        }
        let reload = systemctl(&["daemon-reload"])?;
        anyhow::ensure!(
            reload.status.success(),
            "could not reload the Linux user service manager"
        );
    } else {
        stop_fallback(&directory)?;
        wait_for_lock(&directory, "lifetime.lock", Duration::from_secs(12))?;
        remove_autostart(&directory);
    }
    if descriptor.has_environment {
        secret_clear(&secret_id(&directory))?;
    }
    Ok(())
}

/// Run as the systemd unit's foreground service process.
pub async fn run(directory: &Path) -> anyhow::Result<()> {
    let directory = directory.canonicalize()?;
    let Some(_lifetime) = try_acquire_lock(&directory, "lifetime.lock")? else {
        return Ok(());
    };
    let descriptor = read_descriptor(&directory)?;
    let addr = descriptor
        .url
        .strip_prefix("ws://")
        .and_then(|value| value.strip_suffix("/ws"))
        .ok_or_else(|| anyhow::anyhow!("invalid room service endpoint"))?
        .parse::<SocketAddr>()?;
    let environment = if descriptor.has_environment {
        secret_lookup(&secret_id(&directory))?
    } else {
        HashMap::new()
    };
    crate::serve_room_with_environment(
        &directory.join("room.toml"),
        addr,
        crate::Access::with_token(descriptor.token),
        &environment,
    )
    .await
}

/// Detached fallback monitor used when no systemd user manager is available.
pub fn supervise(directory: &Path) -> anyhow::Result<()> {
    let directory = directory.canonicalize()?;
    let Some(_monitor) = try_acquire_lock(&directory, "monitor.lock")? else {
        return Ok(());
    };
    if directory.join("stop").exists() || read_descriptor(&directory)?.systemd {
        return Ok(());
    }
    write_private(
        &directory.join("supervisor.pid"),
        std::process::id().to_string().as_bytes(),
    )?;
    let executable = std::env::current_exe()?;
    loop {
        if directory.join("stop").exists() {
            break;
        }
        let mut child: Child = Command::new(&executable)
            .args(["room-service-run", "--directory"])
            .arg(&directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        loop {
            if directory.join("stop").exists() {
                terminate_child(&mut child);
                return Ok(());
            }
            if child.try_wait()?.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        if directory.join("stop").exists() {
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    Ok(())
}

fn terminate_child(child: &mut Child) {
    let _ = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("orgbots-supervisor-lock-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn one_lifecycle_operation_holds_the_per_room_lock() {
        let directory = fixture();
        let first = acquire_lock(&directory, "lifecycle.lock").unwrap();
        let second_path = directory.clone();
        let (ready, received) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _second = acquire_lock(&second_path, "lifecycle.lock").unwrap();
            ready.send(()).unwrap();
        });
        assert!(received.recv_timeout(Duration::from_millis(150)).is_err());
        drop(first);
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        thread.join().unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn duplicate_monitor_exits_and_stop_marker_persists_until_explicit_start() {
        let directory = fixture();
        let held = acquire_lock(&directory, "monitor.lock").unwrap();
        supervise(&directory).unwrap();
        assert!(!directory.join("supervisor.pid").exists());
        drop(held);

        stop_fallback(&directory).unwrap();
        assert!(directory.join("stop").exists());
        supervise(&directory).unwrap();
        assert!(!directory.join("supervisor.pid").exists());
        assert!(directory.join("stop").exists());

        clear_stop_marker(&directory).unwrap();
        assert!(!directory.join("stop").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn duplicate_room_runner_exits_without_waiting_for_the_lifetime_lock() {
        let directory = fixture();
        let held = acquire_lock(&directory, "lifetime.lock").unwrap();
        tokio::time::timeout(Duration::from_millis(100), run(&directory))
            .await
            .expect("duplicate runner should not block")
            .unwrap();
        drop(held);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn fallback_service_transitions_when_systemd_becomes_available() {
        assert!(should_reuse_service(true, false, false));
        assert!(!should_reuse_service(true, false, true));
        assert!(should_reuse_service(true, true, true));
        assert!(!should_reuse_service(true, true, false));
    }

    #[test]
    fn keyring_identity_uses_the_canonical_service_path_not_the_room_name() {
        let first_root = fixture();
        let second_root = fixture();
        let first = first_root.join("shared-name");
        let second = second_root.join("shared-name");
        assert_ne!(secret_id(&first), secret_id(&second));
        fs::remove_dir_all(first_root).unwrap();
        fs::remove_dir_all(second_root).unwrap();
    }
}
