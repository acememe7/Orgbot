#!/usr/bin/env python3
"""Run isolated Linux runtime recovery checks and emit a machine-readable report.

This invokes only repository-owned tests with synthetic providers/fixtures. It
never contacts a model provider, opens a room from the user's library, or uses
host power-management commands. The optional notification test runs inside a
private D-Bus session and registers its own synthetic Notifications service.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence


ROOT = Path(__file__).resolve().parents[1]
CORE = ROOT / "core"
APP = ROOT / "app"
DEFAULT_FLUTTER = Path("/home/theaece/development/flutter/bin/flutter")


@dataclass(frozen=True)
class Case:
    name: str
    command: tuple[str, ...]
    cwd: Path
    purpose: str
    optional: bool = False
    expected_test: str | None = None
    expected_runner: str | None = None


def rust_case(name: str, package: str, test_filter: str, purpose: str, *, target: str | None = None) -> Case:
    command = ["cargo", "test", "-p", package]
    if target:
        command.extend(["--test", target])
    command.append(test_filter)
    return Case(
        name,
        tuple(command),
        CORE,
        purpose,
        expected_test=test_filter,
        expected_runner="cargo-test",
    )


def acceptance_cases(flutter: str | None, dbus: str | None) -> list[Case]:
    cases = [
        rust_case(
            "codex_resume_preserves_policy_and_prompt",
            "orgbot-providers",
            "codex_resume_does_not_enable_automatic_review_for_any_policy",
            "Resume arguments retain the policy and delimit a board prompt beginning with hyphens.",
        ),
        rust_case(
            "reviewer_can_verify_without_write_access",
            "orgbot-runtime",
            "review_only_bots_can_verify_but_cannot_write_or_run_workspace_tools",
            "A reviewer can request isolated verification while ordinary mutation tools remain blocked.",
        ),
        rust_case(
            "verification_never_integrates",
            "orgbot-server",
            "protected_verification_is_read_only_and_never_integrates",
            "Run a real Linux sandbox check and verify no changes reach the project.",
        ),
        rust_case(
            "once_skill_released_after_consumption",
            "orgbot-runtime",
            "once_skill_waits_until_consumed_then_redacts_without_orphaning_pair",
            "One-use skill instructions remain available for one response then leave API history.",
        ),
        rust_case(
            "task_capability_expires_before_next_tool",
            "orgbot-runtime",
            "dispatch_refreshes_task_scope_before_executing_a_tool",
            "A released assignment cannot keep using a task-scoped capability in the same response.",
        ),
        rust_case(
            "checkpoint_reopens_claimed_work",
            "orgbot-runtime",
            "a_room_killed_mid_turn_reopens_with_its_work_intact",
            "Restore the same claimed task and bot identity from a saved room checkpoint.",
            target="restart",
        ),
        rust_case(
            "interrupted_room_stays_paused",
            "orgbot-runtime",
            "a_reopened_room_waits_to_be_told_to_carry_on",
            "An interrupted room does not replay work until an owner resumes it.",
            target="restart",
        ),
        rust_case(
            "native_cli_session_survives_interruption",
            "orgbot-providers",
            "first_session_id_is_saved_before_an_interrupted_episode",
            "A fake CLI persists its session ID before a timed-out episode; a new provider restores it.",
        ),
        rust_case(
            "missing_native_session_recovers_fresh",
            "orgbot-providers",
            "a_session_the_cli_no_longer_has_is_dropped_and_the_turn_runs_fresh",
            "A fake CLI rejects a stale session, then completes in a fresh session and updates its private record.",
        ),
        rust_case(
            "mcp_room_unix_socket_bridge",
            "orgbot-runtime",
            "a_room_cycle_claim_work_review_submit_runs_through_the_bridge",
            "Run a full room work cycle through the real local Unix-socket MCP bridge with synthetic agents.",
            target="run",
        ),
        rust_case(
            "mcp_disconnect_reconnects_during_exchange",
            "orgbot-runtime",
            "an_mcp_disconnect_during_exchange_allows_reconnect",
            "An established room MCP Unix socket is dropped mid-exchange; a fresh connection completes a tool call.",
            target="mcp_disconnect_acceptance",
        ),
        rust_case(
            "unavailable_mcp_server_is_skipped",
            "orgbot-runtime",
            "a_server_that_will_not_start_is_skipped_not_fatal",
            "A missing local MCP subprocess does not prevent the runtime from starting.",
        ),
        rust_case(
            "owner_question_keeps_its_options_and_waits",
            "orgbot-runtime",
            "an_owner_question_waits_without_spending_planning_turns_or_completing",
            "A real room engine records a two-option owner question and waits without extra model turns.",
            target="run",
        ),
        rust_case(
            "provider_quota_wait_recovery",
            "orgbot-runtime",
            "failed_requests_keep_reservations_and_cooldowns_honor_retry_after",
            "Synthetic provider failures retain reservations and honor retry-after cooldowns.",
        ),
        rust_case(
            "allowance_window_recovery",
            "orgbot-runtime",
            "window_rollover_recovers_but_clock_rollback_does_not_reset",
            "Allowance rollover recovers at the configured boundary and does not reset on clock rollback.",
        ),
        rust_case(
            "server_stop_interrupts_and_checkpoints",
            "orgbot-server",
            "stop_interrupts_an_in_flight_turn_and_checkpoints_promptly",
            "The server interrupts a synthetic in-flight turn and checkpoints promptly on stop.",
        ),
    ]
    if flutter and dbus:
        cases.append(
            Case(
                "private_dbus_notification_and_action_route",
                (
                    dbus,
                    "--",
                    "env",
                    "ORGBOTS_PRIVATE_DBUS_TEST=1",
                    flutter,
                    "test",
                    "--no-pub",
                    "test/attention_dbus_smoke_test.dart",
                ),
                APP,
                "Use a private D-Bus daemon and synthetic service to check Notify payload and default action routing.",
                optional=True,
                expected_test="Linux plugin Notify and synthetic default click route a room",
                expected_runner="flutter-test",
            )
        )
    return cases


def run_case(case: Case, timeout: float) -> dict[str, object]:
    started = time.monotonic()
    try:
        completed = subprocess.run(
            case.command,
            cwd=case.cwd,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=timeout,
            check=False,
            env={**os.environ, "CARGO_TERM_COLOR": "never"},
        )
        output = completed.stdout
        status = "passed" if completed.returncode == 0 else "failed"
        failure_reason = None
        if status == "passed" and case.expected_test:
            if case.expected_runner == "cargo-test":
                observed = re.search(
                    rf"(?m)^test .*{re.escape(case.expected_test)} \.\.\. ok$",
                    output,
                ) is not None
            elif case.expected_runner == "flutter-test":
                observed = case.expected_test in output and "All tests passed!" in output
            else:
                observed = case.expected_test in output
            if not observed:
                status = "failed"
                failure_reason = f"Expected test did not report success: {case.expected_test}"
        result = {
            "name": case.name,
            "status": status,
            "exit_code": completed.returncode,
            "duration_seconds": round(time.monotonic() - started, 3),
            "command": list(case.command),
            "purpose": case.purpose,
            "output_tail": output[-5000:],
        }
        if failure_reason:
            result["failure_reason"] = failure_reason
        return result
    except subprocess.TimeoutExpired as error:
        output = error.stdout or ""
        if isinstance(output, bytes):
            output = output.decode(errors="replace")
        return {
            "name": case.name,
            "status": "timed_out",
            "exit_code": None,
            "duration_seconds": round(time.monotonic() - started, 3),
            "command": list(case.command),
            "purpose": case.purpose,
            "output_tail": output[-5000:],
        }
    except OSError as error:
        return {
            "name": case.name,
            "status": "failed",
            "exit_code": None,
            "duration_seconds": round(time.monotonic() - started, 3),
            "command": list(case.command),
            "purpose": case.purpose,
            "output_tail": f"Could not start test command: {error}",
        }


def save_report(path: Path, report: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8") as stream:
        json.dump(report, stream, indent=2)
        stream.write("\n")


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=float, default=300.0, help="seconds allowed per check (default: 300)")
    parser.add_argument("--flutter", help="Flutter executable; defaults to $FLUTTER or the local development SDK")
    parser.add_argument("--no-dbus", action="store_true", help="skip the private D-Bus notification check")
    parser.add_argument("--report", type=Path, help="write JSON evidence to a new file (refuses to overwrite)")
    args = parser.parse_args(argv)
    if platform.system() != "Linux":
        parser.error("Linux acceptance checks can only run on Linux")
    if args.timeout <= 0:
        parser.error("--timeout must be positive")

    flutter = args.flutter or os.environ.get("FLUTTER")
    if flutter is None and DEFAULT_FLUTTER.is_file():
        flutter = str(DEFAULT_FLUTTER)
    dbus = None if args.no_dbus else shutil.which("dbus-run-session")
    if flutter and not Path(flutter).is_file() and shutil.which(flutter) is None:
        parser.error(f"Flutter executable not found: {flutter}")

    cases = acceptance_cases(flutter, dbus)
    results = [run_case(case, args.timeout) for case in cases]
    if not args.no_dbus and not (flutter and dbus):
        results.append(
            {
                "name": "private_dbus_notification_and_action_route",
                "status": "skipped",
                "exit_code": None,
                "duration_seconds": 0,
                "command": [],
                "purpose": "Optional: requires Flutter and dbus-run-session; no host notification daemon is contacted.",
                "output_tail": "Required local test tools are unavailable.",
            }
        )
    report: dict[str, object] = {
        "schema_version": 1,
        "platform": platform.platform(),
        "scope": "synthetic local acceptance only; no real rooms, provider calls, keys, or host power actions",
        "results": results,
        "passed": sum(item["status"] == "passed" for item in results),
        "failed": sum(item["status"] in {"failed", "timed_out"} for item in results),
        "skipped": sum(item["status"] == "skipped" for item in results),
        "open_gaps": [],
    }
    rendered = json.dumps(report, indent=2)
    print(rendered)
    if args.report:
        save_report(args.report, report)
    return 1 if report["failed"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
