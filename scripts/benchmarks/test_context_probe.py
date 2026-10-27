#!/usr/bin/env python3
import sys
import json
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).parent))
import context_probe


IDS = {"deepseek": "agent_ds", "opencode": "agent_oc", "astra": "agent_as"}


def chat(author, body, target):
    if target == "everyone":
        audience = {"kind": "everyone"}
    else:
        audience = {"kind": "agents", "agents": [IDS[target]]}
    return {"author": {"kind": "agent", "agent": IDS[author]}, "body": body, "audience": audience}


def usage_row(handle, *, calls=1, reported=True, complete=True, tokens=5):
    return {"handle": handle, "calls": calls, "usage_reported": reported,
            "usage_complete": complete,
            "usage": {"input_tokens": tokens, "output_tokens": tokens, "cache_read_tokens": 0, "cache_write_tokens": 0}}


def owner_message(body, target):
    return {"author": {"kind": "human", "name": "Owner"}, "body": body,
            "audience": {"kind": "agents", "agents": [IDS[target]]}}


class ContextProbeTests(unittest.TestCase):
    def test_owner_handle_parser_exposes_multiple_mentions_like_engine(self):
        self.assertEqual(context_probe.owner_handle_mentions("@deepseek seed for @astra"), ["deepseek", "astra"])
        self.assertEqual(context_probe.owner_handle_mentions("@deepseek seed for bot named astra"), ["deepseek"])
        self.assertEqual(context_probe.owner_handle_mentions("@opencode recall for bot named astra"), ["opencode"])

    def test_usage_event_replay_deduplicates_by_sequence(self):
        room = object.__new__(context_probe._Room)
        room.seen_event_seqs = set()
        room.seen_usage_counts = {}
        old = {"type": "event", "envelope": {"seq": 7, "event": {"event": "usage_recorded", "agent": "a"}}}
        current = {"type": "event", "envelope": {"seq": 8, "event": {"event": "usage_recorded", "agent": "a"}}}
        room._observe({"type": "snapshot"})
        room._observe(old)
        room._observe(old)
        room._observe(current)
        self.assertEqual(room.seen_usage_counts, {"a": 2})
        self.assertEqual(room.seen_event_seqs, {7, 8})

    def test_wait_accepts_cli_newline_but_isolated_validator_rejects_suffixes(self):
        self.assertTrue(context_probe.body_matches_wait_target("UNKNOWN\n", "UNKNOWN"))
        self.assertFalse(context_probe.body_matches_wait_target("UNKNOWN extra", "UNKNOWN"))
        checkpoint = {"engine": {"log": []}}
        context_probe.validate_isolated_room(checkpoint, "nonce", [chat("opencode", "UNKNOWN\n", "everyone")], IDS["opencode"])
        with self.assertRaisesRegex(ValueError, "unknown prior context"):
            context_probe.validate_isolated_room(checkpoint, "nonce", [chat("opencode", "UNKNOWN secret", "everyone")], IDS["opencode"])

    def test_primary_message_validator_remains_exact_after_wait_normalization(self):
        nonce = "marker"
        steps = [("deepseek", f"SEED:{nonce}", "opencode")]
        with self.assertRaisesRegex(ValueError, "body"):
            context_probe.validate_message_sequence([chat("deepseek", f"SEED:{nonce}\n", "opencode")], steps, IDS)

    def test_restart_baseline_persists_usage_counts_and_only_session_hash(self):
        checkpoint = {"engine": {"log": [
            {"seq": 3, "event": {"event": "usage_recorded", "agent": "agent_astra"}},
        ]}}
        usage = {"agent_usage": [{**usage_row("astra"), "provider": "cli_agent", "cli": "codex", "model": "gpt-6-astra"}]}
        evidence = context_probe.restart_baseline_evidence(
            checkpoint, usage, ["astra"], {"astra": "agent_astra"}, {"astra": "private-session-hash"},
        )
        self.assertEqual(evidence["usage_event_counts"], {"astra": 1})
        self.assertEqual(evidence["native_session_sha256"], {"astra": "private-session-hash"})
        self.assertEqual(evidence["usage"]["astra"]["model"], "gpt-6-astra")
        self.assertNotIn("session_id", json.dumps(evidence))

    def test_external_interrupt_handler_raises_for_cleanup_path(self):
        with self.assertRaises(InterruptedError):
            context_probe._handle_external_interrupt(15, None)

    def test_snapshot_settled_requires_every_participant_to_be_idle_or_waiting(self):
        self.assertTrue(context_probe.participants_settled({"roster": [
            {"handle": "deepseek", "status": "idle"}, {"handle": "astra", "status": "waiting"},
        ]}, ["deepseek", "astra"]))
        self.assertFalse(context_probe.participants_settled({"roster": [
            {"handle": "deepseek", "status": "idle"}, {"handle": "astra", "status": "working"},
        ]}, ["deepseek", "astra"]))
        with self.assertRaisesRegex(ValueError, "unavailable"):
            context_probe.participants_settled({"roster": [
                {"handle": "deepseek", "status": "faulted"},
            ]}, ["deepseek"])

    def test_expected_message_orchestration_handles_both_roster_sizes(self):
        class RoomStub:
            def __init__(self):
                self.waited = []

            def wait_body(self, body):
                self.waited.append(body)

        nonce = "fixture"
        for steps in (
            [("deepseek", f"SEED:{nonce}", "opencode"),
             ("opencode", f"ACK:{nonce}", "astra"),
             ("astra", f"VERIFIED:{nonce}", "everyone")],
            [("deepseek", f"SEED:{nonce}", "astra"),
             ("astra", f"ACK:{nonce}", "everyone")],
        ):
            room = RoomStub()
            context_probe.wait_expected_messages(room, steps)
            self.assertEqual(room.waited, [step[1] for step in steps])

    def test_owner_mentions_are_validated_as_directed_agent_triggers(self):
        steps = [("@deepseek use skill", "deepseek"), ("@opencode resume", "opencode")]
        messages = [owner_message(body, target) for body, target in steps]
        context_probe.validate_owner_targets(messages, steps, IDS)
        with self.assertRaisesRegex(ValueError, "@handle"):
            context_probe.validate_owner_targets([owner_message(steps[0][0], "astra"), messages[1]], steps, IDS)
        with self.assertRaisesRegex(ValueError, "body"):
            context_probe.validate_owner_targets([owner_message("wrong", "deepseek"), messages[1]], steps, IDS)

    def test_live_model_defaults_match_the_approved_routes(self):
        self.assertEqual(context_probe.DEFAULT_DEEPSEEK_MODEL, "deepseek-flash")
        self.assertEqual(context_probe.DEFAULT_OPENCODE_MODEL, "opencode/deepseek-v4.1-flash-free")
        self.assertEqual(context_probe.DEFAULT_ASTRA_MODEL, "gpt-6-astra")
        self.assertEqual(context_probe.MAX_DEEPSEEK_REQUESTS, 80)

    def test_clean_environment_restores_private_relay_and_bounds_cli_turns(self):
        with patch.dict("os.environ", {
            "DEEPSEEK_API_KEY": "synthetic-key",
            "OPENAI_API_KEY": "unrelated-key",
            "ORGBOT_MCP_RELAY_BIN": "/host/old-orgbot",
            "ORGBOT_CLI_TURN_TIMEOUT_SECS": "9999",
        }, clear=True):
            env = context_probe._clean_env("room-token", Path("/private/allowance.json"), Path("/private/ledger.json"), Path("/private/orgbot"))
        self.assertEqual(env["DEEPSEEK_API_KEY"], "synthetic-key")
        self.assertNotIn("OPENAI_API_KEY", env)
        self.assertEqual(env["ORGBOT_MCP_RELAY_BIN"], "/private/orgbot")
        self.assertEqual(env["ORGBOT_CLI_TURN_TIMEOUT_SECS"], "240")

    def test_failure_reason_redacts_nonce_room_token_and_api_key(self):
        with patch.dict("os.environ", {"DEEPSEEK_API_KEY": "synthetic-api-key"}, clear=True):
            reason = context_probe._safe_failure_reason(
                RuntimeError("nonce=synthetic-nonce token=synthetic-room-token key=synthetic-api-key"),
                "synthetic-nonce", "synthetic-room-token",
            )
        self.assertNotIn("synthetic-nonce", reason)
        self.assertNotIn("synthetic-room-token", reason)
        self.assertNotIn("synthetic-api-key", reason)

    def test_isolated_persona_does_not_assume_missing_context(self):
        with tempfile.TemporaryDirectory(prefix="orgbot-context-probe-test-") as temp:
            config = tomllib.loads(context_probe._room_config(Path(temp) / "isolated", None, "deepseek-flash", "opencode/model", "gpt-6-astra", False))
        persona = config["bots"][0]["persona"]
        self.assertIn("If the requested seed is absent from the current room context", persona)
        self.assertNotIn("No prior marker is available", persona)

    def test_message_sequence_checks_order_author_marker_and_exact_recipient(self):
        nonce = "synthetic-marker"
        steps = [("deepseek", f"SEED:{nonce}", "opencode"),
                 ("opencode", f"ACK:{nonce}", "astra"),
                 ("astra", f"VERIFIED:{nonce}", "everyone")]
        messages = [chat(author, body, target) for author, body, target in steps]
        context_probe.validate_message_sequence(messages, steps, IDS)
        for invalid in (
            [messages[1], messages[0], messages[2]],
            [chat("astra", messages[0]["body"], "opencode"), *messages[1:]],
            [chat("deepseek", messages[0]["body"], "astra"), *messages[1:]],
            [chat("deepseek", "SEED:wrong", "opencode"), *messages[1:]],
        ):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                context_probe.validate_message_sequence(invalid, steps, IDS)

    def test_usage_rejects_missing_incomplete_and_zero_reports(self):
        good = {"agent_usage": [usage_row(handle) for handle in ("deepseek", "opencode", "astra")]}
        context_probe.validate_usage(good, ["deepseek", "opencode", "astra"])
        handles = ["deepseek", "opencode", "astra"]
        variants = [
            {"agent_usage": [usage_row("opencode"), usage_row("astra")]},
            {"agent_usage": [usage_row("deepseek", reported=False), usage_row("opencode"), usage_row("astra")]},
            {"agent_usage": [usage_row("deepseek", complete=False), usage_row("opencode"), usage_row("astra")]},
            {"agent_usage": [usage_row("deepseek", calls=0), usage_row("opencode"), usage_row("astra")]},
            {"agent_usage": [usage_row("deepseek", tokens=0), usage_row("opencode"), usage_row("astra")]},
        ]
        for value in variants:
            with self.subTest(value=value), self.assertRaises(ValueError):
                context_probe.validate_usage(value, handles)

    def test_resume_usage_must_grow_and_never_regress(self):
        before = {"agent_usage": [usage_row("opencode"), usage_row("astra")]}
        after = {"agent_usage": [usage_row("opencode", calls=2, tokens=8), usage_row("astra", calls=2, tokens=9)]}
        context_probe.validate_usage_growth(before, after, ["opencode", "astra"])
        for changed in (
            {"agent_usage": [usage_row("opencode"), usage_row("astra", calls=2, tokens=9)]},
            {"agent_usage": [usage_row("opencode", calls=2, tokens=4), usage_row("astra", calls=2, tokens=9)]},
        ):
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                context_probe.validate_usage_growth(before, changed, ["opencode", "astra"])

    def test_isolated_checkpoint_rejects_nonce_and_requires_unknown_from_opencode(self):
        nonce = "not-for-isolated-room"
        checkpoint = {"engine": {"log": []}}
        context_probe.validate_isolated_room(checkpoint, nonce, [chat("opencode", "UNKNOWN", "everyone")], IDS["opencode"])
        for bad_checkpoint, messages, agent_id in (
            ({"text": nonce}, [chat("opencode", "UNKNOWN", "everyone")], IDS["opencode"]),
            (checkpoint, [chat("opencode", "RESUME:" + nonce, "everyone")], IDS["opencode"]),
            (checkpoint, [chat("astra", "UNKNOWN", "everyone")], IDS["opencode"]),
        ):
            with self.subTest(bad_checkpoint=bad_checkpoint, messages=messages), self.assertRaises(ValueError):
                context_probe.validate_isolated_room(bad_checkpoint, nonce, messages, agent_id)

    def test_native_session_evidence_is_stable_hash_not_raw_id(self):
        with tempfile.TemporaryDirectory(prefix="orgbot-context-session-test-") as temp:
            state = Path(temp) / "state.json"
            session = state.parent / "cli-sessions"
            session.mkdir()
            raw_id = "ses-private-fixture"
            record = {"session_id": raw_id}
            (session / "6f70656e636f6465.json").write_text(json.dumps(record), encoding="utf-8")
            first = context_probe._session_fingerprint(state, "opencode")
            second = context_probe._session_fingerprint(state, "opencode")
            self.assertEqual(first, second)
            self.assertNotEqual(first, raw_id)
            self.assertEqual(len(first), 64)

    def test_generated_configs_are_manual_taskless_and_workspace_separated(self):
        with tempfile.TemporaryDirectory(prefix="orgbot-context-probe-test-") as temp:
            root = Path(temp)
            primary = tomllib.loads(context_probe._room_config(root / "primary", root / "skills", "deepseek-chat", "deepseek/model", "gpt-6-astra", True))
            isolated = tomllib.loads(context_probe._room_config(root / "isolated", None, "deepseek-chat", "deepseek/model", "gpt-6-astra", False))
            paid_subset = tomllib.loads(context_probe._room_config(root / "paid-subset", root / "skills", "deepseek-flash", context_probe.DEFAULT_OPENCODE_MODEL, "gpt-6-astra", True, True))
            isolated_astra = tomllib.loads(context_probe._room_config(root / "isolated-astra", None, "deepseek-flash", context_probe.DEFAULT_OPENCODE_MODEL, "gpt-6-astra", False, True))
            self.assertEqual(primary["room"]["completion"], "manual")
            self.assertNotIn("tasks", primary)
            self.assertEqual([bot["handle"] for bot in primary["bots"]], ["deepseek", "opencode", "astra"])
            self.assertTrue(all(bot["provider"].get("workdir", "") != primary["workspace"]["root"] for bot in primary["bots"] if bot["provider"]["kind"] == "cli_agent"))
            self.assertEqual(isolated["bots"][0]["handle"], "opencode")
            self.assertNotEqual(primary["room"]["state_path"], isolated["room"]["state_path"])
            self.assertEqual([bot["handle"] for bot in paid_subset["bots"]], ["deepseek", "astra"])
            self.assertEqual(paid_subset["bots"][0]["provider"]["model"], "deepseek-flash")
            self.assertEqual(paid_subset["bots"][1]["provider"]["cli"], "codex")
            self.assertEqual(isolated_astra["bots"][0]["handle"], "astra")
            self.assertEqual(isolated_astra["bots"][0]["provider"]["model"], "gpt-6-astra")
            self.assertNotEqual(paid_subset["room"]["state_path"], isolated_astra["room"]["state_path"])

    def test_skip_opencode_flow_uses_direct_deepseek_to_astra_sequence(self):
        nonce = "synthetic-marker"
        steps = [("deepseek", f"SEED:{nonce}", "astra"), ("astra", f"ACK:{nonce}", "everyone")]
        context_probe.validate_message_sequence([chat(author, body, target) for author, body, target in steps], steps, IDS)


if __name__ == "__main__":
    unittest.main()
