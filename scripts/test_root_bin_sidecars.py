#!/usr/bin/env python3
"""Self-tests for the root binary sidecar packer."""

from __future__ import annotations

import contextlib
import os
import pathlib
import shutil
import subprocess
import tarfile
import tempfile
import time


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "root_bin_sidecars.py"


@contextlib.contextmanager
def temporary_directory():
    tmp = tempfile.mkdtemp()
    try:
        yield pathlib.Path(tmp)
    finally:
        last_error: OSError | None = None
        removed = False
        for attempt in range(5):
            try:
                shutil.rmtree(tmp)
                removed = True
                break
            except FileNotFoundError:
                removed = True
                break
            except OSError as exc:
                last_error = exc
                if attempt == 4:
                    break
                time.sleep(0.1)
        if not removed and last_error is not None:
            raise last_error


def run(args: list[str], *, cwd: pathlib.Path, check: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        args,
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if check and result.returncode != 0:
        raise AssertionError(
            f"command failed: {' '.join(args)}\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result


def write(path: pathlib.Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def make_executable(path: pathlib.Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("#!/usr/bin/env sh\nexit 0\n", encoding="utf-8")
    path.chmod(0o755)


def init_repo(root: pathlib.Path) -> pathlib.Path:
    repo = root / "repo"
    repo.mkdir()
    write(
        repo / "Cargo.toml",
        """
[package]
name = "bolt-v2"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "explicit_tool"
path = "tools/explicit_tool.rs"
""",
    )
    write(repo / "src" / "main.rs", "fn main() {}\n")
    write(repo / "src" / "bin" / "stream_to_lake.rs", "fn main() {}\n")
    write(repo / "src" / "bin" / "nested_tool" / "main.rs", "fn main() {}\n")
    write(repo / "tools" / "explicit_tool.rs", "fn main() {}\n")
    return repo


def assert_expected_bins_come_from_cargo_manifest_and_src_bin() -> None:
    with temporary_directory() as tmp:
        repo = init_repo(tmp)
        result = run(["python3", str(SCRIPT_PATH), "expected", "--repo-root", str(repo)], cwd=repo)
        actual = result.stdout.splitlines()
        expected = ["bolt-v2", "explicit_tool", "nested_tool", "stream_to_lake"]
        if actual != expected:
            raise AssertionError(f"expected {expected}, got {actual}")


def assert_pack_includes_only_expected_executable_sidecars() -> None:
    with temporary_directory() as tmp:
        repo = init_repo(tmp)
        target_dir = tmp / "target"
        output = tmp / "sidecars.tar.gz"
        for name in ("bolt-v2", "explicit_tool", "nested_tool", "stream_to_lake", "unrelated_test_bin"):
            make_executable(target_dir / "debug" / name)
        run(
            [
                "python3",
                str(SCRIPT_PATH),
                "pack",
                "--repo-root",
                str(repo),
                "--target-dir",
                str(target_dir),
                "--output",
                str(output),
            ],
            cwd=repo,
        )
        with tarfile.open(output, "r:gz") as archive:
            names = sorted(member.name for member in archive.getmembers() if member.isfile())
        expected = ["debug/bolt-v2", "debug/explicit_tool", "debug/nested_tool", "debug/stream_to_lake"]
        if names != expected:
            raise AssertionError(f"expected {expected}, got {names}")


def assert_pack_fails_when_expected_sidecar_is_missing() -> None:
    with temporary_directory() as tmp:
        repo = init_repo(tmp)
        target_dir = tmp / "target"
        output = tmp / "sidecars.tar.gz"
        for name in ("bolt-v2", "explicit_tool", "stream_to_lake"):
            make_executable(target_dir / "debug" / name)
        result = run(
            [
                "python3",
                str(SCRIPT_PATH),
                "pack",
                "--repo-root",
                str(repo),
                "--target-dir",
                str(target_dir),
                "--output",
                str(output),
            ],
            cwd=repo,
            check=False,
        )
        if result.returncode == 0:
            raise AssertionError("pack must fail when an expected sidecar is missing")
        if "missing root binary sidecars: debug/nested_tool" not in result.stderr:
            raise AssertionError(result.stderr)


def main() -> int:
    assert_expected_bins_come_from_cargo_manifest_and_src_bin()
    assert_pack_includes_only_expected_executable_sidecars()
    assert_pack_fails_when_expected_sidecar_is_missing()
    print("OK: root binary sidecar self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lock_handle = lane_governor.acquire()
    try:
        raise SystemExit(main())
    finally:
        lane_governor.release(lock_handle)
