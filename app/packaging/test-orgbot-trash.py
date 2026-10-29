#!/usr/bin/env python3
import importlib.util
import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch


MODULE_PATH = Path(__file__).with_name("orgbot-trash.py")
SPEC = importlib.util.spec_from_file_location("orgbot_trash", MODULE_PATH)
trash = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(trash)


class TrashTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="orgbot-trash-test-")
        self.base = Path(self.tmp.name)
        self.root = trash._prepare_root(self.base / "trash", require_mount=False)
        self.sources = self.base / "sources"
        self.sources.mkdir(mode=0o700)

    def tearDown(self):
        self.tmp.cleanup()

    def test_move_list_restore_and_no_overwrite(self):
        source = self.sources / "project-data"
        source.mkdir()
        (source / "note.txt").write_text("retained data", encoding="utf-8")
        entry_id = trash.move(self.root, "Demo Project", source)
        self.assertFalse(source.exists())
        row = trash.list_entries(self.root)[0]
        self.assertEqual(row["id"], entry_id)
        self.assertEqual(row["status"], "complete")
        self.assertEqual(row["project"], "Demo Project")
        with self.assertRaisesRegex(trash.TrashError, "already exists"):
            source.mkdir()
            trash.restore(self.root, entry_id)
        source.rmdir()
        trash.restore(self.root, entry_id)
        self.assertEqual((source / "note.txt").read_text(encoding="utf-8"), "retained data")
        self.assertEqual(trash.list_entries(self.root), [])

    def test_move_rejects_symlink_and_protected_roots(self):
        target = self.sources / "target"
        target.write_text("keep", encoding="utf-8")
        link = self.sources / "link"
        link.symlink_to(target)
        with self.assertRaisesRegex(trash.TrashError, "Symlink"):
            trash.move(self.root, "Demo", link)
        with self.assertRaisesRegex(trash.TrashError, "protected"):
            trash._reject_overlap(Path("/mnt/storage"), trash.TRASH_ROOT)
        with self.assertRaisesRegex(trash.TrashError, "overlapping"):
            trash._reject_overlap(trash.TRASH_ROOT / "child", trash.TRASH_ROOT)
        self.assertEqual(target.read_text(encoding="utf-8"), "keep")

    def test_expected_sha256_rejects_replaced_source_before_removal(self):
        source = self.sources / "reviewed.txt"
        source.write_text("owner replacement", encoding="utf-8")
        reviewed = trash.hashlib.sha256(b"agent reviewed bytes").hexdigest()
        with self.assertRaisesRegex(trash.TrashError, "does not match the reviewed SHA-256"):
            trash.move(self.root, "Demo", source, expected_sha256=reviewed)
        self.assertEqual(source.read_text(encoding="utf-8"), "owner replacement")
        self.assertEqual(trash.list_entries(self.root), [])

    def test_default_local_root_roundtrip_without_hdd(self):
        config = self.base / "config"
        data = self.base / "data"
        config.mkdir(mode=0o700)
        with patch.dict(os.environ, {"HOME": str(self.base), "XDG_CONFIG_HOME": str(config), "XDG_DATA_HOME": str(data)}):
            result = trash._config_result()
            self.assertEqual(result["root"], str(data / "orgbots/trash"))
            self.assertEqual(result["source"], "default")
            self.assertFalse(result["requires_selection"])
            root = trash._prepare_root()
            self.assertEqual(root, data / "orgbots/trash")
            source = self.sources / "default.txt"
            source.write_text("local", encoding="utf-8")
            entry = trash.move(root, "Demo", source)
            trash.restore(root, entry)
            self.assertEqual(source.read_text(encoding="utf-8"), "local")

    def test_first_cli_mutation_persists_default_selection(self):
        config = self.base / "config"
        data = self.base / "data"
        config.mkdir(mode=0o700)
        source = self.sources / "first-mutation.txt"
        source.write_text("pin the default", encoding="utf-8")
        with patch.dict(os.environ, {"HOME": str(self.base), "XDG_CONFIG_HOME": str(config), "XDG_DATA_HOME": str(data)}), patch.object(sys, "argv", [str(MODULE_PATH), "move", "--project", "Demo", str(source)]), redirect_stdout(io.StringIO()):
            self.assertEqual(trash._cli(), 0)
        payload = json.loads((config / "orgbots/trash.json").read_text(encoding="utf-8"))
        self.assertEqual(payload["root"], str(data / "orgbots/trash"))
        self.assertEqual(len(trash.list_entries(data / "orgbots/trash")), 1)

    def test_a_bot_trashes_from_its_working_copy_without_the_owners_trash(self):
        # From the live room: Sol could not remove a dead workflow file, because
        # the owner's trash is out of reach inside a bot's sandbox. The bot only
        # edits a copy; the original waits for the owner's approval anyway.
        workspace = self.base / "orgbot-workspace"
        (workspace / ".github/workflows").mkdir(parents=True)
        (workspace / ".github/workflows/pages.yml").write_text("on: push", encoding="utf-8")
        outside = self.sources / "owner-file.txt"
        outside.write_text("not the bot's", encoding="utf-8")
        data = self.base / "private-home-data"
        env = {"HOME": str(self.base), "XDG_CONFIG_HOME": str(self.base / "no-config"), "XDG_DATA_HOME": str(data), "ORGBOT_WORKSPACE": str(workspace)}
        out = io.StringIO()
        with patch.dict(os.environ, env), patch.object(os, "getcwd", return_value=str(workspace)), patch.object(sys, "argv", [str(MODULE_PATH), "move", "--project", "Protest", str(workspace / ".github/workflows/pages.yml")]), redirect_stdout(out):
            self.assertEqual(trash._cli(), 0)
        self.assertFalse((workspace / ".github/workflows/pages.yml").exists())
        kept = list((data / "orgbots/working-copy-trash").rglob("pages.yml"))
        self.assertEqual(len(kept), 1)
        self.assertEqual(kept[0].read_text(encoding="utf-8"), "on: push")
        self.assertIn("until the owner approves", out.getvalue())
        self.assertFalse((self.base / "no-config/orgbots/trash.json").exists(), "the owner's trash selection is untouched")

        # Asking about storage from a working copy says none is needed.
        out = io.StringIO()
        with patch.dict(os.environ, env), patch.object(sys, "argv", [str(MODULE_PATH), "config"]), redirect_stdout(out):
            self.assertEqual(trash._cli(), 0)
        self.assertEqual(json.loads(out.getvalue())["requires_selection"], False)
        with patch.dict(os.environ, env), patch.object(sys, "argv", [str(MODULE_PATH), "configure", "--root", "/mnt/hdd/trash"]), redirect_stdout(io.StringIO()), patch("sys.stderr", io.StringIO()):
            self.assertEqual(trash._cli(), 2)
        self.assertFalse((self.base / "no-config/orgbots/trash.json").exists())

        # Anything outside the working copy still goes through the owner's
        # trash rules, which a sandbox cannot satisfy: it is refused, not moved.
        with patch.dict(os.environ, env), patch.object(sys, "argv", [str(MODULE_PATH), "move", "--project", "Protest", str(outside), str(workspace / "missing")]), redirect_stdout(io.StringIO()), patch("sys.stderr", io.StringIO()):
            self.assertEqual(trash._cli(), 2)
        self.assertTrue(outside.exists())

    def test_config_switch_waits_for_shared_root_lock(self):
        import threading

        config = self.base / "config"
        config.mkdir(mode=0o700)
        data = self.base / "data"
        target = self.base / "other-root"
        target.mkdir(mode=0o700)
        env = {"HOME": str(self.base), "XDG_CONFIG_HOME": str(config), "XDG_DATA_HOME": str(data)}
        with patch.dict(os.environ, env):
            trash._prepare_root()
            entered = threading.Event()
            release = threading.Event()
            switched = threading.Event()
            errors = []

            def hold_selected_root():
                try:
                    with trash._selected_root_locked():
                        entered.set()
                        if not release.wait(timeout=3):
                            raise AssertionError("test did not release shared root lock")
                except BaseException as exc:
                    errors.append(exc)

            def switch_root():
                try:
                    trash._configure_root(str(target / "trash"))
                    switched.set()
                except BaseException as exc:
                    errors.append(exc)

            holder = threading.Thread(target=hold_selected_root)
            holder.start()
            self.assertTrue(entered.wait(timeout=3))
            switcher = threading.Thread(target=switch_root)
            switcher.start()
            self.assertFalse(switched.wait(timeout=0.1), "root switch bypassed shared root lock")
            release.set()
            holder.join(timeout=3)
            switcher.join(timeout=3)
            self.assertFalse(holder.is_alive())
            self.assertFalse(switcher.is_alive())
            self.assertFalse(errors, errors)
            self.assertTrue(switched.is_set())
            self.assertEqual(trash._selected_config()[0], target / "trash")

    def test_custom_root_is_pinned_and_persists(self):
        config = self.base / "config"
        config.mkdir(mode=0o700)
        mount = self.base / "custom-mount"
        mount.mkdir(mode=0o700)
        with patch.dict(os.environ, {"HOME": str(self.base), "XDG_CONFIG_HOME": str(config), "XDG_DATA_HOME": str(self.base / "data")}):
            result = trash._configure_root(str(mount / "trash"))
            self.assertEqual(result["root"], str(mount / "trash"))
            self.assertEqual(result["source"], "configured")
            payload = json.loads((config / "orgbots/trash.json").read_text())
            self.assertEqual(set(payload["identity"]), {"mountpoint", "filesystem", "fsid", "device"})
            root = trash._prepare_root()
            self.assertEqual(root, mount / "trash")
            source = self.sources / "custom.txt"
            source.write_text("external", encoding="utf-8")
            entry = trash.move(root, "Demo", source)
            trash.restore(root, entry)
            self.assertEqual(source.read_text(encoding="utf-8"), "external")

    def test_cli_config_and_configure_emit_json_contract(self):
        config = self.base / "config"
        config.mkdir(mode=0o700)
        target = self.base / "selected"
        target.mkdir(mode=0o700)
        with patch.dict(os.environ, {"HOME": str(self.base), "XDG_CONFIG_HOME": str(config), "XDG_DATA_HOME": str(self.base / "data")}):
            output = io.StringIO()
            with patch.object(sys, "argv", [str(MODULE_PATH), "config"]), redirect_stdout(output):
                self.assertEqual(trash._cli(), 0)
            selected = json.loads(output.getvalue())
            self.assertEqual(set(selected), {"root", "source", "requires_selection"})
            self.assertIsInstance(selected["root"], str)
            output = io.StringIO()
            with patch.object(sys, "argv", [str(MODULE_PATH), "configure", "--root", str(target / "trash")]), redirect_stdout(output):
                self.assertEqual(trash._cli(), 0)
            configured = json.loads(output.getvalue())
            self.assertEqual(configured, {"root": str(target / "trash"), "source": "configured", "requires_selection": False})

    def test_config_symlink_and_corrupt_config_fail_closed(self):
        config = self.base / "config"
        config.mkdir(mode=0o700)
        with patch.dict(os.environ, {"HOME": str(self.base), "XDG_CONFIG_HOME": str(config)}):
            orgbots = config / "orgbots"
            orgbots.mkdir(mode=0o700)
            path = orgbots / "trash.json"
            path.write_text("not-json", encoding="utf-8")
            path.chmod(0o600)
            with self.assertRaisesRegex(trash.TrashError, "Cannot read trash config"):
                trash._selected_config()
            path.unlink()
            path.symlink_to(self.base / "missing")
            with self.assertRaisesRegex(trash.TrashError, "Cannot read trash config safely"):
                trash._selected_config()

    def test_symlinked_trash_root_ancestor_is_rejected(self):
        config = self.base / "config"
        config.mkdir(mode=0o700)
        real = self.base / "real"
        real.mkdir(mode=0o700)
        link = self.base / "link"
        link.symlink_to(real, target_is_directory=True)
        with patch.dict(os.environ, {"HOME": str(self.base), "XDG_CONFIG_HOME": str(config)}):
            with self.assertRaisesRegex(trash.TrashError, "symlink component"):
                trash._configure_root(str(link / "trash"))

    def test_offline_pinned_filesystem_fails_without_fallback(self):
        config = self.base / "config"
        config.mkdir(mode=0o700)
        root_parent = self.base / "disk"
        root_parent.mkdir(mode=0o700)
        with patch.dict(os.environ, {"HOME": str(self.base), "XDG_CONFIG_HOME": str(config)}):
            trash._configure_root(str(root_parent / "trash"))
            payload = json.loads((config / "orgbots/trash.json").read_text())
            payload["identity"]["fsid"] += 1
            (config / "orgbots/trash.json").write_text(json.dumps(payload) + "\n", encoding="utf-8")
            (config / "orgbots/trash.json").chmod(0o600)
            with self.assertRaisesRegex(trash.TrashError, "identity changed"):
                trash._prepare_root()
            self.assertEqual(trash._selected_config()[0], root_parent / "trash")

    def test_legacy_install_selection_preserves_hdd_even_when_offline(self):
        config = self.base / "config"
        config.mkdir(mode=0o700)
        legacy = self.base / ".local/lib/orgbot"
        legacy.mkdir(parents=True)
        (legacy / "orgbot-trash.py").write_text("old helper", encoding="utf-8")
        hdd = self.base / "hdd"
        old_root = hdd / "OrgBots Trash"
        old_root.mkdir(parents=True, mode=0o700)
        old_entry = old_root / "projects/old-entry/payload"
        old_entry.parent.mkdir(parents=True, mode=0o700)
        old_entry.write_text("keep", encoding="utf-8")
        with patch.dict(os.environ, {"HOME": str(self.base), "XDG_CONFIG_HOME": str(config)}), patch.object(trash, "LEGACY_POLICY", self.base / "policy"), patch.object(trash, "HDD_MOUNT", hdd), patch.object(trash, "LEGACY_TRASH_ROOT", old_root), patch.object(trash.os.path, "ismount", return_value=False):
            selected, source, requires_selection, _ = trash._selected_config()
            self.assertEqual(selected, trash.LEGACY_TRASH_ROOT)
            self.assertEqual(source, "legacy-hdd")
            self.assertFalse(requires_selection)
            with self.assertRaisesRegex(trash.TrashError, "unavailable"):
                trash._prepare_root()
            self.assertEqual(old_entry.read_text(encoding="utf-8"), "keep")

    def test_policy_marker_without_legacy_requires_selection(self):
        config = self.base / "config"
        config.mkdir(mode=0o700)
        marker = self.base / "legacy-policy"
        marker.touch()
        with patch.dict(os.environ, {"HOME": str(self.base), "XDG_CONFIG_HOME": str(config)}), patch.object(trash, "LEGACY_POLICY", marker):
            result = trash._config_result()
            self.assertEqual(result["source"], "policy-requires-selection")
            self.assertTrue(result["requires_selection"])
            with self.assertRaisesRegex(trash.TrashError, "requires explicit owner selection"):
                trash._prepare_root()

    def test_policy_marker_can_explicitly_select_default_when_data_parent_is_missing(self):
        config = self.base / "config"
        config.mkdir(mode=0o700)
        data = self.base / "not-created-yet"
        marker = self.base / "legacy-policy"
        marker.touch()
        with patch.dict(os.environ, {"HOME": str(self.base), "XDG_CONFIG_HOME": str(config), "XDG_DATA_HOME": str(data)}), patch.object(trash, "LEGACY_POLICY", marker):
            result = trash._configure_root(str(data / "orgbots/trash"))
            self.assertEqual(result["root"], str(data / "orgbots/trash"))
            self.assertFalse(result["requires_selection"])
            self.assertTrue((data / "orgbots/trash/projects").is_dir())

    def test_nonempty_current_root_denies_switch(self):
        config = self.base / "config"
        config.mkdir(mode=0o700)
        data = self.base / "data"
        data.mkdir(mode=0o700)
        with patch.dict(os.environ, {"HOME": str(self.base), "XDG_CONFIG_HOME": str(config), "XDG_DATA_HOME": str(data)}):
            root = trash._prepare_root()
            source = self.sources / "retained.txt"
            source.write_text("retain", encoding="utf-8")
            trash.move(root, "Demo", source)
            target = self.base / "other"
            target.mkdir(mode=0o700)
            with self.assertRaisesRegex(trash.TrashError, "contains entries"):
                trash._configure_root(str(target / "trash"))

    def test_systemd_dropin_has_exact_escaped_path(self):
        config = self.base / "config"
        config.mkdir(mode=0o700)
        unit_dir = config / "systemd/user"
        unit_dir.mkdir(parents=True)
        (unit_dir / "orgbot-trash-cleanup.service").write_text("[Service]\n", encoding="utf-8")
        mount = self.base / 'drive "x%y'
        mount.mkdir(mode=0o700)
        manager_env = trash.subprocess.CompletedProcess([], 0, stdout=f"XDG_CONFIG_HOME={config}\n".encode())
        daemon_reload = trash.subprocess.CompletedProcess([], 0)
        with patch.dict(os.environ, {"HOME": str(self.base), "XDG_CONFIG_HOME": str(config)}), patch.object(trash, "_systemctl_executable", return_value=Path("/usr/bin/systemctl")), patch.object(trash.subprocess, "run", side_effect=[manager_env, daemon_reload]) as run:
            trash._sync_systemd(mount)
        dropin = unit_dir / "orgbot-trash-cleanup.service.d/20-trash-root.conf"
        expected = str(mount).replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%').replace(' ', '\\x20')
        contents = dropin.read_text(encoding="utf-8")
        self.assertIn("# Managed by orgbot-trash", contents)
        self.assertIn('Environment=' + trash._systemd_escape_path(f"XDG_CONFIG_HOME={config}") + " " + trash._systemd_escape_path(f"XDG_DATA_HOME={self.base / '.local/share'}"), contents)
        self.assertIn('ReadWritePaths="' + expected + '"', contents)
        self.assertIn('ExecStart=/usr/bin/python3 ' + trash._systemd_escape_path(str(MODULE_PATH.resolve())) + ' cleanup', contents)
        self.assertEqual(run.call_args_list[0].args[0], ["/usr/bin/systemctl", "--user", "show-environment"])
        self.assertEqual(run.call_args_list[1].args[0], ["/usr/bin/systemctl", "--user", "daemon-reload"])
        self.assertEqual(run.call_args_list[1].kwargs["timeout"], 10)

    def test_manager_xdg_mismatch_rejects_config_switch_before_timer_changes(self):
        config = self.base / "config"
        config.mkdir(mode=0o700)
        unit_dir = config / "systemd/user"
        unit_dir.mkdir(parents=True)
        (unit_dir / "orgbot-trash-cleanup.service").write_text("[Service]\n", encoding="utf-8")
        target = self.base / "new-root"
        target.mkdir(mode=0o700)
        manager_env = trash.subprocess.CompletedProcess([], 0, stdout=b"XDG_CONFIG_HOME=/manager/config\n")
        with patch.dict(os.environ, {"HOME": str(self.base), "XDG_CONFIG_HOME": str(config)}), patch.object(trash, "_systemctl_executable", return_value=Path("/usr/bin/systemctl")), patch.object(trash.subprocess, "run", return_value=manager_env):
            with self.assertRaisesRegex(trash.TrashError, "align XDG_CONFIG_HOME"):
                trash._configure_root(str(target / "trash"))
        self.assertFalse((config / "orgbots/trash.json").exists())
        self.assertFalse((unit_dir / "orgbot-trash-cleanup.service.d/20-trash-root.conf").exists())

    def test_failed_timer_sync_rolls_back_config_and_dropin(self):
        config = self.base / "config"
        config.mkdir(mode=0o700)
        data = self.base / "data"
        data.mkdir(mode=0o700)
        unit_dir = config / "systemd/user"
        unit_dir.mkdir(parents=True)
        (unit_dir / "orgbot-trash-cleanup.service").write_text("[Service]\n", encoding="utf-8")
        dropin_dir = unit_dir / "orgbot-trash-cleanup.service.d"
        dropin_dir.mkdir(mode=0o700)
        old_dropin = dropin_dir / "20-trash-root.conf"
        old_content = b"[Service]\n# Managed by orgbot-trash\nReadWritePaths=\nReadWritePaths=\"/old\"\n"
        old_dropin.write_bytes(old_content)
        old_dropin.chmod(0o600)
        target = self.base / "new-disk"
        target.mkdir(mode=0o700)
        manager_env = trash.subprocess.CompletedProcess([], 0, stdout=f"XDG_CONFIG_HOME={config}\n".encode())
        failed_reload = trash.subprocess.CompletedProcess([], 1)
        with patch.dict(os.environ, {"HOME": str(self.base), "XDG_CONFIG_HOME": str(config), "XDG_DATA_HOME": str(data)}), patch.object(trash, "_systemctl_executable", return_value=Path("/usr/bin/systemctl")), patch.object(trash.subprocess, "run", side_effect=[manager_env, failed_reload, failed_reload]):
            with self.assertRaisesRegex(trash.TrashError, "systemd rejected"):
                trash._configure_root(str(target / "trash"))
        self.assertFalse((config / "orgbots/trash.json").exists())
        self.assertEqual(old_dropin.read_bytes(), old_content)

    def test_failed_reload_hides_uncommitted_root_from_concurrent_move(self):
        import threading

        config = self.base / "config"
        config.mkdir(mode=0o700)
        old_root = self.base / "old-root"
        new_root = self.base / "new-root"
        old_root.mkdir(mode=0o700)
        new_root.mkdir(mode=0o700)
        env = {"HOME": str(self.base), "XDG_CONFIG_HOME": str(config), "XDG_DATA_HOME": str(self.base / "data")}
        source = self.sources / "during-reload.txt"
        source.write_text("do not lose me", encoding="utf-8")
        with patch.dict(os.environ, env):
            trash._configure_root(str(old_root / "trash"))
            unit_dir = config / "systemd/user"
            unit_dir.mkdir(parents=True)
            (unit_dir / "orgbot-trash-cleanup.service").write_text("[Service]\n", encoding="utf-8")
            reload_entered = threading.Event()
            finish_reload = threading.Event()
            switch_errors = []
            move_results = []

            reload_calls = 0

            def blocked_failed_reload(*args, **kwargs):
                nonlocal reload_calls
                if args[0][-1] == "show-environment":
                    return trash.subprocess.CompletedProcess(args[0], 0, stdout=f"XDG_CONFIG_HOME={config}\n".encode())
                reload_calls += 1
                if reload_calls > 1:
                    return trash.subprocess.CompletedProcess(args[0], 1)
                reload_entered.set()
                if not finish_reload.wait(timeout=3):
                    raise AssertionError("test did not release systemd reload")
                return trash.subprocess.CompletedProcess(args[0], 1)

            def switch_root():
                try:
                    trash._configure_root(str(new_root / "trash"))
                except BaseException as exc:
                    switch_errors.append(exc)

            def move_during_switch():
                output = io.StringIO()
                with patch.object(sys, "argv", [str(MODULE_PATH), "move", "--project", "Demo", str(source)]), redirect_stdout(output):
                    move_results.append(trash._cli())

            switcher = threading.Thread(target=switch_root)
            move_waiting = threading.Event()
            original_locked = trash._locked

            def observe_destination_lock(root):
                if threading.current_thread().name == "concurrent-trash-move" and root == new_root / "trash":
                    move_waiting.set()
                return original_locked(root)

            with patch.object(trash, "_systemctl_executable", return_value=Path("/usr/bin/systemctl")), patch.object(trash.subprocess, "run", side_effect=blocked_failed_reload):
                switcher.start()
                self.assertTrue(reload_entered.wait(timeout=3))
                with patch.object(trash, "_locked", side_effect=observe_destination_lock):
                    mover = threading.Thread(target=move_during_switch, name="concurrent-trash-move")
                    mover.start()
                    self.assertTrue(move_waiting.wait(timeout=3), "concurrent move did not observe the pending destination selection")
                    self.assertTrue(source.exists(), "move consumed source before pending reload committed")
                    self.assertEqual(trash.list_entries(new_root / "trash"), [])
                    finish_reload.set()
                    switcher.join(timeout=3)
                    mover.join(timeout=3)
            self.assertFalse(switcher.is_alive())
            self.assertFalse(mover.is_alive())
            self.assertEqual(len(switch_errors), 1)
            self.assertRegex(str(switch_errors[0]), "systemd rejected")
            self.assertEqual(move_results, [0])
            self.assertFalse(source.exists())
            self.assertEqual(trash._selected_config()[0], old_root / "trash")
            self.assertEqual(len(trash.list_entries(old_root / "trash")), 1)
            self.assertEqual(trash.list_entries(new_root / "trash"), [])

    def test_move_rejects_nested_symlinks_without_removing_source(self):
        source = self.sources / "tree"
        source.mkdir()
        target = self.sources / "outside.txt"
        target.write_text("preserved", encoding="utf-8")
        (source / "link").symlink_to(target)
        with self.assertRaisesRegex(trash.TrashError, "symlink|Unsupported"):
            trash.move(self.root, "Demo", source)
        self.assertTrue(source.is_dir())
        self.assertEqual(target.read_text(encoding="utf-8"), "preserved")

    def test_cleanup_uses_entry_time_and_exact_seven_day_boundary(self):
        source = self.sources / "old.txt"
        source.write_text("old", encoding="utf-8")
        os.utime(source, (1, 1))
        entry_id = trash.move(self.root, "Demo", source)
        entry, manifest = trash._find_entry(self.root, entry_id)
        moved_at = manifest["moved_at"]
        self.assertGreater(moved_at, 1)
        moved, errors = trash.cleanup(self.root, now=moved_at + trash.RETENTION_SECONDS - 1, dry_run=True)
        self.assertEqual((moved, errors), ([], []))
        at_boundary, errors = trash.cleanup(self.root, now=moved_at + trash.RETENTION_SECONDS, dry_run=True)
        self.assertEqual((at_boundary, errors), ([entry_id], []))
        purged, errors = trash.cleanup(self.root, now=moved_at + trash.RETENTION_SECONDS)
        self.assertEqual((purged, errors), ([entry_id], []))
        self.assertEqual(trash.list_entries(self.root), [])

    def test_cleanup_preserves_tampered_or_incomplete_entries(self):
        source = self.sources / "keep.txt"
        source.write_text("safe", encoding="utf-8")
        entry_id = trash.move(self.root, "Demo", source)
        entry, manifest = trash._find_entry(self.root, entry_id)
        manifest["moved_at"] = 1.0
        manifest["status"] = "pending_source_removal"
        trash._write_manifest(entry, manifest)
        self.assertEqual(trash.cleanup(self.root, now=1.0 + trash.RETENTION_SECONDS), ([], []))
        manifest["status"] = "complete"
        trash._write_manifest(entry, manifest)
        (entry / "payload" / "keep.txt").write_text("tampered", encoding="utf-8")
        purged, errors = trash.cleanup(self.root, now=1.0 + trash.RETENTION_SECONDS)
        self.assertEqual(purged, [])
        self.assertEqual(len(errors), 1)
        self.assertTrue(entry.exists())

    def test_pending_complete_copy_can_be_restored_without_erasing_recovery_entry(self):
        source = self.sources / "recover.txt"
        source.write_text("verified copy", encoding="utf-8")
        entry_id = trash.move(self.root, "Demo", source)
        entry, manifest = trash._find_entry(self.root, entry_id)
        manifest["status"] = "pending_source_removal"
        trash._write_manifest(entry, manifest)
        trash.restore(self.root, entry_id)
        self.assertEqual(source.read_text(encoding="utf-8"), "verified copy")
        self.assertEqual(trash._find_entry(self.root, entry_id)[1]["status"], "pending_source_removal")

    def test_restore_parent_replacement_keeps_trash_backup(self):
        source = self.sources / "redirect.txt"
        source.write_text("preserved", encoding="utf-8")
        entry_id = trash.move(self.root, "Demo", source)
        original_verify = trash._verify_restore_parent

        def replace_parent(path, parent_fd):
            moved_parent = path.with_name(path.name + ".moved")
            os.rename(path, moved_parent)
            path.mkdir(mode=0o700)
            original_verify(path, parent_fd)

        trash._verify_restore_parent = replace_parent
        try:
            with self.assertRaisesRegex(trash.TrashError, "backup was retained"):
                trash.restore(self.root, entry_id)
        finally:
            trash._verify_restore_parent = original_verify
        entry, _ = trash._find_entry(self.root, entry_id)
        self.assertTrue(entry.exists())
        self.assertEqual((self.sources.with_name("sources.moved") / "redirect.txt").read_text(encoding="utf-8"), "preserved")

    def test_cleanup_preserves_unrelated_entry_files(self):
        source = self.sources / "extra.txt"
        source.write_text("protected", encoding="utf-8")
        entry_id = trash.move(self.root, "Demo", source)
        entry, manifest = trash._find_entry(self.root, entry_id)
        manifest["moved_at"] = 1.0
        trash._write_manifest(entry, manifest)
        (entry / "unrelated").write_text("keep", encoding="utf-8")
        purged, errors = trash.cleanup(self.root, now=1.0 + trash.RETENTION_SECONDS)
        self.assertEqual(purged, [])
        self.assertIn("unexpected", errors[0])
        self.assertEqual((entry / "unrelated").read_text(encoding="utf-8"), "keep")

    def test_cleanup_continues_after_a_partial_entry_purge(self):
        failing_source = self.sources / "partial"
        failing_source.mkdir()
        (failing_source / "one.txt").write_text("one", encoding="utf-8")
        (failing_source / "two.txt").write_text("two", encoding="utf-8")
        healthy_source = self.sources / "healthy.txt"
        healthy_source.write_text("healthy", encoding="utf-8")
        failing_id = trash.move(self.root, "Demo", failing_source)
        healthy_id = trash.move(self.root, "Demo", healthy_source)
        for entry_id in (failing_id, healthy_id):
            entry, manifest = trash._find_entry(self.root, entry_id)
            manifest["moved_at"] = 1.0
            trash._write_manifest(entry, manifest)
        original_remove = trash._remove_entry

        def interrupted_remove(entry, manifest):
            if manifest["id"] == failing_id:
                (entry / "payload" / "partial" / "one.txt").unlink()
                raise OSError("simulated interruption after one item")
            original_remove(entry, manifest)

        trash._remove_entry = interrupted_remove
        try:
            purged, errors = trash.cleanup(self.root, now=1.0 + trash.RETENTION_SECONDS)
        finally:
            trash._remove_entry = original_remove
        self.assertEqual(purged, [healthy_id])
        self.assertEqual(len(errors), 1)
        self.assertIn(failing_id, errors[0])
        self.assertTrue(trash._find_entry(self.root, failing_id)[0].exists())

    def test_source_file_is_not_removed_when_copy_changes(self):
        source = self.sources / "changed.txt"
        source.write_text("still here", encoding="utf-8")
        original = trash._scan_payload
        calls = 0

        def mutate_after_copy(payload, name):
            nonlocal calls
            calls += 1
            result = original(payload, name)
            if calls == 1:
                source.write_text("changed while moving", encoding="utf-8")
            return result

        trash._scan_payload = mutate_after_copy
        try:
            with self.assertRaises(trash.TrashError):
                trash.move(self.root, "Demo", source)
        finally:
            trash._scan_payload = original
        self.assertEqual(source.read_text(encoding="utf-8"), "changed while moving")
        self.assertEqual(trash.list_entries(self.root)[0]["status"], "copying")

    def test_same_size_write_after_quarantine_is_restored_not_removed(self):
        source = self.sources / "tree"
        source.mkdir()
        child = source / "same.txt"
        child.write_text("original", encoding="utf-8")
        original_remove = trash._remove_known_tree

        def write_same_size(parent_fd, name, records):
            root_fd = trash._open_dir(name, dir_fd=parent_fd)
            try:
                fd = os.open("same.txt", os.O_WRONLY | os.O_NOFOLLOW, dir_fd=root_fd)
                try:
                    os.write(fd, b"updated!")
                    os.ftruncate(fd, len(b"updated!"))
                    os.fsync(fd)
                finally:
                    os.close(fd)
            finally:
                os.close(root_fd)
            original_remove(parent_fd, name, records)

        trash._remove_known_tree = write_same_size
        try:
            with self.assertRaisesRegex(trash.TrashError, "changed"):
                trash.move(self.root, "Demo", source)
        finally:
            trash._remove_known_tree = original_remove
        self.assertEqual(child.read_text(encoding="utf-8"), "updated!")
        self.assertTrue(source.is_dir())

    def test_replacement_before_final_removal_is_preserved(self):
        source = self.sources / "tree"
        source.mkdir()
        child = source / "same.txt"
        child.write_text("original", encoding="utf-8")
        original_remove = trash._remove_known_tree

        def replace_same_size(parent_fd, name, records):
            root_fd = trash._open_dir(name, dir_fd=parent_fd)
            try:
                replacement_fd = os.open("replacement.tmp", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600, dir_fd=root_fd)
                try:
                    os.write(replacement_fd, b"replaced")
                finally:
                    os.close(replacement_fd)
                os.replace("replacement.tmp", "same.txt", src_dir_fd=root_fd, dst_dir_fd=root_fd)
            finally:
                os.close(root_fd)
            original_remove(parent_fd, name, records)

        trash._remove_known_tree = replace_same_size
        try:
            with self.assertRaisesRegex(trash.TrashError, "changed"):
                trash.move(self.root, "Demo", source)
        finally:
            trash._remove_known_tree = original_remove
        self.assertEqual(child.read_text(encoding="utf-8"), "replaced")


if __name__ == "__main__":
    unittest.main()
