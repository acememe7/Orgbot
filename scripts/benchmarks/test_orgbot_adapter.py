#!/usr/bin/env python3
import json
import sys
import unittest
import tomllib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import orgbot_adapter


def joined(agent, handle, provider):
    return {"event": {"event": "agent_joined", "spec": {
        "id": agent, "handle": handle, "provider": provider,
    }}}


def usage(agent, *, actual=None, equivalent=None, reported=True, complete=True, inputs=1):
    return {"event": {"event": "usage_recorded", "agent": agent, "usage": {
        "reported": reported, "complete": complete,
        "input_tokens": inputs if reported else 0, "output_tokens": 2 if reported else 0,
        "cache_read_tokens": 3 if reported else 0, "cache_write_tokens": 4 if reported else 0,
        "actual_api_cost_microusd": actual,
        "api_equivalent_cost_microusd": equivalent,
    }}}


class OrgBotAdapterTests(unittest.TestCase):
    def test_rust_serialized_checkpoint_fixture_is_read_with_real_event_shape(self):
        fixture = Path(__file__).with_name("fixtures") / "engine-checkpoint-usage.json"
        result = orgbot_adapter.read_engine_usage(json.loads(fixture.read_text(encoding="utf-8")))
        self.assertEqual(result["measurement_source"], "orgbot_engine")
        self.assertEqual(result["usage"], {
            "input_tokens": 11, "output_tokens": 5,
            "cache_read_tokens": 7, "cache_write_tokens": 2,
        })
        self.assertEqual(result["actual_api_cost_microusd"], 31)
        self.assertEqual(result["api_equivalent_cost_microusd"], 31)
        self.assertEqual(result["model"], "deepseek-chat")
        self.assertEqual(result["agent_usage"][0]["provider"], "deep_seek")
        self.assertEqual(result["agent_usage"][0]["billing_kind"], "api")

    def test_missing_engine_call_marks_usage_unknown_and_incomplete(self):
        result = orgbot_adapter.read_engine_usage({"engine": {"log": [
            joined("api", "api", {"provider": "deep_seek", "model": "m"}),
            usage("api", reported=False, complete=False, actual=None, equivalent=None),
        ]}})
        self.assertFalse(result["usage_reported"])
        self.assertFalse(result["usage_complete"])
        self.assertIsNone(result["actual_api_cost_microusd"])

    def test_incomplete_api_call_does_not_discard_known_token_totals(self):
        result = orgbot_adapter.read_engine_usage({"engine": {"log": [
            joined("api", "api", {"provider": "deep_seek", "model": "m"}),
            usage("api", actual=5, equivalent=5),
            usage("api", reported=False, complete=False, actual=None, equivalent=None),
        ]}})
        self.assertTrue(result["usage_reported"])
        self.assertFalse(result["usage_complete"])
        self.assertEqual(result["usage"]["input_tokens"], 1)
        self.assertIsNone(result["actual_api_cost_microusd"])
        self.assertIsNone(result["api_equivalent_cost_microusd"])

    def test_room_costs_group_by_provider_and_exclude_subscription_from_actual_api_spend(self):
        result = orgbot_adapter.read_engine_usage({"engine": {"log": [
            joined("api", "backend", {"provider": "deep_seek", "model": "deepseek-v4"}),
            joined("cli", "frontend", {"provider": "cli_agent", "cli": "claude", "model": "claude-code"}),
            usage("api", actual=23, equivalent=23, inputs=10),
            usage("cli", actual=None, equivalent=41, inputs=20),
        ]}})
        self.assertEqual(result["billing_kind"], "mixed")
        self.assertEqual(result["actual_api_cost_microusd"], 23)
        self.assertEqual(result["api_equivalent_cost_microusd"], 64)
        self.assertIsNone(result["model"])
        self.assertEqual(result["usage"]["input_tokens"], 30)
        self.assertEqual({row["handle"]: row["actual_api_cost_microusd"] for row in result["agent_usage"]}, {
            "backend": 23, "frontend": None,
        })
        self.assertEqual(len(result["provider_usage"]), 2)

    def test_unclassified_provider_makes_actual_room_spend_unknown(self):
        result = orgbot_adapter.read_engine_usage({"engine": {"log": [
            joined("api", "api", {"provider": "deep_seek", "model": "m"}),
            joined("other", "other", {"provider": "future_kind", "model": "x"}),
            usage("api", actual=23, equivalent=23),
            usage("other", actual=None, equivalent=9),
        ]}})
        self.assertIsNone(result["actual_api_cost_microusd"])
        self.assertEqual(result["api_equivalent_cost_microusd"], 32)

    def test_remote_provider_requires_both_paid_switches(self):
        config = {"bots": [{"provider": {"kind": "cli_agent"}}]}
        with self.assertRaisesRegex(ValueError, "require benchmark"):
            orgbot_adapter.enforce_paid_gate(config, "recorded", "0")
        orgbot_adapter.enforce_paid_gate(config, "paid", "1")
        with self.assertRaisesRegex(ValueError, "require benchmark"):
            orgbot_adapter.enforce_paid_gate({"bots": [{"provider": {"provider": "deep_seek"}}]}, "recorded", "0")

    def test_template_serializes_paths_once_and_supports_custom_team_values(self):
        template = 'workdir={{WORKSPACE}}\nstate={{STATE_PATH}}\nprovider={{PROVIDER_CONFIG}}\nname={{TASK_TITLE}}\ngoal={{TASK}}\nbackend={{BACKEND}}'
        rendered = orgbot_adapter.render_template(
            template, 'a "quoted" task', Path('/tmp/a'), Path('/tmp/state'), 'fixture',
            '{ kind = "scripted" }', {"BACKEND": '{ kind = "deepseek", model = "m" }'},
        )
        config = tomllib.loads(rendered)
        self.assertEqual(config["workdir"], "/tmp/a")
        self.assertEqual(config["backend"]["kind"], "deepseek")
        self.assertEqual(config["goal"], 'a "quoted" task')

    def test_shipped_single_room_template_and_explicit_team_template_are_valid_toml(self):
        root = Path(__file__).parent
        single = (root / "orgbot-room-template.toml").read_text(encoding="utf-8")
        config = tomllib.loads(orgbot_adapter.render_template(
            single, "Implement the fixture", Path("/tmp/project"), Path("/tmp/room.json"), "crud",
            '{ provider = "scripted", script = "local" }',
        ))
        self.assertEqual(config["room"]["state_path"], "/tmp/room.json")
        self.assertEqual(config["tasks"][0]["brief"], "Implement the fixture")
        self.assertEqual(config["bots"][0]["provider"]["provider"], "scripted")

        team = (root / "orgbot-team-room-template.toml").read_text(encoding="utf-8")
        team_config = tomllib.loads(orgbot_adapter.render_template(
            team, "Implement the fixture", Path("/tmp/project"), Path("/tmp/room.json"), "crud",
            template_values={
                "COORDINATOR_PROVIDER": '{ provider = "cli_agent", cli = "codex", model = "OWNER_MODEL", workdir = "/tmp/project", policy = "workspace_write" }',
                "FRONTEND_PROVIDER": '{ provider = "cli_agent", cli = "claude", model = "OWNER_MODEL", workdir = "/tmp/project", policy = "workspace_write" }',
                "BACKEND_PROVIDER": '{ provider = "deep_seek", model = "OWNER_MODEL" }',
            },
        ))
        self.assertEqual([bot["provider"]["provider"] for bot in team_config["bots"]], ["cli_agent", "cli_agent", "deep_seek"])


if __name__ == "__main__":
    unittest.main()
