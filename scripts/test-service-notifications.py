#!/usr/bin/python3
"""Exercise server-owned desktop delivery against a private D-Bus fixture."""

from __future__ import annotations

import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CORE = ROOT / "core"
SCRIPT = Path(__file__).resolve()
GENERIC_BODY = "Open the room to see the update and answer pending questions."
PRIVATE_BODY = "private body must not reach the desktop"


def fail(message: str) -> None:
    raise RuntimeError(message)


def fixture_gui(directory: Path) -> Path:
    gui = directory / "gui"
    gui.write_text(
        "#!/usr/bin/python3\n"
        "import json, os, pathlib, sys\n"
        f"root = pathlib.Path({str(directory)!r})\n"
        "fd = os.open(root / 'gui-argv.json', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)\n"
        "with os.fdopen(fd, 'w', encoding='utf-8') as stream:\n"
        "    json.dump({'argv': sys.argv[1:], 'uid': os.getuid()}, stream)\n",
        encoding="utf-8",
    )
    gui.chmod(0o700)
    return gui


def validate_fixture(directory: Path) -> dict[str, object]:
    directory_info = directory.stat(follow_symlinks=False)
    if (
        not stat.S_ISDIR(directory_info.st_mode)
        or directory_info.st_uid != os.getuid()
        or stat.S_IMODE(directory_info.st_mode) != 0o700
    ):
        fail("notification fixture directory is not owner-private")
    notification_file = directory / "notifications.json"
    notification_info = notification_file.stat(follow_symlinks=False)
    if (
        not stat.S_ISREG(notification_info.st_mode)
        or notification_info.st_uid != os.getuid()
        or stat.S_IMODE(notification_info.st_mode) != 0o600
    ):
        fail("synthetic service capture is not owner-only")
    notifications = json.loads(notification_file.read_text())
    if len(notifications) != 1:
        fail(f"expected one synthetic notification, got {len(notifications)}")
    item = notifications[0]
    if item.get("body") != GENERIC_BODY:
        fail("notification body exposed private content or did not use the generic body")
    if PRIVATE_BODY in json.dumps(item):
        fail("private notification body reached the synthetic desktop service")
    actions = item.get("actions")
    if actions != ["default", "Open room"]:
        fail(f"unexpected notification actions: {actions!r}")
    if item.get("markers_before_action") is not True:
        fail("notification ID/head were not persisted before the owner action")

    for name in ("notification-id", "notification-head"):
        path = directory / name
        info = path.stat(follow_symlinks=False)
        if not stat.S_ISREG(info.st_mode) or stat.S_ISLNK(info.st_mode):
            fail(f"{name} is not a regular file")
        if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o600:
            fail(f"{name} is not owner-only (uid={info.st_uid}, mode={stat.S_IMODE(info.st_mode):04o})")
        value = path.read_text(encoding="ascii").strip()
        if not value.isdecimal() or int(value) <= 0:
            fail(f"{name} has an invalid persisted value")
    if (directory / "notification-head").read_text().strip() != "42":
        fail("notification-head did not persist the delivered event sequence")

    capture = directory / "gui-argv.json"
    if not capture.exists():
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline and not capture.exists():
            time.sleep(0.025)
    if not capture.exists():
        fail("the default action did not launch the fake GUI")
    capture_info = capture.stat(follow_symlinks=False)
    if capture_info.st_uid != os.getuid() or stat.S_IMODE(capture_info.st_mode) != 0o600:
        fail("GUI argument capture is not owner-only")
    launched = json.loads(capture.read_text(encoding="utf-8"))
    if launched.get("uid") != os.getuid():
        fail("fake GUI ran under a different owner")
    argv = launched.get("argv")
    if not isinstance(argv, list) or len(argv) != 2 or argv[0] != "--orgbot-attention":
        fail(f"unexpected fake GUI arguments: {argv!r}")
    payload = json.loads(argv[1])
    if payload != {
        "room_id": "fixture_room",
        "event_id": "fixture_event",
        "reason": "decision",
    }:
        fail(f"unexpected room activation payload: {payload!r}")
    if PRIVATE_BODY in argv[1]:
        fail("private notification body reached the activation payload")
    return {
        "notification_id": int((directory / "notification-id").read_text().strip()),
        "notification_head": 42,
        "action": "default",
        "activation_payload": payload,
        "private_body_exposed": False,
        "owner_only_markers": True,
    }


def run_private_session(directory: Path) -> dict[str, object]:
    dbus_run = shutil.which("dbus-run-session")
    if dbus_run is None:
        fail("dbus-run-session is required")
    if not Path("/usr/bin/notify-send").is_file():
        fail("/usr/bin/notify-send is required")
    gui = fixture_gui(directory)
    command = [
        dbus_run,
        "--",
        sys.executable,
        os.fspath(SCRIPT),
        "--inside",
        os.fspath(directory),
    ]
    env = os.environ.copy()
    env.pop("DBUS_SESSION_BUS_ADDRESS", None)
    result = subprocess.run(
        command, cwd=ROOT, text=True, capture_output=True, env=env
    )
    if result.stdout:
        sys.stdout.write(result.stdout)
    if result.stderr:
        sys.stderr.write(result.stderr)
    if result.returncode != 0:
        fail(f"private D-Bus notification test exited {result.returncode}")
    return validate_fixture(directory)


