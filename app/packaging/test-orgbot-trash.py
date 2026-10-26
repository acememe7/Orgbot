#!/usr/bin/env python3
import importlib.util
import os
import tempfile
from types import SimpleNamespace
import unittest
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

    def test_production_root_fails_closed_when_hdd_is_unmounted(self):
        with patch.object(trash.os.path, "ismount", return_value=False):
            with self.assertRaisesRegex(trash.TrashError, "unavailable"):
                trash._prepare_root(trash.TRASH_ROOT, require_mount=True)

    def test_read_only_hdd_parent_allows_writable_trash_bind_mount(self):
        hdd = self.base / "hdd"
        root = hdd / "OrgBots Trash"
        hdd.mkdir()
        root.mkdir(mode=0o700)
        with (
            patch.object(trash, "HDD_MOUNT", hdd),
            patch.object(trash, "TRASH_ROOT", root),
            patch.object(trash.os.path, "ismount", return_value=True),
            patch.object(trash, "_mount_options", return_value=("ext4", {"ro"})),
            patch.object(trash, "_effective_mount_options", return_value=(root, "ext4", {"rw"})),
            patch.object(trash.os, "statvfs", return_value=SimpleNamespace(f_flag=0)),
        ):
            trash._verify_hdd_mount()

    def test_read_only_hdd_parent_rejects_missing_trash_root(self):
        hdd = self.base / "hdd"
        root = hdd / "OrgBots Trash"
        hdd.mkdir()
        with (
            patch.object(trash, "HDD_MOUNT", hdd),
            patch.object(trash, "TRASH_ROOT", root),
            patch.object(trash.os.path, "ismount", return_value=True),
            patch.object(trash, "_mount_options", return_value=("ext4", {"ro"})),
            patch.object(trash, "_effective_mount_options", return_value=(hdd, "ext4", {"ro"})),
        ):
            with self.assertRaisesRegex(trash.TrashError, "read-only"):
                trash._verify_hdd_mount()

    def test_destination_statvfs_read_only_flag_rejects_read_only_bind(self):
        hdd = self.base / "hdd"
        root = hdd / "OrgBots Trash"
        hdd.mkdir()
        root.mkdir(mode=0o700)
        with (
            patch.object(trash, "HDD_MOUNT", hdd),
            patch.object(trash, "TRASH_ROOT", root),
            patch.object(trash.os.path, "ismount", return_value=True),
            patch.object(trash, "_mount_options", return_value=("ext4", {"ro"})),
            patch.object(trash, "_effective_mount_options", return_value=(root, "ext4", {"rw"})),
            patch.object(trash.os, "statvfs", return_value=SimpleNamespace(f_flag=getattr(os, "ST_RDONLY", 1))),
        ):
            with self.assertRaisesRegex(trash.TrashError, "read-only"):
                trash._verify_hdd_mount()

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
