import contextlib
import hashlib
import io
import importlib.util
import json
import os
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import threading
import time
from types import SimpleNamespace
import unittest
from pathlib import Path
from unittest.mock import patch


MODULE_PATH = Path(__file__).with_name("orgbot-workspace.py")
SPEC = importlib.util.spec_from_file_location("orgbot_workspace", MODULE_PATH)
workspace = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(workspace)


class WorkspaceTests(unittest.TestCase):
    def setUp(self):
        # Keep the fixture on the read-only host root rather than /tmp, which
        # the sandbox intentionally replaces with private tmpfs.
        self.tmp = tempfile.TemporaryDirectory(prefix="orgbot-workspace-test-", dir=Path.home())
        self.root = Path(self.tmp.name)
        self.project = self.root / "project"
        self.project.mkdir(mode=0o700)
        self.state = self.root / "state"
        self.state.mkdir(mode=0o700)
        (self.project / "src").mkdir()
        (self.project / "src/main.py").write_text("original\n", encoding="utf-8")
        os.chmod(self.project / "src/main.py", 0o644)
        (self.project / "README.md").write_text("keep me\n", encoding="utf-8")

    def tearDown(self):
        self.tmp.cleanup()

    def prepare(self, agent="dev", read_only=False):
        result = workspace._prepare(os.fspath(self.project), os.fspath(self.state), agent, read_only)
        return Path(result["workspace"])

    def _fake_trash(self, project, target, expected_sha256=None):
        if expected_sha256 is not None:
            actual = workspace._entry(target, project)["sha256"]
            if actual != expected_sha256:
                raise workspace.WorkspaceError("fake trash rejected unexpected file contents")
        trash = self.root / "synthetic-trash"
        trash.mkdir(mode=0o700, exist_ok=True)
        backup = trash / f"{len(list(trash.iterdir()))}-{target.name}"
        os.replace(target, backup)
        return os.fspath(backup)

    def test_prepare_is_independent_and_reuses_workspace(self):
        first = self.prepare()
        independent = first / "src/main.py"
        self.assertNotEqual(os.stat(independent).st_ino, os.stat(self.project / "src/main.py").st_ino)
        independent.write_text("agent version\n", encoding="utf-8")
        second = self.prepare()
        self.assertEqual(first, second)
        self.assertEqual(independent.read_text(encoding="utf-8"), "agent version\n")
        self.assertEqual((self.project / "src/main.py").read_text(encoding="utf-8"), "original\n")

    def test_private_mount_contract_is_explicit(self):
        result = workspace._prepare(os.fspath(self.project), os.fspath(self.state), "dev", False)
        self.assertEqual(result["private_home_mount"], f"/run/user/{os.getuid()}/orgbot-home")
        self.assertEqual(result["mcp_socket_mount"], f"/run/user/{os.getuid()}/orgbot-room-mcp.sock")

    def test_readonly_source_mode_is_preserved_and_not_misreported(self):
        source = self.project / "src/main.py"
        os.chmod(source, 0o444)
        work = self.prepare()
        self.assertEqual(workspace._status(os.fspath(self.state), "dev")["changed"], [])
        (work / "src/main.py").write_text("agent edit\n", encoding="utf-8")
        with patch.object(workspace, "_trash_replacement", self._fake_trash):
            result = workspace._apply(os.fspath(self.state), "dev", review_token=None)
        self.assertTrue(result["ok"])
        self.assertEqual(stat.S_IMODE(source.stat().st_mode), 0o444)

    def test_prepare_rebases_clean_files_and_preserves_agent_edits(self):
        work = self.prepare()
        (work / "src/main.py").write_text("agent edit\n", encoding="utf-8")
        (self.project / "README.md").write_text("owner update\n", encoding="utf-8")
        self.prepare()
        self.assertEqual((work / "src/main.py").read_text(encoding="utf-8"), "agent edit\n")
        self.assertEqual((work / "README.md").read_text(encoding="utf-8"), "owner update\n")
        self.assertEqual(workspace._status(os.fspath(self.state), "dev")["changed"], [{"path": "src/main.py", "kind": "modified"}])

    def test_prepare_reports_same_file_refresh_conflict(self):
        work = self.prepare()
        (work / "src/main.py").write_text("agent edit\n", encoding="utf-8")
        (self.project / "src/main.py").write_text("owner edit\n", encoding="utf-8")
        (self.project / "README.md").write_text("independent owner update\n", encoding="utf-8")
        self.prepare()
        status = workspace._status(os.fspath(self.state), "dev")
        self.assertEqual(status["conflicts"], ["src/main.py"])
        self.assertEqual((self.project / "src/main.py").read_text(encoding="utf-8"), "owner edit\n")
        self.assertEqual((work / "src/main.py").read_text(encoding="utf-8"), "agent edit\n")
        self.assertEqual((work / "README.md").read_text(encoding="utf-8"), "independent owner update\n")

    def test_apply_preserves_replaced_bytes_and_records_journal(self):
        work = self.prepare()
        (work / "src/main.py").write_text("agent edit\n", encoding="utf-8")
        (work / "new file.txt").write_text("new\n", encoding="utf-8")
        with patch.object(workspace, "_trash_replacement", self._fake_trash):
            result = workspace._apply(os.fspath(self.state), "dev", review_token=None)
        self.assertTrue(result["ok"])
        self.assertEqual((self.project / "src/main.py").read_text(encoding="utf-8"), "agent edit\n")
        self.assertEqual((self.project / "new file.txt").read_text(encoding="utf-8"), "new\n")
        backups = list((self.root / "synthetic-trash").iterdir())
        self.assertEqual([path.read_text(encoding="utf-8") for path in backups], ["original\n"])
        journal = json.loads((work.parent / "apply-journal.json").read_text(encoding="utf-8"))
        self.assertEqual(journal["phase"], "complete")

    def test_apply_uses_configured_real_trash_and_keeps_deletion_gate(self):
        home = self.root / "home"
        home.mkdir(mode=0o700)
        recovery = self.root / "recovery"
        helper = MODULE_PATH.with_name("orgbot-trash.py")
        environment = {
            "HOME": str(home),
            "XDG_CONFIG_HOME": str(home / "config"),
            "XDG_DATA_HOME": str(home / "data"),
        }
        with patch.dict(os.environ, environment):
            subprocess.run([sys.executable, str(helper), "configure", "--root", str(recovery)],
                           check=True, capture_output=True, text=True)
            work = self.prepare()
            (work / "src/main.py").write_text("tested update\n", encoding="utf-8")
            (work / "README.md").unlink()
            applied = workspace._apply(str(self.state), "dev", review_token=None)
            self.assertTrue(applied["ok"])
            self.assertEqual((self.project / "src/main.py").read_text(), "tested update\n")
            self.assertTrue((self.project / "README.md").exists())
            status = workspace._status(str(self.state), "dev")
            self.assertEqual(status["deleted"], ["README.md"])
            applied = workspace._apply(str(self.state), "dev", review_token=status["review_token"])
            self.assertTrue(applied["ok"])
            self.assertFalse((self.project / "README.md").exists())
            payloads = sorted(path.read_text() for path in recovery.glob("projects/*/*/payload/*"))
            self.assertEqual(payloads, ["keep me\n", "original\n"])
            self.assertTrue((home / "config/orgbots/trash.json").is_file())

    def test_apply_then_refresh_does_not_overwrite_another_agents_update(self):
        first = self.prepare("agent-a")
        second = self.prepare("agent-b")
        (first / "src/main.py").write_text("agent a\n", encoding="utf-8")
        (second / "README.md").write_text("agent b\n", encoding="utf-8")
        with patch.object(workspace, "_trash_replacement", self._fake_trash):
            self.assertTrue(workspace._apply(os.fspath(self.state), "agent-b", review_token=None)["ok"])
            self.assertTrue(workspace._apply(os.fspath(self.state), "agent-a", review_token=None)["ok"])
        self.assertEqual((self.project / "README.md").read_text(encoding="utf-8"), "agent b\n")
        workspace._prepare(os.fspath(self.project), os.fspath(self.state), "agent-a", False)
        with patch.object(workspace, "_trash_replacement", self._fake_trash):
            self.assertTrue(workspace._apply(os.fspath(self.state), "agent-a", review_token=None)["ok"])
        self.assertEqual((self.project / "README.md").read_text(encoding="utf-8"), "agent b\n")

    def test_conflict_rejects_every_change_before_mutation(self):
        work = self.prepare()
        (work / "src/main.py").write_text("agent edit\n", encoding="utf-8")
        (work / "new file.txt").write_text("new\n", encoding="utf-8")
        (self.project / "src/main.py").write_text("owner edit\n", encoding="utf-8")
        with patch.object(workspace, "_trash_replacement", self._fake_trash):
            result = workspace._apply(os.fspath(self.state), "dev", review_token=None)
        self.assertFalse(result["ok"])
        self.assertEqual(result["conflicts"], ["src/main.py"])
        self.assertFalse((self.project / "new file.txt").exists())
        self.assertFalse((self.root / "synthetic-trash").exists())

    def test_interrupted_replacement_keeps_original_recoverable_and_journaled(self):
        work = self.prepare()
        (work / "src/main.py").write_text("agent edit\n", encoding="utf-8")

        def preserve_then_interrupt(project, target, expected_sha256=None):
            self._fake_trash(project, target, expected_sha256)
            raise workspace.WorkspaceError("simulated interruption after preservation")

        with patch.object(workspace, "_trash_replacement", preserve_then_interrupt):
            with self.assertRaisesRegex(workspace.WorkspaceError, "simulated interruption"):
                workspace._apply(os.fspath(self.state), "dev", review_token=None)
        backups = list((self.root / "synthetic-trash").iterdir())
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_text(encoding="utf-8"), "original\n")
        journal = json.loads((work.parent / "apply-journal.json").read_text(encoding="utf-8"))
        self.assertEqual(journal["phase"], "applying")
        self.assertEqual(journal["operations"][0]["state"], "preserving")

    def test_deletion_requires_explicit_owner_approval(self):
        work = self.prepare()
        (work / "README.md").unlink()
        reviewed = workspace._status(os.fspath(self.state), "dev")
        denied = workspace._apply(os.fspath(self.state), "dev", review_token=None)
        self.assertTrue(denied["ok"])
        self.assertEqual(denied["pending_delete"], ["README.md"])
        self.assertTrue((self.project / "README.md").exists())
        with patch.object(workspace, "_trash_replacement", self._fake_trash):
            approved = workspace._apply(os.fspath(self.state), "dev", review_token=reviewed["review_token"])
        self.assertTrue(approved["ok"])
        self.assertEqual(approved["review_token"], workspace._status(os.fspath(self.state), "dev")["review_token"])
        self.assertFalse((self.project / "README.md").exists())
        self.assertEqual(list((self.root / "synthetic-trash").iterdir())[0].read_text(encoding="utf-8"), "keep me\n")

    def test_blanket_allow_delete_flag_is_not_accepted(self):
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as caught:
                workspace._parser().parse_args([
                    "apply",
                    "--state",
                    os.fspath(self.state),
                    "--agent",
                    "dev",
                    "--allow-delete",
                ])
        self.assertEqual(caught.exception.code, 2)

    def test_verify_builds_minimal_networkless_readonly_sandbox(self):
        work = self.prepare("reviewer", read_only=True)
        with patch.object(workspace.shutil, "which", return_value="/usr/bin/bwrap"), patch.object(
            workspace.subprocess, "run", return_value=SimpleNamespace(returncode=0)
        ) as run:
            result = workspace._run(
                os.fspath(self.state), "reviewer", ["/usr/bin/python3", "-B", "-m", "unittest"],
                read_only=True, profiles=[], private_home_arg=None, mcp_socket=None, verify=True,
            )
        self.assertEqual(result, 0)
        args = run.call_args.args[0]
        self.assertIn("--unshare-net", args)
        self.assertIn("--clearenv", args)
        self.assertIn("--tmpfs", args)
        for index, value in enumerate(args):
            if value == "--tmpfs":
                self.assertEqual(args[index - 2], "--size")
        self.assertIn("/", args)
        self.assertIn("--ro-bind", args)
        self.assertNotIn("--bind", args)
        self.assertNotIn("--mcp-socket", args)
        self.assertNotIn(os.fspath(self.state), args)
        self.assertNotIn(os.fspath(Path.home()), args)
        self.assertNotIn("/", [args[i + 1] for i, value in enumerate(args[:-1]) if value == "--ro-bind"])

    @unittest.skipUnless(sys.platform.startswith("linux") and workspace.shutil.which("bwrap"),
                         "requires Linux bubblewrap user namespaces")
    def test_verify_runs_python_tests_without_exposing_host_or_applying_changes(self):
        (self.project / "test_verify.py").write_text(
            "import os, pathlib, tempfile, unittest\n"
            "class VerifyTests(unittest.TestCase):\n"
            " def test_workspace_and_private_tmp(self):\n"
            "  self.assertEqual(os.getcwd(), os.environ['ORGBOT_WORKSPACE'])\n"
            "  with tempfile.NamedTemporaryFile() as f: self.assertTrue(pathlib.Path(f.name).exists())\n",
            encoding="utf-8",
        )
        work = self.prepare("reviewer", read_only=True)
        secret = self.root / "host-secret.txt"
        secret.write_text("must stay hidden", encoding="utf-8")
        original = self.project / "src/main.py"
        original_bytes = original.read_bytes()
        probe = (
            "import pathlib,socket,sys; "
            "assert pathlib.Path('/tmp/orgbot-workspace/src/main.py').exists(); "
            "p=pathlib.Path('/tmp/orgbot-workspace/src/main.py'); "
            "exec(\"try: p.write_text('changed')\\nexcept OSError: pass\\nelse: raise AssertionError('workspace writable')\"); "
            f"assert not pathlib.Path({str(secret)!r}).exists(); "
            "s=socket.socket(); s.settimeout(.5); "
            "exec(\"try: s.connect(('1.1.1.1',80))\\nexcept OSError: pass\\nelse: raise AssertionError('network available')\")"
        )
        result = workspace._run(
            os.fspath(self.state), "reviewer", ["/usr/bin/python3", "-B", "-m", "unittest", "-v"],
            read_only=True, profiles=[], private_home_arg=None, mcp_socket=None, verify=True,
        )
        self.assertEqual(result, 0)
        result = workspace._run(
            os.fspath(self.state), "reviewer", ["/usr/bin/python3", "-B", "-c", probe],
            read_only=True, profiles=[], private_home_arg=None, mcp_socket=None, verify=True,
        )
        self.assertEqual(result, 0)
        self.assertEqual(original.read_bytes(), original_bytes)
        self.assertEqual(workspace._status(os.fspath(self.state), "reviewer")["changed"], [])

    def test_stale_deletion_approval_refuses_all_mutation(self):
        work = self.prepare()
        (work / "README.md").unlink()
        reviewed = workspace._status(os.fspath(self.state), "dev")
        (work / "src/main.py").write_text("new agent edit after review\n", encoding="utf-8")
        result = workspace._apply(os.fspath(self.state), "dev", review_token=reviewed["review_token"])
        self.assertFalse(result["ok"])
        self.assertEqual(result["pending_delete"], ["README.md"])
        self.assertTrue((self.project / "README.md").exists())
        self.assertEqual((self.project / "src/main.py").read_text(encoding="utf-8"), "original\n")

    def test_unexpected_owner_file_created_after_scan_is_never_trashed_or_overwritten(self):
        work = self.prepare()
        (work / "owner-created-race.txt").write_text("agent version\n", encoding="utf-8")
        scan_workspace = workspace._scan_workspace

        def create_owner_file_after_scan(root, baseline):
            entries = scan_workspace(root, baseline)
            (self.project / "owner-created-race.txt").write_text("owner bytes\n", encoding="utf-8")
            return entries

        with patch.object(workspace, "_scan_workspace", side_effect=create_owner_file_after_scan):
            with self.assertRaisesRegex(workspace.WorkspaceError, "target appeared after conflict checking"):
                workspace._apply(os.fspath(self.state), "dev", review_token=None)
        self.assertEqual((self.project / "owner-created-race.txt").read_text(encoding="utf-8"), "owner bytes\n")
        self.assertFalse((self.root / "synthetic-trash").exists())

    def test_malformed_manifest_and_unsafe_agent_path_fail_closed(self):
        with self.assertRaisesRegex(workspace.WorkspaceError, "Agent id"):
            self.prepare("../outside")
        work = self.prepare()
        manifest = self.state / "manifests" / workspace._project_id(self.project) / "dev.json"
        manifest.write_text("{}", encoding="utf-8")
        with self.assertRaisesRegex(workspace.WorkspaceError, "identity or version"):
            workspace._status(os.fspath(self.state), "dev")
        self.assertTrue(work.is_dir())

    def test_external_symlinked_project_entries_fail_closed(self):
        outside = self.root / "outside"
        outside.write_text("outside\n", encoding="utf-8")
        (self.project / "escape").symlink_to(outside)
        with self.assertRaisesRegex(workspace.WorkspaceError, "project symlinks are not supported|resolve inside the project"):
            self.prepare()

    def test_safe_internal_symlinks_work_inside_copy_but_not_baseline(self):
        (self.project / "src/link.py").symlink_to("main.py")
        generated = self.project / "node_modules"
        generated.mkdir()
        (generated / "large-cache").write_text("ignore this cache\n", encoding="utf-8")
        git = self.project / ".git"
        git.mkdir()
        (git / "HEAD").write_text("ref: refs/heads/main\n", encoding="utf-8")
        work = self.prepare()
        self.assertEqual((work / "src/link.py").read_text(encoding="utf-8"), "original\n")
        self.assertTrue((work / ".git/HEAD").is_file())
        self.assertFalse((work / "node_modules").exists())
        (work / "src/link.py").write_text("changed through symlink\n", encoding="utf-8")
        self.assertEqual((work / "src/main.py").read_text(encoding="utf-8"), "changed through symlink\n")
        status = workspace._status(os.fspath(self.state), "dev")
        self.assertEqual(status["changed"], [{"path": "src/main.py", "kind": "modified"}])
        manifest = self.state / "manifests" / workspace._project_id(self.project) / "dev.json"
        self.assertNotIn(".git/HEAD", workspace._manifest_read(manifest, self.project, "dev")["entries"])

    def test_hardlinked_project_files_fail_closed(self):
        os.link(self.project / "README.md", self.project / "README-copy.md")
        with self.assertRaisesRegex(workspace.WorkspaceError, "Multiply-linked files"):
            self.prepare()

    def test_workspace_symlink_cannot_redirect_apply(self):
        work = self.prepare()
        outside = self.root / "outside"
        outside.mkdir()
        (outside / "main.py").write_text("protected\n", encoding="utf-8")
        (work / "src").rename(work / "src-real")
        (work / "src").symlink_to(outside, target_is_directory=True)
        with self.assertRaisesRegex(workspace.WorkspaceError, "project symlinks are not supported|resolve inside the project"):
            workspace._apply(os.fspath(self.state), "dev", review_token=None)
        self.assertEqual((outside / "main.py").read_text(encoding="utf-8"), "protected\n")

    def test_bwrap_denies_original_state_and_other_agent_writes(self):
        work = self.prepare("dev")
        other = self.prepare("reviewer", read_only=True)
        manifest = self.state / "manifests" / workspace._project_id(self.project) / "dev.json"
        code = f"""
import os
from pathlib import Path
paths = [{str(self.project / 'src/main.py')!r}, {str(manifest)!r}, {str(other / 'README.md')!r}]
if Path(paths[0]).read_text() != 'original\\n':
    raise SystemExit(30)
for path in paths:
    try:
        Path(path).write_text('overwritten')
        raise SystemExit(31)
    except OSError:
        pass
try:
    Path({str(self.project / 'README.md')!r}).unlink()
    raise SystemExit(32)
except OSError:
    pass
try:
    import shutil
    shutil.rmtree({str(self.project / 'src')!r})
    raise SystemExit(34)
except OSError:
    pass
if any(os.getenv(name) for name in ('SSH_AUTH_SOCK', 'DBUS_SESSION_BUS_ADDRESS', 'ORGBOT_ROOM_TOKEN', 'DEEPSEEK_API_KEY', 'ORGBOT_FAKE_API_KEY')):
    raise SystemExit(33)
if Path('/run/lock').exists() or Path('/var/run/lock').exists():
    raise SystemExit(35)
Path('/tmp/orgbot-workspace/agent.txt').write_text('sandbox write')
Path('/tmp/private.txt').write_text('temporary')
"""
        with patch.dict(os.environ, {"SSH_AUTH_SOCK": "/tmp/fake-agent.sock", "DBUS_SESSION_BUS_ADDRESS": "unix:path=/tmp/fake-bus", "ORGBOT_ROOM_TOKEN": "test-room-token", "DEEPSEEK_API_KEY": "test-provider-key", "ORGBOT_FAKE_API_KEY": "test-orgbot-key"}):
            result = workspace._run(
                os.fspath(self.state),
                "dev",
                [sys.executable, "-c", code],
                read_only=False,
                profiles=[],
                private_home_arg=None,
                mcp_socket=None,
            )
        self.assertEqual(result, 0)
        self.assertFalse((self.project / "agent.txt").exists())
        self.assertEqual((work / "agent.txt").read_text(encoding="utf-8"), "sandbox write")
        self.assertEqual((self.project / "src/main.py").read_text(encoding="utf-8"), "original\n")
        self.assertEqual((self.project / "README.md").read_text(encoding="utf-8"), "keep me\n")

    def test_bwrap_readonly_reviewer_cannot_write_workspace(self):
        work = self.prepare("reviewer", read_only=True)
        private_home = self.state / "agents" / workspace._project_id(self.project) / "reviewer" / "profiles" / "home"
        home_mount = f"/run/user/{os.getuid()}/orgbot-home"
        code = f"from pathlib import Path; Path({(home_mount + '/session')!r}).write_text('refresh');\ntry: Path('/tmp/orgbot-workspace/blocked').write_text('no'); raise SystemExit(4)\nexcept OSError: pass"
        result = workspace._run(os.fspath(self.state), "reviewer", [sys.executable, "-c", code], read_only=False, profiles=[], private_home_arg=os.fspath(private_home), mcp_socket=None)
        self.assertEqual(result, 0)
        self.assertFalse((work / "blocked").exists())
        self.assertEqual((private_home / "session").read_text(encoding="utf-8"), "refresh")

    def test_bwrap_maps_native_cli_profile_and_working_directory(self):
        work = self.prepare("dev")
        private_home = self.state / "agents" / workspace._project_id(self.project) / "dev" / "profiles" / "home"
        profile = private_home / ".codex"
        mount = f"/run/user/{os.getuid()}/orgbot-home"
        resolver_hash = hashlib.sha256(Path("/etc/resolv.conf").read_bytes()).hexdigest()
        code = f"from pathlib import Path; import hashlib, os; assert os.getcwd() == '/tmp/orgbot-workspace'; resolver = Path('/etc/resolv.conf'); assert hashlib.sha256(resolver.read_bytes()).hexdigest() == {resolver_hash!r};\ntry: resolver.write_bytes(b'blocked'); raise SystemExit(5)\nexcept OSError: pass\nassert os.environ['HOME'] == {mount!r}; assert os.environ['CODEX_HOME'] == {mount + '/.codex'!r}; assert os.environ['CLAUDE_CONFIG_DIR'] == {mount + '/.claude'!r}; assert os.environ['GROK_HOME'] == {mount + '/.grok'!r}; Path.home().joinpath('.codex/session').write_text('saved'); Path('cwd-check').write_text('yes')"
        result = workspace._run(os.fspath(self.state), "dev", [sys.executable, "-c", code], read_only=False, profiles=[f"codex={profile}"], private_home_arg=os.fspath(private_home), mcp_socket=None)
        self.assertEqual(result, 0)
        self.assertEqual((profile / "session").read_text(encoding="utf-8"), "saved")
        self.assertEqual((work / "cwd-check").read_text(encoding="utf-8"), "yes")
        self.assertFalse((self.project / "cwd-check").exists())

    def test_antigravity_fails_before_spawn_without_verified_isolated_auth(self):
        (self.project / ".agents").mkdir()
        hook_source = self.project / ".agents/hooks.json"
        hook_source.write_text('{"hooks":{"startup":"must-remain-inert"}}', encoding="utf-8")
        work = self.prepare("agy")
        hook_copy = work / ".agents/hooks.json"
        marker = self.root / "agy-was-spawned"
        command = [sys.executable, "-c", f"from pathlib import Path; Path({str(marker)!r}).touch()"]
        with self.assertRaisesRegex(workspace.WorkspaceError, "no verified isolated authentication source"):
            workspace._run(
                os.fspath(self.state), "agy", command,
                read_only=False, profiles=[], private_home_arg=None,
                mcp_socket=None, agent_cli="agy",
            )
        self.assertFalse(marker.exists())
        self.assertEqual(hook_source.read_text(encoding="utf-8"), '{"hooks":{"startup":"must-remain-inert"}}')
        self.assertEqual(hook_copy.read_text(encoding="utf-8"), hook_source.read_text(encoding="utf-8"))

    @unittest.skipUnless(sys.platform.startswith("linux") and workspace.shutil.which("bwrap"),
                         "Linux bubblewrap is required")
    def test_opencode_uses_only_fixed_private_profile_environment(self):
        self.prepare("opencode")
        private_home = self.state / "agents" / workspace._project_id(self.project) / "opencode" / "profiles" / "home-opencode"
        private_home.mkdir(mode=0o700, parents=True, exist_ok=True)
        (private_home / ".config/opencode").mkdir(mode=0o700, parents=True, exist_ok=True)
        (private_home / ".opencode").mkdir(mode=0o700, exist_ok=True)
        for relative in (".local/share", ".local/state", ".cache"):
            (private_home / relative).mkdir(mode=0o700, parents=True, exist_ok=True)
        marker = b"node_modules\npackage.json\npackage-lock.json\nbun.lock\n.gitignore"
        (private_home / ".config/opencode/.gitignore").write_bytes(marker)
        os.chmod(private_home / ".config/opencode/.gitignore", 0o600)
        (private_home / ".opencode/.gitignore").write_bytes(marker)
        os.chmod(private_home / ".opencode/.gitignore", 0o600)
        config = private_home / "opencode.json"
        config.write_text('{"permission":{"*":"deny"},"mcp":{}}', encoding="utf-8")
        os.chmod(config, 0o600)
        code = f"""import os
from pathlib import Path
assert os.environ['HOME'] == '/run/user/{os.getuid()}/orgbot-home'
assert os.environ['OPENCODE_CONFIG'] == os.environ['HOME'] + '/opencode.json'
assert os.environ['OPENCODE_DISABLE_PROJECT_CONFIG'] == 'true'
assert 'OPENCODE_DISABLE_DEFAULT_PLUGINS' not in os.environ
assert os.environ['OPENCODE_PURE'] == 'true'
assert 'DBUS_SESSION_BUS_ADDRESS' not in os.environ
assert os.environ['XDG_STATE_HOME'] == os.environ['HOME'] + '/.local/state'
assert Path(os.environ['OPENCODE_CONFIG']).read_text() == '{{"permission":{{"*":"deny"}},"mcp":{{}}}}'
Path(os.environ['XDG_DATA_HOME'], 'writable.json').write_text('ok')
Path(os.environ['XDG_STATE_HOME'], 'writable.json').write_text('ok')
Path(os.environ['XDG_CACHE_HOME'], 'writable.json').write_text('ok')
for path in (Path(os.environ['OPENCODE_CONFIG_DIR']), Path(os.environ['HOME']) / '.opencode'):
    assert (path / '.gitignore').read_bytes() == b'node_modules\\npackage.json\\npackage-lock.json\\nbun.lock\\n.gitignore'
    try:
        (path / 'foreign.json').write_text('{{}}')
    except OSError:
        pass
    else:
        raise AssertionError(f'OpenCode config directory was writable: {{path}}')
"""
        with patch.dict(os.environ, {"DBUS_SESSION_BUS_ADDRESS": "unix:path=/tmp/host-bus"}):
            result = workspace._run(
                os.fspath(self.state), "opencode", [sys.executable, "-c", code],
                read_only=False, profiles=[], private_home_arg=os.fspath(private_home),
                mcp_socket=None, agent_cli="opencode",
            )
        self.assertEqual(result, 0)

    def test_project_lock_is_released_while_child_runs(self):
        self.prepare()
        lock_path = f"/run/user/{os.getuid()}/orgbot-workspace-locks/{workspace._project_id(self.project)}.lock"

        def verify_released(*_args, **_kwargs):
            fd = os.open(lock_path, os.O_RDWR)
            try:
                workspace.fcntl.flock(fd, workspace.fcntl.LOCK_EX | workspace.fcntl.LOCK_NB)
                workspace.fcntl.flock(fd, workspace.fcntl.LOCK_UN)
            finally:
                os.close(fd)
            return SimpleNamespace(returncode=0)

        with patch.object(workspace.subprocess, "run", side_effect=verify_released):
            result = workspace._run(os.fspath(self.state), "dev", ["/bin/true"], read_only=False, profiles=[], private_home_arg=None, mcp_socket=None)
        self.assertEqual(result, 0)

    def test_run_fails_closed_when_bubblewrap_is_unavailable(self):
        self.prepare()
        with patch.object(workspace.shutil, "which", return_value=None):
            with self.assertRaisesRegex(workspace.WorkspaceError, "bubblewrap is required"):
                workspace._run(os.fspath(self.state), "dev", ["/bin/true"], read_only=False, profiles=[], private_home_arg=None, mcp_socket=None)

    def test_only_explicit_room_socket_is_mounted_into_runtime_dir(self):
        work = self.prepare()
        socket_path = self.root / "room.sock"
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        listener.bind(os.fspath(socket_path))
        listener.listen(1)
        listener.settimeout(3)
        received = []

        def receive():
            connection, _ = listener.accept()
            with connection:
                received.append(connection.recv(32))

        thread = threading.Thread(target=receive, daemon=True)
        thread.start()
        endpoint = f"/run/user/{os.getuid()}/orgbot-room-mcp.sock"
        code = f"import socket; s=socket.socket(socket.AF_UNIX); s.connect({endpoint!r}); s.sendall(b'room-only'); s.close()"
        try:
            result = workspace._run(os.fspath(self.state), "dev", [sys.executable, "-c", code], read_only=False, profiles=[], private_home_arg=None, mcp_socket=os.fspath(socket_path))
            self.assertEqual(result, 0)
            thread.join(timeout=3)
            self.assertEqual(received, [b"room-only"])
        finally:
            listener.close()

    def test_agy_status_is_pending_without_creating_or_reading_host_secrets(self):
        xdg_data = self.root / "xdg-data"
        xdg_data.mkdir(mode=0o700)
        with patch.dict(os.environ, {"XDG_DATA_HOME": os.fspath(xdg_data)}), \
             patch.object(workspace, "_set_process_limits"), \
             patch.object(workspace, "_secret_item_paths_for_host", side_effect=AssertionError("host Secret Service queried")), \
             patch.object(workspace, "_agy_private_secret_service", side_effect=AssertionError("private store should not start")):
            result = workspace._agy_auth_status()
        self.assertEqual(result, {"ok": True, "credentials_present": False, "state": "pending_login"})
        self.assertFalse((xdg_data / "orgbot/agy").exists())

    def test_agy_login_requires_tty_before_secret_service_access(self):
        with patch.object(workspace, "_set_process_limits"), \
             patch.object(workspace.os, "isatty", return_value=False), \
             patch.object(workspace, "_secret_item_paths_for_host", side_effect=AssertionError("host Secret Service queried")):
            with self.assertRaisesRegex(workspace.WorkspaceError, "interactive terminal"):
                workspace._agy_auth_login()

    def test_agy_profile_uses_exact_deny_settings_and_rejects_mutation(self):
        xdg_data = self.root / "xdg-data"
        xdg_data.mkdir(mode=0o700)
        with patch.dict(os.environ, {"XDG_DATA_HOME": os.fspath(xdg_data)}):
            profile = workspace._agy_profile_paths(create=True)
            self.assertIsNotNone(profile)
            settings = json.loads(profile["settings"].read_text(encoding="utf-8"))
            self.assertFalse(settings["useG1Credits"])
            self.assertEqual(settings["permissions"]["deny"], [
                "command(*)", "unsandboxed(*)", "read_file(*)", "write_file(*)",
                "read_url(*)", "execute_url(*)", "mcp(*)",
            ])
            self.assertEqual(list(profile["plugins"].iterdir()), [])
            self.assertEqual(list((profile["project"] / ".agents").iterdir()), [])
            self.assertEqual(stat.S_IMODE(profile["settings"].stat().st_mode), 0o600)
            profile["settings"].write_text("{}", encoding="utf-8")
            with self.assertRaisesRegex(workspace.WorkspaceError, "unmanaged"):
                workspace._agy_profile_paths(create=True)

    def test_agy_wrapping_key_never_uses_ambiguous_or_missing_existing_secret(self):
        xdg_data = self.root / "xdg-data"
        xdg_data.mkdir(mode=0o700)
        with patch.dict(os.environ, {"XDG_DATA_HOME": os.fspath(xdg_data)}):
            profile = workspace._agy_profile_paths(create=True)
            self.assertIsNotNone(profile)
            with patch.object(workspace, "_secret_item_paths_for_host", return_value=["/a", "/b"]), \
                 patch.object(workspace, "_agy_keyring_has_data", return_value=False), \
                 patch.object(workspace, "_secret_tool") as secret_tool, \
                 self.assertRaisesRegex(workspace.WorkspaceError, "Multiple OrgBots"):
                workspace._host_wrapping_passphrase(profile)
            secret_tool.assert_not_called()
            with patch.object(workspace, "_secret_item_paths_for_host", return_value=[]), \
                 patch.object(workspace, "_agy_keyring_has_data", return_value=True), \
                 patch.object(workspace, "_secret_tool") as secret_tool, \
                 self.assertRaisesRegex(workspace.WorkspaceError, "refusing reset"):
                workspace._host_wrapping_passphrase(profile)
            secret_tool.assert_not_called()

    def test_agy_secret_item_creation_passes_generated_key_only_on_stdin(self):
        xdg_data = self.root / "xdg-data"
        xdg_data.mkdir(mode=0o700)
        with patch.dict(os.environ, {"XDG_DATA_HOME": os.fspath(xdg_data), "DBUS_SESSION_BUS_ADDRESS": "unix:path=/synthetic-bus"}):
            profile = workspace._agy_profile_paths(create=True)
            self.assertIsNotNone(profile)
            with patch.object(workspace, "_secret_item_paths_for_host", side_effect=([], ["/org/freedesktop/secrets/item/1"])), \
                 patch.object(workspace, "_agy_keyring_has_data", return_value=False), \
                 patch.object(workspace, "_secret_tool", return_value="/usr/bin/secret-tool"), \
                 patch.object(workspace, "_agy_run", return_value=subprocess.CompletedProcess([], 0)) as run:
                value = workspace._host_wrapping_passphrase(profile)
            self.assertGreaterEqual(len(value), 40)
            args, kwargs = run.call_args
            self.assertNotIn(value.decode(), " ".join(args[0]))
            self.assertIn("DBUS_SESSION_BUS_ADDRESS", kwargs["env"])
            self.assertNotIn(value.decode(), " ".join(f"{key}={item}" for key, item in kwargs["env"].items()))
            self.assertEqual(kwargs["input"].rstrip(b"\n"), bytes(value))
            workspace._zero_bytes(value)

    def test_agy_sandbox_mounts_only_private_home_project_config_and_bus(self):
        xdg_data = self.root / "xdg-data"
        xdg_data.mkdir(mode=0o700)
        with patch.dict(os.environ, {"XDG_DATA_HOME": os.fspath(xdg_data)}):
            profile = workspace._agy_profile_paths(create=True)
            self.assertIsNotNone(profile)
            runtime = self.root / "runtime"
            runtime.mkdir(mode=0o700)
            (runtime / "bus.sock").touch()
            binary = self.root / "agy"
            binary.write_text("synthetic binary", encoding="utf-8")
            args = workspace._agy_sandbox_args(profile, binary, runtime)
        rendered = " ".join(args)
        self.assertIn("--tmpfs /", rendered)
        self.assertIn("--ro-bind " + os.fspath(profile["home"]) + " /home/orgbot", rendered)
        self.assertIn("--bind " + os.fspath(profile["data"]) + " /home/orgbot/.local/share", rendered)
        self.assertIn("--bind " + os.fspath(profile["cli_config"]) + " /home/orgbot/.gemini/antigravity-cli", rendered)
        self.assertIn("--ro-bind " + os.fspath(profile["settings"]) + " /home/orgbot/.gemini/antigravity-cli/settings.json", rendered)
        self.assertNotIn("--bind " + os.fspath(profile["home"]) + " /home/orgbot", rendered)
        self.assertIn("--ro-bind " + os.fspath(profile["global_config"]) + " /home/orgbot/.gemini/config", rendered)
        self.assertIn("--ro-bind " + os.fspath(profile["project"]) + " /tmp/orgbot-workspace", rendered)
        self.assertIn("--ro-bind " + os.fspath(runtime / "bus.sock"), rendered)
        self.assertNotIn(os.fspath(profile["keyring_data"]), rendered)
        self.assertNotIn(os.fspath(profile["service_home"]), rendered)
        self.assertNotIn("--ro-bind /run ", rendered)
        self.assertNotIn("--ro-bind /home ", rendered)
        self.assertIn("--clearenv", args)
        self.assertIn("/bin/false", args)

    @unittest.skipUnless(sys.platform.startswith("linux") and workspace.shutil.which("bwrap"),
                         "Linux bubblewrap is required")
    def test_agy_sandbox_prevents_settings_directory_rename_and_settings_write(self):
        xdg_data = self.root / "xdg-data"
        xdg_data.mkdir(mode=0o700)
        with patch.dict(os.environ, {"XDG_DATA_HOME": os.fspath(xdg_data)}):
            profile = workspace._agy_profile_paths(create=True)
            runtime = self.root / "runtime"
            runtime.mkdir(mode=0o700)
            (runtime / "bus.sock").touch()
            binary = self.root / "agy-probe"
            binary.write_text(
                "#!/bin/sh\n"
                "if mv /home/orgbot/.gemini /home/orgbot/.gemini-moved 2>/dev/null; then\n"
                "  echo gemini-rename-allowed\n"
                "else echo gemini-rename-blocked; fi\n"
                "if mv /home/orgbot/.gemini/antigravity-cli /home/orgbot/.gemini/antigravity-cli-moved 2>/dev/null; then\n"
                "  echo rename-allowed\n"
                "else echo rename-blocked; fi\n"
                "if printf changed > /home/orgbot/.gemini/antigravity-cli/settings.json 2>/dev/null; then\n"
                "  echo settings-write-allowed\n"
                "else echo settings-write-blocked; fi\n"
                "printf synthetic-log > /home/orgbot/.gemini/antigravity-cli/cli.log 2>/dev/null && echo cli-log-write-allowed\n",
                encoding="utf-8",
            )
            os.chmod(binary, 0o700)
            args = workspace._agy_sandbox_args(profile, binary, runtime)
        result = subprocess.run(
            args,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env={"PATH": "/usr/local/bin:/usr/bin:/bin", "LANG": "C.UTF-8", "PWD": "/tmp/orgbot-workspace"},
            timeout=15,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr[-1000:])
        self.assertEqual(result.stdout.splitlines(), [
            "gemini-rename-blocked", "rename-blocked", "settings-write-blocked", "cli-log-write-allowed",
        ])
        self.assertTrue((profile["home"] / ".gemini/antigravity-cli").is_dir())
        self.assertEqual(json.loads(profile["settings"].read_text(encoding="utf-8"))["useG1Credits"], False)
        self.assertEqual((profile["cli_config"] / "cli.log").read_text(encoding="utf-8"), "synthetic-log")

    def test_agy_partial_store_fails_without_reset(self):
        xdg_data = self.root / "xdg-data"
        root = xdg_data / "orgbot/agy"
        root.mkdir(parents=True, mode=0o700)
        (root / "keyring-data").mkdir(mode=0o700)
        with patch.dict(os.environ, {"XDG_DATA_HOME": os.fspath(xdg_data)}):
            with self.assertRaisesRegex(workspace.WorkspaceError, "incomplete; refusing reset"):
                workspace._agy_profile_paths(create=False)
            with self.assertRaisesRegex(workspace.WorkspaceError, "incomplete; refusing reset"):
                workspace._agy_profile_paths(create=True)
            self.assertEqual({path.name for path in root.iterdir()}, {"keyring-data"})

    def test_agy_auth_lock_contention_fails_immediately(self):
        xdg_data = self.root / "xdg-data"
        xdg_data.mkdir(mode=0o700)
        with patch.dict(os.environ, {"XDG_DATA_HOME": os.fspath(xdg_data)}):
            profile = workspace._agy_profile_paths(create=True)
            first = workspace._agy_auth_lock(profile["root"])
            try:
                with self.assertRaisesRegex(workspace.WorkspaceError, "already running"):
                    workspace._agy_auth_lock(profile["root"])
            finally:
                os.close(first)

    def test_agy_secret_service_rejects_malformed_success_reply(self):
        valid = (
            b'method return time=0.000 sender=:1.1 -> dest=:1.2 reply_serial=3\n'
            b'   array [\n'
            b'      object path "/org/freedesktop/secrets/collection/login/1"\n'
            b'   ]\n'
            b'   array [\n'
            b'      object path "/org/freedesktop/secrets/collection/login/2"\n'
            b'   ]\n'
        )
        with patch.object(workspace, "_agy_run", return_value=subprocess.CompletedProcess([], 0, valid, b"")):
            self.assertEqual(workspace._secret_item_paths("unix:path=/private/bus", {}), [
                "/org/freedesktop/secrets/collection/login/1",
                "/org/freedesktop/secrets/collection/login/2",
            ])
        malformed = b"method return time=0 sender=:1.1 -> dest=:1.2 reply_serial=3\nmetadata missing\n"
        with patch.object(workspace, "_agy_run", return_value=subprocess.CompletedProcess([], 0, malformed, b"")):
            with self.assertRaisesRegex(workspace.WorkspaceError, "malformed metadata"):
                workspace._secret_item_paths("unix:path=/private/bus", {})

    def test_agy_run_routes_input_to_pipe_and_rejects_stdin_conflict(self):
        result = workspace._agy_run(
            [sys.executable, "-c", "import sys; print(sys.stdin.buffer.read().decode(), end='')"],
            timeout=3,
            input=b"synthetic-stdin-only",
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, b"synthetic-stdin-only")
        with patch.object(workspace, "_agy_owned_popen") as popen:
            with self.assertRaisesRegex(ValueError, "stdin and input"):
                workspace._agy_run(
                    ["/bin/true"], timeout=1, input=b"synthetic", stdin=subprocess.DEVNULL,
                )
            popen.assert_not_called()

    def test_agy_existing_host_wrapping_key_lookup_fails_closed(self):
        xdg_data = self.root / "xdg-data"
        xdg_data.mkdir(mode=0o700)
        with patch.dict(os.environ, {"XDG_DATA_HOME": os.fspath(xdg_data), "DBUS_SESSION_BUS_ADDRESS": "unix:path=/synthetic-host-bus"}):
            profile = workspace._agy_profile_paths(create=True)
            for stdout, code in ((b"", 1), (b"", 0), (b"synthetic-key\nextra\n", 0), (b"not a token!\n", 0)):
                with self.subTest(stdout=stdout, code=code), \
                     patch.object(workspace, "_secret_item_paths_for_host", return_value=["/org/freedesktop/secrets/item/1"]), \
                     patch.object(workspace, "_agy_keyring_has_data", return_value=False), \
                     patch.object(workspace, "_secret_tool", return_value="/usr/bin/secret-tool"), \
                     patch.object(workspace, "_agy_run", return_value=subprocess.CompletedProcess([], code, stdout, b"")):
                    with self.assertRaisesRegex(workspace.WorkspaceError, "locked or invalid"):
                        workspace._host_wrapping_passphrase(profile)

    def test_agy_private_profile_rejects_symlink_ancestors_and_hardlinked_files(self):
        xdg_data = self.root / "real-xdg-data"
        xdg_data.mkdir(mode=0o700)
        alias = self.root / "xdg-alias"
        alias.symlink_to(xdg_data, target_is_directory=True)
        with patch.dict(os.environ, {"XDG_DATA_HOME": os.fspath(alias)}):
            with self.assertRaisesRegex(workspace.WorkspaceError, "Unsafe directory component"):
                workspace._agy_profile_paths(create=True)
        with patch.dict(os.environ, {"XDG_DATA_HOME": os.fspath(xdg_data)}):
            profile = workspace._agy_profile_paths(create=True)
            hardlink = self.root / "settings-copy"
            os.link(profile["settings"], hardlink)
            with self.assertRaisesRegex(workspace.WorkspaceError, "file is unsafe"):
                workspace._agy_profile_paths(create=False)

    @unittest.skipUnless(sys.platform.startswith("linux"), "Linux parent-death signal is required")
    def test_agy_sigterm_cancels_and_reaps_owned_child(self):
        child = workspace._agy_owned_popen(["/bin/sleep", "30"], stdin=subprocess.DEVNULL,
                                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            with self.assertRaises(workspace._AgyAuthCancelled):
                with workspace._agy_signal_scope():
                    os.kill(os.getpid(), signal.SIGTERM)
                    time.sleep(0.1)
        finally:
            workspace._stop_owned_process(child)
        self.assertIsNotNone(child.poll())
        self.assertNotIn(child, workspace._AGY_CHILDREN)

    @unittest.skipUnless(sys.platform.startswith("linux"), "Linux parent-death signal is required")
    def test_agy_owned_child_stops_when_synthetic_parent_dies(self):
        pidfile = self.root / "owned-child.pid"
        code = (
            "import importlib.util, os, sys; "
            f"spec=importlib.util.spec_from_file_location('orgbot_workspace', {str(MODULE_PATH)!r}); "
            "m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m); "
            "p=m._agy_owned_popen(['/bin/sleep','30'], stdin=m.subprocess.DEVNULL, "
            "stdout=m.subprocess.DEVNULL, stderr=m.subprocess.DEVNULL); "
            f"open({str(pidfile)!r},'w').write(str(p.pid)); os._exit(0)"
        )
        parent = subprocess.Popen([sys.executable, "-B", "-c", code], stdin=subprocess.DEVNULL,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.assertEqual(parent.wait(timeout=5), 0)
        child_pid = int(pidfile.read_text(encoding="ascii"))

        def child_running():
            try:
                state = Path(f"/proc/{child_pid}/stat").read_text(encoding="ascii").split()[2]
                return state != "Z"
            except FileNotFoundError:
                return False

        deadline = time.monotonic() + 3
        while time.monotonic() < deadline and child_running():
            time.sleep(0.05)
        self.assertFalse(child_running(), "synthetic owned child survived its parent")

    @unittest.skipUnless(sys.platform.startswith("linux") and workspace.shutil.which("dbus-daemon") and
                         workspace.shutil.which("gnome-keyring-daemon") and workspace.shutil.which("secret-tool") and
                         workspace.shutil.which("dbus-send"), "Linux Secret Service tools are required")
    def test_agy_private_keyring_persists_synthetic_item_across_daemon_restart(self):
        xdg_data = self.root / "xdg-data"
        xdg_data.mkdir(mode=0o700)
        with patch.dict(os.environ, {"XDG_DATA_HOME": os.fspath(xdg_data)}):
            profile = workspace._agy_profile_paths(create=True)
            self.assertIsNotNone(profile)
            passphrase = bytearray(b"synthetic-orgbots-private-keyring-passphrase")
            env = None
            try:
                with workspace._agy_private_secret_service(profile, passphrase) as (env, _runtime):
                    stored = workspace._agy_run(
                        ["/usr/bin/secret-tool", "store", "--label=OrgBots synthetic test",
                         "service", "orgbots", "purpose", "agy-test", "version", "1"],
                        input=b"synthetic-only\n", stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                        env=env, timeout=5,
                    )
                    self.assertEqual(stored.returncode, 0, stored.stderr.decode(errors="replace"))
                    paths = workspace._secret_item_paths(env["DBUS_SESSION_BUS_ADDRESS"], {})
                    self.assertEqual(len(paths), 1)
                with workspace._agy_private_secret_service(profile, passphrase) as (env, _runtime):
                    readback = workspace._agy_run(
                        ["/usr/bin/secret-tool", "lookup", "service", "orgbots", "purpose", "agy-test", "version", "1"],
                        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                        env=env, timeout=5,
                    )
                    self.assertEqual(readback.returncode, 0, readback.stderr.decode(errors="replace"))
                    self.assertEqual(readback.stdout.strip(), b"synthetic-only")
                    self.assertEqual(len(workspace._secret_item_paths(env["DBUS_SESSION_BUS_ADDRESS"], {})), 1)
            finally:
                workspace._zero_bytes(passphrase)
            self.assertTrue(workspace._agy_keyring_has_data(profile))


if __name__ == "__main__":
    unittest.main()
