#!/usr/bin/env python3
"""Focused tests for the Linux acceptance runner's reporting and scope."""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("linux-acceptance.py")
SPEC = importlib.util.spec_from_file_location("linux_acceptance", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
linux_acceptance = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = linux_acceptance
SPEC.loader.exec_module(linux_acceptance)


class LinuxAcceptanceTests(unittest.TestCase):
    def test_matrix_is_synthetic_and_notification_check_is_optional(self) -> None:
        cases = linux_acceptance.acceptance_cases("/flutter", "/dbus-run-session")
        self.assertIn("checkpoint_reopens_claimed_work", {case.name for case in cases})
        self.assertIn("native_cli_session_survives_interruption", {case.name for case in cases})
        self.assertIn("owner_question_keeps_its_options_and_waits", {case.name for case in cases})
        self.assertTrue(next(case for case in cases if case.name.startswith("private_dbus")).optional)
        self.assertFalse(any("suspend" in " ".join(case.command) or "reboot" in " ".join(case.command) for case in cases))

    def test_success_and_failure_exit_codes_are_recorded(self) -> None:
        case = linux_acceptance.Case(
            "fixture",
            (sys.executable, "-c", "print('synthetic acceptance output')"),
            Path.cwd(),
            "runner unit fixture",
        )
        result = linux_acceptance.run_case(case, timeout=5)
        self.assertEqual(result["status"], "passed")
        self.assertIn("synthetic acceptance output", result["output_tail"])

        cargo_case = linux_acceptance.Case(
            "cargo-test",
            (sys.executable, "-c", "print('test real_case ... ok')"),
            Path.cwd(),
            "test marker fixture",
            expected_test="real_case",
            expected_runner="cargo-test",
        )
        self.assertEqual(linux_acceptance.run_case(cargo_case, timeout=5)["status"], "passed")

        false_pass = linux_acceptance.Case(
            "filtered-out",
            (sys.executable, "-c", "print('running 0 tests\\ntest result: ok')"),
            Path.cwd(),
            "filter miss fixture",
            expected_test="expected_case",
            expected_runner="cargo-test",
        )
        result = linux_acceptance.run_case(false_pass, timeout=5)
        self.assertEqual(result["status"], "failed")
        self.assertEqual(result["exit_code"], 0)
        self.assertIn("did not report success", result["failure_reason"])

        failed = linux_acceptance.Case(
            "failure",
            (sys.executable, "-c", "raise SystemExit(7)"),
            Path.cwd(),
            "runner failure fixture",
        )
        result = linux_acceptance.run_case(failed, timeout=5)
        self.assertEqual(result["status"], "failed")
        self.assertEqual(result["exit_code"], 7)

    def test_report_creation_refuses_to_overwrite_existing_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "acceptance.json"
            report = {"schema_version": 1, "results": []}
            linux_acceptance.save_report(path, report)
            self.assertEqual(json.loads(path.read_text()), report)
            with self.assertRaises(FileExistsError):
                linux_acceptance.save_report(path, report)


if __name__ == "__main__":
    unittest.main()
