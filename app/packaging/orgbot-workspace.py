#!/usr/bin/env python3
"""Create, run, and safely merge isolated OrgBots project workspaces."""

from __future__ import annotations

import argparse
import contextlib
import ctypes
import errno
import fcntl
import glob
import hashlib
import json
import os
import pwd
import re
import resource
import secrets
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path, PurePosixPath
from typing import Any


FORMAT_VERSION = 1
AGY_WRAP_ATTRIBUTES = {
    "service": "orgbots",
    "purpose": "antigravity-private-keyring-passphrase",
    "version": "1",
}
AGY_DEFAULT_PROJECT = {
    "id": "default-cli-project",
    "name": "CLI Project",
    "projectResources": {},
}
AGY_DEFAULT_PROJECT_FILENAME = "default-cli-project.json"
_AGY_CHILDREN: set[subprocess.Popen[bytes]] = set()
MAX_ENTRIES = 150_000
MAX_FILE_BYTES = 512 * 1024 * 1024
MAX_TREE_BYTES = 20 * 1024 * 1024 * 1024
AGENT_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,63}\Z")
PROFILE_RE = re.compile(r"(?:codex|claude|grok|opencode|agy)\Z")
AGENT_CLI_RE = re.compile(r"(?:opencode|agy)\Z")
# These directories contain reproducible caches, build output, or per-agent Git
# metadata. Git is copied into each workspace once, but is not diffed/applied.
EXCLUDED_DIRS = {
    ".git", ".dart_tool", "target", "build", "node_modules", "venv", ".venv",
    "out", "dist", "coverage", "__pycache__", ".gradle", ".next", ".turbo",
    ".pytest_cache", ".mypy_cache", ".ruff_cache", ".cache",
}
EXCLUDED_FILES = {".flutter-plugins", ".flutter-plugins-dependencies"}
FICLONE = 0x40049409
RENAME_NOREPLACE = 1


class WorkspaceError(Exception):
    pass


def _fail(message: str) -> None:
    raise WorkspaceError(message)


def _json(value: dict[str, Any]) -> None:
    print(json.dumps(value, ensure_ascii=False, sort_keys=True))


def _canonical_directory(value: str, label: str, *, create: bool = False) -> Path:
    raw = Path(value)
    if not raw.is_absolute():
        _fail(f"{label} must be an absolute path")
    if create:
        _mkdir_chain_no_symlinks(raw)
    else:
        current = Path(raw.anchor)
        for part in raw.parts[1:]:
            current /= part
            try:
                info = current.lstat()
            except OSError as exc:
                _fail(f"Cannot verify {label}: {exc}")
            if stat.S_ISLNK(info.st_mode):
                _fail(f"Symlink path components are not permitted for {label}: {current}")
    try:
        resolved = raw.resolve(strict=True)
        info = resolved.lstat()
    except OSError as exc:
        _fail(f"Cannot verify {label}: {exc}")
    if not stat.S_ISDIR(info.st_mode):
        _fail(f"{label} must be a real directory")
    return resolved


def _mkdir_chain_no_symlinks(path: Path, mode: int = 0o700) -> None:
    if not path.is_absolute():
        _fail("Directory path must be absolute")
    current = Path(path.anchor)
    for part in path.parts[1:]:
        current /= part
        try:
            info = current.lstat()
        except FileNotFoundError:
            try:
                current.mkdir(mode=mode)
            except FileExistsError:
                pass
            info = current.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            _fail(f"Unsafe directory component: {current}")


def _check_private_dir(path: Path) -> None:
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        _fail(f"Unsafe or non-private directory: {path}")


def _ensure_private_dir(path: Path) -> None:
    try:
        info = path.lstat()
    except FileNotFoundError:
        path.mkdir(mode=0o700)
        info = path.lstat()
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        _fail(f"Unsafe or non-private directory: {path}")


def _safe_state_child(state: Path, relative: str, *, create: bool = False) -> Path:
    result = state
    for part in PurePosixPath(relative).parts:
        result = result / part
        if create:
            _ensure_private_dir(result)
        else:
            info = result.lstat()
            if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
                _fail(f"Unsafe state directory: {result}")
    return result


def _safe_agent(agent: str) -> str:
    if not AGENT_RE.fullmatch(agent):
        _fail("Agent id must use 1-64 letters, digits, dots, underscores, or hyphens")
    return agent


def _project_id(project: Path) -> str:
    return hashlib.sha256(os.fsencode(project)).hexdigest()[:24]


def _lock(project: Path):
    lock_root = Path(f"/run/user/{os.getuid()}/orgbot-workspace-locks")
    _mkdir_chain_no_symlinks(lock_root)
    _check_private_dir(lock_root)
    lock_path = lock_root / f"{_project_id(project)}.lock"
    flags = os.O_CREAT | os.O_RDWR | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(lock_path, flags, 0o600)
    info = os.fstat(fd)
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        os.close(fd)
        _fail("Unsafe project integration lock")
    fcntl.flock(fd, fcntl.LOCK_EX)
    return fd


def _layout(state: Path, project: Path, agent: str) -> tuple[Path, Path, Path]:
    project_id = _project_id(project)
    agent_root = state / "agents" / project_id / agent
    workspace = agent_root / "project"
    manifest = state / "manifests" / project_id / f"{agent}.json"
    return agent_root, workspace, manifest


def _manifest_candidates(state: Path, agent: str) -> list[Path]:
    root = _safe_state_child(state, "manifests")
    result = []
    with os.scandir(root) as entries:
        for entry in entries:
            info = entry.stat(follow_symlinks=False)
            if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
                _fail("Unsafe workspace manifest directory")
            candidate = Path(entry.path) / f"{agent}.json"
            try:
                candidate_info = candidate.lstat()
            except FileNotFoundError:
                continue
            if stat.S_ISLNK(candidate_info.st_mode) or not stat.S_ISREG(candidate_info.st_mode):
                _fail("Unsafe workspace baseline manifest")
            result.append(candidate)
    return result


def _ensure_root_paths(state: Path, project: Path) -> None:
    try:
        common = Path(os.path.commonpath((os.fspath(state), os.fspath(project))))
    except ValueError:
        return
    if common == state or common == project:
        _fail("State and project trees must not overlap")


def _relative(path: str) -> PurePosixPath:
    rel = PurePosixPath(path)
    if rel.is_absolute() or not rel.parts or any(p in ("", ".", "..") for p in rel.parts):
        _fail(f"Unsafe relative path in workspace metadata: {path!r}")
    try:
        encoded = path.encode("utf-8")
    except UnicodeEncodeError:
        _fail("Project paths must be valid UTF-8")
    if rel.as_posix() != path or "\x00" in path or len(encoded) > 4096:
        _fail("Invalid or excessively long workspace path")
    return rel


def _excluded(relative: str, *, include_git: bool = False) -> bool:
    parts = PurePosixPath(relative).parts
    excluded = EXCLUDED_DIRS - ({".git"} if include_git else set())
    return any(part in excluded for part in parts) or bool(parts and parts[-1] in EXCLUDED_FILES)


def _validate_internal_symlink(path: Path, root: Path) -> None:
    target = os.readlink(path)
    if os.path.isabs(target):
        _fail(f"Absolute project symlinks are not supported: {path.relative_to(root)}")
    try:
        resolved = (path.parent / target).resolve(strict=True)
        relative = resolved.relative_to(root).as_posix()
        if _excluded(relative):
            _fail(f"Project symlink resolves into an excluded generated directory: {path.relative_to(root)}")
    except (OSError, ValueError):
        _fail(f"Project symlink must resolve inside the project: {path.relative_to(root)}")


def _entry(path: Path, root: Path, *, limit: bool = True) -> dict[str, Any]:
    info = path.lstat()
    if stat.S_ISLNK(info.st_mode):
        _validate_internal_symlink(path, root)
        _fail(f"Symlink entries are not mergeable: {path.relative_to(root)}")
    if not stat.S_ISREG(info.st_mode):
        _fail(f"Unsupported project file type: {path.relative_to(root)}")
    if info.st_nlink != 1:
        _fail(f"Multiply-linked files are not supported in isolated workspaces: {path.relative_to(root)}")
    if limit and info.st_size > MAX_FILE_BYTES:
        _fail(f"File exceeds the {MAX_FILE_BYTES}-byte workspace limit: {path.relative_to(root)}")
    digest = hashlib.sha256()
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(path, flags)
    try:
        opened = os.fstat(fd)
        if (opened.st_dev, opened.st_ino) != (info.st_dev, info.st_ino) or not stat.S_ISREG(opened.st_mode):
            _fail(f"File changed while scanning: {path.relative_to(root)}")
        with os.fdopen(fd, "rb", closefd=False) as stream:
            while True:
                chunk = stream.read(1024 * 1024)
                if not chunk:
                    break
                digest.update(chunk)
        final = os.fstat(fd)
        if (final.st_size, final.st_mtime_ns, final.st_ctime_ns) != (opened.st_size, opened.st_mtime_ns, opened.st_ctime_ns):
            _fail(f"File changed while scanning: {path.relative_to(root)}")
    finally:
        os.close(fd)
    return {"kind": "file", "sha256": digest.hexdigest(), "mode": stat.S_IMODE(info.st_mode) & 0o777, "size": info.st_size}


def _scan(root: Path) -> dict[str, dict[str, Any]]:
    entries: dict[str, dict[str, Any]] = {}
    total_bytes = 0
    count = 0
    for current, dirs, files in os.walk(root, topdown=True, followlinks=False):
        base = Path(current)
        safe_dirs = []
        for name in dirs:
            path = base / name
            info = path.lstat()
            rel = path.relative_to(root).as_posix()
            if _excluded(rel):
                continue
            if stat.S_ISLNK(info.st_mode):
                _validate_internal_symlink(path, root)
                continue
            if not stat.S_ISDIR(info.st_mode):
                _fail(f"Unsafe project directory entry: {path.relative_to(root)}")
            count += 1
            if count > MAX_ENTRIES:
                _fail("Project exceeds workspace entry limit")
            safe_dirs.append(name)
        dirs[:] = safe_dirs
        for name in files:
            path = base / name
            rel = path.relative_to(root).as_posix()
            if _excluded(rel):
                continue
            if path.is_symlink():
                _validate_internal_symlink(path, root)
                continue
            _relative(rel)
            row = _entry(path, root)
            total_bytes += row["size"]
            count += 1
            if count > MAX_ENTRIES or total_bytes > MAX_TREE_BYTES:
                _fail("Project exceeds workspace entry or total-size limit")
            entries[rel] = row
    return entries


def _scan_workspace(root: Path, baseline: dict[str, dict[str, Any]]) -> dict[str, dict[str, Any]]:
    entries = _scan(root)
    for name, entry in entries.items():
        before = baseline.get(name)
        if before is not None and entry["mode"] == (before["mode"] | 0o200):
            entry["mode"] = before["mode"]
    return entries


