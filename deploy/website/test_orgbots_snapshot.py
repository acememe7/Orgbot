import io
import tempfile
import tarfile
import unittest
from pathlib import Path

from orgbots_snapshot import NGINX_MEMBER, create_snapshot, restore_check, verify_snapshot


class SnapshotTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.site = self.root / "site"
        self.site.mkdir()
        (self.site / "assets").mkdir()
        (self.site / "index.html").write_text("<main>fixture</main>")
        (self.site / "assets" / "icon.bin").write_bytes(b"\x00\x01\x02")
        self.config = self.root / "orgbots.nginx.conf"
        self.config.write_text("server {\n  server_name orgbots.theaecestudio.com;\n}\n")
        self.archive = self.root / "snapshot.tar.gz"

    def tearDown(self):
        self.temp.cleanup()

    def test_snapshot_restores_site_and_vhost_with_matching_hashes(self):
        create_snapshot(self.site, self.config, self.archive)
        manifest, files = verify_snapshot(self.archive)
        target = self.root / "restored"
        restored = restore_check(self.archive, target)
        self.assertEqual(manifest, restored)
        self.assertEqual((target / "site" / "index.html").read_text(), "<main>fixture</main>")
        self.assertEqual((target / "site" / "assets" / "icon.bin").read_bytes(), b"\x00\x01\x02")
        self.assertEqual((target / Path(NGINX_MEMBER)).read_text(), self.config.read_text())
        self.assertEqual(len(files), 3)

    def test_snapshot_rejects_hidden_files_and_symlinks(self):
        (self.site / ".env").write_text("must not be archived")
        with self.assertRaisesRegex(ValueError, "hidden paths"):
            create_snapshot(self.site, self.config, self.archive)
        (self.site / ".env").unlink()
        (self.site / "link").symlink_to(self.site / "index.html")
        with self.assertRaisesRegex(ValueError, "regular file"):
            create_snapshot(self.site, self.config, self.archive)

    def test_restore_rejects_content_that_does_not_match_manifest(self):
        create_snapshot(self.site, self.config, self.archive)
        tampered = self.root / "tampered.tar.gz"
        with tarfile.open(self.archive, "r:gz") as original, tarfile.open(tampered, "w:gz") as changed:
            for member in original.getmembers():
                data = original.extractfile(member).read()
                if member.name == "site/index.html":
                    data = b"<main>changed</main>"
                item = tarfile.TarInfo(member.name)
                item.size = len(data)
                changed.addfile(item, io.BytesIO(data))
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            restore_check(tampered, self.root / "rejected")
        self.assertFalse((self.root / "rejected").exists())

    def test_snapshot_never_overwrites_an_existing_archive(self):
        self.archive.write_bytes(b"keep")
        with self.assertRaises(FileExistsError):
            create_snapshot(self.site, self.config, self.archive)
        self.assertEqual(self.archive.read_bytes(), b"keep")

    def test_snapshot_rejects_a_vhost_for_another_domain(self):
        self.config.write_text("server {\n  server_name example.com;\n}\n")
        with self.assertRaisesRegex(ValueError, "OrgBots server name"):
            create_snapshot(self.site, self.config, self.archive)


if __name__ == "__main__":
    unittest.main()