def inside_session(directory: Path) -> None:
    address = os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")
    if not address.startswith("unix:"):
        fail("dbus-run-session did not provide a private Unix session bus")
    env = os.environ.copy()
    env["ORGBOT_PRIVATE_DBUS"] = "1"
    env["ORGBOT_NOTIFICATION_TEST_DIR"] = os.fspath(directory)
    service = subprocess.Popen(
        [sys.executable, os.fspath(SCRIPT), "--service", os.fspath(directory)],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if service.poll() is not None:
                stderr = service.stderr.read() if service.stderr else ""
                fail(f"synthetic notification service exited early: {stderr.strip()}")
            if (directory / "service-ready").exists():
                break
            time.sleep(0.025)
        else:
            fail("synthetic notification service did not become ready")
        result = subprocess.run(
            [
                "cargo",
                "test",
                "-p",
                "orgbot-server",
                "--lib",
                "desktop_notifications::tests::service_delivery_and_action",
                "--",
                "--ignored",
                "--exact",
            ],
            cwd=CORE,
            env=env,
            check=False,
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            if result.stdout:
                sys.stderr.write(result.stdout)
            if result.stderr:
                sys.stderr.write(result.stderr)
            fail(f"Rust notification service test exited {result.returncode}")
    finally:
        service.terminate()
        try:
            service.wait(timeout=3)
        except subprocess.TimeoutExpired:
            service.kill()
            service.wait()


def notification_service(directory: Path) -> None:
    import dbus
    import dbus.service
    from dbus.mainloop.glib import DBusGMainLoop
    from gi.repository import GLib

    DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus()
    name = dbus.service.BusName("org.freedesktop.Notifications", bus=bus)
    loop = GLib.MainLoop()

    class Notifications(dbus.service.Object):
        def __init__(self) -> None:
            super().__init__(name, "/org/freedesktop/Notifications")
            self.calls: list[dict[str, object]] = []

        def _save_calls(self) -> None:
            path = directory / "notifications.json"
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, "w", encoding="utf-8") as stream:
                json.dump(self.calls, stream)
                stream.flush()

        @dbus.service.method(
            "org.freedesktop.Notifications",
            in_signature="susssasa{sv}i",
            out_signature="u",
        )
        def Notify(
            self,
            app_name: str,
            replaces_id: int,
            app_icon: str,
            summary: str,
            body: str,
            actions: list[str],
            hints: dict[str, object],
            expire_timeout: int,
        ) -> int:
            self.calls.append(
                {
                    "app_name": str(app_name),
                    "replaces_id": int(replaces_id),
                    "app_icon": str(app_icon),
                    "summary": str(summary),
                    "body": str(body),
                    "actions": [str(value) for value in actions],
                    "hints": {str(key): str(value) for key, value in hints.items()},
                    "expire_timeout": int(expire_timeout),
                }
            )
            self._save_calls()
            notification_id = 73
            GLib.timeout_add(1000, self._emit_action, notification_id)
            return notification_id

        def _emit_action(self, notification_id: int) -> bool:
            self.calls[-1]["markers_before_action"] = all(
                (directory / name).is_file()
                for name in ("notification-id", "notification-head")
            )
            self._save_calls()
            self.ActionInvoked(notification_id, "default")
            GLib.timeout_add(100, self._close_notification, notification_id)
            GLib.timeout_add(500, loop.quit)
            return GLib.SOURCE_REMOVE

        def _close_notification(self, notification_id: int) -> bool:
            self.NotificationClosed(notification_id, 3)
            return GLib.SOURCE_REMOVE

        @dbus.service.signal("org.freedesktop.Notifications", signature="us")
        def ActionInvoked(self, notification_id: int, action_key: str) -> None:
            del notification_id, action_key

        @dbus.service.signal("org.freedesktop.Notifications", signature="uu")
        def NotificationClosed(self, notification_id: int, reason: int) -> None:
            del notification_id, reason

        @dbus.service.method(
            "org.freedesktop.Notifications", in_signature="", out_signature="as"
        )
        def GetCapabilities(self) -> list[str]:
            return ["actions", "body"]

        @dbus.service.method(
            "org.freedesktop.Notifications", in_signature="", out_signature="ssss"
        )
        def GetServerInformation(self) -> tuple[str, str, str, str]:
            return ("OrgBots Test Service", "OrgBots", "1", "1.2")

    service = Notifications()
    (directory / "service-ready").write_text("ready", encoding="ascii")
    loop.run()


def main() -> int:
    if len(sys.argv) >= 2 and sys.argv[1] == "--service":
        notification_service(Path(sys.argv[2]))
        return 0
    if len(sys.argv) >= 2 and sys.argv[1] == "--inside":
        inside_session(Path(sys.argv[2]))
        return 0
    required = ["cargo", "dbus-run-session"]
    missing = [name for name in required if shutil.which(name) is None]
    if missing:
        fail(f"missing required commands: {', '.join(missing)}")
    try:
        import dbus  # noqa: F401
        import gi  # noqa: F401
    except ImportError as exc:
        fail(f"system Python needs dbus-python and PyGObject: {exc}")
    try:
        with tempfile.TemporaryDirectory(prefix="orgbot-notifications-") as raw:
            directory = Path(raw)
            directory.chmod(0o700)
            report = run_private_session(directory)
            print(json.dumps({"ok": True, **report}, sort_keys=True))
        return 0
    except Exception as exc:
        print(json.dumps({"ok": False, "error": str(exc)}, sort_keys=True), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
