#!/usr/bin/env python3
"""Create and safely verify a portable snapshot of the public OrgBots site."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import tarfile
import tempfile
from datetime import datetime, timezone

FORMAT_VERSION = 1
MAX_FILES = 2_000
MAX_BYTES = 64 * 1024 * 1024
MAX_FILE_BYTES = 16 * 1024 * 1024
NGINX_MEMBER = "hosting/orgbots.nginx.conf"


def _record(path: Path, archive_name: str) -> tuple[dict[str, object], bytes]:
    mode = path.lstat().st_mode
    if not stat.S_ISREG(mode):
        raise ValueError(f"expected a regular file: {path}")
    if path.stat().st_size > MAX_FILE_BYTES:
        raise ValueError(f"file exceeds the per-file size limit: {path}")
    data = path.read_bytes()
    return ({"path": archive_name, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}, data)


def _nginx_record(path: Path) -> tuple[dict[str, object], bytes]:
    record = _record(path, NGINX_MEMBER)
    server_names = re.findall(rb"(?m)^\s*server_name\s+([^;]+);", record[1])
    if not server_names or any(name.strip() != b"orgbots.theaecestudio.com" for name in server_names):
        raise ValueError("Nginx config must contain only the OrgBots server name")
    return record


def _site_records(root: Path) -> list[tuple[dict[str, object], bytes]]:
    records = []
    total = 0
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root)
        if any(part.startswith(".") for part in relative.parts):
            raise ValueError(f"hidden paths are not included: {relative}")
        if path.is_dir() and not path.is_symlink():
            continue
        name = PurePosixPath("site", *relative.parts).as_posix()
        records.append(_record(path, name))
        total += int(records[-1][0]["size"])
        if total > MAX_BYTES:
            raise ValueError("site exceeds the uncompressed size limit")
        if len(records) > MAX_FILES:
            raise ValueError("site exceeds the file count limit")
    return records


def _manifest(records: list[tuple[dict[str, object], bytes]], config: tuple[dict[str, object], bytes]) -> bytes:
    total = sum(int(record["size"]) for record, _ in records) + int(config[0]["size"])
    if total > MAX_BYTES:
        raise ValueError("snapshot exceeds the uncompressed size limit")
    payload = {
        "format": FORMAT_VERSION,
        "created_utc": datetime.now(timezone.utc).isoformat(),
        "files": [record for record, _ in records] + [config[0]],
    }
    return (json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n").encode()


def _add_bytes(archive: tarfile.TarFile, name: str, data: bytes) -> None:
    item = tarfile.TarInfo(name)
    item.size = len(data)
    item.mode = 0o644
    item.mtime = 0
    archive.addfile(item, io.BytesIO(data))


def create_snapshot(site_root: Path, nginx_config: Path, output: Path) -> None:
    if site_root.is_symlink() or not site_root.is_dir():
        raise ValueError("site root must be a real directory")
    if nginx_config.is_symlink() or not nginx_config.is_file():
        raise ValueError("Nginx config must be a regular file")
    site_root = site_root.resolve(strict=True)
    nginx_config = nginx_config.resolve(strict=True)
    output = output.absolute()
    if output == site_root or site_root in output.parents:
        raise ValueError("snapshot output must be outside the site root")
    if not output.parent.is_dir():
        raise ValueError("snapshot output directory must already exist")

    records = _site_records(site_root)
    config = _nginx_record(nginx_config)
    if int(config[0]["size"]) > MAX_FILE_BYTES:
        raise ValueError("Nginx config exceeds the per-file size limit")
    manifest = _manifest(records, config)
    if len(manifest) > 2 * 1024 * 1024:
        raise ValueError("snapshot manifest exceeds the size limit")

    fd, temporary_name = tempfile.mkstemp(prefix=".orgbots-snapshot-", suffix=".tmp", dir=output.parent)
    os.close(fd)
    temporary = Path(temporary_name)
    try:
        with tarfile.open(temporary, mode="w:gz", format=tarfile.PAX_FORMAT) as archive:
            _add_bytes(archive, "manifest.json", manifest)
            for record, data in records + [config]:
                _add_bytes(archive, str(record["path"]), data)
        os.link(temporary, output)
    finally:
        temporary.unlink(missing_ok=True)


def _safe_member_name(name: str) -> bool:
    path = PurePosixPath(name)
    parts = name.split("/")
    return bool(name) and not path.is_absolute() and "\\" not in name and all(part not in ("", ".", "..") for part in parts)


def _allowed_content_name(name: str) -> bool:
    if not _safe_member_name(name):
        return False
    if name == NGINX_MEMBER:
        return True
    path = PurePosixPath(name)
    return path.parts[0] == "site" and len(path.parts) > 1 and all(not part.startswith(".") for part in path.parts[1:])


def verify_snapshot(archive_path: Path) -> tuple[dict[str, object], dict[str, bytes]]:
    if archive_path.is_symlink() or not archive_path.is_file():
        raise ValueError("snapshot archive must be a regular file")
    if archive_path.stat().st_size > MAX_BYTES:
        raise ValueError("snapshot archive exceeds the size limit")
    with tarfile.open(archive_path, mode="r:gz") as archive:
        members = archive.getmembers()
        if len(members) > MAX_FILES + 2:
            raise ValueError("snapshot contains too many entries")
        if any(not member.isfile() or not _safe_member_name(member.name) for member in members):
            raise ValueError("snapshot contains an unsafe or non-file entry")
        by_name = {member.name: member for member in members}
        if len(by_name) != len(members) or "manifest.json" not in by_name:
            raise ValueError("snapshot has duplicate entries or no manifest")
        if by_name["manifest.json"].size > 2 * 1024 * 1024:
            raise ValueError("snapshot manifest exceeds the size limit")
        manifest_stream = archive.extractfile(by_name["manifest.json"])
        if manifest_stream is None:
            raise ValueError("snapshot manifest is unreadable")
        manifest = json.loads(manifest_stream.read())
        if not isinstance(manifest, dict) or manifest.get("format") != FORMAT_VERSION or not isinstance(manifest.get("files"), list):
            raise ValueError("unsupported snapshot manifest")
        expected: dict[str, dict[str, object]] = {}
        total = 0
        for record in manifest["files"]:
            if not isinstance(record, dict):
                raise ValueError("invalid manifest entry")
            name, size, digest = record.get("path"), record.get("size"), record.get("sha256")
            if not isinstance(name, str) or not _allowed_content_name(name):
                raise ValueError("unsafe manifest path")
            if not isinstance(size, int) or size < 0 or size > MAX_BYTES or not isinstance(digest, str) or len(digest) != 64:
                raise ValueError("invalid manifest size or digest")
            if name in expected:
                raise ValueError("duplicate manifest path")
            expected[name] = record
            total += size
        if total > MAX_BYTES or set(by_name) != {"manifest.json", *expected}:
            raise ValueError("snapshot entries do not match the manifest")
        if len(expected) > MAX_FILES + 1 or NGINX_MEMBER not in expected:
            raise ValueError("snapshot is incomplete")

        contents = {}
        for name, record in expected.items():
            member = by_name[name]
            if member.size != record["size"]:
                raise ValueError(f"size mismatch: {name}")
            stream = archive.extractfile(member)
            if stream is None:
                raise ValueError(f"unreadable archive entry: {name}")
            data = stream.read(MAX_BYTES + 1)
            if len(data) != record["size"] or hashlib.sha256(data).hexdigest() != record["sha256"]:
                raise ValueError(f"checksum mismatch: {name}")
            contents[name] = data
    return manifest, contents


def restore_check(archive_path: Path, destination: Path) -> dict[str, object]:
    manifest, contents = verify_snapshot(archive_path)
    destination = destination.absolute()
    if os.path.lexists(destination):
        raise ValueError("restore destination must not already exist")
    if not destination.parent.is_dir():
        raise ValueError("restore parent directory must already exist")
    temporary = Path(tempfile.mkdtemp(prefix=".orgbots-restore-", dir=destination.parent))
    try:
        for name, data in contents.items():
            target = temporary.joinpath(*PurePosixPath(name).parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            with target.open("xb") as output:
                output.write(data)
            target.chmod(0o644)
        os.rename(temporary, destination)
    finally:
        if temporary.exists():
            for path in sorted(temporary.rglob("*"), reverse=True):
                if path.is_file() or path.is_symlink():
                    path.unlink()
                elif path.is_dir():
                    path.rmdir()
            temporary.rmdir()
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    snapshot = subparsers.add_parser("snapshot", help="create one self-contained snapshot")
    snapshot.add_argument("--site-root", type=Path, required=True)
    snapshot.add_argument("--nginx-config", type=Path, required=True)
    snapshot.add_argument("--output", type=Path, required=True)
    restore = subparsers.add_parser("restore-check", help="verify and restore into a new empty path")
    restore.add_argument("--archive", type=Path, required=True)
    restore.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "snapshot":
            create_snapshot(args.site_root, args.nginx_config, args.output)
            print(f"Snapshot created: {args.output}")
        else:
            manifest = restore_check(args.archive, args.destination)
            print(f"Verified {len(manifest['files'])} files; restored to {args.destination}")
        return 0
    except (OSError, ValueError, tarfile.TarError, json.JSONDecodeError) as error:
        parser.error(str(error))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
