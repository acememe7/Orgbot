#!/usr/bin/env python3
"""Exercise GTK cold and warm notification activation on a private D-Bus."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "app"
DEFAULT_BINARY = APP / "build/linux/x64/debug/bundle/orgbot_app"


def fail(message: str, process: subprocess.Popen[bytes] | None = None) -> None:
    if process is not None:
        try:
            process.terminate()
            process.wait(timeout=5)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            process.kill()
    raise SystemExit(message)


def wait_for_routes(path: Path, count: int, timeout: float = 60) -> list[dict[str, str]]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
            if len(lines) >= count:
                return [json.loads(line) for line in lines]
        except FileNotFoundError:
            pass
        time.sleep(0.1)
    return []


def run_private(binary: Path) -> dict[str, object]:
    if not os.environ.get("DISPLAY"):
        raise SystemExit("DISPLAY is required for the live GTK activation test")
    with tempfile.TemporaryDirectory(prefix="orgbots-activation-") as temporary:
        private = Path(temporary)
        home = private / "home"
        runtime = private / "runtime"
        xdg = {name: private / name.lower() for name in (
            "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME"
        )}
        for directory in (home, runtime, *xdg.values()):
            directory.mkdir(mode=0o700)
            directory.chmod(0o700)
        capture = private / "routes.jsonl"
        stdout = (private / "gtk.stdout").open("wb")
        stderr = (private / "gtk.stderr").open("wb")
        environment = os.environ.copy()
        environment.update({
            "HOME": str(home),
            "XDG_RUNTIME_DIR": str(runtime),
            "ORGBOTS_ACTIVATION_CAPTURE_FILE": str(capture),
        })
        environment.update({name: str(path) for name, path in xdg.items()})
        keyring_executable = shutil.which("gnome-keyring-daemon")
        if keyring_executable is None:
            raise SystemExit("private fixture keyring helper is required")
        keyring = subprocess.run(
            [keyring_executable, "--unlock", "--components=secrets"],
            input=b"fixture-only\n",
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=5,
            check=False,
        )
        if keyring.returncode != 0:
            raise SystemExit(
                f"private fixture keyring failed: {keyring.stderr.decode(errors='replace')}"
            )
        cold = {
            "room_id": "fixture-cold-room",
            "reason": "decision",
            "event_id": "fixture-cold-event",
        }
        warm = {
            "room_id": "fixture-warm-room",
            "reason": "stuck",
            "event_id": "fixture-warm-event",
        }
        primary = subprocess.Popen(
            [str(binary), "--orgbot-attention", json.dumps(cold, separators=(",", ":"))],
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=stdout,
            stderr=stderr,
        )

        def failure_detail() -> str:
            stdout.flush()
            stderr.flush()
            args_capture = capture.with_name(f"{capture.name}.args.json")
            return "\n".join(
                part for part in (
                    f"GTK process return code: {primary.poll()}",
                    f"Dart entrypoint arguments: {args_capture.read_text(errors='replace')}"
                    if args_capture.exists() else "Dart entrypoint did not start",
                    (private / "gtk.stdout").read_text(errors="replace")[-8000:],
                    (private / "gtk.stderr").read_text(errors="replace")[-8000:],
                ) if part
            )

        try:
            routes = wait_for_routes(capture, 1)
            if not routes:
                fail(f"cold GTK launch did not route its argv payload\n{failure_detail()}", primary)
            second = subprocess.run(
                [str(binary), "--orgbot-attention", json.dumps(warm, separators=(",", ":"))],
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=15,
                check=False,
            )
            if second.returncode != 0:
                fail(f"warm activation process exited {second.returncode}: {second.stderr.decode(errors='replace')}", primary)
            routes = wait_for_routes(capture, 2)
            if len(routes) != 2:
                fail("warm GTK launch did not route exactly one payload to the running instance", primary)
            time.sleep(0.3)
            routes = [json.loads(line) for line in capture.read_text(encoding="utf-8").splitlines()]
            if routes != [cold, warm]:
                fail(f"activation payloads were missing, duplicated, or reordered: {routes!r}", primary)
            if primary.poll() is not None:
                fail("primary GTK instance exited after the warm activation", primary)
            return {
                "result": "passed",
                "private_dbus": True,
                "display": environment["DISPLAY"],
                "isolated_home_and_xdg": True,
                "isolated_keyring": True,
                "single_running_instance": True,
                "cold_route": routes[0],
                "warm_route": routes[-1],
                "provider_calls": 0,
            }
        finally:
            if primary.poll() is None:
                primary.send_signal(signal.SIGTERM)
                try:
                    primary.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    primary.kill()
                    primary.wait(timeout=5)
            stdout.close()
            stderr.close()


def main() -> int:
    binary = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else DEFAULT_BINARY.resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise SystemExit(f"executable not found: {binary}")
    if os.environ.get("ORGBOTS_ACTIVATION_PRIVATE_DBUS") == "1":
        print(json.dumps(run_private(binary), sort_keys=True))
        return 0
    if shutil.which("dbus-run-session") is None:
        raise SystemExit("dbus-run-session is required")
    result = subprocess.run(
        ["dbus-run-session", "--", sys.executable, str(Path(__file__).resolve()), str(binary)],
        env={**os.environ, "ORGBOTS_ACTIVATION_PRIVATE_DBUS": "1"},
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )
    if result.stdout:
        print(result.stdout, end="")
    if result.returncode != 0 and result.stderr:
        print(result.stderr[-12000:], file=sys.stderr, end="")
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
