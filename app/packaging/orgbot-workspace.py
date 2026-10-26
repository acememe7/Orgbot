#!/usr/bin/env python3
"""Create, run, and safely merge isolated OrgBots project workspaces."""

from __future__ import annotations

import argparse
import ctypes
import errno
import fcntl
import hashlib
import json
import os
import re
import resource
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path, PurePosixPath
from typing import Any


FORMAT_VERSION = 1
MAX_ENTRIES = 150_000
MAX_FILE_BYTES = 512 * 1024 * 1024
MAX_TREE_BYTES = 20 * 1024 * 1024 * 1024
AGENT_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,63}\Z")
PROFILE_RE = re.compile(r"(?:codex|claude|grok)\Z")
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


def _run(state_arg: str, agent_arg: str, program: list[str], *, read_only: bool, profiles: list[str], private_home_arg: str | None, mcp_socket: str | None) -> int:
    agent = _safe_agent(agent_arg)
    if not program:
        _fail("run requires a program after --")
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
        profile_root = workspace.parent / "profiles"
        private_home = profile_root / "home"
        _safe_state_child(state, f"agents/{_project_id(project)}/{agent}/profiles/home")
        if private_home_arg is not None:
            supplied_home = _canonical_directory(private_home_arg, "private CLI home")
            if supplied_home != private_home.resolve(strict=True):
                _fail("Writable CLI home must be this agent's private profile_root/home directory")
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
        args = ["bwrap", "--die-with-parent", "--new-session", "--unshare-pid", "--unshare-ipc", "--unshare-uts", "--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--size", "1073741824", "--tmpfs", "/tmp", "--dir", "/tmp/orgbot-workspace", "--tmpfs", "/run", "--dir", "/run/user", "--dir", f"/run/user/{os.getuid()}", "--dir", private_home_mount, "--bind", os.fspath(supplied_home), private_home_mount]
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
        env_args = ["--clearenv", "--setenv", "PATH", os.pathsep.join(safe_path), "--setenv", "HOME", private_home_mount, "--setenv", "CODEX_HOME", f"{private_home_mount}/.codex", "--setenv", "CLAUDE_CONFIG_DIR", f"{private_home_mount}/.claude", "--setenv", "GROK_HOME", f"{private_home_mount}/.grok", "--setenv", "TMPDIR", "/tmp", "--setenv", "ORGBOT_WORKSPACE", "/tmp/orgbot-workspace", "--setenv", "LANG", "C.UTF-8"]
        if "TERM" in os.environ and re.fullmatch(r"[A-Za-z0-9_-]{1,32}", os.environ["TERM"]):
            env_args.extend(["--setenv", "TERM", os.environ["TERM"]])
        if socket_path:
            socket_dest = f"/run/user/{os.getuid()}/orgbot-room-mcp.sock"
            args.extend(["--dir", f"/run/user/{os.getuid()}", "--bind", os.fspath(socket_path), socket_dest])
            env_args.extend(["--setenv", "ORGBOT_ROOM_MCP_SOCKET", socket_dest])
        args.extend(env_args)
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
            result = subprocess.run(args, check=False, preexec_fn=_set_process_limits)
        except OSError as exc:
            _fail(f"bubblewrap failed to start: {exc}")
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


def _trash_replacement(project: Path, target: Path, expected_sha256: str) -> str:
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
    if helper is None:
        _fail("Trusted orgbot-trash helper is unavailable; existing bytes were not replaced")
    result = subprocess.run([*command, "move", "--project", project.name, "--expected-sha256", expected_sha256, os.fspath(target)], check=False, capture_output=True, text=True)
    if result.returncode != 0:
        _fail(f"orgbot-trash refused to preserve {target}: {result.stderr.strip()[:400]}")
    return result.stdout.strip()[-200:]


def _apply(state_arg: str, agent_arg: str, review_token: str | None) -> dict[str, Any]:
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
        conflicts = set(diffs["conflicts"])
        deletions_approved = review_token == approval_token
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
        if conflicts:
            diffs["conflicts"] = sorted(conflicts)
            return {"ok": False, "workspace": os.fspath(workspace), **diffs, "pending_delete": pending_delete, "review_token": approval_token, "error": "Conflicts must be resolved before applying any change"}
        journal_path = workspace.parent / "apply-journal.json"
        journal = {"format_version": FORMAT_VERSION, "project": os.fspath(project), "agent": agent, "started_at": int(time.time()), "phase": "applying", "operations": [{"path": name, "action": action, "state": "pending", "expected_current": current.get(name), "agent_entry": agent_entries.get(name)} for name, action in operations]}
        _atomic_json(journal_path, journal)
        for index, (name, action) in enumerate(operations):
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
            if action == "delete":
                journal["operations"][index]["state"] = "preserving"
                _atomic_json(journal_path, journal)
                receipt = _trash_replacement(project, target, expected_current["sha256"])
                journal["operations"][index]["state"] = "preserved"
                journal["operations"][index]["trash_receipt"] = receipt
                _atomic_json(journal_path, journal)
            else:
                if expected_current is not None:
                    journal["operations"][index]["state"] = "preserving"
                    _atomic_json(journal_path, journal)
                    receipt = _trash_replacement(project, target, expected_current["sha256"])
                    journal["operations"][index]["state"] = "preserved"
                    journal["operations"][index]["trash_receipt"] = receipt
                    _atomic_json(journal_path, journal)
                else:
                    journal["operations"][index]["state"] = "writing"
                    _atomic_json(journal_path, journal)
                mode = agent_entries[name]["mode"]
                _copy_from_workspace(workspace, project, name, mode)
            journal["operations"][index]["state"] = "applied"
            journal["updated_at"] = int(time.time())
            _atomic_json(journal_path, journal)
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
        return {"ok": True, "workspace": os.fspath(workspace), "changed": diffs["changed"], "added": diffs["added"], "deleted": diffs["deleted"], "applied": [name for name, _ in operations], "pending_delete": pending_delete, "conflicts": [], "review_token": final_review_token}
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
    run.add_argument("--profile", action="append", default=[])
    run.add_argument("--private-home")
    run.add_argument("--mcp-socket")
    run.add_argument("program", nargs=argparse.REMAINDER)
    apply = sub.add_parser("apply")
    apply.add_argument("--state", required=True)
    apply.add_argument("--agent", required=True)
    apply.add_argument("--review-token")
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
            return _run(args.state, args.agent, program, read_only=args.read_only, profiles=args.profile, private_home_arg=args.private_home, mcp_socket=args.mcp_socket)
        result = _apply(args.state, args.agent, args.review_token)
        _json(result)
        return 0 if result.get("ok") else 1
    except (WorkspaceError, OSError, ValueError, json.JSONDecodeError) as exc:
        _json({"ok": False, "error": str(exc)[:600]})
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
