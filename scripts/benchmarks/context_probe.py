#!/usr/bin/env python3
"""Run a paid, synthetic multi-CLI context and restart probe."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import secrets
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_ENGINE = ROOT / "core/target/release/orgbot"
DEFAULT_DEEPSEEK_MODEL = "deepseek-flash"
DEFAULT_OPENCODE_MODEL = "opencode/deepseek-v4.1-flash-free"
DEFAULT_ASTRA_MODEL = "gpt-6-astra"
MAX_DEEPSEEK_REQUESTS = 80
sys.path.insert(0, str(Path(__file__).parent))
import lifecycle_probe
import orgbot_adapter


def _events(checkpoint: dict[str, Any]) -> list[dict[str, Any]]:
    engine = checkpoint.get("engine")
    if not isinstance(engine, dict) or not isinstance(engine.get("log"), list):
        raise ValueError("checkpoint has no engine event log")
    return [row for row in engine["log"] if isinstance(row, dict) and isinstance(row.get("event"), dict)]


def _agent_messages(checkpoint: dict[str, Any]) -> list[dict[str, Any]]:
    result = []
    for row in _events(checkpoint):
        event = row["event"]
        if event.get("event") == "message_posted" and isinstance(event.get("message"), dict):
            author = event["message"].get("author", {})
            if isinstance(author, dict) and author.get("kind") == "agent":
                result.append(event["message"])
    return result


def _owner_messages(checkpoint: dict[str, Any]) -> list[dict[str, Any]]:
    result = []
    for row in _events(checkpoint):
        event = row["event"]
        if event.get("event") == "message_posted" and isinstance(event.get("message"), dict):
            message = event["message"]
            if message.get("author", {}).get("kind") == "human":
                result.append(message)
    return result


def _agent_ids(checkpoint: dict[str, Any]) -> dict[str, str]:
    result = {}
    for row in _events(checkpoint):
        event = row["event"]
        if event.get("event") == "agent_joined":
            spec = event.get("spec", {})
            if isinstance(spec, dict) and isinstance(spec.get("id"), str) and isinstance(spec.get("handle"), str):
                result[spec["handle"]] = spec["id"]
    return result


def _usage_event_counts(checkpoint: dict[str, Any]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for row in _events(checkpoint):
        event = row["event"]
        if event.get("event") == "usage_recorded" and isinstance(event.get("agent"), str):
            agent = event["agent"]
            counts[agent] = counts.get(agent, 0) + 1
    return counts


def validate_message_sequence(
    messages: list[dict[str, Any]],
    steps: list[tuple[str, str, str]],
    agent_ids: dict[str, str],
) -> None:
    """Check exact agent body/order/recipient; recipient is a handle or human."""
    if len(messages) != len(steps):
        raise ValueError("unexpected number of agent messages")
    for message, (author_handle, body, recipient) in zip(messages, steps, strict=True):
        author = message.get("author", {})
        if author != {"kind": "agent", "agent": agent_ids.get(author_handle)}:
            raise ValueError("message author or order did not match the expected agent")
        if message.get("body") != body:
            raise ValueError("message body did not match the expected marker")
        audience = message.get("audience", {})
        if recipient == "everyone":
            valid = audience == {"kind": "everyone"}
        else:
            valid = audience == {"kind": "agents", "agents": [agent_ids.get(recipient)]}
        if not valid:
            raise ValueError("message recipient did not match the expected target")


def validate_owner_targets(
    messages: list[dict[str, Any]],
    steps: list[tuple[str, str]],
    agent_ids: dict[str, str],
) -> None:
    if len(messages) != len(steps):
        raise ValueError("unexpected number of owner messages")
    for message, (body, target) in zip(messages, steps, strict=True):
        if message.get("author", {}).get("kind") != "human" or message.get("author", {}).get("name") != "Owner":
            raise ValueError("owner message author or order did not match")
        if message.get("body") != body:
            raise ValueError("owner message body did not match")
        if message.get("audience") != {"kind": "agents", "agents": [agent_ids.get(target)]}:
            raise ValueError("@handle did not target the expected agent")


def owner_handle_mentions(body: str) -> list[str]:
    """Mirror the engine's owner-message @tag tokenization for probe checks."""
    found: list[str] = []
    words: list[str] = []
    current: list[str] = []
    for character in body:
        if character.isalnum() or character in "@_-":
            current.append(character)
        elif current:
            words.append("".join(current))
            current.clear()
    if current:
        words.append("".join(current))
    for word in words:
        handle = word[1:] if word.startswith("@") else ""
        if handle and not any(existing.casefold() == handle.casefold() for existing in found):
            found.append(handle)
    return found


