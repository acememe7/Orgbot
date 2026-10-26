#!/usr/bin/env python3
"""Paired baseline/team benchmark runner for local or explicitly paid adapters."""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import signal
import statistics
import subprocess
import sys
import time
from pathlib import PurePosixPath
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parent
DEFAULT_FIXTURES = ROOT / "tasks.json"
TOKENS = ("input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens")


def read_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read JSON file {path}: {error}") from error


def load_fixtures(path: Path) -> list[dict[str, Any]]:
    value = read_json(path)
    if not isinstance(value, list) or not value:
        raise ValueError("fixtures must be a non-empty JSON array")
    seen: set[str] = set()
    for case in value:
        if not isinstance(case, dict) or not isinstance(case.get("id"), str) or not case["id"].strip():
            raise ValueError("each fixture needs a non-empty id")
        if case["id"] in seen:
            raise ValueError(f"duplicate fixture id: {case['id']}")
        seen.add(case["id"])
        if not isinstance(case.get("task"), str) or not case["task"].strip():
            raise ValueError(f"fixture {case['id']} needs task text")
        acceptance = case.get("acceptance", [])
        if not isinstance(acceptance, list) or not all(isinstance(item, str) and item for item in acceptance):
            raise ValueError(f"fixture {case['id']} acceptance must be an array of non-empty strings")
        verify = case.get("verify")
        files = case.get("workspace_files")
        if verify is not None and (not isinstance(verify, list) or not verify or not all(isinstance(item, str) and item for item in verify)):
            raise ValueError(f"fixture {case['id']} verify must be a non-empty command array")
        if files is not None:
            if not isinstance(files, dict) or not files or not all(isinstance(name, str) and isinstance(content, str) for name, content in files.items()):
                raise ValueError(f"fixture {case['id']} workspace_files must map paths to file contents")
            for name in files:
                relative = PurePosixPath(name)
                if relative.is_absolute() or not relative.parts or ".." in relative.parts:
                    raise ValueError(f"fixture {case['id']} has an unsafe workspace path: {name}")
            if verify is None:
                raise ValueError(f"fixture {case['id']} workspace files require an independent verify command")
        elif not acceptance:
            raise ValueError(f"fixture {case['id']} needs acceptance strings or workspace files with a verify command")
    return value


