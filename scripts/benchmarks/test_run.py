import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).parent))
import run


class BenchmarkTests(unittest.TestCase):
    def test_cost_separates_api_bills_from_subscription_equivalent(self):
        rate = {"m": {
            "input_microusd_per_million": 1_000_000,
            "output_microusd_per_million": 2_000_000,
            "cache_read_microusd_per_million": 100_000,
            "cache_write_microusd_per_million": 500_000,
        }}
        rows = [
            {"model": "m", "billing_kind": "api", "measurement_source": "orgbot_engine", "engine_actual_api_cost_microusd": 190, "engine_api_equivalent_cost_microusd": 190, "usage_reported": True, "usage_complete": True, "usage": {"input_tokens": 100, "output_tokens": 20, "cache_read_tokens": 300, "cache_write_tokens": 40}, "latency_ms": 2, "accepted": True, "human_interventions": 0},
            {"model": "m", "billing_kind": "subscription", "measurement_source": "orgbot_engine", "engine_actual_api_cost_microusd": None, "engine_api_equivalent_cost_microusd": 190, "usage_reported": True, "usage_complete": True, "usage": {"input_tokens": 100, "output_tokens": 20, "cache_read_tokens": 300, "cache_write_tokens": 40}, "latency_ms": 4, "accepted": True, "human_interventions": 1},
        ]
        summary = run.summarize(rows, rate)
        self.assertEqual(rows[0]["actual_api_cost_microusd"], 190)
        self.assertIsNone(rows[1]["actual_api_cost_microusd"])
        self.assertEqual(summary["api_equivalent_cost_microusd"], 380)

    def test_run_repeats_identical_fixtures_for_both_variants(self):
        fixture = {"id": "case", "task": "do work", "acceptance": ["done"]}
        def invoke(command, variant, trial, case, mode, timeout):
            return {"variant": variant, "case_id": case["id"], "trial": trial, "latency_ms": 1.0,
                    "model": "m", "billing_kind": "local", "usage": {field: 1 for field in run.TOKENS},
                    "measurement_source": "fixture", "engine_actual_api_cost_microusd": None,
                    "engine_api_equivalent_cost_microusd": None,
                    "usage_reported": True, "usage_complete": True, "human_interventions": 0,
                    "accepted": True, "criteria": case["acceptance"]}

        with patch.object(run, "invoke", side_effect=invoke):
            report = run.run([fixture], "baseline", "team", 3, "recorded", 10, {})
        self.assertEqual(report["paired_runs"], 3)
        self.assertEqual(report["baseline"]["runs"], 3)
        self.assertEqual(report["team"]["acceptance_rate"], 1)
        self.assertEqual(len(report["runs"]), 6)

    def test_missing_cost_rates_stay_unknown(self):
        row = {"model": "m", "billing_kind": "api", "measurement_source": "orgbot_engine",
               "engine_actual_api_cost_microusd": None, "engine_api_equivalent_cost_microusd": None,
               "usage_reported": True, "usage_complete": False,
               "usage": {field: 0 for field in run.TOKENS}, "latency_ms": 1, "accepted": False, "human_interventions": 0}
        summary = run.summarize([row], {})
        self.assertIsNone(summary["actual_api_cost_microusd"])
        self.assertIsNone(summary["api_equivalent_cost_microusd"])

    def test_invalid_acceptance_fixture_is_rejected(self):
        from tempfile import NamedTemporaryFile
        with NamedTemporaryFile(mode="w+", encoding="utf-8") as fixture:
            fixture.write(json.dumps([{"id": "bad", "task": "x", "acceptance": []}]))
            fixture.flush()
            with self.assertRaises(ValueError):
                run.load_fixtures(__import__("pathlib").Path(fixture.name))

    def test_workspace_verifier_decides_acceptance_and_fixture_tokens_stay_unknown(self):
        fixtures = run.load_fixtures(Path(__file__).with_name("tasks.json"))
        case = fixtures[0]
        command = f"{sys.executable} {Path(__file__).with_name('fixture_adapter.py')}"
        row = run.invoke(command, "baseline", 1, case, "recorded", 30)
        self.assertTrue(row["accepted"])
        self.assertEqual(row["acceptance_method"], "independent_verifier_command")
        self.assertEqual(row["measurement_source"], "fixture")
        self.assertFalse(row["usage_reported"])
        self.assertTrue(all(value == 0 for value in row["usage"].values()))
        self.assertIsNone(row["engine_actual_api_cost_microusd"])

    def test_adapter_supplied_usage_is_ignored_without_engine_event_source(self):
        from tempfile import TemporaryDirectory
        with TemporaryDirectory() as directory:
            adapter = Path(directory) / "fake.py"
            adapter.write_text(
                "import json; print(json.dumps({'deliverable':'complete','measurement_source':'fixture','usage_reported':True,'usage_complete':True,'usage':{'input_tokens':999999}}))",
                encoding="utf-8",
            )
            case = {"id": "phrase", "task": "do work", "acceptance": ["complete"]}
            row = run.invoke(f"{sys.executable} {adapter}", "team", 1, case, "recorded", 10)
            self.assertTrue(row["accepted"])
            self.assertFalse(row["usage_reported"])
            self.assertEqual(row["usage"]["input_tokens"], 0)


if __name__ == "__main__":
    unittest.main()