def wait_expected_messages(room: Any, steps: list[tuple[str, str, str]]) -> None:
    for _, body, _ in steps:
        room.wait_body(body)


def body_matches_wait_target(actual: Any, expected: str) -> bool:
    return isinstance(actual, str) and actual.strip() == expected.strip()


def participants_settled(snapshot: dict[str, Any], handles: list[str]) -> bool:
    roster = {row.get("handle"): row.get("status") for row in snapshot.get("roster", []) if isinstance(row, dict)}
    statuses = [roster.get(handle) for handle in handles]
    if any(status in ("offline", "faulted") for status in statuses):
        raise ValueError("an expected agent became unavailable before the room settled")
    return all(status in ("idle", "waiting") for status in statuses)


def validate_usage(usage: dict[str, Any], handles: list[str]) -> dict[str, dict[str, Any]]:
    rows = {row.get("handle"): row for row in usage.get("agent_usage", []) if isinstance(row, dict)}
    totals = {handle: rows.get(handle) for handle in handles}
    for handle, row in totals.items():
        if not isinstance(row, dict) or row.get("usage_reported") is not True or row.get("usage_complete") is not True or row.get("calls", 0) <= 0:
            raise ValueError(f"complete positive usage is missing for {handle}")
        token_values = row.get("usage", {})
        if not isinstance(token_values, dict) or sum(v for v in token_values.values() if isinstance(v, int) and not isinstance(v, bool)) <= 0:
            raise ValueError(f"positive token usage is missing for {handle}")
    return totals


def validate_usage_growth(before: dict[str, Any], after: dict[str, Any], handles: list[str]) -> None:
    old = validate_usage(before, handles)
    new = validate_usage(after, handles)
    for handle in handles:
        if new[handle]["calls"] <= old[handle]["calls"]:
            raise ValueError(f"usage calls did not increase after restart for {handle}")
        for field in orgbot_adapter.USAGE_FIELDS:
            if new[handle]["usage"].get(field, 0) < old[handle]["usage"].get(field, 0):
                raise ValueError(f"persisted usage regressed after restart for {handle}")
        if sum(new[handle]["usage"].values()) <= sum(old[handle]["usage"].values()):
            raise ValueError(f"usage tokens did not increase after restart for {handle}")


def validate_isolated_room(checkpoint: dict[str, Any], nonce: str, messages: list[dict[str, Any]], agent_id: str) -> None:
    if nonce in json.dumps(checkpoint, ensure_ascii=False):
        raise ValueError("isolated room checkpoint contains the primary room nonce")
    if len(messages) != 1:
        raise ValueError("isolated room produced unexpected agent messages")
    message = messages[0]
    if message.get("author") != {"kind": "agent", "agent": agent_id} or not body_matches_wait_target(message.get("body"), "UNKNOWN"):
        raise ValueError("isolated CLI room did not report unknown prior context")
    if message.get("audience") != {"kind": "everyone"}:
        raise ValueError("isolated OpenCode reply was not visible to the owner")


def _session_fingerprint(state_path: Path, handle: str) -> str:
    path = state_path.parent / "cli-sessions" / handle.encode("utf-8").hex()
    path = path.with_suffix(".json")
    record = json.loads(path.read_text(encoding="utf-8"))
    session_id = record.get("session_id")
    if not isinstance(session_id, str) or not session_id:
        raise ValueError("native CLI session ID was not persisted")
    return hashlib.sha256(session_id.encode("utf-8")).hexdigest()


