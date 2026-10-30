#!/usr/bin/env python3
"""Recoverable, project-grouped file trash for OrgBots owners."""

from __future__ import annotations

import argparse
import contextlib
import ctypes
import fcntl
import hashlib
import json
import math
import os
import re
import shutil
import stat
import subprocess
import sys
import time
import uuid
from pathlib import Path
from typing import Any, Iterator


LEGACY_TRASH_ROOT = Path("/mnt/hdd/OrgBots Trash")
HDD_MOUNT = Path("/mnt/hdd")
RETENTION_SECONDS = 7 * 24 * 60 * 60
SCHEMA = 1
COPY_CHUNK = 1024 * 1024


def _absolute_env_path(name: str, fallback: Path) -> Path:
    value = os.environ.get(name)
    candidate = Path(value) if value else fallback
    return candidate if candidate.is_absolute() else fallback


def _config_file() -> Path:
    return _absolute_env_path("XDG_CONFIG_HOME", Path.home() / ".config") / "orgbots" / "trash.json"


def _default_root() -> Path:
    return _absolute_env_path("XDG_DATA_HOME", Path.home() / ".local" / "share") / "orgbots" / "trash"


TRASH_ROOT = _default_root()


class TrashError(RuntimeError):
    pass


def _fail(message: str) -> None:
    raise TrashError(message)


def _safe_component(value: str) -> str:
    slug = re.sub(r"[^A-Za-z0-9._-]+", "-", value.strip()).strip(".-_")[:48]
    digest = hashlib.sha256(value.encode("utf-8")).hexdigest()[:12]
    return f"{slug or 'project'}-{digest}"


def _mount_options(path: Path) -> tuple[str, set[str]] | None:
    try:
        lines = Path("/proc/self/mountinfo").read_text(encoding="utf-8").splitlines()
    except OSError as exc:
        raise TrashError(f"Cannot verify HDD mount: {exc.strerror}") from exc
    wanted = str(path)
    for line in lines:
        left, separator, right = line.partition(" - ")
        fields = left.split()
        if separator and len(fields) > 5 and _unescape_mount_path(fields[4]) == wanted:
            fs_fields = right.split()
            if len(fs_fields) < 3:
                return None
            return fs_fields[0], set(fields[5].split(","))
    return None


def _unescape_mount_path(value: str) -> str:
    return re.sub(r"\\([0-7]{3})", lambda match: chr(int(match.group(1), 8)), value)


def _effective_mount_options(path: Path) -> tuple[Path, str, set[str]] | None:
    try:
        lines = Path("/proc/self/mountinfo").read_text(encoding="utf-8").splitlines()
    except OSError as exc:
        raise TrashError(f"Cannot verify HDD destination mount: {exc.strerror}") from exc
    target = Path(os.path.abspath(path))
    matches: list[tuple[int, Path, str, set[str]]] = []
    for line in lines:
        left, separator, right = line.partition(" - ")
        fields = left.split()
        fs_fields = right.split()
        if not separator or len(fields) <= 5 or len(fs_fields) < 3:
            continue
        mountpoint = Path(_unescape_mount_path(fields[4]))
        if target == mountpoint or mountpoint in target.parents:
            matches.append((len(mountpoint.parts), mountpoint, fs_fields[0], set(fields[5].split(","))))
    if not matches:
        return None
    _, mountpoint, filesystem, options = max(matches, key=lambda match: match[0])
    return mountpoint, filesystem, options


LEGACY_POLICY = Path("/var/lib/orgbots/legacy-trash-policy")
SYSTEMCTL_CANDIDATES = (Path("/usr/bin/systemctl"), Path("/bin/systemctl"))


def _fs_identity(path: Path) -> dict[str, Any]:
    effective = _effective_mount_options(path)
    if effective is None:
        _fail(f"Cannot identify the filesystem containing trash root: {path}")
    mountpoint, filesystem, options = effective
    try:
        info = path.stat()
        values = os.statvfs(path)
    except OSError as exc:
        _fail(f"Cannot inspect selected trash filesystem: {exc.strerror}")
    return {
        "mountpoint": str(mountpoint),
        "filesystem": filesystem,
        "fsid": int(values.f_fsid),
        "device": int(info.st_dev),
        "writable": "rw" in options and not (values.f_flag & getattr(os, "ST_RDONLY", 1)),
    }


def _config_paths() -> tuple[Path, Path]:
    config = _config_file()
    return config, config.parent


def _check_components(path: Path, *, allow_missing_leaf: bool = False) -> None:
    absolute = Path(os.path.abspath(path))
    parent = absolute.parent if allow_missing_leaf else absolute
    try:
        fd = _open_absolute_dir(parent)
        os.close(fd)
    except OSError as exc:
        _fail(f"Trash path has an unavailable or symlink component: {exc.strerror}")


def _private_config_dir(path: Path, *, create: bool = False) -> None:
    if create and not path.exists():
        _create_absolute_dirs(path)
    try:
        fd = _open_absolute_dir(path)
        info = os.fstat(fd)
        os.close(fd)
    except OSError as exc:
        _fail(f"Unsafe configuration directory: {path}: {exc.strerror}")
    if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700:
        _fail(f"Configuration directory must be user-owned and private (0700): {path}")


def _create_absolute_dirs(path: Path) -> None:
    absolute = Path(os.path.abspath(path))
    fd = _open_dir("/")
    try:
        for component in absolute.parts[1:]:
            try:
                os.mkdir(component, 0o700, dir_fd=fd)
            except FileExistsError:
                pass
            next_fd = _open_dir(component, dir_fd=fd)
            os.close(fd)
            fd = next_fd
    finally:
        os.close(fd)


