#!/usr/bin/env python3
"""Run one isolated fixture through the real OrgBots headless engine.

The benchmark runner supplies an empty temporary workspace. This adapter
renders an owner-supplied room template, invokes `orgbot run`, then measures
tokens and costs only from the persisted UsageRecorded events in the engine
checkpoint. Provider configuration uses OrgBots' normal credential lookup.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_ORGBOT = ROOT / "core/target/debug/orgbot"
USAGE_FIELDS = ("input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens")


def provider_billing(provider: dict[str, Any]) -> str:
    kind = provider.get("provider") or provider.get("kind")
    profile = provider.get("profile", "codex")
    if kind in ("anthropic", "claude", "deep_seek", "deepseek"):
        return "api"
    if kind in ("open_ai_compatible", "openai", "openai_compatible"):
        if profile in ("codex", "chatgpt"):
            return "subscription"
        if profile == "ollama":
            return "local"
        return "api"
    if kind in ("cli_agent", "cli"):
        return "subscription"
    if kind == "scripted":
        return "local"
    return "unknown"


def read_engine_usage(checkpoint: dict[str, Any]) -> dict[str, Any]:
    engine = checkpoint.get("engine")
    if not isinstance(engine, dict) or not isinstance(engine.get("log"), list):
        raise ValueError("OrgBots checkpoint did not contain an engine event log")

    providers: dict[str, dict[str, Any]] = {}
    reports: dict[str, list[dict[str, Any]]] = {}
    human_interventions = 0
    for envelope in engine["log"]:
        event = envelope.get("event") if isinstance(envelope, dict) else None
        # Event is an adjacently tagged Rust enum: {"event":"usage_recorded", ...}.
        if not isinstance(event, dict) or not isinstance(event.get("event"), str):
            continue
        event_name = event["event"]
        if event_name == "agent_joined":
            spec = event.get("spec")
            if isinstance(spec, dict) and isinstance(spec.get("id"), str):
                provider = spec.get("provider") if isinstance(spec.get("provider"), dict) else {}
                providers[spec["id"]] = {
                    "handle": spec.get("handle") if isinstance(spec.get("handle"), str) else spec["id"],
                    "provider": provider.get("provider") or provider.get("kind") or "unknown",
                    "model": provider.get("model") if isinstance(provider.get("model"), str) and provider.get("model") else None,
                    "billing_kind": provider_billing(provider),
                }
        elif event_name == "usage_recorded":
            agent = event.get("agent")
            usage = event.get("usage")
            if not isinstance(agent, str) or not isinstance(usage, dict):
                raise ValueError("OrgBots UsageRecorded event is missing agent or usage")
            reports.setdefault(agent, []).append(usage)
        elif event_name == "message_posted":
            message = event.get("message")
            author = message.get("author") if isinstance(message, dict) else None
            if isinstance(author, dict) and author.get("kind") == "human":
                human_interventions += 1

    def token_totals(items: list[dict[str, Any]]) -> dict[str, int]:
        totals = {field: 0 for field in USAGE_FIELDS}
        for item in items:
            if item.get("reported") is not True:
                continue
            for field in USAGE_FIELDS:
                value = item.get(field, 0)
                if isinstance(value, bool) or not isinstance(value, int) or value < 0:
                    raise ValueError(f"OrgBots usage event has invalid {field}")
                totals[field] += value
        return totals

    def summed_cost(items: list[dict[str, Any]], field: str) -> int | None:
        if not items:
            return None
        values = [item.get(field) for item in items]
        if any(isinstance(value, bool) or not isinstance(value, int) or value < 0 for value in values):
            return None
        return sum(values)

    agent_rows: list[dict[str, Any]] = []
    for agent_id, items in reports.items():
        provider = providers.get(agent_id, {
            "handle": agent_id, "provider": "unknown", "model": None, "billing_kind": "unknown",
        })
        reported = any(item.get("reported") is True for item in items)
        complete = bool(items) and all(item.get("reported") is True and item.get("complete") is True for item in items)
        row = {
            "agent": agent_id,
            **provider,
            "calls": len(items),
            "usage_reported": reported,
            "usage_complete": complete,
            "usage": token_totals(items),
            "api_equivalent_cost_microusd": summed_cost(items, "api_equivalent_cost_microusd") if complete else None,
            "actual_api_cost_microusd": None,
        }
        if provider["billing_kind"] == "api" and complete:
            row["actual_api_cost_microusd"] = summed_cost(items, "actual_api_cost_microusd")
        agent_rows.append(row)

    totals = {field: 0 for field in USAGE_FIELDS}
    for row in agent_rows:
        for field in USAGE_FIELDS:
            totals[field] += row["usage"][field]
    called_kinds = {row["billing_kind"] for row in agent_rows if row["calls"] > 0}
    model_ids = {row["model"] for row in agent_rows if row["calls"] > 0 and row["model"]}
    reported = any(row["usage_reported"] for row in agent_rows)
    complete = bool(agent_rows) and all(row["usage_complete"] for row in agent_rows)

    api_rows = [row for row in agent_rows if row["billing_kind"] == "api"]
    unknown_rows = [row for row in agent_rows if row["billing_kind"] == "unknown"]
    actual_api_cost = None
    if api_rows and not unknown_rows and all(row["actual_api_cost_microusd"] is not None for row in api_rows):
        actual_api_cost = sum(row["actual_api_cost_microusd"] for row in api_rows)

    api_equivalent_cost = None
    if agent_rows and all(row["api_equivalent_cost_microusd"] is not None for row in agent_rows):
        api_equivalent_cost = sum(row["api_equivalent_cost_microusd"] for row in agent_rows)

    billing_kind = next(iter(called_kinds)) if len(called_kinds) == 1 else "mixed" if called_kinds else "unknown"
    provider_totals: dict[tuple[str, str | None, str], list[dict[str, Any]]] = {}
    for row in agent_rows:
        provider_totals.setdefault((row["provider"], row["model"], row["billing_kind"]), []).append(row)
    providers_summary = []
    for (kind, model, billing), rows in sorted(provider_totals.items(), key=lambda item: (item[0][0], item[0][1] or "", item[0][2])):
        summary = {field: sum(row["usage"][field] for row in rows) for field in USAGE_FIELDS}
        summary.update({"provider": kind, "model": model, "billing_kind": billing, "agents": [row["handle"] for row in rows]})
        providers_summary.append(summary)

    return {
        "measurement_source": "orgbot_engine",
        "billing_kind": billing_kind,
        "usage_reported": reported,
        "usage_complete": complete,
        "usage": totals,
        "actual_api_cost_microusd": actual_api_cost,
        "api_equivalent_cost_microusd": api_equivalent_cost,
        "human_interventions": human_interventions,
        "agent_usage": agent_rows,
        "provider_usage": providers_summary,
        "model": next(iter(model_ids)) if len(model_ids) == 1 else None,
    }


def render_template(
    template: str,
    task: str,
    workspace: Path,
    state_path: Path,
    case_id: str,
    provider_toml: str | None = None,
    template_values: dict[str, str] | None = None,
) -> str:
    values = {
        "{{TASK}}": json.dumps(task, ensure_ascii=False),
        "{{TASK_TITLE}}": json.dumps(f"Benchmark {case_id}", ensure_ascii=False),
        "{{WORKSPACE}}": json.dumps(str(workspace), ensure_ascii=False),
        "{{STATE_PATH}}": json.dumps(str(state_path), ensure_ascii=False),
    }
    if provider_toml is not None:
        values["{{PROVIDER_CONFIG}}"] = provider_toml
    for name, value in (template_values or {}).items():
        if not name or not name.replace("_", "").isalnum():
            raise ValueError(f"invalid template value name: {name}")
        values["{{" + name + "}}"] = value
    rendered = template
    for marker, replacement in values.items():
        rendered = rendered.replace(marker, replacement)
    unresolved = re.search(r"\{\{[A-Z0-9_]+\}\}", rendered)
    if unresolved:
        raise ValueError(f"room template contains unresolved placeholder {unresolved.group(0)}")
    return rendered


def enforce_paid_gate(config: dict[str, Any], mode: str, allow_paid: str) -> None:
    remote_kinds = {"anthropic", "deep_seek", "deepseek", "open_ai_compatible", "openai", "openai_compatible", "cli_agent", "cli", "codex", "claude", "grok"}
    bots = config.get("bots", [])
    kinds = {
        (provider.get("provider") or provider.get("kind"))
        for bot in bots if isinstance(bot, dict)
        for provider in [bot.get("provider", {})]
        if isinstance(provider, dict)
    }
    if kinds & remote_kinds and not (mode == "paid" and allow_paid == "1"):
        raise ValueError("remote OrgBots providers require benchmark --mode paid --allow-paid")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config-template", type=Path, required=True, help="TOML room template; placeholders are optional")
    parser.add_argument("--orgbot", type=Path, default=Path(os.environ.get("ORGBOT_BIN", DEFAULT_ORGBOT)))
    parser.add_argument("--model", help="model label for cost-rate lookup; usage is still read from the engine checkpoint")
    parser.add_argument("--provider-toml", help='TOML inline value for a single-provider template, e.g. { kind = "cli_agent", cli = "codex", model = "gpt-5.4", workdir={{WORKSPACE}} }')
    parser.add_argument("--template-value", action="append", default=[], metavar="NAME=VALUE", help="replace a custom TOML placeholder; repeat for team templates")
    args = parser.parse_args()

    workspace_text = os.environ.get("ORGBOT_BENCH_WORKSPACE")
    state_text = os.environ.get("ORGBOT_BENCH_STATE")
    if not workspace_text or not state_text:
        parser.error("run through scripts/benchmarks/run.py so it can create isolated workspace and state paths")
    workspace = Path(workspace_text).resolve(strict=True)
    state_path = Path(state_text).resolve()
    try:
        fixture = json.load(sys.stdin)
        if not isinstance(fixture, dict) or not isinstance(fixture.get("task"), str):
            raise ValueError("fixture input needs task text")
        template = args.config_template.read_text(encoding="utf-8")
        template_values = {}
        for pair in args.template_value:
            if "=" not in pair:
                raise ValueError("--template-value must be NAME=VALUE")
            name, value = pair.split("=", 1)
            if name in template_values:
                raise ValueError(f"duplicate --template-value for {name}")
            template_values[name] = value
        rendered = render_template(template, fixture["task"], workspace, state_path, fixture["id"], args.provider_toml, template_values)
        config = tomllib.loads(rendered)
        enforce_paid_gate(config, os.environ.get("ORGBOT_BENCH_MODE", "recorded"), os.environ.get("ORGBOT_BENCH_ALLOW_PAID", "0"))
        if not args.orgbot.is_file():
            raise ValueError(f"OrgBots executable not found: {args.orgbot}")
        config_path = workspace.parent / "benchmark-room.toml"
        config_path.write_text(rendered, encoding="utf-8")
        completed = subprocess.run(
            [str(args.orgbot), "run", str(config_path), "--quiet"],
            cwd=workspace,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            check=False,
            timeout=float(os.environ.get("ORGBOT_BENCH_TIMEOUT", "1799")),
        )
        if completed.returncode != 0:
            raise ValueError(f"OrgBots run failed ({completed.returncode}): {completed.stderr[-2000:]}")
        checkpoint = json.loads(state_path.read_text(encoding="utf-8"))
        measurement = read_engine_usage(checkpoint)
        room = config.get("room", {})
        model = args.model
        if model is None:
            for bot in config.get("bots", []):
                provider = bot.get("provider", {}) if isinstance(bot, dict) else {}
                if isinstance(provider, dict) and isinstance(provider.get("model"), str):
                    model = provider["model"]
                    break
        json.dump(
            {
                "deliverable": completed.stdout.strip() or f"OrgBots completed room {room.get('name', 'benchmark')}",
                "model": model,
                **measurement,
            },
            sys.stdout,
        )
        return 0
    except (OSError, json.JSONDecodeError, tomllib.TOMLDecodeError, ValueError, subprocess.TimeoutExpired) as error:
        print(f"OrgBots benchmark adapter failed: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
