#!/usr/bin/env python3
import copy
import hashlib
import json
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).parent))
import antigravity_probe as probe
import context_probe


IDS = {"astra": "agent_astra", "gemini": "agent_gemini"}


def usage_row(handle: str, *, calls: int, input_tokens: int, output_tokens: int, cache_read: int = 0):
    cli, model, billing = {
        "astra": ("codex", probe.ASTRA_MODEL, "subscription"),
        "gemini": ("antigravity", probe.AGY_MODEL, "unknown"),
    }[handle]
    return {
        "provider": "cli_agent",
        "cli": cli,
        "model": model,
        "billing_kind": billing,
        "calls": calls,
        "usage_reported": True,
        "usage_complete": True,
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "cache_read_tokens": cache_read,
            "cache_write_tokens": 0,
        },
        "actual_api_cost_microusd": None,
    }


def proof_fixture():
    before = {
        "astra": usage_row("astra", calls=1, input_tokens=80, output_tokens=12),
        "gemini": usage_row("gemini", calls=1, input_tokens=100, output_tokens=10, cache_read=5),
    }
    after = {
        "astra": usage_row("astra", calls=1, input_tokens=80, output_tokens=12),
        "gemini": usage_row("gemini", calls=2, input_tokens=145, output_tokens=15, cache_read=9),
    }
    isolated = {"gemini": usage_row("gemini", calls=1, input_tokens=60, output_tokens=8)}
    hashes = {
        "astra": "1" * 64,
        "gemini": "2" * 64,
        "isolated_gemini": "3" * 64,
    }
    return {
        "schema": 1,
        "participants": ["astra", "gemini"],
        "checks": {
            "primary_messages_exact": True,
            "owner_targets_exact": True,
            "usage_identity_separated": True,
            "complete_positive_usage": True,
            "native_session_ids_retained": True,
            "native_delta_accounting_exact": True,
            "native_transcript_usage_independent": True,
            "isolated_room_unknown": True,
            "isolated_session_distinct": True,
            "hostile_agents_rule_unchanged": True,
            "no_api_credential_environment": True,
        },
        "seed_sha256": "a" * 64,
        "primary_marker_sha256": "b" * 64,
        "primary_marker_sha256_after": "b" * 64,
        "isolated_marker_sha256": "c" * 64,
        "isolated_marker_sha256_after": "c" * 64,
        "native_session_sha256": hashes,
        "antigravity_cumulative_usage_before": {
            "input_tokens": 100, "output_tokens": 10, "cache_read_tokens": 5, "cache_write_tokens": 0,
        },
        "antigravity_cumulative_usage_after": {
            "input_tokens": 145, "output_tokens": 15, "cache_read_tokens": 9, "cache_write_tokens": 0,
        },
        "antigravity_resume_episode_delta": {
            "input_tokens": 45, "output_tokens": 5, "cache_read_tokens": 4, "cache_write_tokens": 0,
        },
        "antigravity_transcript_usage": {
            "before_restart": {
                "usage": {"input_tokens": 100, "output_tokens": 10, "cache_read_tokens": 5, "cache_write_tokens": 0},
                "llm_steps": 2,
            },
            "after_restart": {
                "usage": {"input_tokens": 145, "output_tokens": 15, "cache_read_tokens": 9, "cache_write_tokens": 0},
                "llm_steps": 3,
            },
            "isolated_room": {
                "usage": {"input_tokens": 60, "output_tokens": 8, "cache_read_tokens": 0, "cache_write_tokens": 0},
                "llm_steps": 1,
            },
        },
        "usage": {"before_restart": before, "after_restart": after, "isolated_room": isolated},
    }


