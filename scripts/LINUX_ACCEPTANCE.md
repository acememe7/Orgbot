# Linux runtime acceptance harness

Run `python3 scripts/linux-acceptance.py` from any directory in the checkout.
It executes selected Rust tests against temporary, synthetic rooms/providers and
prints a JSON report. The harness includes checkpoint recovery, paused restart,
fake CLI session persistence and stale-session recovery, a real local MCP room
bridge, owner questions with choices, provider cooldown/allowance rollover,
and server interruption/checkpoint behavior.

When Flutter and `dbus-run-session` are available, it also runs the existing
Linux notification smoke test under a private D-Bus daemon. That fixture hosts
its own synthetic Notifications service and checks the payload and default
action route; it does not show a desktop toast or connect to the user's session
bus. Use `--no-dbus` to omit it, `--timeout SECONDS` to bound each check, or
`--report /new/path/report.json` to save the report. Existing evidence files
are never overwritten. `FLUTTER=/path/to/flutter` or `--flutter` selects a
different local SDK.

Test the server-owned notification path separately with
`python3 scripts/test-service-notifications.py`. It starts a new private
session using `dbus-run-session`, registers a synthetic Freedesktop
Notifications service, and runs only the ignored Rust delivery/action test.
The test sends a generic desktop body, persists the notification ID and event
head as owner-only files, then verifies the default action reaches a fake GUI
with only the room/event/reason activation payload. It never contacts the
desktop session bus, starts the real GUI, or opens a saved room.

The checks do not open saved rooms, read API keys, invoke live providers, make
paid calls, or suspend/reboot the machine. Their outputs are evidence for
runtime logic under synthetic conditions, not a claim of installed-package,
desktop-toast, live-provider, or owner-data acceptance.

The room MCP bridge now has a focused integration fixture that drops the
established Unix socket during a tool exchange, reconnects, and completes a
second call through the live bridge. Cargo filters are checked against the
reported test name, so a filter matching zero tests cannot produce a pass.

Test the harness itself with:

```sh
python3 -m unittest -v scripts/test-linux-acceptance.py
```

## Controlled run evidence

On 2026-10-26, the current shared tree passed all 12 configured Linux checks:
12 passed, zero failed, zero skipped. This included the established-socket MCP
disconnect and private synthetic D-Bus notification/action checks.
Machine-readable evidence is `/tmp/gptbots-linux-acceptance-current.json`.
Refresh the run after code changes with
`python3 scripts/linux-acceptance.py --report /tmp/gptbots-linux-acceptance.json`;
the report path must not already exist. The optional private D-Bus notification
test does not touch the desktop session bus.

## Native notification activation

Build the isolated Flutter entrypoint and run the live GTK test:

```sh
(cd app && flutter build linux --debug --target integration_test/linux_activation_main.dart)
python3 scripts/test-linux-activation.py
```

This requires a desktop `DISPLAY`, `dbus-run-session` and
`gnome-keyring-daemon`. It uses private HOME/XDG/keyring directories and a
separate session bus. A cold argument and warm second-process notification
must reach the same running GTK instance exactly once. Desktop portal startup
may take up to the fixture's 60-second timeout. On 2026-10-26 this check passed
with both route payloads and zero provider calls; the local report is
`/tmp/orgbots2-activation-final.log`. Rebuild with `--target lib/main.dart`
before packaging; the fixture is never the distributable app.
