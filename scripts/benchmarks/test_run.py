import json
from pathlib import Path
import sys
import tempfile
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
        def invoke(command, variant, trial, case, mode, timeout, run_dir):
            return {"variant": variant, "case_id": case["id"], "trial": trial, "latency_ms": 1.0,
                    "model": "m", "billing_kind": "local", "usage": {field: 1 for field in run.TOKENS},
                    "measurement_source": "fixture", "engine_actual_api_cost_microusd": None,
                    "engine_api_equivalent_cost_microusd": None,
                    "usage_reported": True, "usage_complete": True, "human_interventions": 0,
                    "accepted": True, "criteria": case["acceptance"]}

        with patch.object(run, "invoke", side_effect=invoke):
            with tempfile.TemporaryDirectory() as directory:
                report = run.run([fixture], "baseline", "team", 3, "recorded", 10, {}, Path(directory))
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
        from tempfile import TemporaryDirectory
        with TemporaryDirectory() as directory:
            row = run.invoke(command, "baseline", 1, case, "recorded", 30, Path(directory) / "run")
        self.assertTrue(row["accepted"])
        self.assertEqual(row["acceptance_method"], "independent_verifier_command")
        self.assertEqual(row["measurement_source"], "fixture")
        self.assertFalse(row["usage_reported"])
        self.assertTrue(all(value == 0 for value in row["usage"].values()))
        self.assertIsNone(row["engine_actual_api_cost_microusd"])

    def test_adapter_supplied_usage_is_ignored_without_engine_event_source(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter = Path(directory) / "fake.py"
            adapter.write_text(
                "import json; print(json.dumps({'deliverable':'complete','measurement_source':'fixture','usage_reported':True,'usage_complete':True,'usage':{'input_tokens':999999}}))",
                encoding="utf-8",
            )
            case = {"id": "phrase", "task": "do work", "acceptance": ["complete"]}
            row = run.invoke(f"{sys.executable} {adapter}", "team", 1, case, "recorded", 10, Path(directory) / "run")
            self.assertTrue(row["accepted"])
            self.assertFalse(row["usage_reported"])
            self.assertEqual(row["usage"]["input_tokens"], 0)

    def test_run_retains_checkpoint_per_agent_usage_and_failure_report(self):
        fixture = {"id": "case", "task": "do work", "acceptance": ["complete"]}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            ok = root / "ok.py"
            ok.write_text(
                "import json,os,pathlib; pathlib.Path(os.environ['ORGBOT_BENCH_STATE']).write_text('{\\\"kept\\\":true}'); "
                "print(json.dumps({'deliverable':'complete','measurement_source':'orgbot_engine','room_phase':'completed','failed_tasks':0,'completion_accepted':True,'usage_reported':True,'usage_complete':True,'usage':{'input_tokens':4},'agent_usage':[{'agent':'a','usage':{'input_tokens':4}}],'provider_usage':[{'model':'m','input_tokens':4}]}))",
                encoding="utf-8",
            )
            failed = root / "failed.py"
            failed.write_text(
                "import json,sys; print('failure evidence',file=sys.stderr); print(json.dumps({'measurement_source':'orgbot_engine','usage_reported':True,'usage_complete':True,'usage':{'input_tokens':12},'agent_usage':[]})); sys.exit(3)",
                encoding="utf-8",
            )
            report = run.run([fixture], f"{sys.executable} {ok}", f"{sys.executable} {failed}", 1, "recorded", 10, {}, root / "report")
            kept = root / "report" / "runs" / "baseline-trial-1-case"
            self.assertTrue((kept / "room-state.json").is_file())
            self.assertEqual(report["runs"][0]["agent_usage"][0]["agent"], "a")
            self.assertEqual(report["runs"][0]["provider_usage"][0]["model"], "m")
            self.assertEqual(report["runs"][1]["status"], "failed")
            self.assertEqual(report["runs"][1]["partial_measurement"]["measurement_source"], "orgbot_engine")
            self.assertTrue(report["runs"][1]["partial_measurement"]["usage_reported"])
            self.assertEqual(report["team"]["tokens"]["input_tokens"], 12)
            self.assertFalse(report["team"]["usage_complete"])
            self.assertIsNone(report["team"]["api_equivalent_cost_microusd"])
            self.assertIn("failure evidence", (root / "report" / "runs" / "team-trial-1-case" / "stderr.log").read_text())
            persisted = json.loads((root / "report" / "report.json").read_text())
            self.assertEqual(persisted["runs"][1]["status"], "failed")

    def test_verifier_refuses_model_modified_fixture_tests(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter = Path(directory) / "tamper.py"
            adapter.write_text(
                "import json,os,pathlib; pathlib.Path(os.environ['ORGBOT_BENCH_WORKSPACE'],'test_acceptance.py').write_text('pass'); print(json.dumps({'deliverable':'done'}))",
                encoding="utf-8",
            )
            case = {"id": "tamper", "task": "work", "acceptance": [], "verify": [sys.executable, "-m", "unittest"],
                    "workspace_files": {"test_acceptance.py": "raise SystemExit(1)\n", "result.py": "pass\n"}}
            row = run.invoke(f"{sys.executable} {adapter}", "baseline", 1, case, "recorded", 10, Path(directory) / "run")
            self.assertFalse(row["accepted"])
            self.assertEqual(row["artifact_verification"]["protected_test_files_changed"], ["test_acceptance.py"])

    def test_timeout_retains_checkpoint_measurement(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            checkpoint = json.loads((Path(__file__).parent / "fixtures" / "engine-checkpoint-usage.json").read_text())
            adapter = root / "slow.py"
            adapter.write_text(
                f"import os,pathlib,time; pathlib.Path(os.environ['ORGBOT_BENCH_STATE']).write_text({json.dumps(checkpoint)!r}); time.sleep(5)",
                encoding="utf-8",
            )
            fixture = {"id": "timeout", "task": "work", "acceptance": ["complete"]}
            report = run.run([fixture], f"{sys.executable} {adapter}", "unused", 1, "recorded", 0.25, {}, root / "report")
            attempt = report["runs"][0]
            self.assertEqual(attempt["status"], "failed")
            self.assertEqual(attempt["partial_measurement"]["usage"]["input_tokens"], 11)
            self.assertEqual(report["baseline"]["tokens"]["input_tokens"], 11)
            self.assertFalse(report["baseline"]["usage_complete"])


if __name__ == "__main__":
    unittest.main()
