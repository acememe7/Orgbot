#!/usr/bin/env python3
"""Run a paid, restart-spanning capability/MCP lifecycle proof against orgbot serve."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import secrets
import socket
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_ENGINE = ROOT / "core/target/release/orgbot"
TOOL = "read_nonce"
MCP_TOOL = "lifecycle_fixture__read_nonce"


def approval_matches(decision: dict[str, Any], agent: str, server: str, tool: str, args: dict[str, str]) -> bool:
    because = decision.get("because", "")
    because, separator, sequence = because.rpartition("; approval_sequence=")
    prefix = f"External MCP call: agent={agent}; server={server};"
    return bool(separator) and sequence == "0" and bool(decision.get("owner_question")) and decision.get("asked_by") == [agent] and decision.get("options") == ["Allow once", "Deny"] and because.startswith(prefix) and f"; tool={tool};" in because and because.endswith("; args=" + json.dumps(args, sort_keys=True, separators=(",", ":")))


def validate_proof(proof: dict[str, Any]) -> None:
    checks = proof.get("checks")
    if not isinstance(checks, dict) or any(checks.get(key) is not True for key in (
        "source_hash_unchanged", "skill_nonce_seen", "tool_nonce_seen", "mcp_invoked_once",
        "tool_arguments_exact", "pending_survived_restart", "approval_exact", "usage_positive", "tool_history_present",
        "pending_persisted_before_restart", "lifecycle_sequence_exact", "skill_once", "schema_loaded_once",
        "schema_released", "successful_tool_result", "handed_to_review",
    )):
        raise ValueError("lifecycle proof is incomplete")
    if proof.get("mcp_invocations") != 1:
        raise ValueError("fixture MCP server must have exactly one successful invocation")
    if proof.get("task_states") != ["in_review"]:
        raise ValueError("the fixture worker must hand its evidence to review before resting")
    skill_nonce, tool_nonce = proof.get("skill_nonce"), proof.get("tool_nonce")
    if not isinstance(skill_nonce, str) or not skill_nonce or not isinstance(tool_nonce, str) or not tool_nonce or skill_nonce not in proof.get("final_agent_message", ""):
        raise ValueError("final agent message does not contain the skill-body nonce")
    if proof.get("skill_result") != "[skill instructions released]":
        raise ValueError("checkpoint does not prove the one-shot skill body was released")
    if proof.get("source_sha256") != proof.get("source_sha256_after"):
        raise ValueError("fixture sources or project sentinel changed during the run")
    decision = proof.get("pending_decision")
    if not isinstance(decision, dict) or not decision.get("id") or proof.get("reloaded_pending_id") != decision.get("id"):
        raise ValueError("the same pending approval was not observed after restart")
    actor = re.match(r"External MCP call: agent=([^;]+); server=", decision.get("because", ""))
    if proof.get("approval_chosen") != "Allow once" or not actor or not approval_matches(decision, actor.group(1), "lifecycle_fixture", MCP_TOOL, {"nonce": tool_nonce}):
        raise ValueError("approval evidence does not match the exact fixture call")
    expected_mcp = [{"name": TOOL, "arguments": {"nonce": tool_nonce}}]
    if proof.get("mcp_call_history") != expected_mcp or not any("FIXTURE_NONCE:" + str(tool_nonce) in result for result in proof.get("tool_result_history", [])):
        raise ValueError("the exact fixture call and its successful model result are not evidenced")
    calls = proof.get("capability_call_history", [])
    name_args = [(call.get("name"), call.get("arguments", {})) for call in calls if isinstance(call, dict)]
    schema_calls = [(n, a) for n, a in name_args if n == "load_mcp_tool" and a.get("server") == "lifecycle_fixture" and a.get("tool") == MCP_TOOL]
    if len(schema_calls) != 2 or any(args.get("lifetime") != "once" for _, args in schema_calls):
        raise ValueError("one-shot MCP schema must be loaded once before the pending attempt and once after reload")
    if proof.get("schema_result_history") != ["[MCP tool schema released]", "[MCP tool schema released]"]:
        raise ValueError("checkpoint does not prove both one-shot MCP schemas were released")
    try:
        find_skill = next(i for i, (n, a) in enumerate(name_args) if n == "find_capabilities" and a.get("kind") == "skill")
        use_skill = next(i for i, (n, a) in enumerate(name_args) if i > find_skill and n == "use_skill" and a.get("skill") == "lifecycle-proof" and a.get("lifetime") == "once")
        list_mcp = next(i for i, (n, a) in enumerate(name_args) if i > use_skill and n == "find_capabilities" and a.get("kind") == "mcp" and "server" not in a)
        inspect_mcp = next(i for i, (n, a) in enumerate(name_args) if i > list_mcp and n == "find_capabilities" and a.get("kind") == "mcp" and a.get("server") == "lifecycle_fixture")
        load_first = next(i for i, (n, a) in enumerate(name_args) if i > inspect_mcp and n == "load_mcp_tool" and a.get("server") == "lifecycle_fixture" and a.get("tool") == MCP_TOOL and a.get("lifetime") == "once")
        call_first = next(i for i, (n, a) in enumerate(name_args) if i > load_first and n == "call_mcp_tool" and a.get("tool") == MCP_TOOL)
        load_again = next(i for i, (n, a) in enumerate(name_args) if i > call_first and n == "load_mcp_tool" and a.get("server") == "lifecycle_fixture" and a.get("tool") == MCP_TOOL and a.get("lifetime") == "once")
        retry_call = next(i for i, (n, a) in enumerate(name_args) if i > load_again and n == "call_mcp_tool" and a.get("tool") == MCP_TOOL)
        if name_args[call_first][1].get("arguments") != {"nonce": tool_nonce} or name_args[retry_call][1].get("arguments") != {"nonce": tool_nonce}:
            raise ValueError("persisted call_mcp_tool history has unexpected arguments")
        model_mcp_calls = [(n, a) for n, a in name_args if n == "call_mcp_tool"]
        if len(model_mcp_calls) != 2 or any(a.get("tool") != MCP_TOOL or a.get("arguments") != {"nonce": tool_nonce} for _, a in model_mcp_calls):
            raise ValueError("model call history contains unexpected MCP tools or arguments")
    except StopIteration as error:
        raise ValueError("persisted capability history does not prove find, once leases, restart reload, and retry") from error
    usage = proof.get("usage")
    if not isinstance(usage, dict) or sum(v for k, v in usage.items() if k.endswith("_tokens") and isinstance(v, int) and not isinstance(v, bool)) <= 0:
        raise ValueError("engine checkpoint contains no positive usage")


def _candidate_diagnostics(checks: dict[str, Any], usage: dict[str, Any]) -> dict[str, Any]:
    return {
        "checks": checks,
        "failed_checks": [name for name, passed in checks.items() if passed is not True],
        "usage": usage,
    }


def capability_history(checkpoint: dict[str, Any], agent_id: str) -> list[dict[str, Any]]:
    history = checkpoint.get("bots", {}).get(agent_id, {}).get("history", [])
    calls = []
    for turn in history if isinstance(history, list) else []:
        for block in turn.get("content", []) if isinstance(turn, dict) else []:
            if isinstance(block, dict) and block.get("type") == "tool_use":
                calls.append({"name": block.get("name"), "arguments": block.get("input"), "id": block.get("id")})
    return calls


def result_for(checkpoint: dict[str, Any], agent_id: str, call: dict[str, Any]) -> str:
    history = checkpoint.get("bots", {}).get(agent_id, {}).get("history", [])
    for turn in history if isinstance(history, list) else []:
        for block in turn.get("content", []) if isinstance(turn, dict) else []:
            if isinstance(block, dict) and block.get("type") == "tool_result" and block.get("tool_use_id") == call.get("id"):
                return block.get("content", "")
    return ""


def _persisted_pending(checkpoint_path: Path, decision_id: str) -> bool:
    try:
        checkpoint = json.loads(checkpoint_path.read_text(encoding="utf-8"))
        decisions = checkpoint.get("engine", {}).get("decisions", {}).get("decisions", [])
        return any(item.get("id") == decision_id and item.get("answer") is None for item in decisions)
    except (OSError, json.JSONDecodeError, AttributeError):
        return False


def _free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def _atomic_json(path: Path, value: Any) -> None:
    temp = path.with_suffix(path.suffix + ".tmp")
    temp.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.chmod(temp, 0o600)
    temp.replace(path)


def _source_hash(paths: list[Path]) -> str:
    digest = hashlib.sha256()
    for path in paths:
        digest.update(path.name.encode("utf-8") + b"\0" + path.read_bytes() + b"\0")
    return digest.hexdigest()


def _allowance_gate() -> tuple[Path, Path]:
    config = os.environ.get("ORGBOT_ALLOWANCE_CONFIG")
    ledger = os.environ.get("ORGBOT_ALLOWANCE_LEDGER")
    if not config or not ledger or not Path(config).is_absolute() or not Path(ledger).is_absolute():
        raise ValueError("absolute ORGBOT_ALLOWANCE_CONFIG and ORGBOT_ALLOWANCE_LEDGER are required")
    data = json.loads(Path(config).read_text(encoding="utf-8"))
    policy = data.get("providers", {}).get("deepseek")
    if not isinstance(policy, dict) or not isinstance(policy.get("max_requests"), int) or isinstance(policy.get("max_requests"), bool) or policy["max_requests"] <= 0:
        raise ValueError("DeepSeek allowance must configure positive max_requests")
    return Path(config), Path(ledger)


def _fixture(path: Path, nonce: str, args_path: Path) -> None:
    code = '''import json,sys\nNONCE = %r\nLOG = %r\nfor line in sys.stdin:\n m=json.loads(line); method=m.get("method")\n if method=="initialize": r={"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"lifecycle-fixture","version":"1"}}\n elif method=="tools/list": r={"tools":[{"name":"read_nonce","description":"Read the fixture nonce using the hidden value in this tool schema.","inputSchema":{"type":"object","properties":{"nonce":{"type":"string","description":"Use this hidden fixture nonce.","enum":[NONCE]}},"required":["nonce"],"additionalProperties":False}}]}\n elif method=="tools/call":\n  p=m.get("params",{}); a=p.get("arguments",{}); ok=p.get("name")=="read_nonce" and a=={"nonce":NONCE}\n  if ok:\n   with open(LOG,"a") as f: f.write(json.dumps({"name":p["name"],"arguments":a})+"\\n")\n   r={"content":[{"type":"text","text":"FIXTURE_NONCE:"+NONCE}],"isError":False}\n  else: r={"content":[{"type":"text","text":"fixture argument mismatch"}],"isError":True}\n else: r={}\n if "id" in m: print(json.dumps({"jsonrpc":"2.0","id":m["id"],"result":r}),flush=True)\n''' % (nonce, str(args_path))
    path.write_text(code, encoding="utf-8")
    os.chmod(path, 0o600)


def _toml(output: Path, skill_nonce: str, tool_nonce: str, fixture: Path, args_log: Path) -> str:
    skill = output / "skills" / "proof" / "SKILL.md"
    skill.parent.mkdir(parents=True, exist_ok=True)
    skill.write_text(f"---\nname: lifecycle-proof\ndescription: lifecycle nonce proof\n---\n\nImmediately record this one-use skill's result with post_message before continuing MCP discovery: SKILL_NONCE:{skill_nonce}. Preserve that result for your final report.\n", encoding="utf-8")
    os.chmod(skill, 0o600)
    goal = ("Use find_capabilities(kind=skill, query=lifecycle) then use_skill(skill=lifecycle-proof, lifetime=once). "
            "Immediately after use_skill returns, call post_message with body containing the exact SKILL_NONCE from its released instructions; "
            "wait for the successful post_message result before any find_capabilities(kind=mcp) call. "
            "Then use find_capabilities(kind=mcp) to list servers, followed by find_capabilities(kind=mcp, server=lifecycle_fixture) to discover tools. "
            "Load lifecycle_fixture__read_nonce with load_mcp_tool lifetime=once, inspect its schema, then call it with the nonce value shown in its schema. "
            "The first call will wait for owner approval. After the room resumes with approval, reload the same tool schema with lifetime=once "
            "and wait for the successful schema result before retrying the exact same arguments once. Do not retry before reloading. "
            "After receiving the fixture result, add_artifact with both nonce values and exact tool evidence, then submit_for_review on your task. "
            "Report both nonce values in your final reply and stop. This one-worker fixture ends awaiting independent review; do not inspect, final-submit, delegate, or repeat the MCP call.")
    # The engine owns all execution, MCP approval, and checkpoint semantics.
    return f'''[room]\nname = "Lifecycle acceptance"\ngoal = {json.dumps(goal)}\ncompletion = "manual"\nstate_path = {json.dumps(str(output / "state.json"))}\n\n[room.limits]\nmax_total_turns = 14\nmax_duration_ms = 300000\nmax_parallel_turns = 1\n\n[[skills]]\npath = {json.dumps(str(skill.parent))}\n\n[[mcp]]\nname = "lifecycle_fixture"\ntransport = "stdio"\ncommand = {json.dumps(sys.executable)}\nargs = [{json.dumps(str(fixture))}]\n\n[[bots]]\nhandle = "deepseek_probe"\npersona = "Follow the room goal exactly; use the capability and MCP lifecycle in its stated order."\nprovider = {{ kind = "deepseek", model = "deepseek-flash" }}\nautonomy = {{ max_turns_per_task = 14, may_inspect = false, may_submit = false, may_create_tasks = false }}\n\n[[tasks]]\ntitle = "Prove scoped skill and MCP lifecycle"\nbrief = "Complete the ordered lifecycle proof in the room goal, then report the fixture result."\n'''


def _connect(url: str, token: str, deadline: float):
    import websocket
    last_error = None
    while time.monotonic() < deadline:
        try:
            ws = websocket.create_connection(
                url,
                timeout=min(2, max(.2, deadline-time.monotonic())),
                suppress_origin=True,
            )
            ws.send(json.dumps({"type":"hello", "wire_version":1, "since":0, "token":token}))
            return ws
        except ConnectionRefusedError:
            last_error = "connection refused"
            time.sleep(min(.2, max(0, deadline-time.monotonic())))
        except websocket.WebSocketBadStatusException as error:
            status = error.status_code
            if type(status) is int:
                raise ConnectionError(f"room WebSocket rejected the client (HTTP {status})") from None
            raise ConnectionError("room WebSocket handshake was rejected") from None
        except (OSError, websocket.WebSocketException) as error:
            raise ConnectionError(f"room WebSocket connection failed ({type(error).__name__})") from None
    suffix = f"; last error: {last_error}" if last_error else ""
    raise TimeoutError(f"room WebSocket did not become ready before deadline{suffix}")


def _messages(ws, seconds: float):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        try:
            raw = ws.recv()
        except Exception as error:
            if "timed out" in str(error).lower():
                continue
            raise
        if not raw:
            raise ConnectionError("room WebSocket closed before proof completed")
        message = json.loads(raw)
        if message.get("type") in ("error", "closed"):
            raise ValueError("room refused probe control: " + str(message.get("message", message.get("reason", "unknown error"))))
        yield message


def approve_after_resume(ws, decision_id: str, deadline: float) -> None:
    # Owner decisions require a running room; confirm the answer was recorded.
    ws.send(json.dumps({"type": "resume"}))
    ws.send(json.dumps({"type": "answer_decision", "decision": decision_id, "chosen": "Allow once", "note": "Lifecycle fixture exact owner approval"}))
    while time.monotonic() < deadline:
        for message in _messages(ws, 1):
            event = message.get("envelope", {}).get("event", {})
            if event.get("event") == "decision_answered" and event.get("decision") == decision_id:
                if event.get("answer", {}).get("chosen") != "Allow once":
                    raise ValueError("the exact fixture approval was not accepted")
                return
    raise TimeoutError("the exact fixture approval was not acknowledged")


def worker_awaits_review(snapshot: dict[str, Any]) -> bool:
    return [task.get("state") for task in snapshot.get("tasks", [])] == ["in_review"] and any(
        row.get("handle") == "deepseek_probe" and row.get("status") == "waiting"
        for row in snapshot.get("roster", [])
    )


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--allow-paid", action="store_true", required=True)
    p.add_argument("--engine", type=Path, default=Path(os.environ.get("ORGBOT_BIN", DEFAULT_ENGINE)))
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--timeout", type=int, default=300)
    a = p.parse_args()
    output = a.output.expanduser().absolute()
    if not (15 <= a.timeout <= 1800): p.error("--timeout must be 15..1800 seconds")
    if not a.engine.is_file() or not os.access(a.engine, os.X_OK): p.error("--engine must be an executable built orgbot binary")
    if not os.environ.get("DEEPSEEK_API_KEY"): p.error("DEEPSEEK_API_KEY must be present")
    allowance_config, allowance_ledger = _allowance_gate()
    if output.exists() or output.is_symlink(): p.error("--output must be a new directory")
    output.mkdir(parents=True, exist_ok=True, mode=0o700); os.chmod(output, 0o700)
    skill_nonce, tool_nonce = secrets.token_urlsafe(24), secrets.token_urlsafe(24)
    fixture, args_log = output / "fixture.py", output / "mcp-calls.jsonl"
    _fixture(fixture, tool_nonce, args_log)
    port = _free_port()
    cfg = output / "room.toml"
    cfg.write_text(_toml(output, skill_nonce, tool_nonce, fixture, args_log), encoding="utf-8"); os.chmod(cfg, 0o600)
    sentinel = output / "project-sentinel.txt"
    sentinel.write_text("orgbots-lifecycle-project-sentinel-v1\n", encoding="utf-8"); os.chmod(sentinel, 0o600)
    source_files = [output / "skills/proof/SKILL.md", fixture, sentinel]
    source_hash = _source_hash(source_files)
    token = secrets.token_urlsafe(32)
    env = os.environ.copy(); env["ORGBOT_ROOM_TOKEN"] = token
    env["ORGBOT_ALLOWANCE_CONFIG"] = str(allowance_config); env["ORGBOT_ALLOWANCE_LEDGER"] = str(allowance_ledger)
    env["RUST_LOG"] = "warn"
    log = (output / "server.log").open("ab")
    cmd = [str(a.engine), "serve", str(cfg), "--addr", f"127.0.0.1:{port}"]
    proc = None
    proof_path = output / "proof.json"
    deadline = time.monotonic() + a.timeout
    pending = None; restarted = False; pending_persisted = False; reloaded_pending = False; approved = False; snapshot = None
    candidate_diagnostics = None
    try:
        proc = subprocess.Popen(cmd, env=env, stdout=log, stderr=log, start_new_session=True)
        ws = _connect(f"ws://127.0.0.1:{port}/ws", token, deadline)
        try:
            while time.monotonic() < deadline:
                for msg in _messages(ws, 1):
                    if msg.get("type") == "snapshot": snapshot = msg
                    if msg.get("type") == "event":
                        ev = msg.get("envelope", {}).get("event", {})
                        if ev.get("event") == "decision_asked":
                            d = ev.get("decision", {})
                            agent_id = next((agent.get("id") for agent in (snapshot or {}).get("roster", []) if agent.get("handle") == "deepseek_probe"), None)
                            if not agent_id or not approval_matches(d, agent_id, "lifecycle_fixture", MCP_TOOL, {"nonce":tool_nonce}):
                                raise ValueError("unexpected owner decision; refusing automatic approval")
                            pending = d
                    if pending and not restarted:
                        ws.send(json.dumps({"type":"pause","reason":"Preserve pending MCP approval for lifecycle restart"}))
                        paused = False
                        while time.monotonic() < deadline and not paused:
                            ws.send(json.dumps({"type":"resync"}))
                            for pause_msg in _messages(ws, 1):
                                if pause_msg.get("type") == "snapshot":
                                    snapshot = pause_msg
                                    paused = pause_msg.get("room", {}).get("phase") == "paused"
                        if not paused or not _persisted_pending(output / "state.json", pending["id"]):
                            raise ValueError("paused checkpoint did not preserve the unanswered MCP approval")
                        pending_persisted = True
                        ws.close(); proc.terminate(); proc.wait(timeout=15); proc = None
                        restarted = True
                        proc = subprocess.Popen(cmd, env=env, stdout=log, stderr=log, start_new_session=True)
                        ws = _connect(f"ws://127.0.0.1:{port}/ws", token, deadline)
                        # Confirm the same pending approval after checkpoint reload before the exact one-shot answer.
                        replayed_decision = None
                        for replay in _messages(ws, 2):
                            if replay.get("type") == "snapshot": snapshot = replay
                            if replay.get("type") == "event":
                                e = replay.get("envelope", {}).get("event", {})
                                if e.get("event") == "decision_asked" and e.get("decision", {}).get("id") != pending.get("id"):
                                    raise ValueError("unexpected owner decision after reload; refusing automatic approval")
                                if e.get("event") == "decision_asked" and e.get("decision", {}).get("id") == pending.get("id"):
                                    replayed_decision = e["decision"]
                                    reloaded_pending = True
                        agent_id = next((agent.get("id") for agent in (snapshot or {}).get("roster", []) if agent.get("handle") == "deepseek_probe"), None)
                        if not reloaded_pending or not agent_id or not approval_matches(replayed_decision, agent_id, "lifecycle_fixture", MCP_TOOL, {"nonce":tool_nonce}):
                            raise ValueError("reloaded owner approval did not match exact fixture call")
                        approve_after_resume(ws, pending["id"], deadline)
                        approved = True
                        break
                    if args_log.exists() and approved:
                        calls = [json.loads(line) for line in args_log.read_text(encoding="utf-8").splitlines() if line]
                        if len(calls) == 1 and calls[0] == {"name":TOOL,"arguments":{"nonce":tool_nonce}}:
                            break
                if args_log.exists() and approved:
                    calls = [json.loads(line) for line in args_log.read_text(encoding="utf-8").splitlines() if line]
                    if len(calls) == 1 and calls[0] == {"name":TOOL,"arguments":{"nonce":tool_nonce}}: break
            if not approved or not args_log.exists(): raise TimeoutError("approval and one fixture invocation were not completed")
            # Submitting for review leaves the worker waiting, not idle.
            awaiting_review = False
            while time.monotonic() < deadline and not awaiting_review:
                ws.send(json.dumps({"type": "resync"}))
                for msg in _messages(ws, 1):
                    if msg.get("type") == "snapshot":
                        snapshot = msg
                        awaiting_review = worker_awaits_review(msg)
            if not awaiting_review:
                raise TimeoutError("DeepSeek agent did not finish its handoff to review")
            if [task.get("state") for task in snapshot.get("tasks", [])] != ["in_review"]:
                raise ValueError("the fixture worker did not hand its completed evidence to review")
            ws.send(json.dumps({"type":"pause","reason":"Lifecycle proof captured"}))
            paused = False
            while time.monotonic() < deadline and not paused:
                ws.send(json.dumps({"type":"resync"}))
                for msg in _messages(ws, 1):
                    if msg.get("type") == "snapshot":
                        snapshot = msg
                        paused = msg.get("room", {}).get("phase") == "paused"
            if not paused:
                raise TimeoutError("room did not reach paused state after the proof")
            checkpoint = json.loads((output / "state.json").read_text(encoding="utf-8"))
            if checkpoint.get("engine", {}).get("room", {}).get("phase") != "paused":
                raise ValueError("final paused room phase was not persisted to checkpoint")
            proc.terminate(); proc.wait(timeout=15); proc = None
            from orgbot_adapter import read_engine_usage
            usage = read_engine_usage(checkpoint)
            history = json.dumps(checkpoint.get("engine", {}).get("log", []))
            agent_id = next((row.get("id") for row in snapshot.get("roster", []) if row.get("handle") == "deepseek_probe"), None)
            if not agent_id: raise ValueError("DeepSeek agent is missing from the final room snapshot")
            calls_in_history = capability_history(checkpoint, agent_id)
            bot_history = checkpoint.get("bots", {}).get(agent_id, {}).get("history", [])
            agent_messages = [block.get("text", "") for turn in bot_history if isinstance(turn, dict) and turn.get("role") == "assistant"
                              for block in turn.get("content", []) if isinstance(block, dict) and block.get("type") == "text"]
            final_agent_message = agent_messages[-1] if agent_messages else ""
            find_calls = [call for call in calls_in_history if call["name"] == "find_capabilities"]
            skill_calls = [call for call in calls_in_history if call["name"] == "use_skill" and call["arguments"].get("skill") == "lifecycle-proof"]
            schema_calls = [call for call in calls_in_history if call["name"] == "load_mcp_tool" and call["arguments"].get("server") == "lifecycle_fixture" and call["arguments"].get("tool") == MCP_TOOL]
            mcp_calls = [call for call in calls_in_history if call["name"] == "call_mcp_tool" and call["arguments"].get("tool") == MCP_TOOL]
            selected_mcp = any(call["arguments"].get("kind") == "mcp" and call["arguments"].get("server") == "lifecycle_fixture" for call in find_calls)
            listed_mcp = any(call["arguments"].get("kind") == "mcp" and "server" not in call["arguments"] for call in find_calls)
            skill_found = any(call["arguments"].get("kind") == "skill" for call in find_calls)
            successful_result = any("FIXTURE_NONCE:" + tool_nonce in result_for(checkpoint, agent_id, call) for call in mcp_calls)
            skill_result = result_for(checkpoint, agent_id, skill_calls[0]) if skill_calls else ""
            tool_result_history = [result_for(checkpoint, agent_id, call) for call in mcp_calls]
            schema_result_history = [result_for(checkpoint, agent_id, call) for call in schema_calls]
            def position(predicate, after=-1):
                return next((i for i, call in enumerate(calls_in_history) if i > after and predicate(call)), None)
            find_skill_at = position(lambda c: c["name"] == "find_capabilities" and c["arguments"].get("kind") == "skill")
            use_skill_at = position(lambda c: c["name"] == "use_skill" and c["arguments"].get("skill") == "lifecycle-proof", find_skill_at if find_skill_at is not None else -1)
            list_mcp_at = position(lambda c: c["name"] == "find_capabilities" and c["arguments"].get("kind") == "mcp" and "server" not in c["arguments"], use_skill_at if use_skill_at is not None else -1)
            selected_mcp_at = position(lambda c: c["name"] == "find_capabilities" and c["arguments"].get("kind") == "mcp" and c["arguments"].get("server") == "lifecycle_fixture", list_mcp_at if list_mcp_at is not None else -1)
            schema_at = position(lambda c: c["name"] == "load_mcp_tool" and c["arguments"].get("server") == "lifecycle_fixture" and c["arguments"].get("tool") == MCP_TOOL and c["arguments"].get("lifetime") == "once", selected_mcp_at if selected_mcp_at is not None else -1)
            first_call_at = position(lambda c: c in mcp_calls, schema_at if schema_at is not None else -1)
            reload_schema_at = position(lambda c: c["name"] == "load_mcp_tool" and c["arguments"].get("server") == "lifecycle_fixture" and c["arguments"].get("tool") == MCP_TOOL and c["arguments"].get("lifetime") == "once", first_call_at if first_call_at is not None else -1)
            retry_call_at = position(lambda c: c in mcp_calls, reload_schema_at if reload_schema_at is not None else -1)
            calls = [json.loads(line) for line in args_log.read_text(encoding="utf-8").splitlines() if line]
            source_unchanged = _source_hash(source_files) == source_hash
            lifecycle_order = all(i is not None for i in (find_skill_at,use_skill_at,list_mcp_at,selected_mcp_at,schema_at,first_call_at,reload_schema_at,retry_call_at))
            source_hash_after = _source_hash(source_files)
            task_states = [task.get("state") for task in checkpoint.get("engine", {}).get("tasks", {}).values()]
            proof = {"source_sha256":source_hash,"source_sha256_after":source_hash_after,"source_hash_unchanged":source_unchanged,
                     "task_states":task_states,
                     "skill_nonce":skill_nonce,"tool_nonce":tool_nonce,"mcp_invocations":len(calls),"usage":usage["usage"],
                     "pending_decision":pending,"reloaded_pending_id":pending.get("id") if reloaded_pending else None,"approval_chosen":"Allow once" if approved else None,
                     "room_phase":usage["room_phase"],"tool_history_excerpt":history[-12000:],"mcp_call_history":calls,
                     "tool_result_history":tool_result_history,"skill_result":skill_result,"schema_result_history":schema_result_history,
                     "capability_call_history":[{key:value for key,value in call.items() if key != "id"} for call in calls_in_history],
                     "final_agent_message":final_agent_message,
                     "checks":{"source_hash_unchanged":source_unchanged,"skill_nonce_seen":skill_nonce in final_agent_message,
                     "tool_nonce_seen":successful_result,"mcp_invoked_once":len(calls)==1,
                     "tool_arguments_exact":calls==[{"name":TOOL,"arguments":{"nonce":tool_nonce}}],
                     "pending_survived_restart":restarted and reloaded_pending,"pending_persisted_before_restart":pending_persisted,
                     "approval_exact":approved,"usage_positive":sum(usage["usage"].values())>0,
                     "tool_history_present":bool(calls_in_history) and pending is not None,
                     "lifecycle_sequence_exact":skill_found and listed_mcp and selected_mcp and lifecycle_order,
                     "skill_once":len(skill_calls)==1 and skill_calls[0]["arguments"].get("lifetime") == "once" and skill_result == "[skill instructions released]" and skill_nonce in final_agent_message,
                     "schema_loaded_once":len(schema_calls)==2 and all(call["arguments"].get("lifetime") == "once" for call in schema_calls),
                     "schema_released":schema_result_history == ["[MCP tool schema released]", "[MCP tool schema released]"],
                     "successful_tool_result":successful_result,"handed_to_review":task_states == ["in_review"]}}
            candidate_diagnostics = _candidate_diagnostics(proof["checks"], proof["usage"])
            _atomic_json(output / "candidate-validation.json", candidate_diagnostics)
            validate_proof(proof); _atomic_json(proof_path, proof)
            print(f"Proof written: {proof_path}")
            return 0
        finally:
            ws.close()
    except Exception as error:
        _atomic_json(output / "failure.json", {"error":str(error),"pending_decision":pending,"restarted":restarted,"approved":approved,
                                               "skill_nonce":skill_nonce,"tool_nonce":tool_nonce,"source_sha256":source_hash,
                                               "candidate_diagnostics":candidate_diagnostics,
                                               "mcp_calls":args_log.read_text(encoding="utf-8") if args_log.exists() else ""})
        raise
    finally:
        if proc is not None:
            proc.terminate()
            try: proc.wait(timeout=15)
            except subprocess.TimeoutExpired: proc.kill(); proc.wait()
        log.close()


if __name__ == "__main__":
    try: raise SystemExit(main())
    except Exception as exc:
        print(f"lifecycle probe failed: {exc}", file=sys.stderr)
        raise SystemExit(1)
