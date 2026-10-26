#!/usr/bin/env python3
"""Exercise installer decisions with fake package tools and temporary APT paths."""

from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
INSTALLER = ROOT / "scripts/install-orgbots.sh"
FINGERPRINT = "81445722C3B9265312C12039F831B11611E6D198"


def write_executable(path: Path, source: str) -> None:
    path.write_text(source, encoding="utf-8")
    path.chmod(0o755)


def make_fixture(root: Path) -> tuple[Path, Path, dict[str, str]]:
    fixture = root / "fixture"
    fake_bin = fixture / "bin"
    apt_root = fixture / "etc/apt"
    fake_bin.mkdir(parents=True)
    apt_root.mkdir(parents=True)
    os_release = fixture / "os-release"
    os_release.write_text('ID=ubuntu\nVERSION_ID="24.04"\n', encoding="utf-8")
    rewritten = fixture / "install-orgbots.sh"
    source = INSTALLER.read_text(encoding="utf-8")
    source = source.replace("/etc/apt", str(apt_root))
    source = source.replace("/etc/os-release", str(os_release))
    rewritten.write_text(source, encoding="utf-8")

    write_executable(
        fake_bin / "dpkg",
        '#!/bin/sh\nprintf "%s\\n" "${FAKE_ARCH:-amd64}"\n',
    )
    write_executable(
        fake_bin / "curl",
        """#!/usr/bin/env python3
import os, pathlib, sys
args = sys.argv[1:]
if os.environ.get("FAKE_DOWNLOAD_FAIL") == "1":
    raise SystemExit(22)
pathlib.Path(args[args.index("--output") + 1]).write_bytes(b"fixture-public-key")
""",
    )
    write_executable(
        fake_bin / "gpg",
        """#!/usr/bin/env python3
import os
fingerprint = os.environ.get("FAKE_FINGERPRINT", "81445722C3B9265312C12039F831B11611E6D198")
print("pub:u:3072:1:ABCDEF:0:0::sc:::::23:")
print("fpr:::::::::" + fingerprint + ":")
print("sub:u:3072:1:123456:0:0:::::e:")
print("fpr:::::::::0123456789ABCDEF0123456789ABCDEF01234567:")
if os.environ.get("FAKE_GPG_FAIL") == "1":
    raise SystemExit(2)
if os.environ.get("FAKE_EXTRA_PRIMARY") == "1":
    print("pub:u:3072:1:987654:0:0::sc:::::23:")
    print("fpr:::::::::9876543210ABCDEF9876543210ABCDEF98765432:")
""",
    )
    write_executable(
        fake_bin / "apt-get",
        '#!/bin/sh\nprintf "%s\\n" "$*" >> "$FAKE_APT_LOG"\n',
    )
    write_executable(
        fake_bin / "sudo",
        '#!/bin/sh\nif [ "${1:-}" = -- ]; then shift; fi\nexec "$@"\n',
    )
    write_executable(
        fake_bin / "stat",
        '#!/bin/sh\nprintf "0:0:644\\n"\n',
    )

    environment = os.environ.copy()
    environment.update(
        {
            "PATH": f"{fake_bin}:{environment['PATH']}",
            "FAKE_APT_LOG": str(fixture / "apt.log"),
            "TMPDIR": str(fixture),
        }
    )
    return rewritten, apt_root, environment