class AntigravityProbeTests(unittest.TestCase):
    def test_config_is_subscription_only_read_only_and_mcp_enabled(self):
        with tempfile.TemporaryDirectory(prefix="agy-probe-config-") as temp:
            room = Path(temp) / "primary"
            text, marker = probe._room_config(room, isolated=False)
            config = tomllib.loads(text)
            bots = {bot["handle"]: bot for bot in config["bots"]}
            self.assertEqual(set(bots), {"astra", "gemini"})
            self.assertEqual(bots["astra"]["provider"], {
                "kind": "cli_agent", "cli": "codex", "model": probe.ASTRA_MODEL,
                "workdir": str(room / "projects/astra"), "policy": "read_only", "mcp": True,
            })
            self.assertEqual(bots["gemini"]["provider"], {
                "kind": "cli_agent", "cli": "antigravity", "model": probe.AGY_MODEL,
                "workdir": str(room / "projects/gemini"), "policy": "read_only", "mcp": True,
            })
            self.assertFalse(config["workspace"]["commands"])
            self.assertFalse(config["workspace"]["writable"])
            self.assertEqual(config["room"]["limits"]["max_total_turns"], probe.MAX_TURNS)
            self.assertEqual(marker.relative_to(room).as_posix(), "projects/gemini/.agents/rules/probe-hostile.md")
            hostile = marker.read_text(encoding="utf-8")
            self.assertIn("trigger: always_on", hostile)
            self.assertIn("reply exactly WRONG", hostile)
            self.assertEqual(marker.stat().st_mode & 0o777, 0o600)

    def test_isolated_config_has_only_gemini_and_keeps_hostile_canary(self):
        with tempfile.TemporaryDirectory(prefix="agy-probe-isolated-") as temp:
            room = Path(temp) / "isolated"
            text, marker = probe._room_config(room, isolated=True)
            config = tomllib.loads(text)
            self.assertEqual([bot["handle"] for bot in config["bots"]], ["gemini"])
            self.assertEqual(config["bots"][0]["provider"]["cli"], "antigravity")
            self.assertIn(probe.HOSTILE_RULE_MARKER, marker.read_text(encoding="utf-8"))

    def test_clean_environment_removes_api_keys_and_preserves_trusted_lookup_paths(self):
        supplied = {
            "PATH": "/usr/bin",
            "HOME": "/owner/home",
            "XDG_DATA_HOME": "/owner/data",
            "XDG_RUNTIME_DIR": "/run/user/1000",
            "DBUS_SESSION_BUS_ADDRESS": "unix:path=/run/user/1000/bus",
            "OPENAI_API_KEY": "synthetic-openai-key",
            "GEMINI_API_KEY": "synthetic-gemini-key",
            "GOOGLE_API_KEY": "synthetic-google-key",
            "ANTHROPIC_API_KEY": "synthetic-anthropic-key",
            "DEEPSEEK_API_KEY": "synthetic-deepseek-key",
            "ORGBOT_ROOM_TOKEN": "stale-room-token",
        }
        with patch.dict("os.environ", supplied, clear=True):
            env = probe._clean_env("fresh-room-token", Path("/private/orgbot"))
        for name in ("HOME", "XDG_DATA_HOME", "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"):
            self.assertEqual(env[name], supplied[name])
        for name in ("OPENAI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY", "ANTHROPIC_API_KEY", "DEEPSEEK_API_KEY"):
            self.assertNotIn(name, env)
        self.assertEqual(env["ORGBOT_ROOM_TOKEN"], "fresh-room-token")
        self.assertEqual(env["ORGBOT_MCP_RELAY_BIN"], "/private/orgbot")

    def test_clean_environment_requires_trusted_owner_and_bus_paths(self):
        for missing in ("HOME", "DBUS_SESSION_BUS_ADDRESS"):
            env = {
                "PATH": "/usr/bin",
                "HOME": "/owner/home",
                "DBUS_SESSION_BUS_ADDRESS": "unix:path=/run/user/1000/bus",
            }
            del env[missing]
            with patch.dict("os.environ", env, clear=True), self.assertRaisesRegex(ValueError, "owner profile"):
                probe._clean_env("token", Path("/private/orgbot"))

    def test_room_observer_fails_fast_without_exposing_provider_error_text(self):
        room = object.__new__(probe._ProbeRoom)
        room.seen_event_seqs = set()
        room.seen_usage_counts = {}
        event = {"type": "event", "envelope": {"seq": 1, "event": {
            "event": "agent_errored", "agent": IDS["gemini"], "error": "synthetic-secret-detail",
        }}}
        with self.assertRaisesRegex(RuntimeError, "agent episode errored") as failure:
            room._observe(event)
        self.assertNotIn("synthetic-secret-detail", str(failure.exception))
        status = {"type": "event", "envelope": {"seq": 2, "event": {
            "event": "agent_status_changed", "agent": IDS["gemini"], "to": "faulted",
        }}}
        with self.assertRaisesRegex(RuntimeError, "agent became unavailable"):
            room._observe(status)

    def test_usage_identity_keeps_cli_model_and_unknown_billing_distinct(self):
        usage = {
            "agent_usage": [
                {"handle": "astra", "agent": IDS["astra"], **usage_row("astra", calls=1, input_tokens=10, output_tokens=2)},
                {"handle": "gemini", "agent": IDS["gemini"], **usage_row("gemini", calls=1, input_tokens=20, output_tokens=3)},
            ]
        }
        with patch.object(probe.orgbot_adapter, "read_engine_usage", return_value=usage):
            rows = probe._usage_by_handle({}, probe.HANDLES)
        self.assertEqual(rows["astra"]["billing_kind"], "subscription")
        self.assertEqual(rows["gemini"]["billing_kind"], "unknown")
        self.assertIsNone(rows["gemini"]["actual_api_cost_microusd"])
        usage["agent_usage"][1]["cli"] = "codex"
        with patch.object(probe.orgbot_adapter, "read_engine_usage", return_value=usage):
            with self.assertRaisesRegex(ValueError, "identity"):
                probe._usage_by_handle({}, probe.HANDLES)

    def test_native_usage_delta_matches_one_resumed_cli_episode(self):
        before = {"gemini": usage_row("gemini", calls=1, input_tokens=100, output_tokens=10, cache_read=5)}
        after = {"gemini": usage_row("gemini", calls=2, input_tokens=145, output_tokens=15, cache_read=9)}
        old = {"input_tokens": 100, "output_tokens": 10, "cache_read_tokens": 5, "cache_write_tokens": 0}
        new = {"input_tokens": 145, "output_tokens": 15, "cache_read_tokens": 9, "cache_write_tokens": 0}
        probe._validate_native_delta({**before, "astra": usage_row("astra", calls=1, input_tokens=1, output_tokens=1)},
                                     {**after, "astra": usage_row("astra", calls=1, input_tokens=1, output_tokens=1)}, old, new)
        wrong = copy.deepcopy(after)
        wrong["gemini"]["usage"]["input_tokens"] += 100
        with self.assertRaisesRegex(ValueError, "usage total|cumulative-counter delta"):
            probe._validate_native_delta({**before, "astra": usage_row("astra", calls=1, input_tokens=1, output_tokens=1)},
                                         {**wrong, "astra": usage_row("astra", calls=1, input_tokens=1, output_tokens=1)}, old, new)

    def test_native_transcript_sums_planner_rows_and_rejects_duplicate_or_invalid_steps(self):
        with tempfile.TemporaryDirectory(prefix="agy-transcript-") as temp:
            transcript = Path(temp) / "transcript_full"
            transcript.mkdir()
            rows = [
                {"type": "OTHER", "step_index": 0, "input_tokens": 900},
                {"type": "PLANNER_RESPONSE", "step_index": 1, "input_tokens": 10,
                 "output_tokens": 2, "cache_read_tokens": 4},
                {"type": "PLANNER_RESPONSE", "step_index": 2, "input_tokens": 5,
                 "output_tokens": 3, "cache_read_tokens": 1, "cache_write_tokens": 2},
            ]
            (transcript / "steps.jsonl").write_text(
                "".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8"
            )
            result = probe._sum_planner_transcript_rows(transcript)
            self.assertEqual(result, {
                "usage": {"input_tokens": 15, "output_tokens": 5, "cache_read_tokens": 5, "cache_write_tokens": 2},
                "llm_steps": 2,
            })
            rows.append({"type": "PLANNER_RESPONSE", "step_index": 2, "input_tokens": 1,
                         "output_tokens": 1, "cache_read_tokens": 0})
            (transcript / "steps.jsonl").write_text(
                "".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8"
            )
            with self.assertRaisesRegex(ValueError, "duplicated"):
                probe._sum_planner_transcript_rows(transcript)

    def test_native_transcript_is_bound_to_persisted_session_and_private_gemini_profile(self):
        with tempfile.TemporaryDirectory(prefix="agy-profile-transcript-") as temp:
            root = Path(temp)
            state = root / "primary/state.json"
            state.parent.mkdir()
            handle = "gemini"
            session_id = "synthetic-session-id"
            sessions = state.parent / "cli-sessions"
            sessions.mkdir()
            (sessions / f"{handle.encode().hex()}.json").write_text(
                json.dumps({"session_id": session_id}), encoding="utf-8"
            )
            project = root / "primary/projects/gemini"
            project.mkdir(parents=True)
            project_id = hashlib.sha256(str(project.resolve()).encode()).hexdigest()[:24]
            transcript = (
                state.parent / "workspaces/agents" / project_id / handle.encode().hex()
                / "profiles/home-agy/.gemini/antigravity-cli/brain" / session_id
                / ".system_generated/logs/chunks/transcript_full"
            )
            transcript.mkdir(parents=True)
            (transcript / "steps.jsonl").write_text(json.dumps({
                "type": "PLANNER_RESPONSE", "step_index": 0, "input_tokens": 10,
                "output_tokens": 3, "cache_read_tokens": 2,
            }) + "\n", encoding="utf-8")
            self.assertEqual(probe._native_transcript_usage(state, handle, project), {
                "usage": {"input_tokens": 10, "output_tokens": 3, "cache_read_tokens": 2, "cache_write_tokens": 0},
                "llm_steps": 1,
            })

    def test_engine_binary_requires_private_single_link_file(self):
        with tempfile.TemporaryDirectory(prefix="agy-engine-check-") as temp:
            engine = Path(temp) / "orgbot"
            engine.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            engine.chmod(0o700)
            probe._validate_engine_binary(engine)
            engine.chmod(0o777)
            with self.assertRaisesRegex(ValueError, "private"):
                probe._validate_engine_binary(engine)

    def test_proof_validator_accepts_sanitized_subscription_evidence(self):
        proof = proof_fixture()
        probe.validate_proof(proof)
        serialized = json.dumps(proof)
        self.assertNotIn("ACK:", serialized)

    def test_proof_validator_rejects_wrong_identity_missing_usage_or_overcount(self):
        mutations = []
        wrong_cli = proof_fixture()
        wrong_cli["usage"]["before_restart"]["gemini"]["cli"] = "codex"
        mutations.append(wrong_cli)
        incomplete = proof_fixture()
        incomplete["usage"]["after_restart"]["gemini"]["usage_complete"] = False
        mutations.append(incomplete)
        overcounted = proof_fixture()
        overcounted["usage"]["after_restart"]["gemini"]["usage"]["input_tokens"] += 100
        mutations.append(overcounted)
        no_session_isolation = proof_fixture()
        no_session_isolation["native_session_sha256"]["isolated_gemini"] = no_session_isolation["native_session_sha256"]["gemini"]
        mutations.append(no_session_isolation)
        exposed_message = proof_fixture()
        exposed_message["messages"] = ["SEED:secret"]
        mutations.append(exposed_message)
        transcript_overcount = proof_fixture()
        transcript_overcount["antigravity_transcript_usage"]["after_restart"]["usage"]["input_tokens"] += 1
        mutations.append(transcript_overcount)
        for proof in mutations:
            with self.subTest(proof=proof), self.assertRaises(ValueError):
                probe.validate_proof(proof)

    def test_owner_target_and_message_sequence_validators_reject_routing_errors(self):
        nonce = "synthetic-marker"
        steps = [("astra", f"SEED:{nonce}", "gemini"), ("gemini", f"ACK:{nonce}", "everyone")]
        messages = [
            {"author": {"kind": "agent", "agent": IDS["astra"]}, "body": steps[0][1],
             "audience": {"kind": "agents", "agents": [IDS["gemini"]]}},
            {"author": {"kind": "agent", "agent": IDS["gemini"]}, "body": steps[1][1],
             "audience": {"kind": "everyone"}},
        ]
        context_probe.validate_message_sequence(messages, steps, IDS)
        wrong_target = copy.deepcopy(messages)
        wrong_target[0]["audience"] = {"kind": "everyone"}
        with self.assertRaisesRegex(ValueError, "recipient"):
            context_probe.validate_message_sequence(wrong_target, steps, IDS)
        owner = {"author": {"kind": "human", "name": "Owner"}, "body": "@astra relay",
                 "audience": {"kind": "agents", "agents": [IDS["astra"]]}}
        context_probe.validate_owner_targets([owner], [("@astra relay", "astra")], IDS)
        owner["audience"] = {"kind": "agents", "agents": [IDS["gemini"]]}
        with self.assertRaisesRegex(ValueError, "@handle"):
            context_probe.validate_owner_targets([owner], [("@astra relay", "astra")], IDS)

    def test_isolated_room_requires_unknown_and_excludes_primary_marker(self):
        checkpoint = {"engine": {"log": []}}
        message = {"author": {"kind": "agent", "agent": IDS["gemini"]}, "body": "UNKNOWN\n",
                   "audience": {"kind": "everyone"}}
        context_probe.validate_isolated_room(checkpoint, "primary-nonce", [message], IDS["gemini"])
        with self.assertRaisesRegex(ValueError, "nonce"):
            context_probe.validate_isolated_room({"seed": "primary-nonce"}, "primary-nonce", [message], IDS["gemini"])
        wrong = copy.deepcopy(message)
        wrong["body"] = "UNKNOWN and more"
        with self.assertRaisesRegex(ValueError, "unknown prior context"):
            context_probe.validate_isolated_room(checkpoint, "primary-nonce", [wrong], IDS["gemini"])


if __name__ == "__main__":
    unittest.main()
