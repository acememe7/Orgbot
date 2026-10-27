import json
import sys
import time
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).parent))
import lifecycle_probe
import websocket


class LifecycleProbeTests(unittest.TestCase):
    def test_finished_worker_waits_for_review_instead_of_becoming_idle(self):
        snapshot = {"tasks": [{"state": "in_review"}],
                    "roster": [{"handle": "deepseek_probe", "status": "waiting"}]}
        self.assertTrue(lifecycle_probe.worker_awaits_review(snapshot))
        for state in ("in_progress", "submitted"):
            self.assertFalse(lifecycle_probe.worker_awaits_review({**snapshot, "tasks": [{"state": state}]}))
        for status in ("working", "idle"):
            self.assertFalse(lifecycle_probe.worker_awaits_review({**snapshot, "roster": [{"handle": "deepseek_probe", "status": status}]}))

    def test_resume_precedes_approval_and_requires_acknowledgement(self):
        ws = unittest.mock.Mock()
        ws.recv.return_value = json.dumps({"type": "event", "envelope": {"event": {
            "event": "decision_answered", "decision": "decision-1", "answer": {"chosen": "Allow once"}}}})
        lifecycle_probe.approve_after_resume(ws, "decision-1", lifecycle_probe.time.monotonic() + 5)
        self.assertEqual([json.loads(call.args[0])["type"] for call in ws.send.call_args_list], ["resume", "answer_decision"])
        ws.recv.return_value = json.dumps({"type": "error", "message": "answer rejected"})
        with self.assertRaisesRegex(ValueError, "answer rejected"):
            lifecycle_probe.approve_after_resume(ws, "decision-1", lifecycle_probe.time.monotonic() + 5)

    def test_connect_suppresses_origin_and_sends_the_token_only_in_hello(self):
        class FakeSocket:
            def __init__(self):
                self.messages = []

            def send(self, message):
                self.messages.append(message)

        ws = FakeSocket()
        with patch("websocket.create_connection", return_value=ws) as connect:
            result = lifecycle_probe._connect("ws://127.0.0.1:1234/ws", "fixture-token", time.monotonic() + 2)

        self.assertIs(result, ws)
        self.assertTrue(connect.call_args.kwargs["suppress_origin"])
        self.assertEqual(json.loads(ws.messages[0]), {
            "type": "hello", "wire_version": 1, "since": 0, "token": "fixture-token",
        })

    def test_connect_retries_only_connection_refused(self):
        class FakeSocket:
            def send(self, _message):
                pass

        ws = FakeSocket()
        with patch("websocket.create_connection", side_effect=[ConnectionRefusedError(111, "refused"), ws]) as connect, \
             patch("lifecycle_probe.time.sleep"):
            result = lifecycle_probe._connect("ws://127.0.0.1:1234/ws", "fixture-token", time.monotonic() + 2)

        self.assertIs(result, ws)
        self.assertEqual(connect.call_count, 2)

    def test_connect_fails_fast_for_rejected_or_closed_sockets(self):
        for error, message in (
            (websocket.WebSocketBadStatusException("forbidden", 403, "Forbidden"), "HTTP 403"),
            (websocket.WebSocketConnectionClosedException("closed"), "WebSocketConnectionClosedException"),
        ):
            with self.subTest(error=type(error).__name__), \
                 patch("websocket.create_connection", side_effect=error) as connect, \
                 self.assertRaisesRegex(ConnectionError, message):
                lifecycle_probe._connect("ws://127.0.0.1:1234/ws", "fixture-token", time.monotonic() + 2)
            self.assertEqual(connect.call_count, 1)

    def test_approval_matches_only_the_expected_owner_call_and_arguments(self):
        decision = {"owner_question": True, "asked_by": ["agent-id"], "options": ["Allow once", "Deny"],
                    "because": 'External MCP call: agent=agent-id; server=lifecycle_fixture; tool=lifecycle_fixture__read_nonce; config_sha256=x; room_state=y; args={"nonce":"n"}; approval_sequence=0'}
        self.assertTrue(lifecycle_probe.approval_matches(decision, "agent-id", "lifecycle_fixture", "lifecycle_fixture__read_nonce", {"nonce": "n"}))
        self.assertFalse(lifecycle_probe.approval_matches(decision, "other", "lifecycle_fixture", "lifecycle_fixture__read_nonce", {"nonce": "n"}))
        self.assertFalse(lifecycle_probe.approval_matches(decision, "agent-id", "lifecycle_fixture", "lifecycle_fixture__read_nonce", {"nonce": "wrong"}))
        self.assertFalse(lifecycle_probe.approval_matches({**decision, "options": ["Allow once"]}, "agent-id", "lifecycle_fixture", "lifecycle_fixture__read_nonce", {"nonce": "n"}))
        self.assertFalse(lifecycle_probe.approval_matches({**decision, "asked_by": ["another"]}, "agent-id", "lifecycle_fixture", "lifecycle_fixture__read_nonce", {"nonce": "n"}))
        for suffix in ("", "; approval_sequence=1", "; approval_sequence=0; injected"):
            changed = {**decision, "because": decision["because"].removesuffix("; approval_sequence=0") + suffix}
            self.assertFalse(lifecycle_probe.approval_matches(changed, "agent-id", "lifecycle_fixture", "lifecycle_fixture__read_nonce", {"nonce": "n"}))

    def test_generated_goal_requires_skill_record_and_schema_reload_before_retry(self):
        with tempfile.TemporaryDirectory(prefix="orgbot-lifecycle-goal-test-") as temp:
            root = Path(temp)
            raw = lifecycle_probe._toml(root, "skill-marker", "tool-marker", root / "fixture.py", root / "calls.jsonl")
            goal = tomllib.loads(raw)["room"]["goal"]

        self.assertIn("Immediately after use_skill returns, call post_message", goal)
        self.assertIn("wait for the successful post_message result before any find_capabilities(kind=mcp)", goal)
        self.assertIn("After the room resumes with approval, reload the same tool schema", goal)
        self.assertIn("wait for the successful schema result before retrying", goal)

    def test_candidate_diagnostics_name_failed_gates_and_preserve_usage(self):
        usage = {"input_tokens": 120, "output_tokens": 24}
        result = lifecycle_probe._candidate_diagnostics({"skill_nonce_seen": False, "usage_positive": True}, usage)
        self.assertEqual(result["failed_checks"], ["skill_nonce_seen"])
        self.assertEqual(result["usage"], usage)

    def test_proof_validator_rejects_missing_any_gate_and_accepts_complete_proof(self):
        keys = ("source_hash_unchanged", "skill_nonce_seen", "tool_nonce_seen", "mcp_invoked_once",
                "tool_arguments_exact", "pending_survived_restart", "pending_persisted_before_restart", "approval_exact", "usage_positive", "tool_history_present",
                "lifecycle_sequence_exact", "skill_once", "schema_loaded_once", "schema_released", "successful_tool_result", "handed_to_review")
        skill_nonce, tool_nonce = "skill-secret", "tool-secret"
        def tool(name, args):
            return {"name": name, "arguments": args}
        history = [
            tool("find_capabilities", {"kind": "skill", "query": "lifecycle"}),
            tool("use_skill", {"skill": "lifecycle-proof", "lifetime": "once"}),
            tool("find_capabilities", {"kind": "mcp"}),
            tool("find_capabilities", {"kind": "mcp", "server": "lifecycle_fixture"}),
            tool("load_mcp_tool", {"server": "lifecycle_fixture", "tool": "lifecycle_fixture__read_nonce", "lifetime": "once"}),
            tool("call_mcp_tool", {"tool": "lifecycle_fixture__read_nonce", "arguments": {"nonce": tool_nonce}}),
            tool("load_mcp_tool", {"server": "lifecycle_fixture", "tool": "lifecycle_fixture__read_nonce", "lifetime": "once"}),
            tool("call_mcp_tool", {"tool": "lifecycle_fixture__read_nonce", "arguments": {"nonce": tool_nonce}}),
        ]
        proof = {"checks": {key: True for key in keys}, "mcp_invocations": 1, "task_states": ["in_review"],
                 "usage": {"input_tokens": 1, "output_tokens": 1}, "skill_nonce": skill_nonce, "tool_nonce": tool_nonce,
                 "final_agent_message": f"reported {skill_nonce}", "skill_result": "[skill instructions released]",
                 "source_sha256": "abc", "source_sha256_after": "abc", "reloaded_pending_id": "decision-1",
                 "approval_chosen": "Allow once",
                 "pending_decision": {"id": "decision-1", "owner_question": True, "asked_by": ["agent-id"],
                     "options": ["Allow once", "Deny"],
                     "because": 'External MCP call: agent=agent-id; server=lifecycle_fixture; tool=lifecycle_fixture__read_nonce; config_sha256=hash; room_state=state; args={"nonce":"tool-secret"}; approval_sequence=0'},
                 "mcp_call_history": [{"name": "read_nonce", "arguments": {"nonce": tool_nonce}}],
                 "tool_result_history": [f"FIXTURE_NONCE:{tool_nonce}"], "capability_call_history": history}
        proof["schema_result_history"] = ["[MCP tool schema released]", "[MCP tool schema released]"]
        lifecycle_probe.validate_proof(proof)
        for invalid in (
            {**proof, "reloaded_pending_id": "different"},
            {**proof, "source_sha256_after": "changed"},
            {**proof, "final_agent_message": "initial goal contains a nonce"},
            {**proof, "capability_call_history": history[:5]},
            {**proof, "capability_call_history": history[:6] + [history[5], *history[6:]]},
            {**proof, "schema_result_history": []},
            {**proof, "pending_decision": {**proof["pending_decision"], "because": proof["pending_decision"]["because"].replace("tool-secret", "wrong")}},
        ):
            with self.subTest(invalid=invalid.keys()), self.assertRaises(ValueError):
                lifecycle_probe.validate_proof(invalid)
        proof["checks"]["approval_exact"] = False
        with self.assertRaisesRegex(ValueError, "incomplete"):
            lifecycle_probe.validate_proof(proof)


if __name__ == "__main__":
    unittest.main()