def run(script: Path, env: dict[str, str], *, ok: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        ["/bin/bash", str(script)],
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if (result.returncode == 0) != ok:
        raise AssertionError(
            f"installer return code {result.returncode}; expected success={ok}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result


def main() -> None:
    help_result = subprocess.run(
        ["/bin/bash", str(INSTALLER), "--help"],
        env={"PATH": ""},
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    assert help_result.returncode == 0 and "Usage:" in help_result.stdout

    with tempfile.TemporaryDirectory(prefix="orgbots-installer-test-") as temporary:
        root = Path(temporary)
        script, apt_root, env = make_fixture(root)
        key = apt_root / "keyrings/orgbots-archive-keyring.gpg"
        source = apt_root / "sources.list.d/orgbots.sources"

        result = run(script, env)
        assert key.read_bytes() == b"fixture-public-key"
        assert source.read_text(encoding="utf-8") == (
            "Types: deb\n"
            "URIs: https://acememe7.github.io/Orgbot/apt\n"
            "Suites: stable\n"
            "Components: main\n"
            "Architectures: amd64\n"
            f"Signed-By: {apt_root}/keyrings/orgbots-archive-keyring.gpg\n"
        )
        calls = Path(env["FAKE_APT_LOG"]).read_text(encoding="utf-8").splitlines()
        assert calls == ["update", "install orgbots"], calls
        assert "-y" not in " ".join(calls), calls

        original_key = key.read_bytes()
        original_source = source.read_bytes()
        run(script, env)
        assert key.read_bytes() == original_key
        assert source.read_bytes() == original_source
        assert not list(apt_root.rglob("*.orgbots-backup.*"))

        source.write_text("Types: deb\nURIs: https://owner.invalid/repo\n", encoding="utf-8")
        key.write_bytes(b"owner-key-data")
        run(script, env)
        backups = list(apt_root.rglob("*.orgbots-backup.*"))
        assert len(backups) == 2, backups
        assert any(path.read_bytes() == b"owner-key-data" for path in backups)
        assert any(
            path.read_bytes() == b"Types: deb\nURIs: https://owner.invalid/repo\n"
            for path in backups
        )
        assert key.read_bytes() == b"fixture-public-key"
        assert "URIs: https://acememe7.github.io/Orgbot/apt" in source.read_text(encoding="utf-8")

        for extra, expected in (
            ({"FAKE_FINGERPRINT": "00" * 20}, "fingerprint mismatch"),
            ({"FAKE_EXTRA_PRIMARY": "1"}, "exactly one primary key"),
            ({"FAKE_GPG_FAIL": "1"}, "could not inspect"),
            ({"FAKE_DOWNLOAD_FAIL": "1"}, "could not download"),
            ({"FAKE_ARCH": "arm64"}, "architecture: amd64 only"),
        ):
            isolated_script, isolated_apt, isolated_env = make_fixture(root / f"case-{len(list(root.iterdir()))}")
            isolated_env.update(extra)
            failure = run(isolated_script, isolated_env, ok=False)
            assert expected in failure.stderr, failure.stderr
            assert not (isolated_apt / "keyrings").exists()
            assert not (isolated_apt / "sources.list.d").exists()
            assert not Path(isolated_env["FAKE_APT_LOG"]).exists()

        prerequisite_script, prerequisite_apt, prerequisite_env = make_fixture(root / "missing-prerequisite")
        limited_bin = prerequisite_script.parent / "limited-bin"
        limited_bin.mkdir()
        for executable in ("sed", "head", "tr", "apt-get", "dpkg", "gpg", "cmp", "install", "ln", "mktemp", "date", "stat"):
            source_executable = shutil.which(executable, path=env["PATH"])
            if source_executable is None:
                raise AssertionError(f"test requires {executable}")
            (limited_bin / executable).symlink_to(source_executable)
        prerequisite_env["PATH"] = str(limited_bin)
        failure = run(prerequisite_script, prerequisite_env, ok=False)
        assert "missing prerequisite 'curl'" in failure.stderr
        assert not (prerequisite_apt / "keyrings").exists()
        assert not Path(prerequisite_env["FAKE_APT_LOG"]).exists()

        symlink_script, symlink_apt, symlink_env = make_fixture(root / "symlink")
        key_directory = symlink_apt / "keyrings"
        key_directory.mkdir()
        symlink_target = key_directory / "owner-key"
        symlink_target.write_bytes(b"owner key")
        symlink_key = key_directory / "orgbots-archive-keyring.gpg"
        symlink_key.symlink_to(symlink_target)
        failure = run(symlink_script, symlink_env, ok=False)
        assert "refusing to replace symlink" in failure.stderr
        assert symlink_key.is_symlink() and symlink_target.read_bytes() == b"owner key"
        assert not (symlink_apt / "sources.list.d").exists()
        assert not Path(symlink_env["FAKE_APT_LOG"]).exists()

        unsupported_script, unsupported_apt, unsupported_env = make_fixture(root / "unsupported-os")
        os_file = unsupported_script.parent / "os-release"
        os_file.write_text('ID=ubuntu\nVERSION_ID="22.04"\n', encoding="utf-8")
        failure = run(unsupported_script, unsupported_env, ok=False)
        assert "Ubuntu 24.04 amd64 only" in failure.stderr
        assert not (unsupported_apt / "keyrings").exists()

        print("installer fingerprint, download, platform, idempotence, backup, and APT prompt checks passed")


if __name__ == "__main__":
    main()
