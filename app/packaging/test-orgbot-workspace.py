import contextlib
import io
import importlib.util
import json
import os
import socket
import stat
import subprocess
import sys
import tempfile
import threading
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
        code = f"from pathlib import Path; import os; assert os.getcwd() == '/tmp/orgbot-workspace'; assert os.environ['HOME'] == {mount!r}; assert os.environ['CODEX_HOME'] == {mount + '/.codex'!r}; assert os.environ['CLAUDE_CONFIG_DIR'] == {mount + '/.claude'!r}; assert os.environ['GROK_HOME'] == {mount + '/.grok'!r}; Path.home().joinpath('.codex/session').write_text('saved'); Path('cwd-check').write_text('yes')"
        result = workspace._run(os.fspath(self.state), "dev", [sys.executable, "-c", code], read_only=False, profiles=[f"codex={profile}"], private_home_arg=os.fspath(private_home), mcp_socket=None)
        self.assertEqual(result, 0)
        self.assertEqual((profile / "session").read_text(encoding="utf-8"), "saved")
        self.assertEqual((work / "cwd-check").read_text(encoding="utf-8"), "yes")
        self.assertFalse((self.project / "cwd-check").exists())

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


if __name__ == "__main__":
    unittest.main()
