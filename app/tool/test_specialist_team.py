import copy
import unittest

from specialist_team import (
    _MEMORY_END,
    _MEMORY_START,
    load_preset,
    replace_with_specialist_team,
    validate_preset,
)


class SpecialistTeamTest(unittest.TestCase):
    def test_preset_has_ten_unique_keyless_bots_with_expected_policy(self):
        preset = load_preset()
        bots = preset["bots"]
        self.assertEqual(len(bots), 10)
        self.assertEqual(len({bot["id"] for bot in bots}), 10)
        self.assertEqual(len({bot["handle"] for bot in bots}), 10)
        self.assertTrue(all(bot["apiKey"] == "" for bot in bots))

        by_handle = {bot["handle"]: bot for bot in bots}
        self.assertEqual(
            {
                handle: (bot["provider"], bot["model"], bot["role"])
                for handle, bot in by_handle.items()
            },
            {
                "astra": ("codex", "gpt-6-astra", "security"),
                "opus": ("anthropic", "claude-opus-5-5", "architect"),
                "grok": ("grok", "grok-4.7", "support"),
                "grok-quick": ("grok", "grok-4.7", "engineer"),
                "deepseek-builder": ("deepseek", "deepseek-flash", "engineer"),
                "ollama-deepseek": ("ollama", "deepseek-v4.1-flash:1017", "researcher"),
                "ollama-glm": ("ollama", "glm-5.3-flash", "qa"),
                "sol": ("codex", "gpt-6.1-sol", "engineer"),
                "mira": ("codex", "gpt-6-luna", "engineer"),
                "ellis": ("anthropic", "claude-haiku-5-5", "writer"),
            },
        )
        self.assertEqual(
            by_handle["astra"]["powers"],
            {
                "may_create_tasks": True,
                "may_inspect": True,
                "may_submit": True,
                "review_only": True,
            },
        )
        self.assertTrue(by_handle["opus"]["powers"]["may_create_tasks"])
        self.assertTrue(by_handle["opus"]["powers"]["may_inspect"])
        self.assertTrue(by_handle["opus"]["powers"]["may_submit"])
        self.assertIn("coordinator", by_handle["astra"]["additional_roles"])
        self.assertIn("coordinator", by_handle["opus"]["additional_roles"])
        self.assertEqual(by_handle["grok"]["role"], "support")
        self.assertNotIn("coordinator", by_handle["grok"].get("additional_roles", []))
        self.assertFalse(by_handle["grok"]["powers"]["may_create_tasks"])
        self.assertFalse(by_handle["grok"]["powers"]["may_submit"])
        self.assertTrue(by_handle["ollama-glm"]["powers"]["may_inspect"])
        self.assertEqual(preset["room_limits"], {"max_turns": 0, "parallel_turns": 4})

    def test_transform_preserves_existing_data_and_does_not_mutate_input(self):
        preset = load_preset()
        library = {
            "future_library_field": {"keep": True},
            "user": {"name": "Owner", "profile": "Existing profile", "future": 17},
            "bots": [{"id": "old", "apiKey": "old-secret-marker"}],
            "rooms": [
                {
                    "id": "room-a",
                    "name": "Existing room",
                    "memory": "Owner's existing room memory.",
                    "tasks": ["historical-task-id"],
                    "workspace": {"root": "/existing/workspace", "writable": True},
                    "history": [{"event": "keep"}],
                    "bots": [{"bot_id": "old"}],
                    "max_turns": 500,
                    "parallel_turns": 4,
                    "future_room_field": ["keep"],
                },
                {"id": "room-b", "memory": "", "bots": []},
            ],
            "tasks": [{"id": "global-task", "state": "done"}],
        }
        original = copy.deepcopy(library)

        transformed = replace_with_specialist_team(library, preset)

        self.assertEqual(library, original)
        self.assertEqual(transformed["future_library_field"], {"keep": True})
        self.assertEqual(transformed["user"], original["user"])
        self.assertEqual(transformed["tasks"], original["tasks"])
        self.assertEqual(transformed["bots"], preset["bots"])
        self.assertEqual(len(transformed["bots"]), 10)
        bot_ids = [bot["id"] for bot in preset["bots"]]
        for old, room in zip(original["rooms"], transformed["rooms"], strict=True):
            self.assertEqual(room["bots"], [{"bot_id": bot_id} for bot_id in bot_ids])
            self.assertEqual(room["max_turns"], 0)
            self.assertEqual(room["parallel_turns"], 4)
            self.assertEqual(room.get("tasks"), old.get("tasks"))
            self.assertEqual(room.get("workspace"), old.get("workspace"))
            self.assertEqual(room.get("history"), old.get("history"))
            self.assertEqual(room.get("future_room_field"), old.get("future_room_field"))
        first_memory = transformed["rooms"][0]["memory"]
        self.assertIn("Owner's existing room memory.", first_memory)
        self.assertEqual(first_memory.count(_MEMORY_START), 1)
        self.assertEqual(first_memory.count(_MEMORY_END), 1)
        self.assertEqual(transformed["rooms"][1]["memory"].count(_MEMORY_START), 1)

    def test_transform_is_idempotent_and_refreshes_only_its_routing_block(self):
        preset = load_preset()
        library = {
            "user": {"profile": "preserve"},
            "rooms": [{"id": "r", "memory": "before\n\n" + _MEMORY_START +
                       "\nstale routing\n" + _MEMORY_END + "\n\nafter"}],
        }
        once = replace_with_specialist_team(library, preset)
        twice = replace_with_specialist_team(once, preset)
        self.assertEqual(twice, once)
        memory = once["rooms"][0]["memory"]
        self.assertLess(memory.index("before"), memory.index("after"))
        self.assertIn(preset["routing_memory"], memory)
        self.assertNotIn("stale routing", memory)
        self.assertEqual(memory.count(_MEMORY_START), 1)
        self.assertEqual(memory.count(_MEMORY_END), 1)

    def test_existing_per_bot_key_is_preserved_for_same_provider(self):
        preset = load_preset()
        old = copy.deepcopy(preset["bots"][4])
        old["apiKey"] = "fixture-private-key"
        result = replace_with_specialist_team({"bots": [old], "rooms": []})
        self.assertEqual(result["bots"][4]["apiKey"], "fixture-private-key")
        self.assertEqual(preset["bots"][4]["apiKey"], "")

    def test_every_role_has_an_owner(self):
        import re
        from pathlib import Path
        roles = set(re.findall(r"key: '([^']+)'", (Path(__file__).parents[1] / "lib/orgbot/roles.dart").read_text()))
        assigned = {role for bot in load_preset()["bots"] for role in [bot["role"], *bot.get("additional_roles", [])]}
        self.assertEqual(roles - assigned, set())

    def test_invalid_preset_is_rejected(self):
        preset = load_preset()
        preset["bots"][0]["apiKey"] = "must-never-be-present"
        with self.assertRaisesRegex(ValueError, "API keys must be empty"):
            validate_preset(preset)


if __name__ == "__main__":
    unittest.main()