def _nonnegative_int(value: Any, field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise ValueError(f"runner field {field} must be a nonnegative integer")
    return value


def partial_usage(run_dir: Path) -> dict[str, Any] | None:
    checkpoint = run_dir / "room-state.json"
    if not checkpoint.is_file():
        return None
    try:
        import orgbot_adapter
        return orgbot_adapter.read_engine_usage(json.loads(checkpoint.read_text(encoding="utf-8")))
    except (OSError, ValueError, TypeError, AttributeError, json.JSONDecodeError) as error:
        return {"measurement_error": str(error)}


def invoke(command: str, variant: str, trial: int, case: dict[str, Any], mode: str, timeout: float, run_dir: Path) -> dict[str, Any]:
    argv = shlex.split(command)
    if not argv:
        raise ValueError(f"empty {variant} runner command")
    environment = os.environ.copy()
    environment.update({
        "ORGBOT_BENCH_VARIANT": variant,
        "ORGBOT_BENCH_TRIAL": str(trial),
        "ORGBOT_BENCH_CASE": str(case["id"]),
        "ORGBOT_BENCH_MODE": mode,
        "ORGBOT_BENCH_ALLOW_PAID": "1" if mode == "paid" else "0",
        # Give adapters with their own child processes time to terminate them
        # cleanly before the outer adapter deadline fires.
        "ORGBOT_BENCH_TIMEOUT": str(max(0.1, timeout - 1)),
    })
    started = time.perf_counter()
    try:
        run_dir.mkdir(parents=True, exist_ok=False, mode=0o700)
        run_dir.chmod(0o700)
        workspace = run_dir / "workspace"
        workspace.mkdir(mode=0o700)
        workspace.chmod(0o700)
        for name, content in case.get("workspace_files", {}).items():
            target = workspace.joinpath(*PurePosixPath(name).parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content, encoding="utf-8")
        environment["ORGBOT_BENCH_WORKSPACE"] = str(workspace)
        environment["ORGBOT_BENCH_STATE"] = str(run_dir / "room-state.json")
        process = subprocess.Popen(
            argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=environment,
            start_new_session=(os.name == "posix"),
        )
        try:
            stdout, stderr = process.communicate(json.dumps(case, ensure_ascii=False), timeout=timeout)
        except subprocess.TimeoutExpired as error:
            if os.name == "posix":
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            else:
                process.terminate()
            try:
                stdout, stderr = process.communicate(timeout=2)
            except subprocess.TimeoutExpired:
                if os.name == "posix":
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                else:
                    process.kill()
                stdout, stderr = process.communicate()
            if os.name == "posix":
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            stdout = stdout or error.output or ""
            stderr = stderr or error.stderr or ""
            if isinstance(stdout, bytes):
                stdout = stdout.decode(errors="replace")
            if isinstance(stderr, bytes):
                stderr = stderr.decode(errors="replace")
            (run_dir / "stdout.log").write_text(stdout, encoding="utf-8")
            (run_dir / "stderr.log").write_text(stderr, encoding="utf-8")
            failure = {"variant": variant, "case_id": case["id"], "trial": trial, "status": "failed",
                       "error": f"{variant} runner timed out on {case['id']} trial {trial}", "run_dir": str(run_dir)}
            measurement = partial_usage(run_dir)
            if measurement:
                failure["partial_measurement"] = measurement
            raise ValueError(json.dumps(failure)) from error
        (run_dir / "stdout.log").write_text(stdout, encoding="utf-8")
        (run_dir / "stderr.log").write_text(stderr, encoding="utf-8")
        if process.returncode != 0:
            if os.name == "posix":
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                    time.sleep(0.25)
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            failure: dict[str, Any] = {"variant": variant, "case_id": case["id"], "trial": trial, "status": "failed",
                                       "error": f"{variant} runner exited {process.returncode} on {case['id']} trial {trial}: {stderr[-1000:]}",
                                       "run_dir": str(run_dir)}
            try:
                partial = json.loads(stdout)
                if isinstance(partial, dict) and partial.get("measurement_source") == "orgbot_engine":
                    failure["partial_measurement"] = {key: partial.get(key) for key in ("measurement_source", "model", "billing_kind", "room_phase", "failed_tasks", "completion_accepted", "usage_reported", "usage_complete", "usage", "actual_api_cost_microusd", "api_equivalent_cost_microusd", "agent_usage", "provider_usage")}
            except json.JSONDecodeError as error:
                failure["partial_measurement_error"] = f"runner stdout was not JSON: {error}"
            raise ValueError(json.dumps(failure))
        try:
            output = json.loads(stdout)
        except json.JSONDecodeError as error:
            raise ValueError(f"{variant} runner did not return JSON on {case['id']} trial {trial}") from error
        if not isinstance(output, dict) or not isinstance(output.get("deliverable"), str):
            raise ValueError("runner JSON needs a string deliverable")

        verifier = case.get("verify")
        verification: dict[str, Any] | None = None
        if verifier:
            protected_tests = [name for name in case.get("workspace_files", {}) if PurePosixPath(name).name.startswith("test_")]
            changed_tests = [name for name in protected_tests if not (workspace / name).is_file() or (workspace / name).read_text(encoding="utf-8") != case["workspace_files"][name]]
            if changed_tests:
                verification = {"method": "independent_command", "command": verifier, "exit_code": None,
                                "accepted": False, "protected_test_files_changed": changed_tests}
                accepted = False
            else:
                try:
                    checked = subprocess.run(
                        verifier,
                        cwd=workspace,
                        stdin=subprocess.DEVNULL,
                        capture_output=True,
                        text=True,
                        timeout=timeout,
                        check=False,
                    )
                except subprocess.TimeoutExpired as error:
                    raise ValueError(f"artifact verifier timed out on {case['id']} trial {trial}") from error
                verification = {
                    "method": "independent_command",
                    "command": verifier,
                    "exit_code": checked.returncode,
                    "accepted": checked.returncode == 0,
                    "output_tail": (checked.stdout + checked.stderr)[-5000:],
                }
                accepted = checked.returncode == 0
            acceptance_method = "independent_verifier_command"
        else:
            accepted = all(criterion.casefold() in output["deliverable"].casefold() for criterion in case["acceptance"])
            acceptance_method = "case_insensitive_substring"
    except OSError as error:
        raise ValueError(f"cannot preserve benchmark evidence under {run_dir}: {error}") from error
    elapsed_ms = round((time.perf_counter() - started) * 1000, 3)
    usage = output.get("usage", {})
    if not isinstance(usage, dict):
        raise ValueError("runner usage must be an object")
    measurement_source = output.get("measurement_source")
    if measurement_source not in ("orgbot_engine", "fixture", "unknown"):
        measurement_source = "unknown"
    engine_measured = measurement_source == "orgbot_engine"
    normalized = {name: _nonnegative_int(usage.get(name, 0), name) for name in TOKENS} if engine_measured else {name: 0 for name in TOKENS}
    reported = engine_measured and output.get("usage_reported") is True
    complete = reported and output.get("usage_complete") is True
    interventions = _nonnegative_int(output.get("human_interventions", 0), "human_interventions") if engine_measured else None
    engine_actual = output.get("actual_api_cost_microusd") if engine_measured else None
    engine_equivalent = output.get("api_equivalent_cost_microusd") if engine_measured else None
    for name, value in (("actual_api_cost_microusd", engine_actual), ("api_equivalent_cost_microusd", engine_equivalent)):
        if value is not None:
            _nonnegative_int(value, name)
    completion_accepted = engine_measured and output.get("completion_accepted") is True
    failed_tasks = _nonnegative_int(output.get("failed_tasks", 0), "failed_tasks") if engine_measured else 0
    accepted = accepted and (not engine_measured or completion_accepted and failed_tasks == 0)
    return {
        "variant": variant,
        "case_id": case["id"],
        "trial": trial,
        "latency_ms": elapsed_ms,
        "model": output.get("model") if isinstance(output.get("model"), str) else None,
        "billing_kind": output.get("billing_kind") if output.get("billing_kind") in ("api", "subscription", "local", "mixed") else "unknown",
        "measurement_source": measurement_source,
        "usage": normalized,
        "usage_reported": reported,
        "usage_complete": complete,
        "room_phase": output.get("room_phase") if engine_measured else None,
        "failed_tasks": failed_tasks,
        "completion_accepted": completion_accepted if engine_measured else None,
        "engine_actual_api_cost_microusd": engine_actual,
        "engine_api_equivalent_cost_microusd": engine_equivalent,
        "human_interventions": interventions,
        "accepted": accepted,
        "acceptance_method": acceptance_method,
        "artifact_verification": verification,
        "criteria": case["acceptance"],
        "agent_usage": output.get("agent_usage", []) if engine_measured else [],
        "provider_usage": output.get("provider_usage", []) if engine_measured else [],
        "status": "failed" if engine_measured and not accepted else "completed",
        "error": ("room did not complete cleanly" if not completion_accepted or failed_tasks else "fixture acceptance failed") if engine_measured and not accepted else None,
    }


def token_cost(row: dict[str, Any], rates: dict[str, Any]) -> int | None:
    model = row["model"]
    rate = rates.get(model) if model else None
    if row.get("measurement_source") != "orgbot_engine" or not row["usage_reported"] or not row["usage_complete"] or not isinstance(rate, dict):
        return None
    keys = ("input_microusd_per_million", "output_microusd_per_million", "cache_read_microusd_per_million", "cache_write_microusd_per_million")
    fields = ("input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens")
    if any(isinstance(rate.get(key), bool) or not isinstance(rate.get(key), int) or rate[key] < 0 for key in keys):
        return None
    units = sum(row["usage"][field] * rate[key] for field, key in zip(fields, keys, strict=True))
    return (units + 999_999) // 1_000_000


def summarize(rows: list[dict[str, Any]], rates: dict[str, Any]) -> dict[str, Any]:
    if not rows:
        raise ValueError("cannot summarize an empty run")
    for row in rows:
        equivalent = token_cost(row, rates)
        if row.get("engine_api_equivalent_cost_microusd") is not None:
            equivalent = row["engine_api_equivalent_cost_microusd"]
        row["api_equivalent_cost_microusd"] = equivalent
        row["actual_api_cost_microusd"] = row.get("engine_actual_api_cost_microusd")
    failed = [row for row in rows if row.get("status") == "failed"]
    usage_complete = not failed and all(row.get("usage_reported") and row.get("usage_complete") for row in rows)
    reported_rows = [row for row in rows if row.get("usage_reported")]
    billable_rows = [row for row in rows if row["billing_kind"] in ("api", "mixed")]
    unknown_rows = [row for row in rows if row["billing_kind"] == "unknown"]
    return {
        "runs": len(rows),
        "failed_runs": len(failed),
        "median_latency_ms": round(statistics.median(row["latency_ms"] for row in rows if row.get("latency_ms") is not None), 3) if any(row.get("latency_ms") is not None for row in rows) else None,
        "accepted_runs": sum(row.get("accepted") is True for row in rows),
        "acceptance_rate": round(sum(row.get("accepted") is True for row in rows) / len(rows), 4),
        "human_interventions": sum(row["human_interventions"] for row in rows) if all(row.get("human_interventions") is not None for row in rows) else None,
        "usage_reported_runs": len(reported_rows),
        "usage_complete": usage_complete,
        "tokens": {field: sum(row["usage"][field] for row in reported_rows) if reported_rows else None for field in TOKENS},
        "actual_api_cost_microusd": sum(row["actual_api_cost_microusd"] or 0 for row in billable_rows) if usage_complete and billable_rows and not unknown_rows and all(row["actual_api_cost_microusd"] is not None for row in billable_rows) else None,
        "api_equivalent_cost_microusd": sum(row["api_equivalent_cost_microusd"] or 0 for row in rows) if usage_complete and all(row["api_equivalent_cost_microusd"] is not None for row in rows) else None,
    }


def run(fixtures: list[dict[str, Any]], baseline: str, team: str, trials: int, mode: str, timeout: float, rates: dict[str, Any], output_dir: Path) -> dict[str, Any]:
    rows: list[dict[str, Any]] = []
    output_dir.mkdir(parents=True, exist_ok=True)
    output_dir.chmod(0o700)
    runs_dir = output_dir / "runs"
    runs_dir.mkdir(mode=0o700, exist_ok=True)
    runs_dir.chmod(0o700)

    def save() -> dict[str, Any]:
        groups = {variant: [row for row in rows if row["variant"] == variant] for variant in ("baseline", "team")}
        empty = {"runs": 0, "median_latency_ms": None, "accepted_runs": 0, "acceptance_rate": None,
                 "human_interventions": None, "usage_reported_runs": 0, "usage_complete": False, "failed_runs": 0, "tokens": {field: None for field in TOKENS},
                 "actual_api_cost_microusd": None, "api_equivalent_cost_microusd": None}
        report = {
            "mode": mode,
            "fixture_count": len(fixtures),
            "trials": trials,
            "paired_runs": len(fixtures) * trials,
            "completed_pairs": len(rows) // 2,
            "acceptance_method": "independent fixture verifier commands for code-workspace fixtures; legacy phrase matching is labeled and never used for those fixtures",
            "measurement_sources": {source: sum(row.get("measurement_source", row.get("partial_measurement", {}).get("measurement_source", "unknown")) == source for row in rows) for source in ("orgbot_engine", "fixture", "unknown")},
            "baseline": summarize(groups["baseline"], rates) if groups["baseline"] else empty,
            "team": summarize(groups["team"], rates) if groups["team"] else empty,
            "failed_runs": sum(row.get("status") == "failed" for row in rows),
            "runs": rows,
            "limitations": [
                "Latency includes adapter process startup and local scheduling.",
                "Workspace fixture quality is determined by the fixture's independent verification command, not adapter self-report.",
                "Only usage tagged orgbot_engine is counted; adapter-supplied freeform usage is ignored. Synthetic fixture runs report usage and costs as unknown and provide no savings evidence.",
                "API-equivalent costs use persisted engine estimates when present, otherwise supplied rates; unknown usage or rates stay unknown.",
                "Subscription calls have no actual API bill; only API-equivalent cost is shown.",
                "Each run workspace, logs, room config, and checkpoint are retained under runs/.",
            ],
        }
        output_dir.mkdir(parents=True, exist_ok=True)
        output_dir.chmod(0o700)
        (output_dir / "runs").mkdir(mode=0o700, exist_ok=True)
        (output_dir / "runs").chmod(0o700)
        (output_dir / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        (output_dir / "report.md").write_text(markdown(report), encoding="utf-8")
        return report

    for trial in range(1, trials + 1):
        order = ("baseline", "team") if trial % 2 else ("team", "baseline")
        commands = {"baseline": baseline, "team": team}
        for case in fixtures:
            for variant in order:
                safe_case_id = re.sub(r"[^A-Za-z0-9_.-]", "_", case["id"])
                run_dir = runs_dir / f"{variant}-trial-{trial}-{safe_case_id}"
                try:
                    rows.append(invoke(commands[variant], variant, trial, case, mode, timeout, run_dir))
                except (OSError, ValueError) as error:
                    try:
                        failure = json.loads(str(error))
                    except json.JSONDecodeError:
                        failure = {"variant": variant, "case_id": case["id"], "trial": trial, "status": "failed", "error": str(error), "run_dir": str(run_dir)}
                    if not isinstance(failure, dict) or failure.get("status") != "failed":
                        failure = {"variant": variant, "case_id": case["id"], "trial": trial, "status": "failed", "error": str(error), "run_dir": str(run_dir)}
                    partial = failure.get("partial_measurement")
                    if isinstance(partial, dict):
                        failure.update({
                            "measurement_source": partial.get("measurement_source", "unknown"),
                            "model": partial.get("model"),
                            "billing_kind": partial.get("billing_kind", "unknown"),
                            "usage": {field: partial.get("usage", {}).get(field, 0) for field in TOKENS} if isinstance(partial.get("usage"), dict) else {field: 0 for field in TOKENS},
                            "usage_reported": partial.get("usage_reported") is True,
                            "usage_complete": False,
                            "engine_actual_api_cost_microusd": None,
                            "engine_api_equivalent_cost_microusd": None,
                            "human_interventions": None,
                            "accepted": False,
                        })
                    else:
                        failure.update({"measurement_source": "unknown", "billing_kind": "unknown", "usage": {field: 0 for field in TOKENS}, "usage_reported": False, "usage_complete": False, "accepted": False})
                    rows.append(failure)
                    return save()
                if rows[-1].get("status") == "failed":
                    return save()
                save()
    return save()


def markdown(report: dict[str, Any]) -> str:
    lines = [
        "# Paired usage benchmark",
        "",
        f"Mode: **{report['mode']}** · {report['fixture_count']} fixtures · {report['trials']} repeated trials · {report['paired_runs']} paired tasks",
        "",
        "| Measure | Baseline | Team |",
        "|---|---:|---:|",
    ]
    for label, key in (("Median latency (ms)", "median_latency_ms"), ("Acceptance rate", "acceptance_rate"), ("Human interventions", "human_interventions")):
        lines.append(f"| {label} | {report['baseline'][key]} | {report['team'][key]} |")
    for token in TOKENS:
        lines.append(f"| {token} | {report['baseline']['tokens'][token]} | {report['team']['tokens'][token]} |")
    for key, label in (("actual_api_cost_microusd", "Estimated actual API cost (USD)"), ("api_equivalent_cost_microusd", "API-equivalent cost (USD)")):
        values = [report[variant][key] for variant in ("baseline", "team")]
        display = ["unknown" if value is None else f"${value / 1_000_000:.6f}" for value in values]
        lines.append(f"| {label} | {display[0]} | {display[1]} |")
    lines.extend(["", "## Limits", ""])
    lines.extend(f"- {item}" for item in report["limitations"])
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixtures", type=Path, default=DEFAULT_FIXTURES)
    parser.add_argument("--baseline-cmd", required=True, help="local adapter command; receives fixture JSON on stdin")
    parser.add_argument("--team-cmd", required=True, help="team adapter command; receives identical fixture JSON")
    parser.add_argument("--trials", type=int, default=3)
    parser.add_argument("--timeout", type=float, default=1800)
    parser.add_argument("--mode", choices=("recorded", "paid"), default="recorded")
    parser.add_argument("--allow-paid", action="store_true", help="required together with --mode paid")
    parser.add_argument("--rates", type=Path, help="JSON object keyed by model with four microusd-per-million rates")
    parser.add_argument("--output", type=Path, default=Path("benchmark-report"))
    args = parser.parse_args()
    if args.trials < 1 or args.trials > 100 or args.timeout <= 0:
        parser.error("trials must be 1..100 and timeout must be positive")
    if args.mode == "paid" and not args.allow_paid:
        parser.error("paid mode requires the explicit --allow-paid flag")
    if args.mode != "paid" and args.allow_paid:
        parser.error("--allow-paid is only valid with --mode paid")
    try:
        fixtures = load_fixtures(args.fixtures)
        rates = read_json(args.rates) if args.rates else {}
        if not isinstance(rates, dict):
            raise ValueError("rates file must be a JSON object keyed by model")
        report = run(fixtures, args.baseline_cmd, args.team_cmd, args.trials, args.mode, args.timeout, rates, args.output)
    except (OSError, ValueError) as error:
        print(f"benchmark failed: {error}", file=sys.stderr)
        return 2
    print(f"Wrote {args.output / 'report.md'} and {args.output / 'report.json'} ({report['paired_runs']} paired tasks; mode={args.mode})")
    return 2 if any(row.get("status") == "failed" for row in report["runs"]) else 0


if __name__ == "__main__":
    raise SystemExit(main())