def _toml_quote(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def _write_skill(path: Path, nonce: str, recipient: str) -> None:
    path.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    path.write_text(
        "---\nname: context-seed\ndescription: one-time synthetic context seed\n---\n\n"
        f"The seed value is the raw nonce {nonce}. Do not repeat the SEED: prefix as part of the value. "
        f"When asked, post exactly SEED:{nonce} to @{recipient}.\n",
        encoding="utf-8",
    )
    os.chmod(path, 0o600)


def _room_config(
    room_dir: Path,
    skill_dir: Path | None,
    deepseek_model: str,
    opencode_model: str,
    astra_model: str,
    include_all: bool,
    skip_opencode: bool = False,
) -> str:
    room_dir.mkdir(parents=True, mode=0o700, exist_ok=True)
    os.chmod(room_dir, 0o700)
    projects = room_dir / "projects"
    projects.mkdir(mode=0o700, exist_ok=True)
    os.chmod(projects, 0o700)
    if include_all:
        handles = ("deepseek", "astra") if skip_opencode else ("deepseek", "opencode", "astra")
    else:
        handles = ("astra",) if skip_opencode else ("opencode",)
    for handle in handles:
        project = projects / handle
        project.mkdir(parents=True, mode=0o700, exist_ok=True)
        (project / "synthetic.txt").write_text("Synthetic context probe workspace. No user data.\n", encoding="utf-8")
        os.chmod(project / "synthetic.txt", 0o600)
    skill_clause = f"\n[[skills]]\npath = {_toml_quote(str(skill_dir))}\n" if skill_dir else ""
    bots = ""
    if include_all:
        recipient = "astra" if skip_opencode else "opencode"
        deepseek_persona = f"Only respond when directly addressed. On the owner's first request, discover and use the one-time context-seed skill exactly once. Use its raw nonce value as the marker; do not prepend a second SEED: prefix. Then call post_message with body SEED:<raw nonce> and to_agents=[{recipient}]. Do not include extra text. Ignore all other room messages. Do not run commands, inspect files, create tasks, or spawn agents."
        opencode_persona = "Only respond when directly addressed. On a SEED message, retain its marker and call post_message with body ACK:<exact marker> and to_agents=[astra]. After restart, on an owner recall request, use the prior room transcript/session and call post_message with body RESUME:<exact marker> and to_agents=[astra]. If no marker is present in an isolated room, reply exactly UNKNOWN to the owner. Ignore all other room messages. Do not run commands, inspect files, create tasks, or spawn agents."
        if skip_opencode:
            astra_persona = "Only respond when directly addressed. On SEED:<marker>, retain the exact marker and call post_message with body ACK:<exact marker>. After restart, on the owner's recall request use the same room context and post RESUME:<exact marker>. Leave the recipient unset so the owner can see the proof. Ignore all other messages. Do not run commands, inspect files, create tasks, or spawn agents."
        else:
            astra_persona = "Only respond when directly addressed. On ACK:<marker>, call post_message with body VERIFIED:<exact marker>. After restart, on RESUME:<marker>, call post_message with body RESUME_VERIFIED:<exact marker>. Leave the recipient unset so the owner can see the proof. Ignore all other messages. Do not run commands, inspect files, create tasks, or spawn agents."
        bots += f'''\n[[bots]]\nhandle = "deepseek"\npersona = {_toml_quote(deepseek_persona)}\nprovider = {{ kind = "deepseek", model = {_toml_quote(deepseek_model)} }}\nautonomy = {{ max_turns_per_task = 5, may_inspect = false, may_submit = false, may_create_tasks = false }}\n\n[[bots]]\nhandle = "opencode"\npersona = {_toml_quote(opencode_persona)}\nprovider = {{ kind = "cli_agent", cli = "opencode", model = {_toml_quote(opencode_model)}, workdir = {_toml_quote(str(projects / "opencode"))}, policy = "read_only", mcp = true }}\nautonomy = {{ max_turns_per_task = 5, may_inspect = false, may_submit = false, may_create_tasks = false }}\n\n[[bots]]\nhandle = "astra"\npersona = {_toml_quote(astra_persona)}\nprovider = {{ kind = "cli_agent", cli = "codex", model = {_toml_quote(astra_model)}, workdir = {_toml_quote(str(projects / "astra"))}, policy = "read_only", mcp = true }}\nautonomy = {{ max_turns_per_task = 5, may_inspect = false, may_submit = false, may_create_tasks = false }}\n'''
        if skip_opencode:
            bots = f'''\n[[bots]]\nhandle = "deepseek"\npersona = {_toml_quote(deepseek_persona)}\nprovider = {{ kind = "deepseek", model = {_toml_quote(deepseek_model)} }}\nautonomy = {{ max_turns_per_task = 5, may_inspect = false, may_submit = false, may_create_tasks = false }}\n\n[[bots]]\nhandle = "astra"\npersona = {_toml_quote(astra_persona)}\nprovider = {{ kind = "cli_agent", cli = "codex", model = {_toml_quote(astra_model)}, workdir = {_toml_quote(str(projects / "astra"))}, policy = "read_only", mcp = true }}\nautonomy = {{ max_turns_per_task = 5, may_inspect = false, may_submit = false, may_create_tasks = false }}\n'''
    else:
        isolated_persona = "Only respond when directly addressed. Use only context available in this room. If the requested seed is absent from the current room context, reply exactly UNKNOWN. Do not infer content from another room. Do not run commands, inspect files, create tasks, or spawn agents."
        isolated_handle = "astra" if skip_opencode else "opencode"
        isolated_cli = "codex" if skip_opencode else "opencode"
        isolated_model = astra_model if skip_opencode else opencode_model
        bots += f'''\n[[bots]]\nhandle = "{isolated_handle}"\npersona = {_toml_quote(isolated_persona)}\nprovider = {{ kind = "cli_agent", cli = "{isolated_cli}", model = {_toml_quote(isolated_model)}, workdir = {_toml_quote(str(projects / isolated_handle))}, policy = "read_only", mcp = true }}\nautonomy = {{ max_turns_per_task = 3, may_inspect = false, may_submit = false, may_create_tasks = false }}\n'''
    workspace = projects / ("deepseek" if include_all else ("astra" if skip_opencode else "opencode"))
    return f'''[workspace]\nlabel = "Synthetic context probe"\nroot = {_toml_quote(str(workspace))}\nwritable = false\ncommands = false\n\n[room]\nname = "Synthetic context probe"\ngoal = "Manual completion; wait for directly addressed owner messages."\ncompletion = "manual"\nstate_path = {_toml_quote(str(room_dir / "state.json"))}\n\n[room.limits]\nmax_total_turns = 12\nmax_duration_ms = 600000\nmax_parallel_turns = 1\nmax_consecutive_errors = 1\nhandoff_timeout_ms = 60000\n{skill_clause}{bots}'''


def _clean_env(token: str, allowance_config: Path, allowance_ledger: Path, engine: Path) -> dict[str, str]:
    env = os.environ.copy()
    for key in list(env):
        upper = key.upper()
        secret_env = any(part in upper for part in ("_KEY", "TOKEN", "SECRET", "PASSWORD", "CREDENTIAL"))
        if (secret_env and upper != "DEEPSEEK_API_KEY") or upper.startswith("ORGBOT_") or upper in {
            "OPENAI_BASE_URL", "ANTHROPIC_BASE_URL", "CODEX_HOME", "OPENCODE_CONFIG",
            "OPENCODE_CONFIG_DIR", "OPENCODE_DATA_DIR", "GEMINI_CLI_HOME",
            "DBUS_SESSION_BUS_ADDRESS", "SSH_AUTH_SOCK", "SSH_AGENT_PID",
        }:
            env.pop(key, None)
    env["ORGBOT_ROOM_TOKEN"] = token
    env["ORGBOT_ALLOWANCE_CONFIG"] = str(allowance_config)
    env["ORGBOT_ALLOWANCE_LEDGER"] = str(allowance_ledger)
    env["ORGBOT_MCP_RELAY_BIN"] = str(engine)
    env["ORGBOT_CLI_TURN_TIMEOUT_SECS"] = "240"
    env["RUST_LOG"] = "warn"
    return env


class _Room:
    def __init__(self, config: Path, output: Path, engine: Path, token: str, env: dict[str, str], deadline: float):
        self.config, self.output, self.engine = config, output, engine
        self.token, self.env, self.deadline = token, env, deadline
        self.port = lifecycle_probe._free_port()
        log_path = output / "server.log"
        self.log = log_path.open("ab")
        os.chmod(log_path, 0o600)
        self.proc: subprocess.Popen[bytes] | None = None
        self.ws: Any = None
        self.snapshot: dict[str, Any] | None = None
        self.seen_usage_counts: dict[str, int] = {}
        self.seen_event_seqs: set[int] = set()

    def agent_id(self, handle: str) -> str:
        roster = self.snapshot.get("roster", []) if isinstance(self.snapshot, dict) else []
        agent_id = next((row.get("id") for row in roster if isinstance(row, dict) and row.get("handle") == handle), None)
        if not isinstance(agent_id, str) or not agent_id:
            raise ValueError(f"room snapshot is missing {handle}")
        return agent_id

    def _observe(self, message: dict[str, Any]) -> None:
        envelope = message.get("envelope", {})
        sequence = envelope.get("seq") if isinstance(envelope, dict) else None
        event = envelope.get("event", {}) if isinstance(envelope, dict) else {}
        if isinstance(sequence, bool) or not isinstance(sequence, int):
            return
        if sequence in self.seen_event_seqs:
            return
        self.seen_event_seqs.add(sequence)
        if isinstance(event, dict) and event.get("event") == "usage_recorded" and isinstance(event.get("agent"), str):
            agent_id = event["agent"]
            self.seen_usage_counts[agent_id] = self.seen_usage_counts.get(agent_id, 0) + 1

    def start(self) -> None:
        self.proc = subprocess.Popen(
            [str(self.engine), "serve", str(self.config), "--addr", f"127.0.0.1:{self.port}"],
            env=self.env, stdout=self.log, stderr=self.log, start_new_session=True,
        )
        try:
            self.ws = lifecycle_probe._connect(
                f"ws://127.0.0.1:{self.port}/ws", self.token,
                min(self.deadline, time.monotonic() + 30),
            )
        except TimeoutError:
            if self.proc is not None and self.proc.poll() is not None:
                raise RuntimeError("room server exited before the WebSocket was ready") from None
            raise
        for message in lifecycle_probe._messages(self.ws, min(2, max(.1, self.deadline-time.monotonic()))):
            self._observe(message)
            self._reject_approval(message)
            if message.get("type") == "snapshot":
                self.snapshot = message
                return
        raise TimeoutError("room snapshot did not arrive")

    def send(self, body: str) -> None:
        self.ws.send(json.dumps({"type": "send_message", "author": "Owner", "body": body}))

    def wait_body(self, body: str) -> None:
        while time.monotonic() < self.deadline:
            for message in lifecycle_probe._messages(self.ws, min(1, max(.1, self.deadline-time.monotonic()))):
                self._observe(message)
                self._reject_approval(message)
                event = message.get("envelope", {}).get("event", {})
                chat = event.get("message", {}) if event.get("event") == "message_posted" else {}
                author = chat.get("author", {})
                if author.get("kind") == "agent" and body_matches_wait_target(chat.get("body"), body):
                    return
        raise TimeoutError("expected agent message did not arrive")

    def wait_usage_after(self, baselines: dict[str, int], targets: dict[str, str]) -> None:
        counts = self.seen_usage_counts
        while time.monotonic() < self.deadline:
            if all(counts.get(agent_id, 0) > baselines.get(agent_id, 0) for agent_id in targets.values()):
                return
            for message in lifecycle_probe._messages(self.ws, min(1, max(.1, self.deadline-time.monotonic()))):
                self._observe(message)
                self._reject_approval(message)
                if all(counts.get(agent_id, 0) > baselines.get(agent_id, 0) for agent_id in targets.values()):
                    return
        raise TimeoutError("usage was not persisted for every expected agent episode")

    def wait_settled(self, handles: list[str]) -> None:
        while time.monotonic() < self.deadline:
            self.ws.send(json.dumps({"type": "resync"}))
            for message in lifecycle_probe._messages(self.ws, min(1, max(.1, self.deadline-time.monotonic()))):
                self._observe(message)
                self._reject_approval(message)
                if message.get("type") != "snapshot":
                    continue
                self.snapshot = message
                if participants_settled(message, handles):
                    return
        raise TimeoutError("room participants did not settle before pause")

    def pause(self) -> None:
        self.ws.send(json.dumps({"type": "pause", "reason": "Capture synthetic context checkpoint"}))
        while time.monotonic() < self.deadline:
            self.ws.send(json.dumps({"type": "resync"}))
            for message in lifecycle_probe._messages(self.ws, min(1, max(.1, self.deadline-time.monotonic()))):
                self._observe(message)
                self._reject_approval(message)
                if message.get("type") == "snapshot" and message.get("room", {}).get("phase") == "paused":
                    return
        raise TimeoutError("room did not pause")

    @staticmethod
    def _reject_approval(message: dict[str, Any]) -> None:
        event = message.get("envelope", {}).get("event", {})
        if isinstance(event, dict) and event.get("event") == "decision_asked":
            raise ValueError("unexpected owner approval request; no automatic approval was attempted")

    def stop(self) -> None:
        if self.ws is not None:
            self.ws.close()
            self.ws = None
        if self.proc is not None:
            process_group = self.proc.pid
            try:
                os.killpg(process_group, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                self.proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process_group, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                self.proc.wait()
            try:
                os.killpg(process_group, signal.SIGKILL)
            except ProcessLookupError:
                pass
            self.proc = None
        self.log.close()


def _tool_result(checkpoint: dict[str, Any], agent_id: str, name: str) -> str:
    history = checkpoint.get("bots", {}).get(agent_id, {}).get("history", [])
    target_ids = [block.get("id") for turn in history if isinstance(turn, dict)
                  for block in turn.get("content", []) if isinstance(block, dict)
                  and block.get("type") == "tool_use" and block.get("name") == name]
    for turn in history:
        for block in turn.get("content", []) if isinstance(turn, dict) else []:
            if isinstance(block, dict) and block.get("type") == "tool_result" and block.get("tool_use_id") in target_ids:
                return str(block.get("content", ""))
    return ""


def _usage_digest(usage: dict[str, Any], handles: list[str]) -> dict[str, Any]:
    rows = validate_usage(usage, handles)
    fields = ("provider", "cli", "model", "billing_kind", "calls", "usage",
              "actual_api_cost_microusd", "api_equivalent_cost_microusd")
    return {handle: {field: rows[handle].get(field) for field in fields} for handle in handles}


def restart_baseline_evidence(
    checkpoint: dict[str, Any],
    usage: dict[str, Any],
    handles: list[str],
    agent_ids: dict[str, str],
    session_hashes: dict[str, str],
) -> dict[str, Any]:
    event_counts = _usage_event_counts(checkpoint)
    return {
        "schema": 1,
        "engine_event_count": len(_events(checkpoint)),
        "usage_event_counts": {handle: event_counts.get(agent_ids[handle], 0) for handle in handles},
        "native_session_sha256": session_hashes,
        "usage": _usage_digest(usage, handles),
    }


def _safe_failure_reason(error: Exception, nonce: str, token: str) -> str:
    reason = str(error)
    for secret in (nonce, token, os.environ.get("DEEPSEEK_API_KEY", "")):
        if secret:
            reason = reason.replace(secret, "[redacted]")
    reason = re.sub(r"(?i)(bearer\s+)[A-Za-z0-9._~+/=-]+", r"\1[redacted]", reason)
    reason = re.sub(r"(?i)((?:api[_ -]?key|access[_ -]?token|refresh[_ -]?token)[=: ]+)[^\s,;]+", r"\1[redacted]", reason)
    return reason[:1200]


def run_probe(engine: Path, output: Path, timeout: int, deepseek_model: str, opencode_model: str, astra_model: str, skip_opencode: bool = False) -> dict[str, Any]:
    if not os.environ.get("DEEPSEEK_API_KEY"):
        raise ValueError("DEEPSEEK_API_KEY must be present")
    allowance_config, allowance_ledger = lifecycle_probe._allowance_gate()
    allowance = json.loads(allowance_config.read_text(encoding="utf-8"))
    max_requests = allowance["providers"]["deepseek"]["max_requests"]
    if max_requests > MAX_DEEPSEEK_REQUESTS:
        raise ValueError(f"DeepSeek allowance max_requests must not exceed {MAX_DEEPSEEK_REQUESTS} for this probe")
    if output.exists() or output.is_symlink():
        raise ValueError("--output must be a new directory")
    output.mkdir(parents=True, mode=0o700)
    os.chmod(output, 0o700)
    token = secrets.token_urlsafe(32)
    env = _clean_env(token, allowance_config, allowance_ledger, engine)
    nonce = secrets.token_urlsafe(24)
    primary = output / "primary"
    isolated = output / "isolated"
    primary.mkdir(mode=0o700)
    isolated.mkdir(mode=0o700)
    os.chmod(primary, 0o700)
    os.chmod(isolated, 0o700)
    skill_path = output / "registered-skills" / "context-seed" / "SKILL.md"
    initial_peer = "astra" if skip_opencode else "opencode"
    _write_skill(skill_path, nonce, initial_peer)
    primary_config = primary / "room.toml"
    primary_config.write_text(_room_config(primary, skill_path.parent, deepseek_model, opencode_model, astra_model, True, skip_opencode), encoding="utf-8")
    os.chmod(primary_config, 0o600)
    isolated_config = isolated / "room.toml"
    isolated_config.write_text(_room_config(isolated, None, deepseek_model, opencode_model, astra_model, False, skip_opencode), encoding="utf-8")
    os.chmod(isolated_config, 0o600)
    handles = ["deepseek", "astra"] if skip_opencode else ["deepseek", "opencode", "astra"]
    if skip_opencode:
        expected = [("deepseek", f"SEED:{nonce}", "astra"), ("astra", f"ACK:{nonce}", "everyone")]
        resume_steps = [("astra", f"RESUME:{nonce}", "everyone")]
        first_prompt = "@deepseek Use the one-time context-seed skill and send its exact marker to the bot named astra. No other text."
        resume_prompt = "@astra Recall the exact marker shared earlier in this room and post RESUME:<marker> for the owner."
        isolated_prompt = "@astra Recall the seed from this room and report UNKNOWN if none exists."
        isolated_handle = "astra"
    else:
        expected = [
            ("deepseek", f"SEED:{nonce}", "opencode"),
            ("opencode", f"ACK:{nonce}", "astra"),
            ("astra", f"VERIFIED:{nonce}", "everyone"),
        ]
        resume_steps = [("opencode", f"RESUME:{nonce}", "astra"), ("astra", f"RESUME_VERIFIED:{nonce}", "everyone")]
        first_prompt = "@deepseek Use the one-time context-seed skill and send its exact marker to the bot named opencode. No other text."
        resume_prompt = "@opencode Recall the exact marker shared earlier in this room and send RESUME:<marker> to the bot named astra."
        isolated_prompt = "@opencode Recall the seed from this room and report UNKNOWN if none exists."
        isolated_handle = "opencode"
    for prompt, target in ((first_prompt, "deepseek"), (resume_prompt, initial_peer), (isolated_prompt, isolated_handle)):
        if owner_handle_mentions(prompt) != [target]:
            raise ValueError("each owner prompt must mention only its intended agent")
    deadline = time.monotonic() + timeout
    room: _Room | None = None
    stage = "setup"
    try:
        stage = "primary_initial_exchange"
        room = _Room(primary_config, primary, engine, token, env, deadline)
        room.start()
        runtime_ids = {handle: room.agent_id(handle) for handle in handles}
        room.send(first_prompt)
        wait_expected_messages(room, expected)
        room.wait_usage_after({}, runtime_ids)
        room.wait_settled(handles)
        room.pause()
        first = json.loads((primary / "state.json").read_text(encoding="utf-8"))
        ids = _agent_ids(first)
        if any(not ids.get(handle) for handle in handles):
            raise ValueError("primary room is missing an expected agent identity")
        validate_message_sequence(_agent_messages(first), expected, ids)
        validate_owner_targets(_owner_messages(first), [(first_prompt, "deepseek")], ids)
        first_usage = orgbot_adapter.read_engine_usage(first)
        validate_usage(first_usage, handles)
        first_event_counts = _usage_event_counts(first)
        skill_calls = [call for call in lifecycle_probe.capability_history(first, ids["deepseek"])
                       if call.get("name") == "use_skill" and call.get("arguments", {}).get("skill") == "context-seed"]
        if len(skill_calls) != 1 or skill_calls[0].get("arguments", {}).get("lifetime") != "once" or _tool_result(first, ids["deepseek"], "use_skill") != "[skill instructions released]":
            raise ValueError("one-use skill result was not released from the model history")
        initial_astra = _session_fingerprint(primary / "state.json", "astra")
        initial_open = None if skip_opencode else _session_fingerprint(primary / "state.json", "opencode")
        baseline_sessions = {"astra": initial_astra}
        if not skip_opencode:
            baseline_sessions["opencode"] = initial_open
        lifecycle_probe._atomic_json(primary / "restart-baseline.json", restart_baseline_evidence(
            first, first_usage, handles, ids, baseline_sessions,
        ))
        room.stop()
        room = None

        stage = "primary_restart_exchange"
        room = _Room(primary_config, primary, engine, token, env, deadline)
        room.start()
        room.ws.send(json.dumps({"type": "resume"}))
        room.send(resume_prompt)
        if skip_opencode:
            room.wait_body(f"RESUME:{nonce}")
            resume_handles = ["astra"]
        else:
            room.wait_body(f"RESUME:{nonce}")
            room.wait_body(f"RESUME_VERIFIED:{nonce}")
            resume_handles = ["opencode", "astra"]
        room.wait_usage_after(first_event_counts, {handle: runtime_ids[handle] for handle in resume_handles})
        room.wait_settled(handles)
        room.pause()
        room.stop()
        room = None
        resumed = json.loads((primary / "state.json").read_text(encoding="utf-8"))
        validate_message_sequence(_agent_messages(resumed)[len(expected):], resume_steps, ids)
        validate_owner_targets(_owner_messages(resumed), [(first_prompt, "deepseek"), (resume_prompt, initial_peer)], ids)
        resumed_usage = orgbot_adapter.read_engine_usage(resumed)
        validate_usage_growth(first_usage, resumed_usage, resume_handles)
        if (not skip_opencode and initial_open != _session_fingerprint(primary / "state.json", "opencode")) or initial_astra != _session_fingerprint(primary / "state.json", "astra"):
            raise ValueError("native CLI session changed across process restart")

        stage = "isolated_room_exchange"
        room = _Room(isolated_config, isolated, engine, token, env, deadline)
        room.start()
        isolated_runtime_id = room.agent_id(isolated_handle)
        room.send(isolated_prompt)
        room.wait_body("UNKNOWN")
        room.wait_usage_after({}, {isolated_handle: isolated_runtime_id})
        room.wait_settled([isolated_handle])
        room.pause()
        room.stop()
        room = None
        isolated_checkpoint = json.loads((isolated / "state.json").read_text(encoding="utf-8"))
        isolated_id = _agent_ids(isolated_checkpoint).get(isolated_handle)
        if not isolated_id:
            raise ValueError("isolated room CLI identity is missing")
        validate_isolated_room(isolated_checkpoint, nonce, _agent_messages(isolated_checkpoint), isolated_id)
        validate_owner_targets(_owner_messages(isolated_checkpoint), [(isolated_prompt, isolated_handle)], {isolated_handle: isolated_id})
        isolated_usage = orgbot_adapter.read_engine_usage(isolated_checkpoint)
        validate_usage(isolated_usage, [isolated_handle])
        isolated_session = _session_fingerprint(isolated / "state.json", isolated_handle)
        primary_session = initial_astra if skip_opencode else initial_open
        if isolated_session == primary_session:
            raise ValueError("isolated room reused the primary native CLI session")

        proof = {
            "schema": 1,
            "participants": handles,
            "excluded": {"opencode": "not run: free-model quota is exhausted; no retry or fallback"} if skip_opencode else {},
            "checks": {
                "primary_message_sequence_and_recipients": True,
                "one_time_skill_released": True,
                "primary_participants_complete_positive_usage": True,
                "resume_message_sequence": True,
                "native_cli_sessions_retained": True,
                "usage_increased_without_regression": True,
                "isolated_room_reported_unknown": True,
                "isolated_checkpoint_excludes_primary_nonce": True,
                "isolated_native_session_is_distinct": True,
                "opencode_route": "not_run" if skip_opencode else True,
            },
            "seed_sha256": hashlib.sha256(nonce.encode("utf-8")).hexdigest(),
            "native_session_sha256": ({"astra": initial_astra, "isolated_astra": isolated_session} if skip_opencode else {"opencode": initial_open, "astra": initial_astra, "isolated_opencode": isolated_session}),
            "usage": {"before_restart": _usage_digest(first_usage, handles),
                      "after_restart": _usage_digest(resumed_usage, handles),
                      "isolated_room": _usage_digest(isolated_usage, [isolated_handle])},
        }
        lifecycle_probe._atomic_json(output / "proof.json", proof)
        return proof
    except Exception as error:
        failure = {
            "stage": stage,
            "error_type": type(error).__name__,
            "reason": _safe_failure_reason(error, nonce, token),
            "primary_checkpoint_present": (primary / "state.json").is_file(),
            "isolated_checkpoint_present": (isolated / "state.json").is_file(),
            "primary_log_present": (primary / "server.log").is_file(),
            "isolated_log_present": (isolated / "server.log").is_file(),
        }
        lifecycle_probe._atomic_json(output / "failure.json", failure)
        raise
    finally:
        if room is not None:
            room.stop()


def _handle_external_interrupt(signum: int, _frame: Any) -> None:
    """Turn external termination into a Python exception so owned children stop in finally."""
    raise InterruptedError(f"context probe interrupted by signal {signum}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-paid", action="store_true", required=True)
    parser.add_argument("--engine", type=Path, default=Path(os.environ.get("ORGBOT_BIN", DEFAULT_ENGINE)))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--deepseek-model", default=DEFAULT_DEEPSEEK_MODEL)
    parser.add_argument("--opencode-model", default=DEFAULT_OPENCODE_MODEL)
    parser.add_argument("--astra-model", default=DEFAULT_ASTRA_MODEL)
    parser.add_argument("--skip-opencode", action="store_true", help="run DeepSeek plus Astra only while the OpenCode route is unavailable")
    args = parser.parse_args()
    if not args.engine.is_file() or not os.access(args.engine, os.X_OK):
        parser.error("--engine must be an executable built orgbot binary")
    if not (30 <= args.timeout <= 600):
        parser.error("--timeout must be 30..600 seconds")
    signal.signal(signal.SIGTERM, _handle_external_interrupt)
    signal.signal(signal.SIGINT, _handle_external_interrupt)
    try:
        proof = run_probe(args.engine.resolve(), args.output.expanduser().absolute(), args.timeout,
                          args.deepseek_model, args.opencode_model, args.astra_model, args.skip_opencode)
    except Exception as error:
        print(f"context probe failed ({type(error).__name__}); private diagnostics are in the output directory", file=sys.stderr)
        return 1
    print(f"Context probe passed; private proof: {args.output.expanduser().absolute() / 'proof.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
