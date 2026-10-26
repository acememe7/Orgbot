//! Desktop delivery owned by the room service, including while Flutter is closed.
use std::path::{Path, PathBuf};
use std::time::Duration;

use orgbot_engine::Notification;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[derive(Debug)]
pub struct DesktopNotifications {
    directory: PathBuf,
    room_id: String,
    last_scheduled: u64,
    active: Option<tokio::task::JoinHandle<()>>,
}

impl DesktopNotifications {
    pub fn new(state: &Path) -> Option<Self> {
        let directory = state.parent()?.to_owned();
        let room_id = directory.file_name()?.to_str()?.to_owned();
        if room_id.is_empty()
            || !room_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        {
            return None;
        }
        let last_scheduled = std::fs::read_to_string(directory.join("notification-head"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        Some(Self {
            directory,
            room_id,
            last_scheduled,
            active: None,
        })
    }

    pub fn show(&mut self, seq: u64, event: String, notice: Notification) {
        if seq <= self.last_scheduled {
            return;
        }
        self.last_scheduled = seq;
        // One current alert per room; all unanswered questions stay on its board.
        if let Some(previous) = self.active.take() {
            previous.abort();
        }
        let directory = self.directory.clone();
        let room = self.room_id.clone();
        self.active = Some(tokio::spawn(async move {
            loop {
                match deliver(&directory, &room, seq, &event, &notice).await {
                    Ok(()) => break,
                    Err(error) => {
                        tracing::warn!(%error, "desktop notification unavailable; retrying without model calls");
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    }
                }
            }
        }));
    }
}

impl Drop for DesktopNotifications {
    fn drop(&mut self) {
        if let Some(task) = self.active.take() {
            task.abort();
        }
    }
}

fn payload(room: &str, event: &str, notice: &Notification) -> String {
    use orgbot_engine::notify::Reason;
    let reason = match notice.reason {
        Reason::DecisionNeeded => "decision",
        Reason::SecurityIncident => "security",
        Reason::MilestoneReached => "milestone",
        Reason::RoomFinished => "finished",
        Reason::RoomStuck => "stuck",
        Reason::Mentioned => "mentioned",
        Reason::NeedsAttention => "needsAttention",
        Reason::BudgetSpent => "budget",
    };
    serde_json::json!({"room_id": room, "event_id": event, "reason": reason}).to_string()
}

async fn deliver(
    directory: &Path,
    room: &str,
    seq: u64,
    event: &str,
    notice: &Notification,
) -> anyhow::Result<()> {
    let sibling = std::env::current_exe()?
        .parent()
        .map(|p| p.join("orgbot_app"));
    let gui = sibling
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("/usr/bin/orgbot"));
    deliver_with_gui(directory, room, seq, event, notice, &gui).await
}

async fn deliver_with_gui(
    directory: &Path,
    room: &str,
    seq: u64,
    event: &str,
    notice: &Notification,
    gui: &Path,
) -> anyhow::Result<()> {
    let previous = std::fs::read_to_string(directory.join("notification-id"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);
    // notify-send otherwise buffers its printed ID while waiting for an action.
    let mut command = Command::new("/usr/bin/stdbuf");
    command.args(["--output=L", "/usr/bin/notify-send"]);
    command
        .args([
            "--app-name=OrgBots",
            "--icon=orgbot",
            "--print-id",
            "--wait",
            "--action=default=Open room",
            "--hint=string:desktop-entry:orgbot",
            "--expire-time=86400000",
            "--replace-id",
            &previous.to_string(),
            if notice.urgent {
                "--urgency=critical"
            } else {
                "--urgency=normal"
            },
            "--",
            &notice.title,
            "Open the room to see the update and answer pending questions.",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
            command.env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}/bus", PathBuf::from(runtime).display()),
            );
        }
    }
    let mut child = command.spawn()?;
    let mut lines = BufReader::new(child.stdout.take().expect("piped stdout")).lines();
    let id = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await??
        .ok_or_else(|| anyhow::anyhow!("desktop did not accept the notification"))?;
    let id: u32 = id.trim().parse()?;
    for (name, value) in [
        ("notification-id", id.to_string()),
        ("notification-head", seq.to_string()),
    ] {
        write_marker(directory, name, &value)?;
    }
    let action = tokio::time::timeout(Duration::from_secs(86400), lines.next_line()).await;
    if let Ok(Ok(Some(action))) = action {
        if action.trim() == "default" {
            let mut process = Command::new(gui)
                .arg("--orgbot-attention")
                .arg(payload(room, event, notice))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()?;
            tokio::spawn(async move {
                let _ = process.wait().await;
            });
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
    Ok(())
}

fn write_marker(directory: &Path, name: &str, value: &str) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let temporary = directory.join(format!(".{name}.{}", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(value.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, directory.join(name))?;
        std::fs::File::open(directory)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires scripts/test-service-notifications.py private D-Bus fixture"]
    async fn service_delivery_and_action() {
        assert_eq!(std::env::var("ORGBOT_PRIVATE_DBUS").as_deref(), Ok("1"));
        let directory = PathBuf::from(
            std::env::var_os("ORGBOT_NOTIFICATION_TEST_DIR").expect("isolated fixture"),
        );
        let notice = Notification {
            reason: orgbot_engine::notify::Reason::DecisionNeeded,
            title: "Question".into(),
            body: "private body must not reach the desktop".into(),
            urgent: true,
        };
        tokio::time::timeout(
            Duration::from_secs(20),
            deliver_with_gui(
                &directory,
                "fixture_room",
                42,
                "fixture_event",
                &notice,
                &directory.join("gui"),
            ),
        )
        .await
        .expect("notification action timeout")
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(directory.join("notification-head")).unwrap(),
            "42"
        );
        assert!(!std::fs::read_to_string(directory.join("notification-id"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn markers_do_not_follow_existing_symlinks() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let directory =
            std::env::temp_dir().join(format!("orgbot-notification-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let original = directory.join("original");
        std::fs::write(&original, "preserved").unwrap();
        symlink(&original, directory.join("notification-head")).unwrap();
        write_marker(&directory, "notification-head", "42").unwrap();
        assert_eq!(std::fs::read_to_string(&original).unwrap(), "preserved");
        assert_eq!(
            std::fs::metadata(directory.join("notification-head"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn activation_payload_matches_flutter_reason_names() {
        let notice = Notification {
            reason: orgbot_engine::notify::Reason::DecisionNeeded,
            title: "Question".into(),
            body: "private body".into(),
            urgent: true,
        };
        let value: serde_json::Value =
            serde_json::from_str(&payload("room_1", "event_2", &notice)).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"room_id":"room_1","event_id":"event_2","reason":"decision"})
        );
        assert!(!payload("room_1", "event_2", &notice).contains("private body"));
    }
}