def _copy_file(src: Path, dst: Path, mode: int) -> None:
    src_fd = os.open(src, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    dst_fd = os.open(dst, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    try:
        source_info = os.fstat(src_fd)
        if not stat.S_ISREG(source_info.st_mode) or source_info.st_size > MAX_FILE_BYTES:
            _fail(f"Unsafe file during workspace copy: {src}")
        cloned = False
        try:
            import fcntl as _fcntl

            _fcntl.ioctl(dst_fd, FICLONE, src_fd)
            cloned = True
        except OSError:
            os.ftruncate(dst_fd, 0)
        if not cloned:
            os.lseek(src_fd, 0, os.SEEK_SET)
            while True:
                chunk = os.read(src_fd, 1024 * 1024)
                if not chunk:
                    break
                view = memoryview(chunk)
                while view:
                    written = os.write(dst_fd, view)
                    view = view[written:]
        os.fchmod(dst_fd, mode & 0o777)
        os.fsync(dst_fd)
    finally:
        os.close(src_fd)
        os.close(dst_fd)


def _copy_tree(source: Path, destination: Path) -> None:
    destination.mkdir(mode=0o700)
    total_bytes = 0
    count = 0
    for current, dirs, files in os.walk(source, topdown=True, followlinks=False):
        src_dir = Path(current)
        rel_dir = src_dir.relative_to(source)
        dst_dir = destination / rel_dir
        for name in dirs:
            src = src_dir / name
            relative = src.relative_to(source).as_posix()
            if _excluded(relative, include_git=True):
                continue
            info = src.lstat()
            if stat.S_ISLNK(info.st_mode):
                _validate_internal_symlink(src, source)
                (dst_dir / name).symlink_to(os.readlink(src), target_is_directory=src.resolve().is_dir())
                continue
            if not stat.S_ISDIR(info.st_mode):
                _fail(f"Unsafe project directory: {src}")
            target = dst_dir / name
            target.mkdir(mode=(stat.S_IMODE(info.st_mode) | 0o700) & 0o777)
            count += 1
        for name in files:
            src = src_dir / name
            relative = src.relative_to(source).as_posix()
            if _excluded(relative, include_git=True):
                continue
            if src.is_symlink():
                _validate_internal_symlink(src, source)
                target = dst_dir / name
                target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                target.symlink_to(os.readlink(src), target_is_directory=src.resolve().is_dir())
                count += 1
                continue
            row = _entry(src, source)
            count += 1
            total_bytes += row["size"]
            if count > MAX_ENTRIES or total_bytes > MAX_TREE_BYTES:
                _fail("Project exceeds workspace entry or total-size limit")
            target = dst_dir / name
            target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            _copy_file(src, target, row["mode"] | 0o200)


def _manifest_read(path: Path, project: Path, agent: str) -> dict[str, Any]:
    try:
        data = _read_private_json(path, "workspace baseline manifest")
    except WorkspaceError:
        raise
    except (OSError, json.JSONDecodeError) as exc:
        _fail(f"Cannot read workspace baseline manifest: {exc}")
    if not isinstance(data, dict) or data.get("format_version") != FORMAT_VERSION or data.get("agent") != agent or data.get("project") != os.fspath(project):
        _fail("Workspace baseline manifest has invalid identity or version")
    entries = data.get("entries")
    if not isinstance(entries, dict) or len(entries) > MAX_ENTRIES:
        _fail("Workspace baseline manifest has invalid entries")
    for name, row in entries.items():
        _relative(name)
        if not isinstance(row, dict) or row.get("kind") != "file" or not re.fullmatch(r"[a-f0-9]{64}", str(row.get("sha256", ""))) or not isinstance(row.get("mode"), int) or isinstance(row.get("mode"), bool) or row["mode"] & ~0o777 or not isinstance(row.get("size"), int) or isinstance(row.get("size"), bool) or not 0 <= row["size"] <= MAX_FILE_BYTES:
            _fail("Workspace baseline manifest contains an invalid file record")
    if not isinstance(data.get("read_only"), bool):
        _fail("Workspace baseline manifest has invalid access mode")
    return data


def _read_private_json(path: Path, label: str) -> Any:
    fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077 or info.st_size > 32 * 1024 * 1024:
            _fail(f"Unsafe {label}")
        chunks = []
        remaining = 32 * 1024 * 1024 + 1
        while remaining > 0:
            chunk = os.read(fd, min(1024 * 1024, remaining))
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        if remaining <= 0:
            _fail(f"{label.capitalize()} exceeds the size limit")
        return json.loads(b"".join(chunks).decode("utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        if isinstance(exc, WorkspaceError):
            raise
        _fail(f"Cannot read {label}: {exc}")
    finally:
        os.close(fd)


def _atomic_json(path: Path, data: dict[str, Any]) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    temp_fd, temp_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        os.fchmod(temp_fd, 0o600)
        with os.fdopen(temp_fd, "w", encoding="utf-8") as stream:
            json.dump(data, stream, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temp_name, path)
        dir_fd = os.open(path.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
    except Exception:
        try:
            os.unlink(temp_name)
        except OSError:
            pass
        raise


def _file_at(root: Path, relative: str) -> Path:
    rel = _relative(relative)
    current = root
    for part in rel.parts[:-1]:
        current = current / part
        try:
            info = current.lstat()
        except FileNotFoundError:
            return root.joinpath(*rel.parts)
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            _fail(f"Unsafe path parent: {relative}")
    target = current / rel.parts[-1]
    try:
        info = target.lstat()
    except FileNotFoundError:
        return target
    if not stat.S_ISREG(info.st_mode):
        _fail(f"Unsafe path target: {relative}")
    return target


def _open_parent_fd(root: Path, relative: str, *, create: bool, directory_mode: int) -> tuple[int, str]:
    rel = _relative(relative)
    root_info = root.lstat()
    if stat.S_ISLNK(root_info.st_mode) or not stat.S_ISDIR(root_info.st_mode):
        _fail(f"Unsafe tree root: {root}")
    fd = os.open(root, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0))
    try:
        for part in rel.parts[:-1]:
            try:
                next_fd = os.open(part, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0), dir_fd=fd)
            except FileNotFoundError:
                if not create:
                    _fail(f"Missing parent directory for {relative}")
                try:
                    os.mkdir(part, directory_mode, dir_fd=fd)
                except FileExistsError:
                    pass
                next_fd = os.open(part, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0), dir_fd=fd)
            os.close(fd)
            fd = next_fd
        return fd, rel.parts[-1]
    except Exception:
        os.close(fd)
        raise


def _copy_fd_to_fd(src_fd: int, dst_fd: int, mode: int) -> None:
    source = os.fstat(src_fd)
    if not stat.S_ISREG(source.st_mode) or source.st_nlink != 1 or source.st_size > MAX_FILE_BYTES:
        _fail("Unsafe source file during atomic copy")
    cloned = False
    try:
        fcntl.ioctl(dst_fd, FICLONE, src_fd)
        cloned = True
    except OSError:
        os.ftruncate(dst_fd, 0)
    if not cloned:
        os.lseek(src_fd, 0, os.SEEK_SET)
        while True:
            chunk = os.read(src_fd, 1024 * 1024)
            if not chunk:
                break
            view = memoryview(chunk)
            while view:
                count = os.write(dst_fd, view)
                view = view[count:]
    os.fchmod(dst_fd, mode & 0o777)
    os.fsync(dst_fd)


def _rename_noreplace(parent_fd: int, old_name: str, new_name: str) -> None:
    libc = ctypes.CDLL(None, use_errno=True)
    renameat2 = getattr(libc, "renameat2", None)
    if renameat2 is None:
        _fail("Atomic no-overwrite rename is unavailable; refusing to integrate the file")
    renameat2.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
    renameat2.restype = ctypes.c_int
    result = renameat2(parent_fd, os.fsencode(old_name), parent_fd, os.fsencode(new_name), RENAME_NOREPLACE)
    if result != 0:
        error = ctypes.get_errno()
        if error == errno.EEXIST:
            _fail(f"Destination appeared while integrating {new_name}; preserved it")
        raise OSError(error, os.strerror(error), new_name)


def _atomic_copy_file(source_root: Path, source_relative: str, destination_root: Path, destination_relative: str, mode: int, *, create_parents: bool, replace_existing: bool) -> None:
    source_parent_fd, source_name = _open_parent_fd(source_root, source_relative, create=False, directory_mode=0o700)
    destination_parent_fd, destination_name = _open_parent_fd(destination_root, destination_relative, create=create_parents, directory_mode=0o755 if destination_root == source_root else 0o700)
    temp_name = f".orgbot-{os.getpid()}-{os.urandom(8).hex()}"
    try:
        source_stat = os.stat(source_name, dir_fd=source_parent_fd, follow_symlinks=False)
        if not stat.S_ISREG(source_stat.st_mode) or source_stat.st_nlink != 1:
            _fail(f"Unsafe source file: {source_relative}")
        try:
            dest_stat = os.stat(destination_name, dir_fd=destination_parent_fd, follow_symlinks=False)
        except FileNotFoundError:
            dest_stat = None
        if dest_stat is not None and (stat.S_ISLNK(dest_stat.st_mode) or not stat.S_ISREG(dest_stat.st_mode)):
            _fail(f"Unsafe destination file: {destination_relative}")
        if dest_stat is not None and not replace_existing:
            _fail(f"Destination appeared while integrating {destination_relative}; preserved it")
        src_fd = os.open(source_name, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0), dir_fd=source_parent_fd)
        dst_fd = -1
        try:
            opened = os.fstat(src_fd)
            source_snapshot = (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns, opened.st_ctime_ns)
            if source_snapshot[:2] != (source_stat.st_dev, source_stat.st_ino):
                _fail(f"Source changed during copy: {source_relative}")
            dst_fd = os.open(temp_name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600, dir_fd=destination_parent_fd)
            _copy_fd_to_fd(src_fd, dst_fd, mode)
            final_source = os.fstat(src_fd)
            if source_snapshot != (final_source.st_dev, final_source.st_ino, final_source.st_size, final_source.st_mtime_ns, final_source.st_ctime_ns):
                _fail(f"Source changed during copy: {source_relative}")
            if replace_existing:
                os.replace(temp_name, destination_name, src_dir_fd=destination_parent_fd, dst_dir_fd=destination_parent_fd)
            else:
                _rename_noreplace(destination_parent_fd, temp_name, destination_name)
            os.fsync(destination_parent_fd)
        finally:
            os.close(src_fd)
            if dst_fd >= 0:
                os.close(dst_fd)
    finally:
        try:
            os.unlink(temp_name, dir_fd=destination_parent_fd)
        except FileNotFoundError:
            pass
        os.close(source_parent_fd)
        os.close(destination_parent_fd)


def _remove_file(path: Path) -> None:
    try:
        info = path.lstat()
    except FileNotFoundError:
        return
    if not stat.S_ISREG(info.st_mode):
        _fail(f"Refusing to remove non-regular workspace entry: {path}")
    path.unlink()


def _copy_from_project(project: Path, workspace: Path, relative: str) -> None:
    row = _entry(_file_at(project, relative), project)
    _atomic_copy_file(project, relative, workspace, relative, row["mode"] | 0o200, create_parents=True, replace_existing=True)


def _copy_from_workspace(workspace: Path, project: Path, relative: str, mode: int) -> None:
    _atomic_copy_file(workspace, relative, project, relative, mode, create_parents=True, replace_existing=False)


def _changes(base: dict[str, dict[str, Any]], current: dict[str, dict[str, Any]], work: dict[str, dict[str, Any]]) -> dict[str, Any]:
    changed: list[dict[str, str]] = []
    added: list[str] = []
    deleted: list[str] = []
    conflicts: list[str] = []
    for name in sorted(set(base) | set(current) | set(work)):
        before, now, agent = base.get(name), current.get(name), work.get(name)
        if agent == before or agent == now:
            continue
        if before is None and agent is not None:
            added.append(name)
        elif before is not None and agent is None:
            deleted.append(name)
        else:
            changed.append({"path": name, "kind": "modified"})
        if now != before and now != agent:
            conflicts.append(name)
    return {"changed": changed, "added": added, "deleted": deleted, "conflicts": conflicts}


def _review_token(project: Path, agent: str, base: dict[str, dict[str, Any]], current: dict[str, dict[str, Any]], work: dict[str, dict[str, Any]]) -> str:
    snapshot = {
        "version": 1,
        "project": os.fspath(project),
        "agent": agent,
        "baseline": base,
        "project_entries": current,
        "agent_entries": work,
    }
    encoded = json.dumps(snapshot, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _prepare(project_arg: str, state_arg: str, agent_arg: str, read_only: bool) -> dict[str, Any]:
    agent = _safe_agent(agent_arg)
    project = _canonical_directory(project_arg, "project")
    state = _canonical_directory(state_arg, "state", create=True)
    _check_private_dir(state)
    _ensure_root_paths(state, project)
    lock_fd = _lock(project)
    try:
        source_entries = _scan(project)
        agent_root, workspace, manifest_path = _layout(state, project, agent)
        _safe_state_child(state, f"agents/{_project_id(project)}", create=True)
        _ensure_private_dir(agent_root)
        _safe_state_child(state, f"manifests/{_project_id(project)}", create=True)
        profile_root = _safe_state_child(state, f"agents/{_project_id(project)}/{agent}/profiles", create=True)
        private_home = profile_root / "home"
        _ensure_private_dir(private_home)
        for profile_name in (".codex", ".claude", ".grok"):
            profile = private_home / profile_name
            _ensure_private_dir(profile)
            _check_private_dir(profile)
        try:
            manifest_info = manifest_path.lstat()
            if not stat.S_ISREG(manifest_info.st_mode):
                _fail("Unsafe workspace baseline manifest")
            manifest_exists = True
        except FileNotFoundError:
            manifest_exists = False
        if not manifest_exists:
            try:
                workspace.lstat()
                workspace_exists = True
            except FileNotFoundError:
                workspace_exists = False
            if workspace_exists:
                _fail("Workspace exists without a trusted baseline manifest")
            _copy_tree(project, workspace)
            if _scan_workspace(workspace, source_entries) != source_entries or _scan(project) != source_entries:
                _fail("Project changed during workspace preparation; no baseline was written")
            data = {"format_version": FORMAT_VERSION, "project": os.fspath(project), "agent": agent, "read_only": read_only, "created_at": int(time.time()), "entries": source_entries}
            _atomic_json(manifest_path, data)
        else:
            data = _manifest_read(manifest_path, project, agent)
            if not workspace.is_dir() or workspace.is_symlink():
                _fail("Workspace path is missing or unsafe")
            _check_private_dir(workspace)
            agent_entries = _scan_workspace(workspace, data["entries"])
            diffs = _changes(data["entries"], source_entries, agent_entries)
            conflicts = set(diffs["conflicts"])
            merged = dict(data["entries"])
            for name in sorted(set(data["entries"]) | set(source_entries) | set(agent_entries)):
                if name in conflicts:
                    continue
                before, now, ours = data["entries"].get(name), source_entries.get(name), agent_entries.get(name)
                if ours == before == now:
                    continue
                if ours == now and now is not None:
                    merged[name] = now
                    continue
                if ours == before or ours == now:
                    target = _file_at(workspace, name)
                    if now is None:
                        _remove_file(target)
                        merged.pop(name, None)
                    else:
                        _copy_from_project(project, workspace, name)
                        merged[name] = now
                elif ours is not None:
                    merged[name] = before or ours
            data["entries"] = merged
            data["read_only"] = data["read_only"] or read_only
            if conflicts:
                data["refresh_conflicts"] = sorted(conflicts)
            else:
                data.pop("refresh_conflicts", None)
            _atomic_json(manifest_path, data)
        return {"ok": True, "workspace": os.fspath(workspace), "profile_root": os.fspath(agent_root / "profiles"), "private_home": os.fspath(private_home), "private_home_mount": f"/run/user/{os.getuid()}/orgbot-home", "mcp_socket_mount": f"/run/user/{os.getuid()}/orgbot-room-mcp.sock", "read_only": data["read_only"], "conflicts": data.get("refresh_conflicts", []), "project_id": _project_id(project)}
    finally:
        os.close(lock_fd)


def _status(state_arg: str, agent_arg: str) -> dict[str, Any]:
    agent = _safe_agent(agent_arg)
    state = _canonical_directory(state_arg, "state")
    _check_private_dir(state)
    # Project identity comes only from the private manifest; its path must still be real.
    matches = []
    for path in _manifest_candidates(state, agent):
        raw = _read_private_json(path, "workspace baseline manifest")
        if isinstance(raw, dict) and raw.get("format_version") == FORMAT_VERSION:
            matches.append((path, raw.get("project")))
        else:
            _fail("Workspace baseline manifest has invalid identity or version")
    if len(matches) != 1:
        _fail("Expected exactly one workspace manifest for this agent; pass an explicit project to disambiguate")
    manifest_path, project_value = matches[0]
    if not isinstance(project_value, str):
        _fail("Workspace manifest has no project identity")
    project = _canonical_directory(project_value, "project")
    lock_fd = _lock(project)
    try:
        data = _manifest_read(manifest_path, project, agent)
        _, workspace, _ = _layout(state, project, agent)
        _safe_state_child(state, f"agents/{_project_id(project)}/{agent}/profiles/home")
        if workspace.is_symlink() or not workspace.is_dir():
            _fail("Workspace path is missing or unsafe")
        _check_private_dir(workspace)
        current = _scan(project)
        agent_entries = _scan_workspace(workspace, data["entries"])
        diffs = _changes(data["entries"], current, agent_entries)
        diffs["ok"] = True
        diffs["workspace"] = os.fspath(workspace)
        diffs["project_id"] = _project_id(project)
        diffs["review_token"] = _review_token(project, agent, data["entries"], current, agent_entries)
        return diffs
    finally:
        os.close(lock_fd)


def _tree_bytes(root: Path) -> int:
    total = 0
    for directory, _dirs, files in os.walk(root, followlinks=False):
        for name in files:
            try:
                total += os.lstat(os.path.join(directory, name)).st_size
            except OSError:
                pass
    return total


def _prune(state_arg: str, keep: list[str], dry_run: bool = False) -> dict[str, Any]:
    """Remove working copies that belong to no current bot and hold no unmerged work.

    From the live room: copies left by bot ids that were no longer in the room
    had grown to 38 GB, three to four gigabytes each. A copy is removed only
    when everything in it is already in the project (or never left it); one
    with changes that never merged, a missing or unreadable baseline, or a
    project that is gone is kept and reported, because it may be the only
    place that work exists.
    """
    keep_set = {_safe_agent(agent) for agent in keep}
    state = _canonical_directory(state_arg, "state")
    _check_private_dir(state)
    removed: list[dict[str, Any]] = []
    kept: list[dict[str, str]] = []
    agents_root = state / "agents"
    if not agents_root.is_dir() or agents_root.is_symlink():
        return {"ok": True, "removed": removed, "kept": kept, "freed_bytes": 0}
    # No project lock: nothing writes to a copy whose bot is gone, and a
    # project changing during the scan can only make a copy look changed, so
    # it is kept. Holding the lock would stall the room's own file work.
    project_scans: dict[str, dict[str, dict[str, Any]]] = {}
    for project_dir in sorted(agents_root.iterdir()):
        if project_dir.is_symlink() or not project_dir.is_dir():
            continue
        for agent_root in sorted(project_dir.iterdir()):
            agent = agent_root.name
            if agent in keep_set or not AGENT_RE.fullmatch(agent):
                continue
            if agent_root.is_symlink() or not agent_root.is_dir() or agent_root.stat().st_uid != os.getuid():
                kept.append({"agent": agent, "reason": "not a private directory"})
                continue
            manifest = state / "manifests" / project_dir.name / f"{agent}.json"
            try:
                raw = _read_private_json(manifest, "workspace baseline manifest")
                project_value = raw.get("project") if isinstance(raw, dict) else None
                if not isinstance(project_value, str):
                    raise WorkspaceError("no project identity")
                project = _canonical_directory(project_value, "project")
            except (WorkspaceError, OSError, ValueError, json.JSONDecodeError):
                kept.append({"agent": agent, "reason": "its baseline or project could not be read"})
                continue
            if _project_id(project) != project_dir.name:
                kept.append({"agent": agent, "reason": "its baseline names another project"})
                continue
            try:
                data = _manifest_read(manifest, project, agent)
                workspace = agent_root / "project"
                if workspace.is_symlink() or not workspace.is_dir():
                    raise WorkspaceError("workspace missing")
                current = project_scans.get(project_dir.name)
                if current is None:
                    current = project_scans[project_dir.name] = _scan(project)
                diffs = _changes(data["entries"], current, _scan_workspace(workspace, data["entries"]))
            except (WorkspaceError, OSError, ValueError, json.JSONDecodeError):
                kept.append({"agent": agent, "reason": "its changes could not be checked"})
                continue
            pending = len(diffs["changed"]) + len(diffs["added"]) + len(diffs["deleted"])
            if pending:
                kept.append({"agent": agent, "reason": f"{pending} change(s) never merged into the project"})
                continue
            size = _tree_bytes(agent_root)
            if not dry_run:
                shutil.rmtree(agent_root)
                manifest.unlink(missing_ok=True)
            removed.append({"agent": agent, "bytes": size})
    return {"ok": True, "dry_run": dry_run, "removed": removed, "kept": kept, "freed_bytes": sum(item["bytes"] for item in removed)}


def _gpu_device_args() -> list[str]:
    """Pass the machine's graphics card into a sandbox.

    Without it a bot only sees a software renderer, so frame rates, shaders,
    CUDA and GPU tests measure the CPU instead. Device nodes only: no display
    server, input devices, or other hardware.
    """
    devices = ["/dev/dri", "/dev/kfd", "/dev/nvidiactl", "/dev/nvidia-modeset",
               "/dev/nvidia-uvm", "/dev/nvidia-uvm-tools"]
    devices.extend(sorted(glob.glob("/dev/nvidia[0-9]*")))
    args: list[str] = []
    for device in devices:
        args.extend(["--dev-bind-try", device, device])
    return args


# What a bot's runtime must never read from the owner's home. The runtime sees
# the rest of the filesystem read-only, because toolchains and the CLIs
# themselves live all over it (Codex and Grok keep their programs beside their
# sign-ins), so secrets are hidden one by one: whole folders where no program
# lives, single files where one does. OrgBots' own data is hidden too: it holds
# every room's access token, which would let a bot act as the owner anywhere.
_OWNER_SECRET_DIRS = (
    ".ssh", ".gnupg", ".password-store", ".local/share/keyrings", ".pki",
    ".gemini", ".config/opencode", ".local/share/opencode",
    ".aws", ".azure", ".config/gcloud", ".kube", ".docker", ".terraform.d",
    ".config/gh", ".config/hub",
    ".mozilla", ".config/google-chrome", ".config/chromium", ".config/BraveSoftware",
    ".config/Claude", ".local/share/orgbot", ".config/orgbot",
    # The app's own support folder: tool approvals, the allowance ledger and
    # the registry of outside tools.
    ".local/share/com.orgbot.app",
    # Deploy and hosting tools that keep a sign-in in the home folder.
    ".config/.wrangler", ".wrangler", ".cloudflared", ".config/netlify", ".netlify",
    ".vercel", ".local/share/com.vercel.cli", ".fly", ".config/doctl", ".supabase",
    ".config/stripe", ".railway", ".config/configstore",
)

# Never lent, whatever the owner lists: OrgBots' own data, private keys, and
# the model CLIs' sign-ins (bots get their own copies of those).
_NEVER_LENT = (
    ".local/share/orgbot", ".config/orgbot", ".local/share/com.orgbot.app",
    ".ssh", ".gnupg", ".codex", ".claude", ".claude.json", ".grok",
    ".gemini", ".config/opencode", ".local/share/opencode",
)
_OWNER_SECRET_FILES = (
    ".codex/auth.json", ".grok/auth.json", ".claude/.credentials.json",
    ".claude/.firecrawl.env", ".claude/.claude.json", ".claude.json",
    ".netrc", ".git-credentials", ".config/git/credentials", ".npmrc", ".pypirc",
    ".cargo/credentials", ".cargo/credentials.toml", ".vault-token",
    ".config/rclone/rclone.conf", ".bash_history", ".zsh_history", ".python_history",
)


def _bot_ssh_dir() -> Path | None:
    """The folder bots see as ~/.ssh, when the owner has set one up.

    It holds only the keys, host entries and known hosts meant for bots, so
    `ssh <host>` works for the servers the owner chose and reaches nothing
    else. Anything unsafe about it means no SSH access, not a failed turn.
    """
    raw_xdg = os.environ.get("XDG_DATA_HOME")
    account_home = Path(pwd.getpwuid(os.getuid()).pw_dir)
    data = Path(raw_xdg) if raw_xdg and Path(raw_xdg).is_absolute() else account_home / ".local/share"
    folder = data / "orgbot" / "bot-ssh"
    try:
        info = folder.lstat()
    except OSError:
        return None
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        return None
    return folder


def _inside(path: Path, root: Path) -> bool:
    return path == root or root in path.parents


def _owner_secret_mask_args(owner_home: Path, bot_ssh: Path | None, hide: Path | None = None) -> list[str]:
    """Hide the owner's secrets from a runtime that mounts / read-only.

    `bot_ssh` is the bot server key folder to show as ~/.ssh, given only to a
    turn the owner allowed it; otherwise ~/.ssh is empty. `hide` is that folder
    wherever it lives, masked at its own path too, so a folder outside the
    usual data directory cannot be read there instead.
    """
    args: list[str] = []
    for relative, is_dir in [(r, True) for r in _OWNER_SECRET_DIRS] + [(r, False) for r in _OWNER_SECRET_FILES]:
        path = owner_home / relative
        if path.is_symlink():
            path = Path(os.path.realpath(path))
        if is_dir and path.is_dir():
            # OpenSSH reads the account's home, not $HOME, so this is the
            # ~/.ssh a bot's `ssh` sees.
            if relative == ".ssh" and bot_ssh is not None:
                args.extend(["--ro-bind", os.fspath(bot_ssh), os.fspath(path)])
            else:
                args.extend(["--size", "65536", "--tmpfs", os.fspath(path)])
        elif not is_dir and path.is_file():
            args.extend(["--ro-bind", "/dev/null", os.fspath(path)])
    # OrgBots' data follows XDG_DATA_HOME, which can sit outside the home's
    # .local/share: every room's tokens and every bot's private home are there.
    # Before the key folder's own mask, which then shows empty inside it.
    raw_xdg = os.environ.get("XDG_DATA_HOME")
    if raw_xdg and os.path.isabs(raw_xdg):
        data = Path(os.path.realpath(Path(raw_xdg) / "orgbot"))
        if data.is_dir() and data != Path(os.path.realpath(owner_home / ".local/share/orgbot")):
            args.extend(["--size", "65536", "--tmpfs", os.fspath(data)])
    if hide is not None and not _inside(Path(os.path.realpath(hide)), Path(os.path.realpath(owner_home / ".local/share/orgbot"))):
        args.extend(["--size", "65536", "--tmpfs", os.fspath(hide)])
    return args


def _owner_relative(raw: str) -> Path:
    """A path the owner lends, as given: plain and relative to their home."""
    parts = raw.split("/")
    if not raw or raw.startswith("/") or any(part in ("", ".", "..") for part in parts):
        _fail(f"Lent path must be a plain path relative to the home folder: {raw!r}")
    if any(raw == never or raw.startswith(never + "/") or never.startswith(raw + "/") for never in _NEVER_LENT):
        _fail(f"This path is never lent to bots: {raw}")
    return Path(raw)


def _owner_hidden_args(owner_home: Path, hidden: list[str]) -> list[str]:
    """Hide each path the owner lends where it lives, like the built-in secrets."""
    args: list[str] = []
    home = Path(os.path.realpath(owner_home))
    for raw in hidden:
        path = owner_home / _owner_relative(raw)
        real = Path(os.path.realpath(path))
        if not _inside(real, home):
            continue
        if real.is_dir():
            args.extend(["--size", "65536", "--tmpfs", os.fspath(real)])
        elif real.is_file():
            args.extend(["--ro-bind", "/dev/null", os.fspath(real)])
    return args


def _lent_args(owner_home: Path, private_home: Path, private_home_mount: str, lent: list[str]) -> list[str]:
    """Mount what the owner lent this turn into the bot's home, read-write.

    Read-write so a tool can renew its sign-in; the owner's own copy is the
    one mounted, so a renewal reaches them too. A path that leaves the home
    folder by a link or reaches what is never lent stops the turn with a
    reason. One that is not on this computer is skipped with a note: the owner
    may lend a tool they have not signed in to yet, and the bot then finds it
    signed out rather than losing its turn.
    """
    args: list[str] = []
    home = Path(os.path.realpath(owner_home))
    for raw in lent:
        relative = _owner_relative(raw)
        source = Path(os.path.realpath(owner_home / relative))
        if not _inside(source, home) or source == home:
            _fail(f"Lent path leaves the home folder: {raw}")
        real_relative = source.relative_to(home).as_posix()
        _owner_relative(real_relative)
        if not (source.is_dir() or source.is_file()):
            print(f"orgbot-workspace: lent ~/{raw} is not on this computer; continuing without it", file=sys.stderr)
            continue
        # Mount points in the bot's home, created on its private disk folder.
        target = Path(private_home_mount) / relative
        for depth in range(1, len(relative.parts)):
            partial = relative.parts[:depth]
            on_disk = private_home.joinpath(*partial)
            if on_disk.is_symlink():
                _fail(f"Bot home path is a link: {'/'.join(partial)}")
            args.extend(["--dir", os.fspath(Path(private_home_mount).joinpath(*partial))])
        if source.is_dir():
            args.extend(["--dir", os.fspath(target)])
        args.extend(["--bind", os.fspath(source), os.fspath(target)])
    return args


def _keyring_sign_in_args(owner_home: Path, lent: list[str], sandbox_path: list[str]) -> list[str]:
    """Environment for a lent GitHub CLI whose token lives in the keyring.

    From the live room: the owner's GitHub CLI keeps its token in the desktop
    keyring, which a sandbox cannot reach, so a bot lent ~/.config/gh pushed
    as nobody and got a 401. The owner's gh is asked for the token outside
    the sandbox, for this turn only, and gh and git inside use it; the
    keyring itself is never opened to the bot. Nothing found leaves the bot
    signed out, as with a tool the owner has not signed in to. The values go
    to bubblewrap through a pipe, never on a command line.
    """
    if ".config/gh" not in lent:
        return []
    hosts = owner_home / ".config/gh/hosts.yml"
    try:
        if "oauth_token:" in hosts.read_text(encoding="utf-8", errors="replace"):
            return []
    except OSError:
        return []
    gh = shutil.which("gh")
    if gh is None:
        return []
    try:
        result = subprocess.run([gh, "auth", "token", "--hostname", "github.com"], check=False, capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.TimeoutExpired):
        result = None
    token = result.stdout.strip() if result is not None and result.returncode == 0 else ""
    if not re.fullmatch(r"[A-Za-z0-9_]{20,255}", token):
        print("orgbot-workspace: the lent GitHub CLI has no sign-in in the keyring; continuing without it", file=sys.stderr)
        return []
    args = ["--setenv", "GH_TOKEN", token]
    inside = shutil.which("gh", path=os.pathsep.join(sandbox_path))
    if inside is not None:
        # git asks gh, which answers with GH_TOKEN; the empty helper first
        # drops any other helper configured for GitHub.
        args.extend([
            "--setenv", "GIT_CONFIG_COUNT", "2",
            "--setenv", "GIT_CONFIG_KEY_0", "credential.https://github.com.helper", "--setenv", "GIT_CONFIG_VALUE_0", "",
            "--setenv", "GIT_CONFIG_KEY_1", "credential.https://github.com.helper", "--setenv", "GIT_CONFIG_VALUE_1", f"!{os.path.realpath(inside)} auth git-credential",
        ])
    return args


def _packaged_trash_args(path_dirs: list[str], packaged: Path = Path("/usr/lib/orgbot/orgbot-trash.py")) -> list[str]:
    """Make `orgbot-trash` in a bot's sandbox the packaged helper.

    An older per-user install (~/.local/bin/orgbot-trash) can come first on
    PATH, and it runs a helper from $HOME, which in the sandbox is the bot's
    private home, so every removal failed. The owner's copy is left alone
    outside the sandbox: their cleanup timer may still use it.
    """
    if not packaged.is_file():
        return []
    args: list[str] = []
    for entry in path_dirs:
        candidate = Path(entry) / "orgbot-trash"
        if not candidate.exists():
            continue
        if Path(os.path.realpath(candidate)) == Path(os.path.realpath(packaged)):
            break
        args.extend(["--ro-bind", os.fspath(packaged), os.path.realpath(candidate)])
    return args


def _verify_sandbox_args(workspace: Path, program: list[str]) -> list[str]:
    """Build a minimal, networkless root for reviewer test commands."""
    args = [
        "bwrap", "--die-with-parent", "--new-session", "--unshare-pid",
        "--unshare-ipc", "--unshare-uts", "--unshare-net",
        "--size", "1073741824", "--tmpfs", "/",
        "--dev", "/dev", "--proc", "/proc",
        "--size", "268435456", "--tmpfs", "/tmp", "--dir", "/tmp/orgbot-workspace",
        "--size", "67108864", "--tmpfs", "/home", "--dir", "/home/orgbot",
        "--size", "67108864", "--tmpfs", "/root",
        "--size", "16777216", "--tmpfs", "/run", "--dir", "/etc",
        *_gpu_device_args(),
    ]
    # These are runtime files only; the root filesystem starts empty and no
    # host configuration, user data, state, or profiles are mounted.
    for source in ("/usr", "/usr/local", "/bin", "/sbin", "/lib", "/lib64"):
        path = Path(source)
        if path.exists():
            args.extend(["--dir", source, "--ro-bind", source, source])
    cache = Path("/etc/ld.so.cache")
    if cache.is_file() and not cache.is_symlink():
        args.extend(["--ro-bind", os.fspath(cache), "/etc/ld.so.cache"])
    args.extend(["--ro-bind", os.fspath(workspace), "/tmp/orgbot-workspace"])

    safe_path = "/usr/local/bin:/usr/bin:/bin"
    launch = list(program)
    first = Path(launch[0])
    if first.is_absolute():
        try:
            relative = first.relative_to(workspace)
        except ValueError:
            pass
        else:
            launch[0] = f"/tmp/orgbot-workspace/{relative.as_posix()}"
    elif "/" not in launch[0]:
        resolved = shutil.which(launch[0], path=safe_path)
        if resolved is None:
            _fail(f"Program is not available in the verification PATH: {launch[0]}")
        launch[0] = os.path.realpath(resolved)
    args.extend([
        "--clearenv", "--setenv", "PATH", safe_path,
        "--setenv", "HOME", "/home/orgbot", "--setenv", "TMPDIR", "/tmp",
        "--setenv", "LANG", "C.UTF-8", "--setenv", "ORGBOT_WORKSPACE", "/tmp/orgbot-workspace",
        "--chdir", "/tmp/orgbot-workspace", "--", *launch,
    ])
    return args


def _agy_profile_paths(*, create: bool) -> dict[str, Path] | None:
    raw_xdg = os.environ.get("XDG_DATA_HOME")
    xdg_data = Path(raw_xdg) if raw_xdg else Path.home() / ".local/share"
    if not xdg_data.is_absolute():
        _fail("XDG_DATA_HOME must be an absolute path for Antigravity auth")
    root = xdg_data / "orgbot" / "agy"
    if create:
        _mkdir_chain_no_symlinks(root)
    else:
        current = Path(root.anchor)
        for part in root.parts[1:]:
            current /= part
            try:
                info = current.lstat()
            except FileNotFoundError:
                return None
            except OSError as exc:
                _fail(f"Cannot inspect private Antigravity auth path: {exc}")
            if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
                _fail("Private Antigravity auth path contains an unsafe component")
    if not root.exists():
        return None
    _ensure_private_dir(root)
    initialize_store = not any(root.iterdir())
    managed_root = {
        "home", "keyring-data", "keyring-service-home", "keyring-service-config",
        "keyring-service-state", "keyring-service-cache", "empty-project", "auth.lock",
    }
    if {entry.name for entry in root.iterdir()} - managed_root:
        _fail("Private Antigravity auth store contains unmanaged files")
    home = root / "home"
    keyring_data = root / "keyring-data"
    service_home = root / "keyring-service-home"
    service_config = root / "keyring-service-config"
    service_state = root / "keyring-service-state"
    service_cache = root / "keyring-service-cache"
    project = root / "empty-project"
    for path in (home, keyring_data, service_home, service_config, service_state, service_cache, project):
        if create:
            if not initialize_store and not path.exists():
                _fail("Private Antigravity auth profile is incomplete; refusing reset")
            _ensure_private_dir(path)
        elif not path.exists():
            if any(root.iterdir()):
                _fail("Private Antigravity auth profile is incomplete; refusing reset")
            return None
        else:
            _check_private_dir(path)
    config = home / ".config"
    data = home / ".local/share"
    state = home / ".local/state"
    cache = home / ".cache"
    for path in (config, data, state, cache):
        if create:
            if not initialize_store and not path.exists():
                _fail("Private Antigravity auth profile is incomplete; refusing reset")
            _mkdir_chain_no_symlinks(path)
        elif not path.exists():
            if any(root.iterdir()):
                _fail("Private Antigravity auth profile is incomplete; refusing reset")
            return None
        _check_private_dir(path)
    gemini = home / ".gemini"
    cli_config = gemini / "antigravity-cli"
    global_config = gemini / "config"
    plugins = global_config / "plugins"
    projects = global_config / "projects"
    for path in (gemini, cli_config, global_config, plugins):
        if create:
            if not initialize_store and not path.exists():
                _fail("Private Antigravity auth profile is incomplete; refusing reset")
            _mkdir_chain_no_symlinks(path)
        elif not path.exists():
            if any(root.iterdir()):
                _fail("Private Antigravity auth profile is incomplete; refusing reset")
            return None
        _check_private_dir(path)
    settings = cli_config / "settings.json"
    mcp_config = global_config / "mcp_config.json"
    if create:
        if not initialize_store and (not settings.exists() or not mcp_config.exists()):
            _fail("Private Antigravity auth profile is incomplete; refusing reset")
        settings_value = {
            "useG1Credits": False,
            "permissions": {
                "allow": [],
                "ask": [],
                "deny": [
                    "command(*)",
                    "unsandboxed(*)",
                    "read_file(*)",
                    "write_file(*)",
                    "read_url(*)",
                    "execute_url(*)",
                    "mcp(*)",
                ],
            },
        }
        _write_private_json(settings, settings_value)
        _write_private_json(mcp_config, {"mcpServers": {}})
    for path in (settings, mcp_config):
        _check_private_file(path)
    if settings.read_bytes() != _canonical_json_bytes({
        "useG1Credits": False,
        "permissions": {
            "allow": [],
            "ask": [],
            "deny": [
                "command(*)", "unsandboxed(*)", "read_file(*)", "write_file(*)",
                "read_url(*)", "execute_url(*)", "mcp(*)",
            ],
        },
    }) or mcp_config.read_bytes() != _canonical_json_bytes({"mcpServers": {}}):
        _fail("Private Antigravity settings differ from the controlled deny profile")
    if {entry.name for entry in plugins.iterdir()}:
        _fail("Private Antigravity plugin directory must remain empty")
    global_config_entries = {entry.name for entry in global_config.iterdir()}
    if global_config_entries not in ({"mcp_config.json", "plugins"}, {"mcp_config.json", "plugins", "projects"}):
        _fail("Private Antigravity global configuration contains unmanaged files")
    if global_config_entries == {"mcp_config.json", "plugins", "projects"}:
        _check_private_dir(projects)
    elif projects.exists():
        _fail("Private Antigravity projects path is inconsistent")
    _ensure_default_project_seed(
        {"projects": projects, "global_config": global_config, "mcp_config": mcp_config, "plugins": plugins},
        create=create,
    )
    agents = project / ".agents"
    if create:
        if not initialize_store and not agents.exists():
            _fail("Private Antigravity auth profile is incomplete; refusing reset")
        _ensure_private_dir(agents)
    elif not agents.exists():
        if any(root.iterdir()):
            _fail("Private Antigravity auth profile is incomplete; refusing reset")
        return None
    _check_private_dir(agents)
    if any(agents.iterdir()):
        _fail("Private Antigravity workspace .agents mask must be empty")
    if {entry.name for entry in project.iterdir()} != {".agents"}:
        _fail("Private Antigravity sign-in workspace must remain empty")
    return {
        "root": root, "home": home, "project": project, "config": config,
        "data": data, "state": state, "cache": cache, "keyring_data": keyring_data,
        "service_home": service_home, "service_config": service_config,
        "service_state": service_state, "service_cache": service_cache,
        "settings": settings, "cli_config": cli_config, "global_config": global_config,
        "mcp_config": mcp_config, "plugins": plugins, "projects": projects,
    }


def _canonical_json_bytes(value: dict[str, Any]) -> bytes:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")


def _validate_default_project_seed(path: Path) -> None:
    _check_private_file(path)
    if path.stat().st_size > 16 * 1024:
        _fail("Private Antigravity default project metadata is too large")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        _fail("Private Antigravity default project metadata is invalid")
    if not isinstance(value, dict) or value != AGY_DEFAULT_PROJECT:
        _fail("Private Antigravity default project metadata is not the controlled seed")


def _ensure_default_project_seed(profile: dict[str, Path], *, create: bool) -> None:
    projects = profile["projects"]
    global_config = profile["global_config"]
    try:
        projects_info = projects.lstat()
    except FileNotFoundError:
        if not create:
            return
        if {entry.name for entry in global_config.iterdir()} != {"mcp_config.json", "plugins"}:
            _fail("Private Antigravity project seed migration is unsafe")
        if profile["mcp_config"].read_bytes() != _canonical_json_bytes({"mcpServers": {}}) or any(profile["plugins"].iterdir()):
            _fail("Private Antigravity project seed migration is unsafe")
        _mkdir_chain_no_symlinks(projects)
        projects_info = projects.lstat()
    except OSError as exc:
        _fail(f"Cannot inspect private Antigravity project metadata: {exc}")
    if stat.S_ISLNK(projects_info.st_mode) or not stat.S_ISDIR(projects_info.st_mode) or projects_info.st_uid != os.getuid() or projects_info.st_mode & 0o077:
        _fail("Private Antigravity projects directory is unsafe")
    entries = list(projects.iterdir())
    seed = projects / AGY_DEFAULT_PROJECT_FILENAME
    if any(entry.name != AGY_DEFAULT_PROJECT_FILENAME for entry in entries):
        _fail("Private Antigravity projects directory contains unmanaged files")
    try:
        seed.lstat()
    except FileNotFoundError:
        if not create:
            return
        _write_private_json(seed, AGY_DEFAULT_PROJECT)
        dir_fd = os.open(projects, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
        return
    except OSError as exc:
        _fail(f"Cannot inspect private Antigravity project metadata: {exc}")
    _validate_default_project_seed(seed)


def _check_private_file(path: Path) -> None:
    try:
        info = path.lstat()
    except OSError as exc:
        _fail(f"Private Antigravity file is unavailable: {exc}")
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077 or info.st_nlink != 1:
        _fail("Private Antigravity file is unsafe")


def _write_private_json(path: Path, value: dict[str, Any]) -> None:
    payload = _canonical_json_bytes(value)
    try:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    except FileExistsError:
        _check_private_file(path)
        if path.read_bytes() != payload:
            _fail("Existing private Antigravity configuration is unmanaged")
        return
    except OSError as exc:
        _fail(f"Cannot create private Antigravity configuration: {exc}")
    try:
        view = memoryview(payload)
        while view:
            written = os.write(fd, view)
            view = view[written:]
        os.fsync(fd)
    finally:
        os.close(fd)


def _agy_keyring_has_data(profile: dict[str, Path]) -> bool:
    keyrings = profile["keyring_data"] / "keyrings"
    try:
        info = keyrings.lstat()
    except FileNotFoundError:
        return False
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        _fail("Private Antigravity keyring directory is unsafe")
    found = False
    for parent, directories, files in os.walk(keyrings, followlinks=False):
        parent_path = Path(parent)
        _check_private_dir(parent_path)
        for name in directories:
            child = parent_path / name
            child_info = child.lstat()
            if stat.S_ISLNK(child_info.st_mode) or not stat.S_ISDIR(child_info.st_mode) or child_info.st_uid != os.getuid() or child_info.st_mode & 0o077:
                _fail("Private Antigravity keyring contains an unsafe directory")
            found = True
        for name in files:
            child = parent_path / name
            _check_private_file(child)
            found = True
    return found


def _secret_item_paths(bus_address: str, attributes: dict[str, str], *, timeout: float = 5.0) -> list[str]:
    executable = shutil.which("dbus-send", path="/usr/bin:/bin")
    if executable is None:
        _fail("dbus-send is required for Antigravity keyring metadata checks")
    flat_attributes = [part for pair in attributes.items() for part in pair]
    dictionary = "dict:string:string:" + ",".join(flat_attributes)
    env = {"PATH": "/usr/bin:/bin", "DBUS_SESSION_BUS_ADDRESS": bus_address}
    try:
        result = _agy_run(
            [executable, "--session", "--print-reply", "--dest=org.freedesktop.secrets",
             "/org/freedesktop/secrets", "org.freedesktop.Secret.Service.SearchItems", dictionary],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            timeout=timeout, env=env,
        )
    except _AgyAuthCancelled:
        raise
    except (OSError, subprocess.TimeoutExpired):
        _fail("Antigravity Secret Service metadata query failed")
    if result.returncode != 0 or len(result.stdout) > 1024 * 1024:
        _fail("Antigravity Secret Service metadata query failed")
    path_line = rb'\s*object path "(/org/freedesktop/secrets/[A-Za-z0-9_/]+)"\n'
    reply = re.fullmatch(
        rb'method return [^\n]*\n\s*array \[\n(?P<unlocked>(?:' + path_line + rb')*)\s*\]\n'
        rb'\s*array \[\n(?P<locked>(?:' + path_line + rb')*)\s*\]\s*',
        result.stdout,
    )
    if reply is None:
        _fail("Antigravity Secret Service returned malformed metadata")
    return [path.decode("ascii") for path in re.findall(
        rb'object path "(/org/freedesktop/secrets/[A-Za-z0-9_/]+)"',
        reply.group("unlocked") + reply.group("locked"),
    )]


def _secret_item_paths_for_host(attributes: dict[str, str]) -> list[str]:
    bus_address = os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")
    if not bus_address.startswith("unix:"):
        _fail("A desktop Secret Service session is required for Antigravity sign-in")
    return _secret_item_paths(bus_address, attributes)


def _secret_tool() -> str:
    executable = shutil.which("secret-tool", path="/usr/bin:/bin")
    if executable is None:
        _fail("libsecret-tools is required for Antigravity sign-in")
    return executable


def _host_wrapping_passphrase(profile: dict[str, Path]) -> bytearray:
    matches = _secret_item_paths_for_host(AGY_WRAP_ATTRIBUTES)
    existing_store = _agy_keyring_has_data(profile)
    if len(matches) > 1:
        _fail("Multiple OrgBots Antigravity wrapping-key items exist; refusing ambiguous auth state")
    if not matches and existing_store:
        _fail("Private Antigravity keyring exists without its wrapping key; refusing reset")
    tool = _secret_tool()
    host_bus = os.environ["DBUS_SESSION_BUS_ADDRESS"]
    host_env = {"PATH": "/usr/bin:/bin", "DBUS_SESSION_BUS_ADDRESS": host_bus}
    if matches:
        try:
            result = _agy_run(
                [tool, "lookup", *[part for pair in AGY_WRAP_ATTRIBUTES.items() for part in pair]],
                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                timeout=8, env=host_env,
            )
        except _AgyAuthCancelled:
            raise
        except (OSError, subprocess.TimeoutExpired):
            _fail("OrgBots Antigravity wrapping-key item is locked or unavailable")
        lines = result.stdout.splitlines()
        if result.returncode != 0 or len(lines) != 1 or not re.fullmatch(rb"[A-Za-z0-9_-]{40,64}", lines[0]):
            _fail("OrgBots Antigravity wrapping-key item is locked or invalid")
        return bytearray(lines[0])
    passphrase = bytearray(secrets.token_urlsafe(32).encode("ascii"))
    try:
        try:
            result = _agy_run(
                [tool, "store", "--label=OrgBots Antigravity private keyring",
                 *[part for pair in AGY_WRAP_ATTRIBUTES.items() for part in pair]],
                input=bytes(passphrase) + b"\n", stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                timeout=10, env=host_env,
            )
        except _AgyAuthCancelled:
            raise
        except (OSError, subprocess.TimeoutExpired):
            _fail("Could not store OrgBots Antigravity wrapping key in Secret Service")
        if result.returncode != 0 or len(_secret_item_paths_for_host(AGY_WRAP_ATTRIBUTES)) != 1:
            _fail("OrgBots Antigravity wrapping key was not stored uniquely")
        return passphrase
    except Exception:
        _zero_bytes(passphrase)
        raise


def _zero_bytes(value: bytearray) -> None:
    for index in range(len(value)):
        value[index] = 0


class _AgyAuthCancelled(Exception):
    pass


def _agy_process_preexec(parent_pid: int) -> None:
    _set_process_limits()
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(1, signal.SIGTERM, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "prctl(PR_SET_PDEATHSIG) failed")
    if os.getppid() != parent_pid:
        os.kill(os.getpid(), signal.SIGTERM)


def _agy_owned_popen(args: list[str], **kwargs: Any) -> subprocess.Popen[bytes]:
    parent_pid = os.getpid()
    process = subprocess.Popen(args, preexec_fn=lambda: _agy_process_preexec(parent_pid), **kwargs)
    _AGY_CHILDREN.add(process)
    return process


def _agy_run(args: list[str], *, timeout: float, **kwargs: Any) -> subprocess.CompletedProcess[bytes]:
    input_bytes = kwargs.pop("input", None)
    if input_bytes is not None:
        if "stdin" in kwargs:
            raise ValueError("stdin and input arguments may not both be used")
        kwargs["stdin"] = subprocess.PIPE
    process = _agy_owned_popen(args, **kwargs)
    try:
        stdout, stderr = process.communicate(input_bytes, timeout=timeout)
    except subprocess.TimeoutExpired:
        _stop_owned_process(process)
        raise
    except BaseException:
        _stop_owned_process(process)
        raise
    finally:
        _AGY_CHILDREN.discard(process)
    return subprocess.CompletedProcess(args, process.returncode, stdout, stderr)


def _agy_signal_handler(_signum: int, _frame: Any) -> None:
    for process in tuple(_AGY_CHILDREN):
        if process.poll() is None:
            process.terminate()
    raise _AgyAuthCancelled()


@contextlib.contextmanager
def _agy_signal_scope():
    old_int = signal.signal(signal.SIGINT, _agy_signal_handler)
    old_term = signal.signal(signal.SIGTERM, _agy_signal_handler)
    try:
        yield
    finally:
        signal.signal(signal.SIGINT, old_int)
        signal.signal(signal.SIGTERM, old_term)


def _private_dbus_config(path: Path, runtime: Path) -> None:
    body = f'''<busconfig>
  <type>session</type>
  <listen>unix:path={runtime / "bus.sock"}</listen>
  <policy context="default">
    <deny send_destination="*"/>
    <allow send_destination="org.freedesktop.DBus"/>
    <allow send_destination="org.freedesktop.secrets"/>
    <allow send_type="method_return" send_requested_reply="true"/>
    <allow send_type="error" send_requested_reply="true"/>
    <allow receive_sender="*"/>
    <allow own="org.freedesktop.secrets"/>
  </policy>
</busconfig>
'''
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    try:
        os.write(fd, body.encode("utf-8"))
        os.fsync(fd)
    finally:
        os.close(fd)


def _stop_owned_process(process: subprocess.Popen[bytes] | None) -> None:
    if process is None:
        return
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=3)
    _AGY_CHILDREN.discard(process)


@contextlib.contextmanager
def _agy_private_secret_service(profile: dict[str, Path], passphrase: bytearray | None):
    with tempfile.TemporaryDirectory(prefix="orgbots-agy-", dir="/tmp") as temporary:
        runtime = Path(temporary)
        os.chmod(runtime, 0o700)
        config = runtime / "dbus.conf"
        _private_dbus_config(config, runtime)
        bus_address = f"unix:path={runtime / 'bus.sock'}"
        private_env = {
            "PATH": "/usr/local/bin:/usr/bin:/bin",
            "HOME": os.fspath(profile["service_home"]),
            "XDG_CONFIG_HOME": os.fspath(profile["service_config"]),
            "XDG_DATA_HOME": os.fspath(profile["keyring_data"]),
            "XDG_STATE_HOME": os.fspath(profile["service_state"]),
            "XDG_CACHE_HOME": os.fspath(profile["service_cache"]),
            "XDG_RUNTIME_DIR": os.fspath(runtime),
            "DBUS_SESSION_BUS_ADDRESS": bus_address,
            "LANG": "C.UTF-8",
        }
        dbus = shutil.which("dbus-daemon", path="/usr/bin:/bin")
        keyring = shutil.which("gnome-keyring-daemon", path="/usr/bin:/bin")
        if dbus is None or keyring is None:
            _fail("dbus-daemon and gnome-keyring-daemon are required for Antigravity auth")
        bus_process = None
        keyring_process = None
        try:
            bus_process = _agy_owned_popen(
                [dbus, f"--config-file={config}", "--nofork", "--nopidfile"],
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                env={"PATH": private_env["PATH"], "HOME": private_env["HOME"], "XDG_RUNTIME_DIR": private_env["XDG_RUNTIME_DIR"]},
            )
            socket_path = runtime / "bus.sock"
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline and not socket_path.exists():
                if bus_process.poll() is not None:
                    _fail("Private Antigravity D-Bus did not start")
                time.sleep(0.05)
            if not socket_path.exists() or not stat.S_ISSOCK(socket_path.lstat().st_mode):
                _fail("Private Antigravity D-Bus socket is unavailable")
            key_args = [keyring, "--foreground", "--components=secrets"]
            if passphrase is not None:
                key_args.append("--unlock")
            keyring_process = _agy_owned_popen(
                key_args, stdin=subprocess.PIPE if passphrase is not None else subprocess.DEVNULL,
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=private_env,
            )
            if passphrase is not None:
                assert keyring_process.stdin is not None
                keyring_process.stdin.write(bytes(passphrase) + b"\n")
                keyring_process.stdin.flush()
                keyring_process.stdin.close()
            deadline = time.monotonic() + 5
            owner = False
            while time.monotonic() < deadline:
                if keyring_process.poll() is not None:
                    _fail("Private Antigravity Secret Service did not start")
                try:
                    result = _agy_run(
                        [shutil.which("dbus-send", path="/usr/bin:/bin") or "dbus-send", "--session", "--print-reply",
                         "--dest=org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus.NameHasOwner",
                         "string:org.freedesktop.secrets"],
                        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                        timeout=0.5, env={"PATH": private_env["PATH"], "DBUS_SESSION_BUS_ADDRESS": bus_address},
                    )
                except (OSError, subprocess.TimeoutExpired):
                    result = None
                if result is not None and result.returncode == 0 and b"boolean true" in result.stdout:
                    owner = True
                    break
                time.sleep(0.05)
            if not owner:
                _fail("Private Antigravity Secret Service did not acquire its bus name")
            yield private_env, runtime
        finally:
            _stop_owned_process(keyring_process)
            _stop_owned_process(bus_process)


def _agy_sandbox_args(
    profile: dict[str, Path],
    binary: Path,
    runtime: Path,
    *,
    program: list[str] | None = None,
    relay: Path | None = None,
    room_socket: Path | None = None,
    create_project_seed: bool = True,
    writable_xdg: bool = True,
) -> list[str]:
    _ensure_default_project_seed(profile, create=create_project_seed)
    empty_agents = profile.get("empty_agents", profile["project"] / ".agents")
    uid = os.getuid()
    runtime_mount = f"/run/user/{uid}/orgbots-agy-runtime"
    args = [
        "bwrap", "--die-with-parent", "--new-session", "--unshare-pid", "--unshare-ipc", "--unshare-uts",
        "--size", "1073741824", "--tmpfs", "/", "--dev", "/dev", "--proc", "/proc",
        "--size", "268435456", "--tmpfs", "/tmp", "--dir", "/tmp/orgbot-workspace",
        "--dir", "/opt", "--dir", "/opt/orgbots",
        "--size", "67108864", "--tmpfs", "/home", "--dir", "/home/orgbot",
        "--size", "67108864", "--tmpfs", "/root", "--size", "16777216", "--tmpfs", "/run",
        "--dir", "/run/user", "--dir", f"/run/user/{uid}", "--dir", runtime_mount,
        *_gpu_device_args(),
    ]
    for source in ("/usr", "/usr/local", "/bin", "/sbin", "/lib", "/lib64"):
        if Path(source).exists():
            args.extend(["--dir", source, "--ro-bind", source, source])
    args.extend(["--dir", "/etc"])
    for source in ("/etc/ld.so.cache", "/etc/passwd", "/etc/group", "/etc/nsswitch.conf", "/etc/hosts", "/etc/ssl/certs/ca-certificates.crt"):
        path = Path(source)
        if path.is_file() and not path.is_symlink():
            args.extend(["--ro-bind", source, source])
    resolver = Path("/etc/resolv.conf")
    if resolver.is_file():
        resolver_bytes = resolver.read_bytes()
        resolver_copy = runtime / "resolv.conf"
        fd = os.open(resolver_copy, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
        try:
            os.write(fd, resolver_bytes)
        finally:
            os.close(fd)
        args.extend(["--ro-bind", os.fspath(resolver_copy), "/etc/resolv.conf"])
    args.extend([
        "--dir", "/home/orgbot/.config", "--dir", "/home/orgbot/.local", "--dir", "/home/orgbot/.local/share",
        "--dir", "/home/orgbot/.local/state", "--dir", "/home/orgbot/.cache",
        "--ro-bind", os.fspath(profile["home"]), "/home/orgbot",
    ])
    if writable_xdg:
        for key, target in (
            ("config", "/home/orgbot/.config"),
            ("data", "/home/orgbot/.local/share"),
            ("state", "/home/orgbot/.local/state"),
            ("cache", "/home/orgbot/.cache"),
        ):
            args.extend(["--bind", os.fspath(profile[key]), target])
    args.extend(["--bind", os.fspath(profile["cli_config"]), "/home/orgbot/.gemini/antigravity-cli"])
    args.extend([
        "--dir", runtime_mount,
        "--ro-bind", os.fspath(runtime / "bus.sock"), f"{runtime_mount}/bus.sock",
        "--ro-bind", os.fspath(binary), "/opt/orgbots/agy",
        "--ro-bind", os.fspath(profile["project"]), "/tmp/orgbot-workspace",
        "--ro-bind", os.fspath(empty_agents), "/tmp/orgbot-workspace/.agents",
        "--ro-bind", os.fspath(profile["settings"]), "/home/orgbot/.gemini/antigravity-cli/settings.json",
        "--ro-bind", os.fspath(profile["global_config"]), "/home/orgbot/.gemini/config",
    ])
    if relay is not None:
        args.extend(["--ro-bind", os.fspath(relay), "/opt/orgbots/room-relay"])
    if room_socket is not None:
        socket_mount = f"/run/user/{uid}/orgbot-room-mcp.sock"
        args.extend(["--bind", os.fspath(room_socket), socket_mount])
    private_bus = f"unix:path={runtime_mount}/bus.sock"
    args.extend([
        "--clearenv", "--setenv", "PATH", "/opt/orgbots:/usr/local/bin:/usr/bin:/bin",
        "--setenv", "HOME", "/home/orgbot", "--setenv", "XDG_CONFIG_HOME", "/home/orgbot/.config",
        "--setenv", "XDG_DATA_HOME", "/home/orgbot/.local/share",
        "--setenv", "XDG_STATE_HOME", "/home/orgbot/.local/state",
        "--setenv", "XDG_CACHE_HOME", "/home/orgbot/.cache",
        "--setenv", "XDG_RUNTIME_DIR", runtime_mount,
        "--setenv", "DBUS_SESSION_BUS_ADDRESS", private_bus,
        "--setenv", "BROWSER", "/bin/false", "--setenv", "LANG", "C.UTF-8",
        "--setenv", "TERM", "xterm-256color",
    ])
    if room_socket is not None:
        args.extend(["--setenv", "ORGBOT_ROOM_MCP_SOCKET", f"/run/user/{uid}/orgbot-room-mcp.sock"])
    args.extend(["--chdir", "/tmp/orgbot-workspace", "--", "/opt/orgbots/agy"])
    if program is not None:
        args.extend(program[1:])
    return args


def _agy_runtime_command(program: list[str], binary: Path) -> list[str]:
    if not program or not Path(program[0]).is_absolute():
        _fail("Antigravity command must start with its validated absolute binary path")
    try:
        requested = Path(program[0]).resolve(strict=True)
    except OSError as exc:
        _fail(f"Antigravity binary path is unavailable: {exc}")
    if requested != binary:
        _fail("Antigravity command binary does not match the installed CLI")
    arguments = program[1:]
    if len(arguments) < 5 or arguments[0] != "-p" or not arguments[1]:
        _fail("Antigravity command is not the controlled headless invocation")
    if arguments[2:5] != ["--disable-slash-commands", "--output-format", "stream-json"]:
        _fail("Antigravity command is not the controlled headless invocation")
    index = 5
    if index < len(arguments) and arguments[index] == "--conversation":
        if index + 1 >= len(arguments) or not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", arguments[index + 1]):
            _fail("Antigravity conversation identifier is invalid")
        index += 2
    if index + 2 != len(arguments) or arguments[index] != "--model" or not re.fullmatch(r"[A-Za-z0-9._-]{1,128}", arguments[index + 1]):
        _fail("Antigravity command must specify one explicit model")
    return arguments


def _agy_safe_executable(path: Path, label: str) -> Path:
    if not path.is_absolute():
        _fail(f"{label} path must be absolute")
    current = Path(path.anchor)
    for part in path.parts[1:]:
        current /= part
        try:
            if stat.S_ISLNK(current.lstat().st_mode):
                _fail(f"{label} path cannot contain symlink components")
        except OSError as exc:
            _fail(f"{label} path is unavailable: {exc}")
    resolved = path.resolve(strict=True)
    info = resolved.lstat()
    if (
        not stat.S_ISREG(info.st_mode)
        or info.st_uid not in (0, os.getuid())
        or info.st_nlink != 1
        or info.st_mode & 0o022
        or not os.access(resolved, os.X_OK)
    ):
        _fail(f"{label} binary is unsafe")
    return resolved


def _agy_json_file(path: Path, expected: dict[str, Any], label: str) -> None:
    _check_private_file(path)
    if path.stat().st_size > 16 * 1024:
        _fail(f"Private Antigravity {label} is too large")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        _fail(f"Private Antigravity {label} is invalid")
    if value != expected:
        _fail(f"Private Antigravity {label} differs from the controlled profile")


def _agy_validate_runtime_home(home: Path, room_socket: Path | None) -> dict[str, Path]:
    _check_private_dir(home)
    config = home / ".config"
    local = home / ".local"
    data = local / "share"
    state = local / "state"
    cache = home / ".cache"
    gemini = home / ".gemini"
    cli_config = gemini / "antigravity-cli"
    global_config = gemini / "config"
    settings = cli_config / "settings.json"
    mcp_config = global_config / "mcp_config.json"
    plugins = global_config / "plugins"
    projects = global_config / "projects"
    empty_agents = home / ".orgbot-empty-agents"
    for directory in (config, local, data, state, cache, gemini, cli_config, global_config, plugins, projects, empty_agents):
        _check_private_dir(directory)
    for directory, allowed in (
        (home, {".config", ".local", ".cache", ".gemini", ".orgbot-empty-agents"}),
        (config, set()),
        (local, {"share", "state"}),
        (data, set()),
        (state, set()),
        (cache, set()),
        (gemini, {"antigravity-cli", "config"}),
        (global_config, {"mcp_config.json", "plugins", "projects"}),
        (plugins, set()),
        (projects, {AGY_DEFAULT_PROJECT_FILENAME}),
        (empty_agents, set()),
    ):
        try:
            entries = {entry.name for entry in directory.iterdir()}
        except OSError as exc:
            _fail(f"Cannot inspect private Antigravity profile: {exc}")
        if directory == cli_config:
            if "settings.json" not in entries:
                _fail("Private Antigravity settings are missing")
        elif entries != allowed:
            _fail("Private Antigravity profile contains unmanaged configuration")
    settings_value = {
        "useG1Credits": False,
        "permissions": {
            "allow": ["mcp(orgbot_room/*)"] if room_socket is not None else [],
            "ask": [],
            "deny": [
                "command(*)", "unsandboxed(*)", "read_file(*)", "write_file(*)",
                "read_url(*)", "execute_url(*)",
            ] + ([] if room_socket is not None else ["mcp(*)"]),
        },
    }
    socket_mount = f"/run/user/{os.getuid()}/orgbot-room-mcp.sock"
    mcp_value = {
        "mcpServers": ({
            "orgbot_room": {
                "command": "/opt/orgbots/room-relay",
                "args": ["room-mcp", "--socket", socket_mount],
            },
        } if room_socket is not None else {}),
    }
    _agy_json_file(settings, settings_value, "settings")
    _agy_json_file(mcp_config, mcp_value, "MCP configuration")
    _validate_default_project_seed(projects / AGY_DEFAULT_PROJECT_FILENAME)
    _agy_validate_native_cli_data(cli_config)
    return {
        "home": home,
        "config": config, "data": data, "state": state, "cache": cache,
        "cli_config": cli_config, "global_config": global_config,
        "settings": settings, "mcp_config": mcp_config, "plugins": plugins,
        "projects": projects, "empty_agents": empty_agents,
    }


def _agy_validate_native_cli_data(cli_config: Path) -> None:
    count = 0
    total_size = 0
    for parent, directories, files in os.walk(cli_config, followlinks=False):
        parent_path = Path(parent)
        _check_private_dir(parent_path)
        for name in directories:
            child = parent_path / name
            try:
                info = child.lstat()
            except OSError as exc:
                _fail(f"Cannot inspect private Antigravity CLI data: {exc}")
            if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
                _fail("Private Antigravity CLI data contains an unsafe directory")
            count += 1
        for name in files:
            child = parent_path / name
            if child == cli_config / "settings.json":
                continue
            try:
                info = child.lstat()
            except OSError as exc:
                _fail(f"Cannot inspect private Antigravity CLI data: {exc}")
            if child == cli_config / "cli.log" and stat.S_ISLNK(info.st_mode):
                target_info = _agy_cli_log_link_target(cli_config, child, info)
                total_size += target_info.st_size
                count += 1
                continue
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077 or info.st_nlink != 1:
                _fail("Private Antigravity CLI data contains an unsafe file")
            total_size += info.st_size
            count += 1
        if count > MAX_ENTRIES or total_size > MAX_TREE_BYTES:
            _fail("Private Antigravity CLI data exceeds its safety limits")


def _agy_cli_log_link_target(cli_config: Path, link: Path, link_info: os.stat_result) -> os.stat_result:
    if link_info.st_uid != os.getuid() or link_info.st_nlink != 1:
        _fail("Private Antigravity CLI data contains an unsafe entry")
    try:
        relative = os.readlink(link)
    except OSError as exc:
        _fail(f"Cannot inspect private Antigravity CLI log link: {exc}")
    if not re.fullmatch(r"log/cli-[0-9]{8}_[0-9]{6}\.log", relative):
        _fail("Private Antigravity CLI data contains an unsafe entry")
    log_dir = cli_config / "log"
    _check_private_dir(log_dir)
    target = log_dir / relative.removeprefix("log/")
    try:
        info = target.lstat()
    except OSError as exc:
        _fail(f"Cannot inspect private Antigravity CLI log target: {exc}")
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077 or info.st_nlink != 1:
        _fail("Private Antigravity CLI data contains an unsafe file")
    return info


def _agy_ensure_workspace_agents_mount(workspace: Path) -> None:
    agents = workspace / ".agents"
    try:
        info = agents.lstat()
    except FileNotFoundError:
        agents.mkdir(mode=0o700)
        info = agents.lstat()
    except OSError as exc:
        _fail(f"Cannot inspect Antigravity workspace rules mask: {exc}")
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
        _fail("Antigravity workspace .agents mask is unsafe")


def _agy_run_protected(
    program: list[str],
    private_home: Path,
    workspace: Path,
    room_socket: Path | None,
    relay: Path | None,
    project_lock_fd: int,
) -> int:
    auth_lock_fd: int | None = None
    passphrase: bytearray | None = None
    previous_umask = os.umask(0o077)
    try:
        with _agy_signal_scope():
            auth_profile = _agy_profile_paths(create=False)
            if auth_profile is None:
                _fail("Antigravity private sign-in is required; run agy-auth login first")
            auth_lock_fd = _agy_auth_lock(auth_profile["root"])
            auth_profile = _agy_profile_paths(create=False)
            if auth_profile is None:
                _fail("Antigravity private sign-in is required; run agy-auth login first")
            if not _agy_keyring_has_data(auth_profile):
                _fail("Antigravity private sign-in is required; run agy-auth login first")
            passphrase = _host_wrapping_passphrase(auth_profile)
            binary = _agy_binary()
            runtime_arguments = _agy_runtime_command(program, binary)
            runtime_home = _agy_validate_runtime_home(private_home, room_socket)
            if room_socket is not None:
                if relay is None:
                    _fail("Antigravity room MCP relay is unavailable")
                relay = _agy_safe_executable(relay, "Antigravity MCP relay")
            _agy_ensure_workspace_agents_mount(workspace)
            runtime_home["project"] = workspace
            runtime_home["empty_agents"] = private_home / ".orgbot-empty-agents"
            with _agy_private_secret_service(auth_profile, passphrase) as (private_env, runtime):
                if not _secret_item_paths(private_env["DBUS_SESSION_BUS_ADDRESS"], {}):
                    _fail("Antigravity private sign-in has no stored credentials")
                args = _agy_sandbox_args(
                    runtime_home,
                    binary,
                    runtime,
                    program=[os.fspath(binary), *runtime_arguments],
                    relay=relay,
                    room_socket=room_socket,
                    create_project_seed=False,
                    writable_xdg=False,
                )
                os.close(project_lock_fd)
                project_lock_fd = -1
                process = _agy_owned_popen(
                    args,
                    env={"PATH": "/usr/local/bin:/usr/bin:/bin", "LANG": "C.UTF-8"},
                )
                try:
                    return process.wait()
                except BaseException:
                    _stop_owned_process(process)
                    raise
                finally:
                    _AGY_CHILDREN.discard(process)
    except _AgyAuthCancelled:
        _fail("Antigravity process was cancelled")
    finally:
        if passphrase is not None:
            _zero_bytes(passphrase)
        if auth_lock_fd is not None:
            os.close(auth_lock_fd)
        if project_lock_fd >= 0:
            os.close(project_lock_fd)


def _agy_binary() -> Path:
    owner_home = Path(pwd.getpwuid(os.getuid()).pw_dir)
    if not owner_home.is_absolute():
        _fail("The current owner home directory is invalid")
    raw = shutil.which("agy", path=os.pathsep.join((os.fspath(owner_home / ".local/bin"), "/usr/local/bin", "/usr/bin", "/bin")))
    if raw is None:
        _fail("Antigravity CLI `agy` is not installed")
    path = Path(raw).resolve(strict=True)
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or not os.access(path, os.X_OK) or info.st_nlink != 1 or info.st_mode & 0o022:
        _fail("Installed Antigravity CLI binary is unsafe")
    if info.st_uid not in (os.getuid(), 0):
        _fail("Installed Antigravity CLI has an unexpected owner")
    return path


def _agy_auth_preflight() -> None:
    for executable in ("bwrap", "dbus-daemon", "dbus-send", "gnome-keyring-daemon", "secret-tool"):
        if shutil.which(executable, path="/usr/local/bin:/usr/bin:/bin") is None:
            _fail(f"{executable} is required for isolated Antigravity sign-in")


def _agy_auth_status() -> dict[str, Any]:
    previous_umask = os.umask(0o077)
    try:
        return _agy_auth_status_inner()
    finally:
        os.umask(previous_umask)


def _agy_auth_status_inner() -> dict[str, Any]:
    _set_process_limits()
    profile = _agy_profile_paths(create=False)
    if profile is None:
        return {"ok": True, "credentials_present": False, "state": "pending_login"}
    lock_fd = _agy_auth_lock(profile["root"])
    try:
        if not _agy_keyring_has_data(profile):
            return {"ok": True, "credentials_present": False, "state": "pending_login"}
        with _agy_private_secret_service(profile, None) as (private_env, _runtime):
            paths = _secret_item_paths(private_env["DBUS_SESSION_BUS_ADDRESS"], {})
        return {
            "ok": True,
            "credentials_present": bool(paths),
            "state": "pending_preflight" if paths else "pending_login",
        }
    finally:
        os.close(lock_fd)


def _agy_auth_lock(root: Path) -> int:
    lock_path = root / "auth.lock"
    flags = os.O_CREAT | os.O_RDWR | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(lock_path, flags, 0o600)
    info = os.fstat(fd)
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077 or info.st_nlink != 1:
        os.close(fd)
        _fail("Private Antigravity auth lock is unsafe")
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        os.close(fd)
        _fail("Another Antigravity authentication operation is already running")
    return fd


def _agy_watch_credentials(process: subprocess.Popen[bytes], bus_address: str) -> tuple[bool, int | None, bool]:
    while True:
        # Keep a pre-query poll for process reaping, then re-poll after the DBus wait.
        process.poll()
        paths = _secret_item_paths(bus_address, {})
        exit_code = process.poll()
        if paths:
            stopped_by_helper = False
            if exit_code is None:
                _stop_owned_process(process)
                exit_code = process.returncode
                stopped_by_helper = exit_code == -signal.SIGTERM
            return True, exit_code, stopped_by_helper
        if exit_code is not None:
            return False, exit_code, False
        time.sleep(0.05)


def _agy_auth_login() -> dict[str, Any]:
    previous_umask = os.umask(0o077)
    try:
        with _agy_signal_scope():
            return _agy_auth_login_inner()
    except _AgyAuthCancelled:
        state = _agy_auth_status()
        return {
            "ok": False,
            "credentials_present": state["credentials_present"],
            "state": state["state"],
            "error": "Antigravity sign-in cancelled",
        }
    finally:
        os.umask(previous_umask)


def _agy_auth_login_inner() -> dict[str, Any]:
    _set_process_limits()
    if not (os.isatty(0) and os.isatty(1) and os.isatty(2)):
        _fail("Antigravity sign-in must run from an interactive terminal")
    profile = _agy_profile_paths(create=False)
    binary: Path | None = None
    if profile is None:
        binary = _agy_binary()
        _agy_auth_preflight()
        profile = _agy_profile_paths(create=True)
        assert profile is not None
    lock_fd = _agy_auth_lock(profile["root"])
    passphrase = None
    try:
        existing_store = _agy_keyring_has_data(profile)
        if existing_store:
            with _agy_private_secret_service(profile, None) as (locked_env, _runtime):
                locked_paths = _secret_item_paths(locked_env["DBUS_SESSION_BUS_ADDRESS"], {})
            if locked_paths:
                return {"ok": True, "credentials_present": True, "state": "pending_preflight"}
        if binary is None:
            binary = _agy_binary()
            _agy_auth_preflight()
        passphrase = _host_wrapping_passphrase(profile)
        detected_credentials = False
        native_exit_code: int | None = None
        stopped_by_helper = False
        with _agy_private_secret_service(profile, passphrase) as (_private_env, runtime):
            bus_address = _private_env["DBUS_SESSION_BUS_ADDRESS"]
            existing_paths = _secret_item_paths(bus_address, {})
            if existing_paths:
                detected_credentials = True
            else:
                assert binary is not None
                args = _agy_sandbox_args(profile, binary, runtime)
                process = _agy_owned_popen(
                    args, env={"PATH": "/usr/local/bin:/usr/bin:/bin", "LANG": "C.UTF-8"},
                )
                try:
                    detected_credentials, native_exit_code, stopped_by_helper = _agy_watch_credentials(process, bus_address)
                except BaseException:
                    _stop_owned_process(process)
                    raise
                finally:
                    _AGY_CHILDREN.discard(process)
        if not detected_credentials:
            return {
                "ok": False,
                "credentials_present": False,
                "state": "pending_login",
                "agy_exit_code": native_exit_code,
                "error": (f"Antigravity sign-in exited with code {native_exit_code} before saving credentials"
                          if native_exit_code is not None else "Antigravity sign-in ended before saving credentials"),
            }
        with _agy_private_secret_service(profile, None) as (locked_env, _runtime):
            persisted_paths = _secret_item_paths(locked_env["DBUS_SESSION_BUS_ADDRESS"], {})
        if not persisted_paths:
            _fail("Antigravity credentials did not persist after the private keyring relocked")
        if not stopped_by_helper and native_exit_code not in (None, 0):
            return {
                "ok": False,
                "credentials_present": True,
                "state": "pending_preflight",
                "agy_exit_code": native_exit_code,
                "error": f"Antigravity exited with code {native_exit_code} after storing credentials",
            }
        return {
            "ok": True,
            "credentials_present": True,
            "state": "pending_preflight",
        }
    finally:
        if passphrase is not None:
            _zero_bytes(passphrase)
        os.close(lock_fd)


def _run(state_arg: str, agent_arg: str, program: list[str], *, read_only: bool, profiles: list[str], private_home_arg: str | None, mcp_socket: str | None, agent_cli: str | None = None, mcp_relay: str | None = None, verify: bool = False, server_access: bool = False, hide: list[str] | None = None, lend: list[str] | None = None) -> int:
    agent = _safe_agent(agent_arg)
    if not program:
        _fail("run requires a program after --")
    if agent_cli is not None and not AGENT_CLI_RE.fullmatch(agent_cli):
        _fail("Agent CLI must be opencode or agy")
    if agent_cli == "agy":
        if sys.platform != "linux":
            _fail("Antigravity protected execution is available only on Linux")
        if private_home_arg is None:
            _fail("Antigravity requires its guarded per-agent private HOME")
        if profiles:
            _fail("Antigravity cannot mount legacy CLI profiles")
        if (mcp_socket is None) != (mcp_relay is None):
            _fail("Antigravity room MCP requires both a socket and the fixed relay binary")
    elif mcp_relay is not None:
        _fail("The MCP relay binary is supported only by Antigravity")
    state = _canonical_directory(state_arg, "state")
    _check_private_dir(state)
    manifest_paths = _manifest_candidates(state, agent)
    if len(manifest_paths) != 1:
        _fail("Expected exactly one workspace manifest for this agent")
    manifest_path = manifest_paths[0]
    raw = _read_private_json(manifest_path, "workspace baseline manifest")
    if not isinstance(raw, dict) or raw.get("format_version") != FORMAT_VERSION or raw.get("agent") != agent or not isinstance(raw.get("project"), str):
        _fail("Workspace baseline manifest has invalid identity or version")
    project = _canonical_directory(str(raw.get("project", "")), "project")
    lock_fd = _lock(project)
    try:
        data = _manifest_read(manifest_path, project, agent)
        _, workspace, _ = _layout(state, project, agent)
        _safe_state_child(state, f"agents/{_project_id(project)}/{agent}")
        if not workspace.is_dir() or workspace.is_symlink():
            _fail("Workspace path is missing or unsafe")
        _check_private_dir(workspace)
        if not shutil.which("bwrap"):
            _fail("bubblewrap is required; refusing to run without filesystem isolation")
        if verify:
            if not read_only or not data["read_only"]:
                _fail("Verification requires a read-only prepared workspace")
            if profiles or private_home_arg is not None or mcp_socket is not None or agent_cli is not None:
                _fail("Verification cannot mount CLI profiles, private homes, or MCP sockets")
            args = _verify_sandbox_args(workspace, program)
            os.close(lock_fd)
            lock_fd = None
            try:
                result = subprocess.run(args, check=False, preexec_fn=_set_process_limits)
            except OSError as exc:
                _fail(f"bubblewrap failed to start: {exc}")
            return result.returncode
        profile_root = workspace.parent / "profiles"
        private_home_name = f"home-{agent_cli}" if agent_cli in ("opencode", "agy") else "home"
        private_home = profile_root / private_home_name
        _safe_state_child(state, f"agents/{_project_id(project)}/{agent}/profiles/{private_home_name}")
        if private_home_arg is not None:
            supplied_home = _canonical_directory(private_home_arg, "private CLI home")
            if supplied_home != private_home.resolve(strict=True):
                _fail("Writable CLI home must be this agent's private CLI profile directory")
        else:
            supplied_home = private_home.resolve(strict=True)
        _check_private_dir(supplied_home)
        ro = read_only or data["read_only"]
        private_home_mount = f"/run/user/{os.getuid()}/orgbot-home"
        binds: list[tuple[Path, str]] = []
        seen: set[str] = set()
        for item in profiles:
            if "=" not in item:
                _fail("Profiles must use NAME=ABSOLUTE_PATH")
            name, raw_path = item.split("=", 1)
            if not PROFILE_RE.fullmatch(name) or name in seen:
                _fail("Invalid or duplicate CLI profile name")
            profile = _canonical_directory(raw_path, "CLI profile")
            expected = private_home / f".{name}"
            if profile != expected.resolve(strict=True) or profile.is_symlink():
                _fail("Writable CLI profiles must be the named private directory under this agent's profile root")
            _check_private_dir(profile)
            binds.append((profile, f"{private_home_mount}/.{name}"))
            seen.add(name)
        socket_path = None
        if mcp_socket:
            socket_path = Path(mcp_socket)
            if not socket_path.is_absolute() or socket_path.is_symlink():
                _fail("Room MCP socket must be an absolute, non-symlink path")
            if socket_path.resolve(strict=True) != socket_path:
                _fail("Room MCP socket path cannot contain symlink components")
            socket_info = socket_path.lstat()
            if not stat.S_ISSOCK(socket_info.st_mode) or socket_info.st_uid != os.getuid():
                _fail("Room MCP endpoint must be a Unix socket")
        if agent_cli == "agy":
            project_lock_fd = lock_fd
            lock_fd = None
            return _agy_run_protected(
                program,
                supplied_home,
                workspace,
                socket_path,
                Path(mcp_relay) if mcp_relay is not None else None,
                project_lock_fd,
            )
        private_cli_overlays: list[tuple[Path, str]] = []
        private_home_bind = "--ro-bind" if agent_cli == "opencode" else "--bind"
        args = ["bwrap", "--die-with-parent", "--new-session", "--unshare-pid", "--unshare-ipc", "--unshare-uts", "--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--size", "1073741824", "--tmpfs", "/tmp", "--dir", "/tmp/orgbot-workspace", "--tmpfs", "/run", "--dir", "/run/user", "--dir", f"/run/user/{os.getuid()}", "--dir", private_home_mount, private_home_bind, os.fspath(supplied_home), private_home_mount, *_gpu_device_args()]
        # The bot server key only for a turn the owner allowed it, and never
        # for a read-only one: a reviewer has no business on a server.
        bot_ssh = _bot_ssh_dir()
        # `ro` and not the flag: a workspace that was ever read-only stays so.
        granted = bot_ssh if server_access and not ro else None
        owner_home = Path(pwd.getpwuid(os.getuid()).pw_dir)
        args.extend(_owner_secret_mask_args(owner_home, granted, hide=bot_ssh))
        # Everything the owner lends is hidden where it lives; what is lent to
        # this turn appears in the bot's own home instead, never for a
        # read-only turn, and not for CLIs whose private home is read-only.
        args.extend(_owner_hidden_args(owner_home, hide or []))
        if lend and not ro:
            if agent_cli is not None:
                print(f"orgbot-workspace: lending is not available to {agent_cli}; continuing without it", file=sys.stderr)
            else:
                args.extend(_lent_args(owner_home, supplied_home, private_home_mount, lend))
        if agent_cli == "opencode":
            for relative in (".local/share", ".local/state", ".cache"):
                directory = supplied_home / relative
                try:
                    info = directory.lstat()
                except OSError as exc:
                    _fail(f"OpenCode writable data directory is unavailable: {exc}")
                if not stat.S_ISDIR(info.st_mode) or stat.S_ISLNK(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
                    _fail("OpenCode writable data directory is unsafe")
                args.extend(["--bind", os.fspath(directory), f"{private_home_mount}/{relative}"])
        if not Path("/var/run").is_symlink():
            args.extend(["--tmpfs", "/var/run"])
        resolver = Path("/etc/resolv.conf")
        if resolver.is_symlink():
            target = Path(os.path.abspath(os.path.join(resolver.parent, os.readlink(resolver))))
            target = Path(os.path.realpath(target.parent)) / target.name
            try:
                relative_target = target.relative_to("/run")
            except ValueError:
                pass
            else:
                # /run is private in the sandbox, but its resolver file is needed for network access.
                for depth in range(1, len(relative_target.parts)):
                    parent = Path("/run").joinpath(*relative_target.parts[:depth])
                    if parent not in (Path("/run/user"), Path(f"/run/user/{os.getuid()}")):
                        args.extend(["--dir", os.fspath(parent)])
                args.extend(["--ro-bind", os.fspath(resolver), os.fspath(target)])
        for profile, target in binds:
            args.extend(["--bind", os.fspath(profile), target])
        if not ro:
            args.extend(["--bind", os.fspath(workspace), "/tmp/orgbot-workspace"])
        else:
            args.extend(["--ro-bind", os.fspath(workspace), "/tmp/orgbot-workspace"])
        if agent_cli == "opencode":
            opencode_config = supplied_home / "opencode.json"
            opencode_config_dir = supplied_home / ".config/opencode"
            empty_system_config = supplied_home / ".orgbot-empty-opencode-system"
            try:
                config_info = opencode_config.lstat()
                config_dir_info = opencode_config_dir.lstat()
            except OSError as exc:
                _fail(f"OpenCode private profile is unavailable: {exc}")
            if not stat.S_ISREG(config_info.st_mode) or config_info.st_uid != os.getuid() or config_info.st_mode & 0o077:
                _fail("OpenCode private profile is unsafe")
            if not stat.S_ISDIR(config_dir_info.st_mode) or stat.S_ISLNK(config_dir_info.st_mode) or config_dir_info.st_uid != os.getuid() or config_dir_info.st_mode & 0o077:
                _fail("OpenCode private config directory is unsafe")
            config_marker = opencode_config_dir / ".gitignore"
            try:
                marker_info = config_marker.lstat()
            except OSError as exc:
                _fail(f"OpenCode private config marker is unavailable: {exc}")
            expected_marker = b"node_modules\npackage.json\npackage-lock.json\nbun.lock\n.gitignore"
            try:
                marker_bytes = config_marker.read_bytes()
            except OSError as exc:
                _fail(f"OpenCode private config marker cannot be read: {exc}")
            if (
                not stat.S_ISREG(marker_info.st_mode)
                or stat.S_ISLNK(marker_info.st_mode)
                or marker_info.st_uid != os.getuid()
                or marker_info.st_mode & 0o077
                or marker_bytes != expected_marker
                or {entry.name for entry in opencode_config_dir.iterdir()} != {".gitignore"}
            ):
                _fail("OpenCode private config directory contains unmanaged files")
            legacy_config_dir = supplied_home / ".opencode"
            try:
                legacy_info = legacy_config_dir.lstat()
            except OSError as exc:
                _fail(f"OpenCode legacy config mask is unavailable: {exc}")
            legacy_marker = legacy_config_dir / ".gitignore"
            try:
                legacy_marker_info = legacy_marker.lstat()
                legacy_marker_bytes = legacy_marker.read_bytes()
            except OSError as exc:
                _fail(f"OpenCode legacy config marker is unavailable: {exc}")
            if (
                not stat.S_ISDIR(legacy_info.st_mode)
                or stat.S_ISLNK(legacy_info.st_mode)
                or legacy_info.st_uid != os.getuid()
                or legacy_info.st_mode & 0o077
                or not stat.S_ISREG(legacy_marker_info.st_mode)
                or stat.S_ISLNK(legacy_marker_info.st_mode)
                or legacy_marker_info.st_uid != os.getuid()
                or legacy_marker_info.st_mode & 0o077
                or legacy_marker_bytes != expected_marker
                or {entry.name for entry in legacy_config_dir.iterdir()} != {".gitignore"}
            ):
                _fail("OpenCode legacy config directory is unsafe")
            private_cli_overlays.append((opencode_config, f"{private_home_mount}/opencode.json"))
            args.extend(["--ro-bind", os.fspath(opencode_config_dir), f"{private_home_mount}/.config/opencode"])
            try:
                system_config_info = Path("/etc/opencode").lstat()
            except FileNotFoundError:
                system_config_info = None
            if system_config_info is not None:
                if not stat.S_ISDIR(system_config_info.st_mode) or stat.S_ISLNK(system_config_info.st_mode):
                    _fail("System OpenCode configuration path is unsafe")
                try:
                    empty_info = empty_system_config.lstat()
                    if not stat.S_ISDIR(empty_info.st_mode) or stat.S_ISLNK(empty_info.st_mode) or empty_info.st_uid != os.getuid() or empty_info.st_mode & 0o077 or any(empty_system_config.iterdir()):
                        _fail("Private OpenCode system-config mask is unsafe")
                except OSError as exc:
                    _fail(f"Private OpenCode system-config mask is unavailable: {exc}")
                args.extend(["--ro-bind", os.fspath(empty_system_config), "/etc/opencode"])
        for source, destination in private_cli_overlays:
            args.extend(["--ro-bind", os.fspath(source), destination])
        safe_path = []
        for entry in os.environ.get("PATH", "").split(os.pathsep):
            candidate = Path(entry)
            if not candidate.is_absolute() or not candidate.is_dir():
                continue
            resolved = candidate.resolve(strict=True)
            if resolved == project or project in resolved.parents or resolved == state or state in resolved.parents or resolved == supplied_home or supplied_home in resolved.parents:
                continue
            if os.fspath(resolved) not in safe_path:
                safe_path.append(os.fspath(resolved))
        for standard in ("/usr/local/bin", "/usr/bin", "/bin"):
            if Path(standard).is_dir() and standard not in safe_path:
                safe_path.append(standard)
        args.extend(_packaged_trash_args(safe_path))
        env_args = ["--clearenv", "--setenv", "PATH", os.pathsep.join(safe_path), "--setenv", "HOME", private_home_mount, "--setenv", "CODEX_HOME", f"{private_home_mount}/.codex", "--setenv", "CLAUDE_CONFIG_DIR", f"{private_home_mount}/.claude", "--setenv", "GROK_HOME", f"{private_home_mount}/.grok", "--setenv", "TMPDIR", "/tmp", "--setenv", "ORGBOT_WORKSPACE", "/tmp/orgbot-workspace", "--setenv", "LANG", "C.UTF-8"]
        if agent_cli is not None:
            xdg_config = f"{private_home_mount}/.config"
            xdg_data = f"{private_home_mount}/.local/share"
            xdg_state = f"{private_home_mount}/.local/state"
            env_args.extend([
                "--setenv", "XDG_CONFIG_HOME", xdg_config,
                "--setenv", "XDG_DATA_HOME", xdg_data,
                "--setenv", "XDG_STATE_HOME", xdg_state,
                "--setenv", "XDG_CACHE_HOME", f"{private_home_mount}/.cache",
            ])
            if agent_cli == "opencode":
                env_args.extend([
                    "--setenv", "OPENCODE_CONFIG", f"{private_home_mount}/opencode.json",
                    "--setenv", "OPENCODE_CONFIG_DIR", f"{xdg_config}/opencode",
                    "--setenv", "OPENCODE_DISABLE_PROJECT_CONFIG", "true",
                    "--setenv", "OPENCODE_DISABLE_EXTERNAL_SKILLS", "true",
                    "--setenv", "OPENCODE_DISABLE_CLAUDE_CODE", "true",
                    "--setenv", "OPENCODE_DISABLE_CLAUDE_CODE_PROMPT", "true",
                    "--setenv", "OPENCODE_DISABLE_CLAUDE_CODE_SKILLS", "true",
                    "--setenv", "OPENCODE_PURE", "true",
                ])
        if "TERM" in os.environ and re.fullmatch(r"[A-Za-z0-9_-]{1,32}", os.environ["TERM"]):
            env_args.extend(["--setenv", "TERM", os.environ["TERM"]])
        if socket_path:
            socket_dest = f"/run/user/{os.getuid()}/orgbot-room-mcp.sock"
            args.extend(["--dir", f"/run/user/{os.getuid()}", "--bind", os.fspath(socket_path), socket_dest])
            env_args.extend(["--setenv", "ORGBOT_ROOM_MCP_SOCKET", socket_dest])
        args.extend(env_args)
        secret_args = _keyring_sign_in_args(owner_home, lend, safe_path) if lend and not ro and agent_cli is None else []
        secret_fd = None
        if secret_args:
            secret_fd, writer = os.pipe()
            try:
                os.write(writer, b"".join(arg.encode() + b"\0" for arg in secret_args))
            finally:
                os.close(writer)
            args.extend(["--args", str(secret_fd)])
        launch_program = list(program)
        first = Path(launch_program[0])
        if first.is_absolute():
            try:
                relative_program = first.relative_to(project)
            except ValueError:
                pass
            else:
                launch_program[0] = f"/tmp/orgbot-workspace/{relative_program.as_posix()}"
        elif "/" not in launch_program[0]:
            resolved_program = shutil.which(launch_program[0], path=os.pathsep.join(safe_path))
            if resolved_program is None:
                _fail(f"Program is not available in the sandbox PATH: {launch_program[0]}")
            launch_program[0] = os.path.realpath(resolved_program)
        args.extend(["--chdir", "/tmp/orgbot-workspace", "--", *launch_program])
        # Project integration is serialized, but a running agent may call its
        # room MCP server, which can prepare/apply the same workspace. Never
        # hold the project lock across the child process.
        os.close(lock_fd)
        lock_fd = None
        try:
            result = subprocess.run(args, check=False, preexec_fn=_set_process_limits, pass_fds=(secret_fd,) if secret_fd is not None else ())
        except OSError as exc:
            _fail(f"bubblewrap failed to start: {exc}")
        finally:
            if secret_fd is not None:
                os.close(secret_fd)
        return result.returncode
    finally:
        if lock_fd is not None:
            os.close(lock_fd)


def _set_process_limits() -> None:
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    for limit, maximum in ((resource.RLIMIT_FSIZE, MAX_FILE_BYTES * 2), (resource.RLIMIT_NOFILE, 1024)):
        soft, hard = resource.getrlimit(limit)
        bound = min(maximum, hard) if hard != resource.RLIM_INFINITY else maximum
        resource.setrlimit(limit, (min(soft, bound) if soft != resource.RLIM_INFINITY else bound, bound))


def _trash_command() -> list[str]:
    """The trusted orgbot-trash helper, as a command to run."""
    candidates = [
        Path(__file__).with_name("orgbot-trash.py"),
        Path.home() / ".local/lib/orgbot/orgbot-trash.py",
        Path("/usr/lib/orgbot/orgbot-trash.py"),
        Path.home() / ".local/bin/orgbot-trash",
        Path("/usr/local/bin/orgbot-trash"),
        Path("/usr/bin/orgbot-trash"),
    ]
    helper = None
    command: list[str] | None = None
    for candidate in candidates:
        try:
            info = candidate.lstat()
        except OSError:
            continue
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode) or info.st_uid not in (0, os.getuid()) or info.st_mode & 0o022:
            continue
        if candidate.suffix == ".py":
            helper = candidate
            command = [sys.executable, os.fspath(candidate)]
            break
        if os.access(candidate, os.X_OK):
            helper = candidate
            command = [os.fspath(candidate)]
            break
    if helper is None or command is None:
        _fail("Trusted orgbot-trash helper is unavailable; existing bytes were not replaced")
    return command


TRASH_BATCH = 200


def _trash_replacements(project: Path, items: list[tuple[Path, str]]) -> list[str]:
    """Move each (target, expected SHA-256) to the trash in one helper run.

    From the live room: 1,549 approved removals took thirteen minutes, one
    helper process per file. Returns a receipt per file, in order. When the
    helper stops part way, the files it did move are returned with the error,
    so the caller can record them before failing.
    """
    payload = json.dumps([[os.fspath(target), digest] for target, digest in items])
    result = subprocess.run([*_trash_command(), "move", "--project", project.name, "--expected-json", "-"], input=payload, check=False, capture_output=True, text=True)
    receipts = [line.strip()[-200:] for line in result.stdout.splitlines() if line.strip()]
    if result.returncode != 0 or len(receipts) != len(items):
        raise PartialTrash(receipts, f"orgbot-trash refused to preserve {items[len(receipts)][0] if len(receipts) < len(items) else 'a file'}: {result.stderr.strip()[:400]}")
    return receipts


class PartialTrash(WorkspaceError):
    def __init__(self, receipts: list[str], message: str) -> None:
        super().__init__(message)
        self.receipts = receipts


def _progress(stage: str, done: int, total: int) -> None:
    """A progress line on stderr for the app to show while it waits."""
    print(json.dumps({"progress": {"stage": stage, "done": done, "total": total}}), file=sys.stderr, flush=True)


def _set_aside_conflicts(project: Path, workspace: Path, conflicts: list[str], current: dict[str, dict[str, Any]], agent_entries: dict[str, dict[str, Any]]) -> list[dict[str, Any]]:
    """Keep the bot's version of each conflicting file and take the project's.

    From the live room: a bot whose copy conflicted with files others had
    changed could run no command at all, because every command ends by
    merging, and the merge refused. The project's newer version is never
    overwritten: the bot's version is copied into its private home under
    `~/orgbot-conflicts/<time>/`, where it can compare and redo its change, and
    its copy takes the project's version so everything else merges.
    """
    stamp = time.strftime("%Y%m%dT%H%M%S", time.gmtime()) + f"-{os.getpid()}"
    home = workspace.parent / "profiles" / "home"
    kept_root = home / "orgbot-conflicts" / stamp
    set_aside: list[dict[str, Any]] = []
    for name in conflicts:
        ours = agent_entries.get(name)
        kept = None
        if ours is not None:
            source = _file_at(workspace, name)
            destination = kept_root / name
            _mkdir_chain_no_symlinks(destination.parent)
            fd = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
            try:
                with open(source, "rb") as handle:
                    while chunk := handle.read(1 << 20):
                        os.write(fd, chunk)
            finally:
                os.close(fd)
            kept = f"~/orgbot-conflicts/{stamp}/{name}"
        if name in current:
            _copy_from_project(project, workspace, name)
        else:
            try:
                _remove_file(_file_at(workspace, name))
            except FileNotFoundError:
                pass
        set_aside.append({"path": name, "kept_at": kept, "project_has": "a newer version" if name in current else "no such file"})
    return set_aside


def _apply(state_arg: str, agent_arg: str, review_token: str | None, set_aside: bool = False) -> dict[str, Any]:
    agent = _safe_agent(agent_arg)
    state = _canonical_directory(state_arg, "state")
    _check_private_dir(state)
    manifests = _manifest_candidates(state, agent)
    if len(manifests) != 1:
        _fail("Expected exactly one workspace manifest for this agent")
    manifest_path = manifests[0]
    raw = _read_private_json(manifest_path, "workspace baseline manifest")
    if not isinstance(raw, dict) or raw.get("format_version") != FORMAT_VERSION or raw.get("agent") != agent or not isinstance(raw.get("project"), str):
        _fail("Workspace baseline manifest has invalid identity or version")
    project = _canonical_directory(str(raw.get("project", "")), "project")
    _ensure_root_paths(state, project)
    lock_fd = _lock(project)
    try:
        data = _manifest_read(manifest_path, project, agent)
        if data["read_only"]:
            _fail("Read-only reviewer workspaces cannot apply changes")
        _, workspace, _ = _layout(state, project, agent)
        _safe_state_child(state, f"agents/{_project_id(project)}/{agent}")
        if not workspace.is_dir() or workspace.is_symlink():
            _fail("Workspace path is missing or unsafe")
        _check_private_dir(workspace)
        current = _scan(project)
        agent_entries = _scan_workspace(workspace, data["entries"])
        base = data["entries"]
        diffs = _changes(base, current, agent_entries)
        approval_token = _review_token(project, agent, base, current, agent_entries)
        if review_token is not None and not re.fullmatch(r"[a-f0-9]{64}", review_token):
            _fail("Review token must be a 64-character lowercase SHA-256 value")
        if review_token is not None and review_token != approval_token:
            return {"ok": False, "workspace": os.fspath(workspace), **diffs, "pending_delete": sorted(diffs["deleted"]), "review_token": approval_token, "error": "Workspace changed after deletion review; refresh the changes and approve the current deletion list"}
        deletions_approved = review_token == approval_token

        def plan() -> tuple[set[str], list[str], list[tuple[str, str]]]:
            conflicts = set(diffs["conflicts"])
            pending_delete = sorted(name for name in diffs["deleted"] if not deletions_approved)
            operations: list[tuple[str, str]] = []
            for name in sorted(set(base) | set(current) | set(agent_entries)):
                before, now, ours = base.get(name), current.get(name), agent_entries.get(name)
                if ours == before or ours == now:
                    continue
                if now != before and now != ours:
                    conflicts.add(name)
                    continue
                if ours is None:
                    if deletions_approved:
                        operations.append((name, "delete"))
                else:
                    operations.append((name, "write"))
            return conflicts, pending_delete, operations

        conflicts, pending_delete, operations = plan()
        set_aside_files: list[dict[str, Any]] = []
        # Only for the bot's own merges: an owner's deletion approval is for
        # the exact list they saw, which setting files aside would change.
        if conflicts and set_aside and review_token is None:
            set_aside_files = _set_aside_conflicts(project, workspace, sorted(conflicts), current, agent_entries)
            for name in conflicts:
                if name in current:
                    base[name] = current[name]
                else:
                    base.pop(name, None)
            data["entries"] = base
            _atomic_json(manifest_path, data)
            agent_entries = _scan_workspace(workspace, base)
            diffs = _changes(base, current, agent_entries)
            conflicts, pending_delete, operations = plan()
        if conflicts:
            diffs["conflicts"] = sorted(conflicts)
            return {"ok": False, "workspace": os.fspath(workspace), **diffs, "pending_delete": pending_delete, "review_token": approval_token, "error": "Conflicts must be resolved before applying any change"}
        journal_path = workspace.parent / "apply-journal.json"
        journal = {"format_version": FORMAT_VERSION, "project": os.fspath(project), "agent": agent, "started_at": int(time.time()), "phase": "applying", "operations": [{"path": name, "action": action, "state": "pending", "expected_current": current.get(name), "agent_entry": agent_entries.get(name)} for name, action in operations]}
        _atomic_json(journal_path, journal)
        # Every target must still be as reviewed before anything moves.
        for name, action in operations:
            target = _file_at(project, name)
            expected_current = current.get(name)
            if expected_current is not None:
                actual = _entry(target, project)
                if actual != expected_current:
                    _fail(f"Project changed after conflict checking; preserved {name}")
            else:
                try:
                    target.lstat()
                except FileNotFoundError:
                    pass
                else:
                    _fail(f"Project target appeared after conflict checking; preserved {name}")
        # Whatever is removed or replaced goes to the trash first, in batches:
        # the helper re-checks each file's content as it moves it. The journal
        # is written per batch, not three times per file, which made a long
        # review quadratic.
        preserve = [index for index, (name, _) in enumerate(operations) if current.get(name) is not None]
        for start in range(0, len(preserve), TRASH_BATCH):
            batch = preserve[start:start + TRASH_BATCH]
            for index in batch:
                journal["operations"][index]["state"] = "preserving"
            _atomic_json(journal_path, journal)
            items = [(_file_at(project, operations[index][0]), current[operations[index][0]]["sha256"]) for index in batch]
            try:
                receipts = _trash_replacements(project, items)
            except PartialTrash as partial:
                for index, receipt in zip(batch, partial.receipts):
                    journal["operations"][index]["state"] = "preserved"
                    journal["operations"][index]["trash_receipt"] = receipt
                _atomic_json(journal_path, journal)
                _fail(str(partial))
            for index, receipt in zip(batch, receipts):
                journal["operations"][index]["state"] = "preserved"
                journal["operations"][index]["trash_receipt"] = receipt
            _atomic_json(journal_path, journal)
            _progress("trash", start + len(batch), len(preserve))
        for index, (name, action) in enumerate(operations):
            if action != "delete":
                mode = agent_entries[name]["mode"]
                _copy_from_workspace(workspace, project, name, mode)
            journal["operations"][index]["state"] = "applied"
            if (index + 1) % TRASH_BATCH == 0 or index + 1 == len(operations):
                journal["updated_at"] = int(time.time())
                _atomic_json(journal_path, journal)
                _progress("apply", index + 1, len(operations))
        final_entries = _scan(project)
        # Advance the baseline only after clean paths have also been refreshed
        # in this agent's copy. Otherwise another agent's applied change can
        # become an apparent local edit and overwrite that agent next time.
        for name in sorted(set(base) | set(final_entries) | set(agent_entries)):
            if agent_entries.get(name) != base.get(name):
                continue
            if final_entries.get(name) == base.get(name):
                continue
            if name in final_entries:
                _copy_from_project(project, workspace, name)
            else:
                _remove_file(_file_at(workspace, name))
        data["entries"] = final_entries
        data.pop("refresh_conflicts", None)
        _atomic_json(manifest_path, data)
        final_agent_entries = _scan_workspace(workspace, final_entries)
        final_review_token = _review_token(project, agent, final_entries, final_entries, final_agent_entries)
        journal["phase"] = "complete"
        journal["completed_at"] = int(time.time())
        _atomic_json(journal_path, journal)
        result = {"ok": True, "workspace": os.fspath(workspace), "changed": diffs["changed"], "added": diffs["added"], "deleted": diffs["deleted"], "applied": [name for name, _ in operations], "pending_delete": pending_delete, "conflicts": [], "review_token": final_review_token}
        if set_aside_files:
            result["set_aside"] = set_aside_files
            result["note"] = (
                "These files changed in the project after your copy was made, so your versions were not merged: "
                + ", ".join(item["path"] for item in set_aside_files)
                + ". Your copy now has the project's version; yours are kept under ~/orgbot-conflicts/ "
                "(read them with workspace_run). Redo your change on top of the project's version if it is still needed."
            )
        return result
    finally:
        os.close(lock_fd)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Isolate and safely integrate one OrgBots agent workspace")
    sub = parser.add_subparsers(dest="command", required=True)
    prepare = sub.add_parser("prepare")
    prepare.add_argument("--project", required=True)
    prepare.add_argument("--state", required=True)
    prepare.add_argument("--agent", required=True)
    prepare.add_argument("--read-only", action="store_true")
    status = sub.add_parser("status")
    status.add_argument("--state", required=True)
    status.add_argument("--agent", required=True)
    run = sub.add_parser("run")
    run.add_argument("--state", required=True)
    run.add_argument("--agent", required=True)
    run.add_argument("--read-only", action="store_true")
    run.add_argument("--verify", action="store_true", help="Run with a minimal read-only, networkless runtime sandbox")
    run.add_argument("--profile", action="append", default=[])
    run.add_argument("--private-home")
    run.add_argument("--mcp-socket")
    run.add_argument("--server-access", action="store_true")
    run.add_argument("--hide", action="append", default=[], help="A path under the owner's home the owner lends, hidden unless lent")
    run.add_argument("--lend", action="append", default=[], help="A path under the owner's home lent to this turn, mounted in the bot's home")
    run.add_argument("--mcp-relay")
    run.add_argument("--agent-cli", choices=("opencode", "agy"))
    run.add_argument("program", nargs=argparse.REMAINDER)
    prune = sub.add_parser("prune", help="Remove working copies of bots no longer in the room that hold no unmerged work")
    prune.add_argument("--state", required=True)
    prune.add_argument("--keep", action="append", default=[])
    prune.add_argument("--dry-run", action="store_true", help="Report what would be removed without removing it")
    apply = sub.add_parser("apply")
    apply.add_argument("--state", required=True)
    apply.add_argument("--agent", required=True)
    apply.add_argument("--review-token")
    apply.add_argument("--set-aside-conflicts", action="store_true", help="Keep the bot's version of each conflicting file in its private home and take the project's")
    agy_auth = sub.add_parser("agy-auth")
    agy_auth.add_argument("action", choices=("login", "status"))
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        if args.command == "prepare":
            _json(_prepare(args.project, args.state, args.agent, args.read_only))
            return 0
        if args.command == "status":
            _json(_status(args.state, args.agent))
            return 0
        if args.command == "run":
            program = args.program[1:] if args.program and args.program[0] == "--" else args.program
            return _run(args.state, args.agent, program, read_only=args.read_only, profiles=args.profile, private_home_arg=args.private_home, mcp_socket=args.mcp_socket, agent_cli=args.agent_cli, mcp_relay=args.mcp_relay, verify=args.verify, server_access=args.server_access, hide=args.hide, lend=args.lend)
        if args.command == "prune":
            _json(_prune(args.state, args.keep, args.dry_run))
            return 0
        if args.command == "agy-auth":
            result = _agy_auth_login() if args.action == "login" else _agy_auth_status()
            _json(result)
            return 0 if result.get("ok") else 2
        result = _apply(args.state, args.agent, args.review_token, set_aside=args.set_aside_conflicts)
        _json(result)
        return 0 if result.get("ok") else 1
    except (WorkspaceError, OSError, ValueError, json.JSONDecodeError) as exc:
        if args.command == "run" and getattr(args, "agent_cli", None) == "agy":
            print(f"Antigravity workspace helper: {str(exc)[:600]}", file=sys.stderr)
        _json({"ok": False, "error": str(exc)[:600]})
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