def _read_config() -> dict[str, Any] | None:
    config_path, config_dir = _config_paths()
    if not config_path.exists() and not config_path.is_symlink():
        return None
    _private_config_dir(config_dir)
    fd = -1
    try:
        fd = os.open(config_path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o600:
            _fail("Trash config must be a user-owned regular file with mode 0600")
        chunks = []
        while True:
            chunk = os.read(fd, 64 * 1024)
            if not chunk:
                break
            chunks.append(chunk)
        payload = json.loads(b"".join(chunks).decode("utf-8"))
    except (OSError, json.JSONDecodeError, UnicodeDecodeError) as exc:
        _fail(f"Cannot read trash config safely: {exc}")
    finally:
        if fd >= 0:
            os.close(fd)
    if not isinstance(payload, dict) or payload.get("schema") != SCHEMA or not isinstance(payload.get("root"), str) or not isinstance(payload.get("identity"), dict):
        _fail("Trash config has an unsupported or invalid schema")
    root = Path(payload["root"])
    identity = payload["identity"]
    if not root.is_absolute() or str(Path(os.path.normpath(root))) != payload["root"] or root == Path("/"):
        _fail("Trash config root must be a normalized absolute path")
    if not isinstance(identity.get("mountpoint"), str) or not Path(identity["mountpoint"]).is_absolute() or not isinstance(identity.get("filesystem"), str) or any(type(identity.get(key)) is not int for key in ("fsid", "device")):
        _fail("Trash config filesystem identity is invalid")
    return payload


def _legacy_install_evidence() -> bool:
    home = Path.home()
    for path in (
        home / ".local/lib/orgbot/orgbot-trash.py",
        _absolute_env_path("XDG_CONFIG_HOME", home / ".config") / "systemd/user/orgbot-trash-cleanup.service",
    ):
        try:
            info = path.lstat()
        except FileNotFoundError:
            continue
        if stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid():
            return True
    return False


def _selected_config() -> tuple[Path, str, bool, dict[str, Any] | None]:
    payload = _read_config()
    if payload is not None:
        return Path(payload["root"]), "configured", False, payload
    if _legacy_install_evidence():
        return LEGACY_TRASH_ROOT, "legacy-hdd", False, None
    if LEGACY_POLICY.exists():
        return _default_root(), "policy-requires-selection", True, None
    return _default_root(), "default", False, None


def _write_config(root: Path, identity: dict[str, Any]) -> None:
    config_path, config_dir = _config_paths()
    _private_config_dir(config_dir, create=True)
    payload = {"schema": SCHEMA, "root": str(root), "identity": {key: identity[key] for key in ("mountpoint", "filesystem", "fsid", "device")}}
    temporary = config_path.with_name(f".{config_path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        data = (json.dumps(payload, sort_keys=True) + "\n").encode("utf-8")
        with os.fdopen(fd, "wb", closefd=False) as stream:
            stream.write(data)
            stream.flush()
            os.fsync(fd)
        os.replace(temporary, config_path)
        dir_fd = _open_absolute_dir(config_dir)
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
    finally:
        os.close(fd)
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def _remove_config() -> None:
    config_path, config_dir = _config_paths()
    try:
        config_path.unlink()
    except FileNotFoundError:
        return
    dir_fd = _open_absolute_dir(config_dir)
    try:
        os.fsync(dir_fd)
    finally:
        os.close(dir_fd)


def _restore_config(previous: dict[str, Any] | None) -> None:
    if previous is None:
        _remove_config()
    else:
        _write_config(Path(previous["root"]), previous["identity"])


def _commit_config(root: Path, identity: dict[str, Any], previous: dict[str, Any] | None, *, sync_systemd: bool = True) -> None:
    _write_config(root, identity)
    try:
        if sync_systemd:
            _sync_systemd(root)
    except Exception:
        _restore_config(previous)
        raise


def _verify_identity(root: Path, expected: dict[str, Any] | None) -> None:
    _check_components(root, allow_missing_leaf=True)
    identity_path = root if root.exists() else root.parent
    current = _fs_identity(identity_path)
    if not current["writable"]:
        _fail("Selected trash filesystem is read-only")
    if expected is not None:
        for key in ("filesystem", "fsid", "device"):
            if current[key] != expected.get(key):
                _fail("Selected trash filesystem identity changed or is unavailable; operation stopped")
        if current["mountpoint"] not in (expected.get("mountpoint"), str(root)):
            _fail("Selected trash mountpoint changed; operation stopped")


def _config_result() -> dict[str, Any]:
    root, source, requires_selection, _ = _selected_config()
    return {"root": str(root), "source": source, "requires_selection": requires_selection}


def _systemd_escape_path(value: str) -> str:
    escaped = []
    for char in value:
        code = ord(char)
        if char == "%":
            escaped.append("%%")
        elif char in ('\\', '"'):
            escaped.append("\\" + char)
        elif code <= 0x20 or code == 0x7f:
            escaped.append(f"\\x{code:02x}")
        else:
            escaped.append(char)
    return "\"" + "".join(escaped) + "\""


def _systemctl_executable() -> Path | None:
    return next((candidate for candidate in SYSTEMCTL_CANDIDATES if candidate.is_file() and os.access(candidate, os.X_OK)), None)


def _require_manager_config_home(systemctl: Path, config_home: Path) -> None:
    default_home = Path.home() / ".config"
    try:
        result = subprocess.run(
            [str(systemctl), "--user", "show-environment"],
            timeout=10,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise TrashError("Cannot verify the user manager's XDG_CONFIG_HOME; refusing to configure its cleanup timer") from exc
    if result.returncode != 0:
        _fail("Cannot verify the user manager's XDG_CONFIG_HOME; refusing to configure its cleanup timer")
    manager_config = None
    for line in result.stdout.decode("utf-8", errors="replace").splitlines():
        if line.startswith("XDG_CONFIG_HOME="):
            manager_config = Path(line.partition("=")[2])
            break
    if manager_config is None:
        manager_config = default_home
    if manager_config != config_home:
        _fail(f"User manager searches {manager_config}/systemd/user, but OrgBots units are under {config_home}/systemd/user; align XDG_CONFIG_HOME before installing or syncing the cleanup timer")


def _sync_systemd(root: Path) -> None:
    config_home = _absolute_env_path("XDG_CONFIG_HOME", Path.home() / ".config")
    unit_dir = config_home / "systemd/user"
    service = unit_dir / "orgbot-trash-cleanup.service"
    if not service.exists():
        return
    if service.is_symlink() or service.stat().st_uid != os.getuid():
        _fail("Refusing to update an unsafe cleanup service")
    systemctl = _systemctl_executable()
    if systemctl is None:
        _fail("systemctl is required to apply the selected trash root to the cleanup timer")
    _require_manager_config_home(systemctl, config_home)
    data_home = _absolute_env_path("XDG_DATA_HOME", Path.home() / ".local/share")
    helper = Path(__file__).resolve(strict=True)
    helper_info = helper.stat()
    if not stat.S_ISREG(helper_info.st_mode) or not os.access(helper, os.X_OK):
        _fail("Current trash helper is not a trusted executable file")
    dropin_dir = unit_dir / "orgbot-trash-cleanup.service.d"
    _create_absolute_dirs(dropin_dir)
    _private_config_dir(dropin_dir)
    dropin = dropin_dir / "20-trash-root.conf"
    old_content: bytes | None = None
    if dropin.exists() or dropin.is_symlink():
        info = dropin.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o600:
            _fail("Refusing to overwrite an unmanaged trash service drop-in")
        old_content = dropin.read_bytes()
        if b"# Managed by orgbot-trash" not in old_content:
            _fail("Refusing to overwrite an unmanaged trash service drop-in")
    content = (
        "[Service]\n"
        "# Managed by orgbot-trash\n"
        "Environment=" + _systemd_escape_path("XDG_CONFIG_HOME=" + str(config_home)) + " " + _systemd_escape_path("XDG_DATA_HOME=" + str(data_home)) + "\n"
        "ExecStart=\n"
        "ExecStart=/usr/bin/python3 " + _systemd_escape_path(str(helper)) + " cleanup\n"
        "ReadWritePaths=\n"
        "ReadWritePaths=" + _systemd_escape_path(str(root)) + "\n"
    )
    temporary = dropin.with_name(f".{dropin.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        os.write(fd, content.encode("utf-8"))
        os.fsync(fd)
        os.replace(temporary, dropin)
    finally:
        os.close(fd)
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass
    try:
        completed = subprocess.run(
            [str(systemctl), "--user", "daemon-reload"],
            timeout=10,
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        result_error = "systemd rejected the trash-root cleanup configuration" if completed.returncode != 0 else ""
    except (OSError, subprocess.TimeoutExpired):
        result_error = "systemd did not reload the trash-root cleanup configuration"
    if result_error:
        if old_content is None:
            try:
                dropin.unlink()
            except FileNotFoundError:
                pass
        else:
            _write_dropin(dropin, old_content)
        if systemctl is not None:
            try:
                subprocess.run([str(systemctl), "--user", "daemon-reload"], timeout=10, check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            except (OSError, subprocess.TimeoutExpired):
                pass
        _fail(result_error)


def _write_dropin(path: Path, content: bytes) -> None:
    temporary = path.with_name(f".{path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        os.write(fd, content)
        os.fsync(fd)
        os.replace(temporary, path)
        parent_fd = _open_absolute_dir(path.parent)
        try:
            os.fsync(parent_fd)
        finally:
            os.close(parent_fd)
    finally:
        os.close(fd)
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def _configure_root_locked_root(requested: Path, current_root: Path | None, previous: dict[str, Any] | None, *, sync_systemd: bool) -> dict[str, Any]:
    if current_root is not None and current_root != requested and list_entries(current_root):
        _fail("Cannot change trash location while the current trash contains entries")
    if not requested.parent.exists() and requested == _default_root():
        _create_absolute_dirs(requested.parent)
        _private_dir(requested.parent)
    _check_components(requested, allow_missing_leaf=True)
    if not requested.parent.is_dir():
        _fail("Trash root parent must already exist")
    if requested.exists() or requested.is_symlink():
        _private_dir(requested)
    identity = _fs_identity(requested if requested.exists() else requested.parent)
    if not identity["writable"]:
        _fail("Selected trash filesystem is read-only")
    if not requested.exists():
        _private_dir(requested, create=True)
    _private_dir(requested / "projects", create=True)
    _commit_config(requested, identity, previous, sync_systemd=sync_systemd)
    return _config_result()


def _configure_root(raw_root: str, *, defer_systemd: bool = False) -> dict[str, Any]:
    requested = Path(raw_root)
    if not requested.is_absolute() or "\x00" in raw_root:
        _fail("Trash root must be an absolute path")
    requested = Path(os.path.normpath(raw_root))
    if requested == Path("/"):
        _fail("Trash root cannot be the filesystem root")
    # Serialize competing root switches, then lock both roots in one stable
    # order. Publishing the config while holding the destination lock prevents
    # CLI operations from acting on a selection whose systemd sync can roll back.
    with _selection_locked():
        while True:
            current, _, needs_selection, previous = _selected_config()
            if needs_selection:
                if requested == _default_root() and not requested.parent.exists():
                    _create_absolute_dirs(requested.parent)
                    _private_dir(requested.parent)
                _prepare_root(requested, require_mount=True)
                with _locked(requested):
                    latest, _, still_needs_selection, previous = _selected_config()
                    if not still_needs_selection:
                        continue
                    return _configure_root_locked_root(requested, None, previous, sync_systemd=not defer_systemd)
            current_root = _prepare_root()
            _prepare_root(requested, require_mount=True)
            roots = sorted({current_root, requested}, key=lambda root: os.fspath(root))
            with contextlib.ExitStack() as stack:
                for root in roots:
                    stack.enter_context(_locked(root))
                latest, _, latest_needs_selection, latest_payload = _selected_config()
                if latest_needs_selection or latest != current_root:
                    continue
                current_root = _prepare_root()
                return _configure_root_locked_root(requested, current_root, latest_payload, sync_systemd=not defer_systemd)


def _verify_hdd_mount() -> None:
    expected = _fs_identity(HDD_MOUNT)
    if not os.path.ismount(HDD_MOUNT) or expected["filesystem"] != "ext4":
        _fail("Legacy HDD mount is unavailable or no longer ext4")
    try:
        root_info = LEGACY_TRASH_ROOT.lstat()
    except FileNotFoundError:
        return
    if stat.S_ISLNK(root_info.st_mode) or not stat.S_ISDIR(root_info.st_mode):
        _fail("Legacy HDD trash root is not a real directory")
    destination = _fs_identity(LEGACY_TRASH_ROOT)
    if destination["filesystem"] != "ext4" or not destination["writable"] or destination["device"] != expected["device"]:
        _fail("Legacy HDD trash root is unavailable or has changed filesystem")


def _private_dir(path: Path, *, create: bool = False) -> None:
    if create:
        try:
            path.mkdir(mode=0o700, parents=False)
        except FileExistsError:
            pass
        else:
            parent_fd = _open_dir(path.parent)
            try:
                os.fsync(parent_fd)
            finally:
                os.close(parent_fd)
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or stat.S_ISLNK(info.st_mode):
        _fail(f"Unsafe trash directory: {path}")
    if info.st_uid != os.getuid():
        _fail(f"Trash directory is not owned by this user: {path}")
    if stat.S_IMODE(info.st_mode) != 0o700:
        _fail(f"Trash directory permissions must be private (0700): {path}")


def _prepare_root(root: Path | None = None, *, require_mount: bool = True) -> Path:
    if root is None:
        root, source, requires_selection, payload = _selected_config()
        if requires_selection:
            _fail("Trash location requires explicit owner selection; run orgbot-trash configure --root ABSOLUTE_PATH")
    else:
        source = "explicit-test-root"
        payload = None
    root = Path(os.path.abspath(root))
    expected = payload["identity"] if payload is not None else None
    if require_mount:
        if source == "legacy-hdd":
            if root != LEGACY_TRASH_ROOT or root.parent != HDD_MOUNT:
                _fail("Legacy trash location does not match the supported HDD path")
            _verify_hdd_mount()
            _verify_identity(root, None)
        else:
            if source == "default" and not root.parent.exists():
                _create_absolute_dirs(root.parent)
                _private_dir(root.parent)
            _verify_identity(root, expected)
    else:
        _check_components(root, allow_missing_leaf=True)
        if not root.parent.exists():
            _fail("Trash parent directory must already exist")
        parent_info = root.parent.lstat()
        if stat.S_ISLNK(parent_info.st_mode) or not stat.S_ISDIR(parent_info.st_mode) or parent_info.st_uid != os.getuid():
            _fail("Test trash parent must be a real directory owned by this user")
    _private_dir(root, create=True)
    projects = root / "projects"
    _private_dir(projects, create=True)
    return root


@contextlib.contextmanager
def _locked(root: Path) -> Iterator[None]:
    lock_path = root / ".lock"
    fd = os.open(lock_path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) & 0o077:
            _fail("Unsafe trash lock file")
        fcntl.flock(fd, fcntl.LOCK_EX)
        yield
    finally:
        os.close(fd)


@contextlib.contextmanager
def _selection_locked() -> Iterator[None]:
    _, config_dir = _config_paths()
    _private_config_dir(config_dir, create=True)
    lock_path = config_dir / ".selection.lock"
    fd = os.open(lock_path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o600:
            _fail("Unsafe trash selection lock")
        fcntl.flock(fd, fcntl.LOCK_EX)
        yield
    finally:
        os.close(fd)


@contextlib.contextmanager
def _selected_root_locked(*, persist_selection: bool = False) -> Iterator[Path]:
    while True:
        root, _, requires_selection, _ = _selected_config()
        if requires_selection:
            _fail("Trash location requires explicit owner selection; run orgbot-trash configure --root ABSOLUTE_PATH")
        root = _prepare_root()
        with _locked(root):
            latest_root, _, latest_requires_selection, latest_payload = _selected_config()
            if latest_requires_selection or latest_root != root:
                continue
            root = _prepare_root()
            if persist_selection and latest_payload is None:
                identity = _fs_identity(root)
                _commit_config(root, identity, None)
            yield root
            return


def _reject_overlap(path: Path, root: Path) -> None:
    protected_roots = (Path.home(), Path("/mnt/storage"), HDD_MOUNT)
    for protected in protected_roots:
        try:
            if path == protected or path in protected.parents:
                _fail(f"Refusing protected or overlapping path: {path}")
        except OSError as exc:
            raise TrashError(f"Cannot inspect path: {exc.strerror}") from exc
    if path == root or path in root.parents or root in path.parents:
        _fail(f"Refusing protected or overlapping path: {path}")


def _open_dir(path: str | os.PathLike[str], *, dir_fd: int | None = None) -> int:
    return os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=dir_fd)


def _open_absolute_dir(path: Path) -> int:
    absolute = Path(os.path.abspath(path))
    fd = _open_dir("/")
    try:
        for component in absolute.parts[1:]:
            next_fd = _open_dir(component, dir_fd=fd)
            os.close(fd)
            fd = next_fd
        return fd
    except Exception:
        os.close(fd)
        raise


def _fsync_dir(fd: int) -> None:
    os.fsync(fd)


def _rename_noreplace(old: str | os.PathLike[str], new: str | os.PathLike[str], *, old_fd: int, new_fd: int) -> None:
    renameat2 = getattr(ctypes.CDLL(None, use_errno=True), "renameat2", None)
    if renameat2 is None:
        _fail("This Linux system lacks atomic no-overwrite rename support")
    renameat2.argtypes = (ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint)
    renameat2.restype = ctypes.c_int
    if renameat2(old_fd, os.fsencode(old), new_fd, os.fsencode(new), 1) != 0:
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error), os.fspath(old))


def _source_parent(path: Path, trash_root: Path = TRASH_ROOT) -> tuple[int, str, os.stat_result]:
    absolute = Path(os.path.abspath(path))
    try:
        if absolute.resolve(strict=True) != absolute:
            _fail("Symlink paths are not supported")
    except OSError as exc:
        raise TrashError(f"Cannot resolve source path: {exc.strerror}") from exc
    _reject_overlap(absolute, trash_root)
    name = absolute.name
    if not name or name in (".", ".."):
        _fail("Source must name a file or directory")
    parent_fd = _open_absolute_dir(absolute.parent)
    try:
        parent_stat = os.fstat(parent_fd)
        item_stat = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
        if stat.S_ISLNK(item_stat.st_mode) or not (stat.S_ISREG(item_stat.st_mode) or stat.S_ISDIR(item_stat.st_mode)):
            _fail("Only regular files and directories can be moved to trash")
        if item_stat.st_dev != parent_stat.st_dev or item_stat.st_nlink != 1 and stat.S_ISREG(item_stat.st_mode):
            _fail("Source mount boundaries and multiply-linked files are not supported")
        return parent_fd, name, item_stat
    except Exception:
        os.close(parent_fd)
        raise


def _node_record(relative: str, info: os.stat_result, kind: str, digest: str | None = None) -> dict[str, Any]:
    return {
        "path": relative,
        "type": kind,
        "mode": stat.S_IMODE(info.st_mode),
        "size": info.st_size if kind == "file" else 0,
        "sha256": digest,
        "dev": info.st_dev,
        "ino": info.st_ino,
    }


def _copy_tree(src_fd: int, dst_fd: int, source_dev: int, prefix: str = "") -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    for name in sorted(os.listdir(src_fd)):
        info = os.stat(name, dir_fd=src_fd, follow_symlinks=False)
        relative = f"{prefix}/{name}" if prefix else name
        if info.st_dev != source_dev or stat.S_ISLNK(info.st_mode):
            _fail(f"Unsupported symlink or mount boundary in source: {relative}")
        if stat.S_ISDIR(info.st_mode):
            os.mkdir(name, 0o700, dir_fd=dst_fd)
            child_src = _open_dir(name, dir_fd=src_fd)
            child_dst = _open_dir(name, dir_fd=dst_fd)
            try:
                child_records = _copy_tree(child_src, child_dst, source_dev, relative)
                after = os.fstat(child_src)
                if (after.st_dev, after.st_ino) != (info.st_dev, info.st_ino):
                    _fail(f"Source directory changed while copying: {relative}")
                os.fchmod(child_dst, stat.S_IMODE(info.st_mode))
                _fsync_dir(child_dst)
            finally:
                os.close(child_src)
                os.close(child_dst)
            records.append(_node_record(relative, info, "directory"))
            records.extend(child_records)
        elif stat.S_ISREG(info.st_mode) and info.st_nlink == 1:
            source_fd = os.open(name, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=src_fd)
            target_fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600, dir_fd=dst_fd)
            digest = hashlib.sha256()
            try:
                before = os.fstat(source_fd)
                if not stat.S_ISREG(before.st_mode) or (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns):
                    _fail(f"Source changed while copying: {relative}")
                while True:
                    data = os.read(source_fd, COPY_CHUNK)
                    if not data:
                        break
                    digest.update(data)
                    view = memoryview(data)
                    while view:
                        written = os.write(target_fd, view)
                        view = view[written:]
                after = os.fstat(source_fd)
                if (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns) != (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns):
                    _fail(f"Source changed while copying: {relative}")
                os.fchmod(target_fd, stat.S_IMODE(info.st_mode))
                os.fsync(target_fd)
            finally:
                os.close(source_fd)
                os.close(target_fd)
            records.append(_node_record(relative, info, "file", digest.hexdigest()))
        else:
            _fail(f"Unsupported source item: {relative}")
    return records


def _write_manifest(entry: Path, manifest: dict[str, Any]) -> None:
    temporary = entry / ".manifest.tmp"
    with temporary.open("x", encoding="utf-8") as stream:
        os.chmod(temporary, 0o600)
        json.dump(manifest, stream, sort_keys=True, separators=(",", ":"))
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, entry / "manifest.json")
    directory_fd = _open_dir(entry)
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)


def _scan_payload(payload_root: Path, top_name: str) -> list[dict[str, Any]]:
    root_fd = _open_absolute_dir(payload_root)
    try:
        return _scan_payload_at(root_fd, top_name, top_name)
    finally:
        os.close(root_fd)


def _scan_payload_at(parent_fd: int, item_name: str, record_name: str) -> list[dict[str, Any]]:
    top_info = os.stat(item_name, dir_fd=parent_fd, follow_symlinks=False)
    if stat.S_ISLNK(top_info.st_mode):
        _fail("Trash payload cannot be a symlink")
    root_record: list[dict[str, Any]] = []
    if stat.S_ISREG(top_info.st_mode):
        fd = os.open(item_name, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent_fd)
        digest = hashlib.sha256()
        try:
            before = os.fstat(fd)
            if not stat.S_ISREG(before.st_mode) or (before.st_dev, before.st_ino, before.st_size, before.st_ctime_ns) != (top_info.st_dev, top_info.st_ino, top_info.st_size, top_info.st_ctime_ns):
                _fail("Trash payload changed while validating")
            while True:
                data = os.read(fd, COPY_CHUNK)
                if not data:
                    break
                digest.update(data)
            after = os.fstat(fd)
            if (after.st_dev, after.st_ino, after.st_size, after.st_ctime_ns) != (before.st_dev, before.st_ino, before.st_size, before.st_ctime_ns):
                _fail("Trash payload changed while validating")
        finally:
            os.close(fd)
        root_record.append(_node_record(record_name, top_info, "file", digest.hexdigest()))
        return root_record
    if not stat.S_ISDIR(top_info.st_mode):
        _fail("Trash payload has an unsupported type")
    root_record.append(_node_record(record_name, top_info, "directory"))
    root_fd = _open_dir(item_name, dir_fd=parent_fd)
    try:
        root_info = os.fstat(root_fd)
        if (root_info.st_dev, root_info.st_ino) != (top_info.st_dev, top_info.st_ino):
            _fail("Trash payload changed while validating")
        for rec in _copy_tree_scan(root_fd, top_info.st_dev):
            rec["path"] = f"{record_name}/{rec['path']}"
            root_record.append(rec)
    finally:
        os.close(root_fd)
    return root_record


def _copy_tree_scan(fd: int, device: int, prefix: str = "") -> list[dict[str, Any]]:
    start = os.fstat(fd)
    records: list[dict[str, Any]] = []
    for name in sorted(os.listdir(fd)):
        info = os.stat(name, dir_fd=fd, follow_symlinks=False)
        rel = f"{prefix}/{name}" if prefix else name
        if info.st_dev != device or stat.S_ISLNK(info.st_mode):
            _fail(f"Unsafe item in trash payload: {rel}")
        if stat.S_ISDIR(info.st_mode):
            child = _open_dir(name, dir_fd=fd)
            try:
                child_info = os.fstat(child)
                if (child_info.st_dev, child_info.st_ino) != (info.st_dev, info.st_ino):
                    _fail(f"Trash payload changed while validating: {rel}")
                records.append(_node_record(rel, info, "directory"))
                records.extend(_copy_tree_scan(child, device, rel))
                final_info = os.fstat(child)
                if (final_info.st_dev, final_info.st_ino, final_info.st_ctime_ns, final_info.st_mtime_ns) != (child_info.st_dev, child_info.st_ino, child_info.st_ctime_ns, child_info.st_mtime_ns):
                    _fail(f"Trash payload changed while validating: {rel}")
            finally:
                os.close(child)
        elif stat.S_ISREG(info.st_mode) and info.st_nlink == 1:
            child_fd = os.open(name, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=fd)
            digest = hashlib.sha256()
            try:
                before = os.fstat(child_fd)
                if not stat.S_ISREG(before.st_mode) or (before.st_dev, before.st_ino, before.st_size) != (info.st_dev, info.st_ino, info.st_size):
                    _fail(f"Trash payload changed while validating: {rel}")
                while True:
                    data = os.read(child_fd, COPY_CHUNK)
                    if not data:
                        break
                    digest.update(data)
                after = os.fstat(child_fd)
                if (after.st_dev, after.st_ino, after.st_size, after.st_ctime_ns) != (before.st_dev, before.st_ino, before.st_size, before.st_ctime_ns):
                    _fail(f"Trash payload changed while validating: {rel}")
            finally:
                os.close(child_fd)
            records.append(_node_record(rel, info, "file", digest.hexdigest()))
        else:
            _fail(f"Unsafe item in trash payload: {rel}")
    end = os.fstat(fd)
    if (end.st_dev, end.st_ino, end.st_ctime_ns, end.st_mtime_ns) != (start.st_dev, start.st_ino, start.st_ctime_ns, start.st_mtime_ns):
        _fail(f"Trash payload directory changed while validating: {prefix or '.'}")
    return records


def _snapshot_source(parent_fd: int, name: str) -> list[dict[str, Any]]:
    info = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
    if stat.S_ISDIR(info.st_mode):
        root = [_node_record(name, info, "directory")]
        fd = _open_dir(name, dir_fd=parent_fd)
        try:
            root.extend(_copy_tree_scan(fd, info.st_dev, name))
        finally:
            os.close(fd)
        return root
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        _fail("Source changed to an unsupported file type")
    fd = os.open(name, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent_fd)
    digest = hashlib.sha256()
    try:
        before = os.fstat(fd)
        if not stat.S_ISREG(before.st_mode) or (before.st_dev, before.st_ino, before.st_size, before.st_ctime_ns) != (info.st_dev, info.st_ino, info.st_size, info.st_ctime_ns):
            _fail("Source changed while verifying copy")
        while True:
            data = os.read(fd, COPY_CHUNK)
            if not data:
                break
            digest.update(data)
        after = os.fstat(fd)
        if (after.st_dev, after.st_ino, after.st_size, after.st_ctime_ns) != (before.st_dev, before.st_ino, before.st_size, before.st_ctime_ns):
            _fail("Source changed while verifying copy")
    finally:
        os.close(fd)
    return [_node_record(name, info, "file", digest.hexdigest())]


def _copy_to_stage(source_parent_fd: int, name: str, destination_parent_fd: int, stage_name: str) -> None:
    info = os.stat(name, dir_fd=source_parent_fd, follow_symlinks=False)
    if stat.S_ISDIR(info.st_mode):
        os.mkdir(stage_name, 0o700, dir_fd=destination_parent_fd)
        source_fd = _open_dir(name, dir_fd=source_parent_fd)
        target_fd = _open_dir(stage_name, dir_fd=destination_parent_fd)
        try:
            if (os.fstat(source_fd).st_dev, os.fstat(source_fd).st_ino) != (info.st_dev, info.st_ino):
                _fail("Trash payload changed before restore")
            _copy_tree(source_fd, target_fd, info.st_dev, stage_name)
            os.fchmod(target_fd, stat.S_IMODE(info.st_mode))
            _fsync_dir(target_fd)
        finally:
            os.close(source_fd)
            os.close(target_fd)
        return
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        _fail("Trash payload contains an unsupported item")
    source_fd = os.open(name, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=source_parent_fd)
    target_fd = os.open(stage_name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600, dir_fd=destination_parent_fd)
    try:
        before = os.fstat(source_fd)
        if not stat.S_ISREG(before.st_mode) or (before.st_dev, before.st_ino, before.st_size, before.st_ctime_ns) != (info.st_dev, info.st_ino, info.st_size, info.st_ctime_ns):
            _fail("Trash payload changed before restore")
        while True:
            data = os.read(source_fd, COPY_CHUNK)
            if not data:
                break
            view = memoryview(data)
            while view:
                written = os.write(target_fd, view)
                view = view[written:]
        after = os.fstat(source_fd)
        if (after.st_dev, after.st_ino, after.st_size, after.st_ctime_ns) != (before.st_dev, before.st_ino, before.st_size, before.st_ctime_ns):
            _fail("Trash payload changed during restore")
        os.fchmod(target_fd, stat.S_IMODE(info.st_mode))
        os.fsync(target_fd)
    finally:
        os.close(source_fd)
        os.close(target_fd)


def _verify_restore_parent(path: Path, parent_fd: int) -> None:
    pinned = os.fstat(parent_fd)
    current = os.stat(path, follow_symlinks=False)
    if not stat.S_ISDIR(current.st_mode) or (current.st_dev, current.st_ino) != (pinned.st_dev, pinned.st_ino):
        _fail("Restore parent changed during restore; the trash backup was retained")


def _verify_payload(entry: Path, manifest: dict[str, Any]) -> None:
    names = sorted(p.name for p in entry.iterdir())
    if names != ["manifest.json", "payload"]:
        _fail("Trash entry contains unexpected or missing files")
    actual = _scan_payload(entry / "payload", manifest["item_name"])
    expected = manifest.get("nodes")
    if not isinstance(expected, list) or len(actual) != len(expected):
        _fail("Trash payload does not match its manifest")
    for got, want in zip(actual, expected):
        if any(got.get(k) != want.get(k) for k in ("path", "type", "mode", "size", "sha256")):
            _fail("Trash payload does not match its manifest")


def _load_entry(path: Path) -> dict[str, Any]:
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) & 0o077:
        _fail("Unsafe trash entry")
    manifest_path = path / "manifest.json"
    manifest_info = manifest_path.lstat()
    if not stat.S_ISREG(manifest_info.st_mode) or manifest_info.st_uid != os.getuid() or stat.S_IMODE(manifest_info.st_mode) & 0o077:
        _fail("Unsafe trash manifest")
    data = json.loads(manifest_path.read_text(encoding="utf-8"))
    required = {"schema", "id", "status", "project", "original_path", "item_name", "moved_at", "nodes", "source_recovery"}
    if not isinstance(data, dict) or set(data) != required or data["schema"] != SCHEMA or data["id"] != path.name:
        _fail("Invalid trash manifest")
    if data["status"] not in ("copying", "pending_source_removal", "complete"):
        _fail("Invalid trash entry state")
    if (
        not isinstance(data["project"], str)
        or not isinstance(data["original_path"], str)
        or not Path(data["original_path"]).is_absolute()
        or not isinstance(data["item_name"], str)
        or data["item_name"] in ("", ".", "..")
        or Path(data["item_name"]).name != data["item_name"]
        or Path(data["original_path"]).name != data["item_name"]
        or not isinstance(data["moved_at"], (float, int))
        or isinstance(data["moved_at"], bool)
        or not math.isfinite(data["moved_at"])
        or not isinstance(data["nodes"], list)
        or not isinstance(data["source_recovery"], str)
        or (data["source_recovery"] and not Path(data["source_recovery"]).is_absolute())
    ):
        _fail("Invalid trash manifest values")
    for node in data["nodes"]:
        if not isinstance(node, dict) or set(node) != {"path", "type", "mode", "size", "sha256", "dev", "ino"}:
            _fail("Invalid trash manifest node")
        node_path = Path(node["path"]) if isinstance(node["path"], str) else Path("/")
        if (
            not isinstance(node["path"], str)
            or node_path.is_absolute()
            or ".." in node_path.parts
            or not node_path.parts
            or node_path.parts[0] != data["item_name"]
            or node["type"] not in ("file", "directory")
            or not all(isinstance(node[k], int) and not isinstance(node[k], bool) for k in ("mode", "size", "dev", "ino"))
            or (node["type"] == "file" and (not isinstance(node["sha256"], str) or not re.fullmatch(r"[a-f0-9]{64}", node["sha256"])))
            or (node["type"] == "directory" and node["sha256"] is not None)
        ):
            _fail("Invalid trash manifest node values")
    return data


def _remove_known_tree(parent_fd: int, name: str, records: list[dict[str, Any]]) -> None:
    by_path = {record["path"]: record for record in records}
    children: dict[str, list[str]] = {}
    for record in records:
        item_path = record["path"]
        if "/" in item_path:
            parent, child = item_path.rsplit("/", 1)
            children.setdefault(parent, []).append(child)
    for child_names in children.values():
        child_names.sort()
    _remove_known_tree_inner(parent_fd, name, name, by_path, children)


def _restore_quarantined_name(parent_fd: int, quarantine_name: str, original_name: str) -> None:
    try:
        os.stat(quarantine_name, dir_fd=parent_fd, follow_symlinks=False)
    except FileNotFoundError:
        return
    try:
        os.stat(original_name, dir_fd=parent_fd, follow_symlinks=False)
    except FileNotFoundError:
        _rename_noreplace(quarantine_name, original_name, old_fd=parent_fd, new_fd=parent_fd)
        _fsync_dir(parent_fd)


def _remove_known_tree_inner(
    parent_fd: int,
    name: str,
    relative: str,
    by_path: dict[str, dict[str, Any]],
    children: dict[str, list[str]],
) -> None:
    info = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
    expected = by_path.get(relative)
    if expected is None or stat.S_ISLNK(info.st_mode):
        _fail(f"Refusing to remove unvalidated item: {relative}")
    if expected["type"] == "directory":
        child_fd = _open_dir(name, dir_fd=parent_fd)
        try:
            child_info = os.fstat(child_fd)
            if (child_info.st_dev, child_info.st_ino) != (expected["dev"], expected["ino"]) or stat.S_IMODE(child_info.st_mode) != expected["mode"]:
                _fail(f"Source directory replaced before removal: {relative}")
            names = sorted(os.listdir(child_fd))
            expected_names = children.get(relative, [])
            if names != expected_names:
                _fail(f"Source changed before removal: {relative}")
            for child in names:
                _remove_known_tree_inner(child_fd, child, f"{relative}/{child}", by_path, children)
        finally:
            os.close(child_fd)
        current = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
        if (current.st_dev, current.st_ino) != (expected["dev"], expected["ino"]) or stat.S_IMODE(current.st_mode) != expected["mode"]:
            _fail(f"Source directory replaced before removal: {relative}")
        quarantine_name = f".orgbot-remove-{uuid.uuid4().hex}"
        _rename_noreplace(name, quarantine_name, old_fd=parent_fd, new_fd=parent_fd)
        _fsync_dir(parent_fd)
        try:
            moved = os.stat(quarantine_name, dir_fd=parent_fd, follow_symlinks=False)
            if (moved.st_dev, moved.st_ino) != (expected["dev"], expected["ino"]) or stat.S_IMODE(moved.st_mode) != expected["mode"]:
                _fail(f"Source directory changed during removal: {relative}")
            moved_fd = _open_dir(quarantine_name, dir_fd=parent_fd)
            try:
                if os.listdir(moved_fd):
                    _fail(f"Source directory changed during removal: {relative}")
            finally:
                os.close(moved_fd)
            os.rmdir(quarantine_name, dir_fd=parent_fd)
            _fsync_dir(parent_fd)
        except Exception:
            _restore_quarantined_name(parent_fd, quarantine_name, name)
            raise
    else:
        quarantine_name = f".orgbot-remove-{uuid.uuid4().hex}"
        _rename_noreplace(name, quarantine_name, old_fd=parent_fd, new_fd=parent_fd)
        _fsync_dir(parent_fd)
        try:
            child_fd = os.open(quarantine_name, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent_fd)
            digest = hashlib.sha256()
            try:
                before = os.fstat(child_fd)
                if not stat.S_ISREG(before.st_mode) or (before.st_dev, before.st_ino, before.st_size) != (expected["dev"], expected["ino"], expected["size"]) or stat.S_IMODE(before.st_mode) != expected["mode"]:
                    _fail(f"Source file changed before removal: {relative}")
                while True:
                    data = os.read(child_fd, COPY_CHUNK)
                    if not data:
                        break
                    digest.update(data)
                after = os.fstat(child_fd)
                if (after.st_dev, after.st_ino, after.st_size, after.st_ctime_ns) != (before.st_dev, before.st_ino, before.st_size, before.st_ctime_ns) or digest.hexdigest() != expected["sha256"]:
                    _fail(f"Source file changed before removal: {relative}")
                current = os.stat(quarantine_name, dir_fd=parent_fd, follow_symlinks=False)
                if (current.st_dev, current.st_ino, current.st_size, current.st_ctime_ns) != (after.st_dev, after.st_ino, after.st_size, after.st_ctime_ns):
                    _fail(f"Source file was replaced during removal: {relative}")
                os.unlink(quarantine_name, dir_fd=parent_fd)
                _fsync_dir(parent_fd)
            finally:
                os.close(child_fd)
        except Exception:
            _restore_quarantined_name(parent_fd, quarantine_name, name)
            raise


def move(root: Path, project: str, source: Path, *, expected_sha256: str | None = None) -> str:
    if not project.strip() or len(project) > 160:
        _fail("Project name must contain 1 to 160 characters")
    parent_fd, name, initial = _source_parent(source, root)
    try:
        if expected_sha256 is not None:
            if not re.fullmatch(r"[a-f0-9]{64}", expected_sha256) or not stat.S_ISREG(initial.st_mode):
                _fail("Expected SHA-256 requires a valid regular-file digest")
            source_fd = os.open(name, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent_fd)
            try:
                before = os.fstat(source_fd)
                expected_stat = (initial.st_dev, initial.st_ino, initial.st_size, initial.st_mtime_ns, initial.st_ctime_ns)
                if not stat.S_ISREG(before.st_mode) or (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns) != expected_stat:
                    _fail("Source changed before expected-content verification")
                digest = hashlib.sha256()
                while True:
                    chunk = os.read(source_fd, COPY_CHUNK)
                    if not chunk:
                        break
                    digest.update(chunk)
                after = os.fstat(source_fd)
                if (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns) != expected_stat:
                    _fail("Source changed during expected-content verification")
                if digest.hexdigest() != expected_sha256:
                    _fail("Source content does not match the reviewed SHA-256; source retained")
            finally:
                os.close(source_fd)
        project_dir = root / "projects" / _safe_component(project)
        _private_dir(project_dir, create=True)
        entry_id = uuid.uuid4().hex
        entry = project_dir / entry_id
        entry.mkdir(mode=0o700)
        project_fd = _open_dir(project_dir)
        try:
            _fsync_dir(project_fd)
        finally:
            os.close(project_fd)
        payload = entry / "payload"
        payload.mkdir(mode=0o700)
        entry_fd = _open_dir(entry)
        try:
            _fsync_dir(entry_fd)
        finally:
            os.close(entry_fd)
        manifest = {
            "schema": SCHEMA,
            "id": entry_id,
            "status": "copying",
            "project": project,
            "original_path": str(Path(os.path.abspath(source))),
            "item_name": name,
            "moved_at": time.time(),
            "nodes": [],
            "source_recovery": "",
        }
        _write_manifest(entry, manifest)
        try:
            # Populate payload without following any source symlink.
            if stat.S_ISDIR(initial.st_mode):
                payload_item = payload / name
                payload_item.mkdir(mode=0o700)
                src_fd = _open_dir(name, dir_fd=parent_fd)
                dst_fd = _open_dir(payload_item)
                try:
                    nodes = [_node_record(name, initial, "directory")]
                    nodes.extend(_copy_tree(src_fd, dst_fd, initial.st_dev, name))
                    os.fchmod(dst_fd, stat.S_IMODE(initial.st_mode))
                    _fsync_dir(dst_fd)
                finally:
                    os.close(src_fd)
                    os.close(dst_fd)
            else:
                source_fd = os.open(name, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent_fd)
                payload_item = payload / name
                dest_fd = os.open(payload_item, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
                digest = hashlib.sha256()
                try:
                    before = os.fstat(source_fd)
                    if not stat.S_ISREG(before.st_mode) or (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (initial.st_dev, initial.st_ino, initial.st_size, initial.st_mtime_ns, initial.st_ctime_ns):
                        _fail("Source changed before copy")
                    while True:
                        data = os.read(source_fd, COPY_CHUNK)
                        if not data:
                            break
                        digest.update(data)
                        view = memoryview(data)
                        while view:
                            written = os.write(dest_fd, view)
                            view = view[written:]
                    after = os.fstat(source_fd)
                    if (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns) != (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns):
                        _fail("Source changed while copying")
                    os.fchmod(dest_fd, stat.S_IMODE(initial.st_mode))
                    os.fsync(dest_fd)
                    nodes = [_node_record(name, initial, "file", digest.hexdigest())]
                finally:
                    os.close(source_fd)
                    os.close(dest_fd)
            payload_fd = _open_dir(payload)
            try:
                _fsync_dir(payload_fd)
            finally:
                os.close(payload_fd)
            copied = _scan_payload(payload, name)
            fields = ("path", "type", "mode", "size", "sha256")
            if len(copied) != len(nodes) or any(any(a.get(field) != b.get(field) for field in fields) for a, b in zip(copied, nodes)):
                _fail("Trash copy verification failed")
            source_snapshot = _snapshot_source(parent_fd, name)
            source_fields = fields + ("dev", "ino")
            if len(source_snapshot) != len(nodes) or any(any(a.get(field) != b.get(field) for field in source_fields) for a, b in zip(source_snapshot, nodes)):
                _fail("Source changed during copy; source retained")
            manifest["status"] = "pending_source_removal"
            manifest["nodes"] = nodes
            removal_dir = f".orgbot-trash-{entry_id}"
            manifest["source_recovery"] = str(Path(os.path.abspath(source)).parent / removal_dir)
            _write_manifest(entry, manifest)
            current = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
            if (current.st_dev, current.st_ino) != (initial.st_dev, initial.st_ino):
                _fail("Source was replaced after copy; verified copy retained in pending trash entry")
            try:
                os.mkdir(removal_dir, 0o700, dir_fd=parent_fd)
            except FileExistsError as exc:
                raise TrashError("Source recovery directory already exists") from exc
            removal_fd = _open_dir(removal_dir, dir_fd=parent_fd)
            try:
                os.fchmod(removal_fd, 0o700)
                _fsync_dir(parent_fd)
                _rename_noreplace(name, name, old_fd=parent_fd, new_fd=removal_fd)
                _fsync_dir(parent_fd)
                _fsync_dir(removal_fd)
                moved = os.stat(name, dir_fd=removal_fd, follow_symlinks=False)
                if (moved.st_dev, moved.st_ino) != (initial.st_dev, initial.st_ino):
                    _fail("Source changed during move")
                quarantined_snapshot = _snapshot_source(removal_fd, name)
                source_fields = fields + ("dev", "ino")
                if len(quarantined_snapshot) != len(nodes) or any(any(a.get(field) != b.get(field) for field in source_fields) for a, b in zip(quarantined_snapshot, nodes)):
                    _fail("Source changed after entering recovery quarantine")
                _remove_known_tree(removal_fd, name, nodes)
                _fsync_dir(removal_fd)
            except Exception as exc:
                try:
                    os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
                except FileNotFoundError:
                    try:
                        os.stat(name, dir_fd=removal_fd, follow_symlinks=False)
                    except FileNotFoundError:
                        pass
                    else:
                        _rename_noreplace(name, name, old_fd=removal_fd, new_fd=parent_fd)
                        _fsync_dir(parent_fd)
                if not os.listdir(removal_fd):
                    os.close(removal_fd)
                    removal_fd = -1
                    os.rmdir(removal_dir, dir_fd=parent_fd)
                    _fsync_dir(parent_fd)
                    manifest["source_recovery"] = ""
                    _write_manifest(entry, manifest)
                recovery_note = (
                    f"source recovery directory retained at {manifest['source_recovery']}"
                    if manifest["source_recovery"]
                    else "source path was preserved or restored; the verified trash copy remains pending"
                )
                raise TrashError(f"{exc}; {recovery_note}") from exc
            finally:
                if removal_fd >= 0:
                    os.close(removal_fd)
            os.rmdir(removal_dir, dir_fd=parent_fd)
            _fsync_dir(parent_fd)
            manifest["status"] = "complete"
            manifest["moved_at"] = time.time()
            manifest["source_recovery"] = ""
            _write_manifest(entry, manifest)
            return entry_id
        except Exception:
            raise
    finally:
        os.close(parent_fd)


def _find_entry(root: Path, entry_id: str) -> tuple[Path, dict[str, Any]]:
    if not re.fullmatch(r"[a-f0-9]{32}", entry_id):
        _fail("Invalid entry ID")
    matches: list[Path] = []
    for project_dir in (root / "projects").iterdir():
        if project_dir.is_symlink() or not project_dir.is_dir():
            _fail("Unsafe project directory in trash")
        candidate = project_dir / entry_id
        if candidate.exists() or candidate.is_symlink():
            matches.append(candidate)
    if len(matches) != 1:
        _fail("Trash entry was not found uniquely")
    return matches[0], _load_entry(matches[0])


def list_entries(root: Path) -> list[dict[str, Any]]:
    result = []
    for project_dir in sorted((root / "projects").iterdir()):
        if project_dir.is_symlink() or not project_dir.is_dir():
            _fail("Unsafe project directory in trash")
        for entry in sorted(project_dir.iterdir()):
            manifest = _load_entry(entry)
            result.append(manifest)
    return result


def restore(root: Path, entry_id: str) -> None:
    entry, manifest = _find_entry(root, entry_id)
    if manifest["status"] not in ("complete", "pending_source_removal"):
        _fail("Entry copy is incomplete; inspect the original path before recovery")
    _verify_payload(entry, manifest)
    destination = Path(manifest["original_path"])
    parent = destination.parent
    try:
        parent_fd = _open_absolute_dir(parent)
    except OSError as exc:
        raise TrashError(f"Restore parent is unavailable or contains a symlink: {exc.strerror}") from exc
    try:
        if destination.name != manifest["item_name"]:
            _fail("Manifest destination does not match item name")
        try:
            os.stat(destination.name, dir_fd=parent_fd, follow_symlinks=False)
        except FileNotFoundError:
            pass
        else:
            _fail("Restore destination already exists; it will not be overwritten")
        stage = f".orgbot-restore-{entry_id}"
        try:
            os.stat(stage, dir_fd=parent_fd, follow_symlinks=False)
        except FileNotFoundError:
            pass
        else:
            _fail("Restore staging path already exists")
        payload_fd = _open_dir(entry / "payload")
        try:
            _copy_to_stage(payload_fd, manifest["item_name"], parent_fd, stage)
        finally:
            os.close(payload_fd)
        copied = parent / stage
        staged = _scan_payload_at(parent_fd, stage, stage)
        original = _scan_payload(entry / "payload", manifest["item_name"])
        fields = ("type", "mode", "size", "sha256")
        if len(staged) != len(original) or any(any(a.get(field) != b.get(field) for field in fields) for a, b in zip(staged, original)):
            # Preserve the staged copy for manual recovery when verification fails.
            _fail(f"Restore verification failed; staged data preserved at {copied}")
        _fsync_dir(parent_fd)
        try:
            if manifest["nodes"][0]["type"] == "file":
                os.link(stage, destination.name, src_dir_fd=parent_fd, dst_dir_fd=parent_fd, follow_symlinks=False)
                os.unlink(stage, dir_fd=parent_fd)
            else:
                _rename_noreplace(stage, destination.name, old_fd=parent_fd, new_fd=parent_fd)
            _fsync_dir(parent_fd)
        except FileExistsError:
            _fail("Restore destination appeared during restore; no overwrite occurred")
        _verify_restore_parent(parent, parent_fd)
        if manifest["status"] == "complete":
            _remove_entry(entry, manifest)
    finally:
        os.close(parent_fd)


def _remove_entry(entry: Path, manifest: dict[str, Any]) -> None:
    _verify_payload(entry, manifest)
    record = _scan_payload(entry / "payload", manifest["item_name"])
    parent_fd = _open_dir(entry)
    try:
        payload_fd = _open_dir("payload", dir_fd=parent_fd)
        try:
            _remove_known_tree(payload_fd, manifest["item_name"], record)
            _fsync_dir(payload_fd)
        finally:
            os.close(payload_fd)
        os.rmdir("payload", dir_fd=parent_fd)
        _fsync_dir(parent_fd)
        os.unlink("manifest.json", dir_fd=parent_fd)
        _fsync_dir(parent_fd)
        project_fd = _open_dir(entry.parent)
        os.close(parent_fd)
        parent_fd = -1
        try:
            entry.rmdir()
            _fsync_dir(project_fd)
        finally:
            os.close(project_fd)
    finally:
        if parent_fd >= 0:
            os.close(parent_fd)


def cleanup(root: Path, *, now: float | None = None, dry_run: bool = False) -> tuple[list[str], list[str]]:
    current = time.time() if now is None else now
    purged: list[str] = []
    errors: list[str] = []
    projects = root / "projects"
    for project_dir in sorted(projects.iterdir()):
        if project_dir.is_symlink() or not project_dir.is_dir():
            errors.append(f"Skipped unsafe project directory: {project_dir.name}")
            continue
        for entry in sorted(project_dir.iterdir()):
            try:
                manifest = _load_entry(entry)
                if manifest["status"] != "complete":
                    continue
                moved = manifest["moved_at"]
                if moved > current or current - moved < RETENTION_SECONDS:
                    continue
                _verify_payload(entry, manifest)
                if not dry_run:
                    _remove_entry(entry, manifest)
                purged.append(manifest["id"])
            except (TrashError, OSError, ValueError, json.JSONDecodeError) as exc:
                errors.append(f"Preserved entry {entry.name}: {exc}")
    return purged, errors


def _working_copy() -> Path | None:
    """The bot working copy this helper runs in, if it runs in one.

    OrgBots runs each bot on a private copy of the project and names it in
    ORGBOT_WORKSPACE. The owner's trash is out of reach from there, and need
    not be reached: the original stays in the project until the owner approves
    its removal, and then goes to the owner's trash.
    """
    raw = os.environ.get("ORGBOT_WORKSPACE")
    if not raw or not os.path.isabs(raw) or os.path.islink(raw) or not os.path.isdir(raw):
        return None
    return Path(os.path.realpath(raw))


def _inside(workspace: Path, source: Path) -> Path | None:
    """`source` relative to `workspace`, when it really is in there."""
    absolute = Path(os.path.abspath(source))
    parent = Path(os.path.realpath(absolute.parent))
    if parent != workspace and workspace not in parent.parents:
        return None
    relative = (parent / absolute.name).relative_to(workspace)
    return relative if relative.parts else None


def working_copy_move(workspace: Path, relative: Path) -> str:
    """Take a file out of a bot's working copy, keeping the bot's version."""
    source = workspace / relative
    if not os.path.lexists(source):
        _fail(f"No such file in the working copy: {relative}")
    held = (
        _absolute_env_path("XDG_DATA_HOME", Path.home() / ".local/share")
        / "orgbots" / "working-copy-trash"
        / f"{time.strftime('%Y%m%dT%H%M%S')}-{uuid.uuid4().hex[:8]}"
    )
    destination = held / relative
    destination.parent.mkdir(parents=True, mode=0o700)
    # The working copy and the private home are separate mounts in the
    # sandbox, so this copies where a rename cannot cross.
    shutil.move(os.fspath(source), os.fspath(destination))
    return (
        f"Removed {relative} from your working copy (your version is kept at {destination}). "
        "The original stays in the project until the owner approves its removal, then goes to the owner's trash."
    )


def _cli() -> int:
    parser = argparse.ArgumentParser(description="Move files to recoverable, project-grouped OrgBots trash")
    sub = parser.add_subparsers(dest="command", required=True)
    move_parser = sub.add_parser("move", help="Move files or directories into project trash")
    move_parser.add_argument("--project", required=True)
    move_parser.add_argument("--expected-sha256")
    move_parser.add_argument("paths", nargs="+", type=Path)
    sub.add_parser("list", help="List trashed entries")
    restore_parser = sub.add_parser("restore", help="Restore one entry without overwriting")
    restore_parser.add_argument("entry_id")
    cleanup_parser = sub.add_parser("cleanup", help="Permanently remove validated entries older than seven days")
    cleanup_parser.add_argument("--dry-run", action="store_true")
    sub.add_parser("config", help="Show the selected trash location as JSON")
    configure_parser = sub.add_parser("configure", help="Select and pin a trash filesystem")
    configure_parser.add_argument("--root", required=True, metavar="ABSOLUTE_PATH")
    configure_parser.add_argument("--defer-systemd", action="store_true", help=argparse.SUPPRESS)
    sub.add_parser("sync-systemd", help=argparse.SUPPRESS)
    args = parser.parse_args()
    try:
        if args.command in ("config", "configure") and _working_copy() is not None:
            # Asked from a bot's working copy, where no trash location applies.
            # Reporting the owner's selection here sent a bot to ask the owner
            # to pick storage it neither needs nor can reach.
            if args.command == "configure":
                _fail("Not needed in a working copy: `orgbot-trash move` works here as it is")
            print(json.dumps({
                "mode": "working_copy",
                "requires_selection": False,
                "note": "move takes files out of this working copy and keeps them in your private home; the original goes to the owner's trash when the owner approves its removal",
            }, sort_keys=True))
            return 0
        if args.command == "config":
            print(json.dumps(_config_result(), sort_keys=True))
            return 0
        if args.command == "configure":
            print(json.dumps(_configure_root(args.root, defer_systemd=args.defer_systemd), sort_keys=True))
            return 0
        if args.command == "sync-systemd":
            with _selected_root_locked() as root:
                _sync_systemd(root)
            return 0
        workspace = _working_copy() if args.command == "move" else None
        if workspace is not None:
            inside = [_inside(workspace, source) for source in args.paths]
            if all(inside):
                if args.expected_sha256 is not None:
                    _fail("Expected SHA-256 applies to the owner's project, not a working copy")
                for relative in inside:
                    print(working_copy_move(workspace, relative))
                return 0
            if any(inside):
                _fail("Trash working-copy files and other files in separate commands")
            # From the live room: a bot passed the original project's path,
            # the helper went on to the owner's trash, which a sandbox cannot
            # reach, and the bot was told to configure storage it does not need.
            _fail(
                f"Pass paths inside your working copy ({workspace}), relative to your current "
                "directory. The original project is read-only here; removing a file from your "
                "copy is how its removal reaches the owner, who approves it and keeps the "
                "original in their trash."
            )
        mutating = args.command in ("move", "restore") or (args.command == "cleanup" and not args.dry_run)
        with _selected_root_locked(persist_selection=mutating) as root:
            if args.command == "move":
                if args.expected_sha256 is not None and len(args.paths) != 1:
                    _fail("Expected SHA-256 can be used with exactly one source path")
                for source in args.paths:
                    print(move(root, args.project, source, expected_sha256=args.expected_sha256))
            elif args.command == "list":
                for row in list_entries(root):
                    print(json.dumps({key: row[key] for key in ("id", "project", "status", "moved_at", "original_path", "source_recovery")}, ensure_ascii=False, sort_keys=True))
            elif args.command == "restore":
                restore(root, args.entry_id)
                print(f"Restored {args.entry_id}")
            elif args.command == "cleanup":
                ids, errors = cleanup(root, dry_run=args.dry_run)
                for entry_id in ids:
                    print(("Would permanently remove " if args.dry_run else "Permanently removed ") + entry_id)
                for error in errors:
                    print(error, file=sys.stderr)
                if errors:
                    return 2
        return 0
    except (TrashError, OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"orgbot-trash: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(_cli())
