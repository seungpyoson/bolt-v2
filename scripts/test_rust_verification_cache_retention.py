#!/usr/bin/env python3
"""Self-tests for managed Rust cache retention commands."""

from __future__ import annotations

import json
import importlib.util
import os
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import time


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "rust_verification.py"


def disk_bytes(path: pathlib.Path) -> int:
    result = subprocess.run(
        ["du", "-sk", str(path)],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise AssertionError(result.stderr)
    return int(result.stdout.split()[0]) * 1024


def run_owner(args: list[str], *, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def load_owner_module() -> object:
    spec = importlib.util.spec_from_file_location("rust_verification_under_test", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError("unable to load rust_verification.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def write_executable(path: pathlib.Path, body: str) -> None:
    path.write_text(body, encoding="utf-8")
    path.chmod(0o755)


def write_policy(repo: pathlib.Path) -> None:
    (repo / "ci").mkdir()
    (repo / "ci" / "rust-verification.toml").write_text(
        textwrap.dedent(
            """\
            schema_version = 1
            project_id = "bolt-v2"
            target_namespace = "bolt-v2"

            [commands]

            [commands.test]
            recipe = "managed-test"

            [commands.clippy]
            recipe = "managed-clippy"

            [commands.build]
            recipe = "managed-build"
            artifact_layout = "cargo"
            profile = "release"
            target = "aarch64-unknown-linux-gnu"
            """
        ),
        encoding="utf-8",
    )


def write_policy_with_cache(
    repo: pathlib.Path,
    active_process_patterns: list[str] | None = None,
    *,
    min_free_bytes: int = 10,
    soft_limit_bytes: int = 100,
) -> None:
    write_policy(repo)
    patterns = active_process_patterns or ["cargo", "rustc", "rust_verification.py"]
    with (repo / "ci" / "rust-verification.toml").open("a", encoding="utf-8") as handle:
        handle.write(
            textwrap.dedent(
                f"""\

                [cache]
                min_free_bytes = {min_free_bytes}
                soft_limit_bytes = {soft_limit_bytes}
                active_process_patterns = {json.dumps(patterns)}

                [cache.retention.debug]
                prune_after_days = 14
                prunable = true

                [cache.retention.release]
                prune_after_days = 30
                prunable = true

                [cache.retention.cross-target]
                prune_after_days = 30
                prunable = true

                [cache.retention.tmp]
                prune_after_days = 1
                prunable = true

                [cache.retention.other]
                prunable = false
                """
            )
        )


def assert_cache_status_reports_managed_target_tree() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        (target / "debug").mkdir(parents=True)
        (target / "release").mkdir()
        (target / "debug" / "one.bin").write_bytes(b"abc")
        (target / "release" / "two.bin").write_bytes(b"12345")

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)

        result = run_owner(["cache-status", "--repo", str(repo), "--json"], env=env)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))

        payload = json.loads(result.stdout)
        if payload["status"] != "ok":
            raise AssertionError(payload)
        if payload["policy"] != str(repo / "ci" / "rust-verification.toml"):
            raise AssertionError(payload)
        if payload["target_dir"] != str(target):
            raise AssertionError(payload)
        expected_debug_bytes = disk_bytes(target / "debug" / "one.bin")
        expected_release_bytes = disk_bytes(target / "release" / "two.bin")
        if payload["total_bytes"] != expected_debug_bytes + expected_release_bytes:
            raise AssertionError(payload)
        if payload["skipped_special_entries"] != 0:
            raise AssertionError(payload)
        if not isinstance(payload["filesystem"]["free_bytes"], int) or payload["filesystem"]["free_bytes"] <= 0:
            raise AssertionError(payload)

        subtrees = {entry["relative_path"]: entry for entry in payload["subtrees"]}
        expected = {
            "debug": {"bytes": expected_debug_bytes, "class": "debug"},
            "release": {"bytes": expected_release_bytes, "class": "release"},
        }
        for relative_path, expected_values in expected.items():
            entry = subtrees.get(relative_path)
            if entry is None:
                raise AssertionError(payload)
            for key, value in expected_values.items():
                if entry[key] != value:
                    raise AssertionError(payload)
            if not isinstance(entry["latest_mtime"], float):
                raise AssertionError(payload)


def assert_cache_commands_require_json_flag() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo)

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(tmp_path / "rust-root")

        status = run_owner(["cache-status", "--repo", str(repo)], env=env)
        if status.returncode == 0 or "--json" not in status.stderr:
            raise AssertionError((status.returncode, status.stdout, status.stderr))

        prune = run_owner(["cache-prune", "--repo", str(repo), "--dry-run"], env=env)
        if prune.returncode == 0 or "--json" not in prune.stderr:
            raise AssertionError((prune.returncode, prune.stdout, prune.stderr))


def assert_cache_policy_syntax_works_without_external_toml() -> None:
    system_python = pathlib.Path("/usr/bin/python3")
    if not system_python.exists():
        return
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo)

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(tmp_path / "rust-root")

        result = subprocess.run(
            [str(system_python), "-S", str(SCRIPT), "cache-status", "--repo", str(repo), "--json"],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload["status"] != "ok":
            raise AssertionError(payload)


def assert_cache_status_uses_allocated_disk_bytes_for_sparse_files() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        sparse_file = target / "debug" / "sparse.bin"
        sparse_file.parent.mkdir(parents=True)
        with sparse_file.open("wb") as handle:
            handle.truncate(10 * 1024 * 1024)
        if not hasattr(sparse_file.lstat(), "st_blocks"):
            return

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)

        result = run_owner(["cache-status", "--repo", str(repo), "--json"], env=env)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        subtrees = {entry["relative_path"]: entry for entry in payload["subtrees"]}
        if subtrees["debug"]["bytes"] != disk_bytes(sparse_file):
            raise AssertionError(payload)
        if subtrees["debug"]["bytes"] >= sparse_file.lstat().st_size:
            raise AssertionError("cache-status reported logical size, not allocated disk bytes")


def assert_cache_status_uses_single_scan_for_subtree_bytes() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        debug_file.write_bytes(b"abc")
        expected_debug_bytes = disk_bytes(debug_file)

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        write_executable(
            bin_dir / "du",
            """#!/usr/bin/env bash
exit 1
""",
        )

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(["cache-status", "--repo", str(repo), "--json"], env=env)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        subtrees = {entry["relative_path"]: entry for entry in payload["subtrees"]}
        if subtrees["debug"]["bytes"] != expected_debug_bytes:
            raise AssertionError(payload)
        if subtrees["debug"]["skipped_special_entries"] != 0:
            raise AssertionError(payload)


def assert_cache_status_counts_hardlinked_files_once() -> None:
    if not hasattr(os, "link"):
        return
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "artifact.bin"
        debug_file.parent.mkdir(parents=True)
        debug_file.write_bytes(b"x" * 4096)
        os.link(debug_file, target / "debug" / "artifact-hardlink.bin")
        expected_debug_bytes = disk_bytes(debug_file)

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)

        result = run_owner(["cache-status", "--repo", str(repo), "--json"], env=env)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        subtrees = {entry["relative_path"]: entry for entry in payload["subtrees"]}
        if subtrees["debug"]["bytes"] != expected_debug_bytes:
            raise AssertionError(payload)


def assert_scan_cache_tree_handles_deep_tree_iteratively() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp) / "root"
        leaf_parent = root
        for _index in range(120):
            leaf_parent = leaf_parent / "d"
        leaf_parent.mkdir(parents=True)
        leaf = leaf_parent / "x"
        leaf.write_bytes(b"x")

        previous_limit = sys.getrecursionlimit()
        try:
            sys.setrecursionlimit(100)
            total_bytes, latest_mtime, skipped = owner.scan_cache_tree(root)
        finally:
            sys.setrecursionlimit(previous_limit)
        if total_bytes != disk_bytes(leaf) or latest_mtime <= 0 or skipped != 0:
            raise AssertionError((total_bytes, latest_mtime, skipped))


def assert_cache_prune_dry_run_lists_stale_candidates_without_deleting() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo)

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        other_file = target / "keep-me" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        other_file.parent.mkdir()
        debug_file.write_bytes(b"abc")
        other_file.write_bytes(b"12345")
        old_time = time.time() - (15 * 24 * 60 * 60)
        os.utime(debug_file, (old_time, old_time))
        os.utime(debug_file.parent, (old_time, old_time))
        os.utime(other_file, (old_time, old_time))
        os.utime(other_file.parent, (old_time, old_time))
        expected_debug_bytes = disk_bytes(debug_file)

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)

        result = run_owner(["cache-prune", "--repo", str(repo), "--dry-run", "--json"], env=env)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload["dry_run"] is not True or payload["refused"] is not False:
            raise AssertionError(payload)
        if payload["reclaimable_bytes"] != expected_debug_bytes:
            raise AssertionError(payload)
        candidates = {entry["relative_path"]: entry for entry in payload["candidates"]}
        if set(candidates) != {"debug"}:
            raise AssertionError(payload)
        if candidates["debug"]["class"] != "debug" or candidates["debug"]["bytes"] != expected_debug_bytes:
            raise AssertionError(payload)
        if not debug_file.exists() or not other_file.exists():
            raise AssertionError("dry-run deleted files")


def assert_cache_prune_dry_run_lists_stale_cross_target_candidates() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo)

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        cross_file = target / "aarch64-unknown-linux-gnu" / "old.bin"
        cross_file.parent.mkdir(parents=True)
        cross_file.write_bytes(b"cross")
        old_time = time.time() - (31 * 24 * 60 * 60)
        os.utime(cross_file, (old_time, old_time))
        os.utime(cross_file.parent, (old_time, old_time))
        expected_cross_bytes = disk_bytes(cross_file)

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)

        result = run_owner(["cache-prune", "--repo", str(repo), "--dry-run", "--json"], env=env)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        candidates = {entry["relative_path"]: entry for entry in payload["candidates"]}
        if set(candidates) != {"aarch64-unknown-linux-gnu"}:
            raise AssertionError(payload)
        if candidates["aarch64-unknown-linux-gnu"]["class"] != "cross-target":
            raise AssertionError(payload)
        if payload["reclaimable_bytes"] != expected_cross_bytes:
            raise AssertionError(payload)


def assert_cache_prune_dry_run_preserves_stale_cache_below_thresholds() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, min_free_bytes=1, soft_limit_bytes=10**12)

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        debug_file.write_bytes(b"abc")
        old_time = time.time() - (15 * 24 * 60 * 60)
        os.utime(debug_file, (old_time, old_time))
        os.utime(debug_file.parent, (old_time, old_time))

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)

        result = run_owner(["cache-prune", "--repo", str(repo), "--dry-run", "--json"], env=env)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload["candidates"] or payload["reclaimable_bytes"] != 0:
            raise AssertionError(payload)
        if payload.get("pressure") is not False:
            raise AssertionError(payload)


def assert_cache_status_classifies_subtrees_and_skips_special_files() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        cross_file = target / "aarch64-unknown-linux-gnu" / "obj.bin"
        tmp_file = target / "tmp" / "scratch.bin"
        other_file = target / "keep-me" / "data.bin"
        debug_link = target / "debug" / "outside-link"
        outside = tmp_path / "outside.bin"
        for path in (cross_file, tmp_file, other_file, debug_link):
            path.parent.mkdir(parents=True, exist_ok=True)
        cross_file.write_bytes(b"1234567")
        tmp_file.write_bytes(b"xy")
        other_file.write_bytes(b"keep")
        outside.write_bytes(b"z" * 1024)
        debug_link.symlink_to(outside)
        if hasattr(os, "mkfifo"):
            os.mkfifo(target / "tmp" / "pipe")

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)

        result = run_owner(["cache-status", "--repo", str(repo), "--json"], env=env)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        subtrees = {entry["relative_path"]: entry for entry in payload["subtrees"]}
        expected_classes = {
            "aarch64-unknown-linux-gnu": "cross-target",
            "debug": "debug",
            "keep-me": "other",
            "tmp": "tmp",
        }
        for relative_path, class_name in expected_classes.items():
            if subtrees[relative_path]["class"] != class_name:
                raise AssertionError(payload)
        if subtrees["aarch64-unknown-linux-gnu"]["bytes"] != disk_bytes(cross_file):
            raise AssertionError(payload)
        if subtrees["debug"]["bytes"] == outside.stat().st_size:
            raise AssertionError("symlink target was followed")
        if hasattr(os, "mkfifo") and subtrees["tmp"]["skipped_special_entries"] != 1:
            raise AssertionError(payload)


def assert_cache_status_ignores_broken_top_level_symlink() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        target.mkdir(parents=True)
        broken = target / "broken-link"
        broken.symlink_to(tmp_path / "missing")

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)

        result = run_owner(["cache-status", "--repo", str(repo), "--json"], env=env)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        subtrees = {entry["relative_path"]: entry for entry in payload["subtrees"]}
        if subtrees["broken-link"]["bytes"] != 0:
            raise AssertionError(payload)


def assert_cache_status_skips_permission_denied_top_level_child() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        debug_file.write_bytes(b"stale")

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)

        target.chmod(0o400)
        try:
            result = run_owner(["cache-status", "--repo", str(repo), "--json"], env=env)
        finally:
            target.chmod(0o700)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        subtrees = {entry["relative_path"]: entry for entry in payload["subtrees"]}
        if subtrees["debug"]["skipped_special_entries"] != 1:
            raise AssertionError(payload)


def assert_cache_status_skips_unreadable_subtree_when_du_fails() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        debug_file.write_bytes(b"stale")

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)

        debug_file.parent.chmod(0)
        try:
            result = run_owner(["cache-status", "--repo", str(repo), "--json"], env=env)
        finally:
            debug_file.parent.chmod(0o700)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        subtrees = {entry["relative_path"]: entry for entry in payload["subtrees"]}
        if subtrees["debug"]["bytes"] != 0:
            raise AssertionError(payload)
        if subtrees["debug"]["skipped_special_entries"] != 1:
            raise AssertionError(payload)


def assert_cache_prune_apply_refuses_active_related_process() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, active_process_patterns=["cache-prune-sentinel"])

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        debug_file.write_bytes(b"abc")
        old_time = time.time() - (15 * 24 * 60 * 60)
        os.utime(debug_file, (old_time, old_time))
        os.utime(debug_file.parent, (old_time, old_time))

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        write_executable(
            bin_dir / "ps",
            f"""#!/usr/bin/env bash
printf '123 cache-prune-sentinel {repo}\\n'
""",
        )
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload["refused"] is not True or payload["refusal_code"] != "active_process":
            raise AssertionError(payload)
        if not debug_file.exists():
            raise AssertionError("refused apply deleted files")


def assert_cache_prune_apply_refuses_active_related_process_by_cwd() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, active_process_patterns=["cargo"])

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        debug_file.write_bytes(b"abc")
        old_time = time.time() - (15 * 24 * 60 * 60)
        os.utime(debug_file, (old_time, old_time))
        os.utime(debug_file.parent, (old_time, old_time))

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        write_executable(
            bin_dir / "ps",
            """#!/usr/bin/env bash
printf '123 cargo test\\n'
""",
        )
        proc_dir = tmp_path / "proc" / "123"
        proc_dir.mkdir(parents=True)
        (proc_dir / "cwd").symlink_to(repo)

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["RUST_VERIFICATION_PROCESS_CWD_BASE"] = str(tmp_path / "proc")
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload["refused"] is not True or payload["refusal_code"] != "active_process":
            raise AssertionError(payload)
        active = payload["active_processes"][0]
        if active.get("cwd") != str(repo.resolve()):
            raise AssertionError(payload)
        if not debug_file.exists():
            raise AssertionError("refused apply deleted files")


def assert_cache_prune_active_process_scan_uses_portable_ps_columns() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, active_process_patterns=["cargo"], min_free_bytes=10**15)

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        debug_file.write_bytes(b"abc")
        old_time = time.time() - (15 * 24 * 60 * 60)
        os.utime(debug_file, (old_time, old_time))
        os.utime(debug_file.parent, (old_time, old_time))

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        arg_file = tmp_path / "ps-args"
        write_executable(
            bin_dir / "ps",
            """#!/usr/bin/env bash
actual="$1|$2|$3|$4|$5"
printf '%s' "$actual" > "$ARG_FILE"
test "$actual" = '-ax|-o|pid=|-o|command='
""",
        )

        env = os.environ.copy()
        env["ARG_FILE"] = str(arg_file)
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr, arg_file.read_text()))
        if debug_file.parent.exists():
            raise AssertionError("portable ps scan did not allow candidate deletion")


def assert_cache_prune_apply_ignores_unrelated_process_by_lsof_cwd() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        unrelated = tmp_path / "unrelated"
        unrelated.mkdir()
        write_policy_with_cache(repo, active_process_patterns=["cargo"])

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        debug_file.write_bytes(b"abc")
        old_time = time.time() - (15 * 24 * 60 * 60)
        os.utime(debug_file, (old_time, old_time))
        os.utime(debug_file.parent, (old_time, old_time))

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        write_executable(
            bin_dir / "ps",
            """#!/usr/bin/env bash
printf '123 cargo test\\n'
""",
        )
        write_executable(
            bin_dir / "lsof",
            f"""#!/usr/bin/env bash
if [ "$1|$2|$3|$4|$5|$6" != '-a|-p|123|-d|cwd|-Fn' ]; then
  exit 1
fi
printf 'p123\\nn{unrelated}\\n'
""",
        )

        env = os.environ.copy()
        env["RUST_VERIFICATION_PROCESS_CWD_BASE"] = str(tmp_path / "missing-proc")
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload["dry_run"] is not False or payload["refused"] is not False:
            raise AssertionError(payload)
        removed = {entry["relative_path"] for entry in payload["removed"]}
        if removed != {"debug"}:
            raise AssertionError(payload)
        if debug_file.parent.exists():
            raise AssertionError("lsof-visible unrelated cargo process blocked candidate deletion")


def assert_cache_prune_apply_ignores_visible_unrelated_process_by_cwd() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        unrelated = tmp_path / "unrelated"
        unrelated.mkdir()
        write_policy_with_cache(repo, active_process_patterns=["cargo"])

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        debug_file.write_bytes(b"abc")
        old_time = time.time() - (15 * 24 * 60 * 60)
        os.utime(debug_file, (old_time, old_time))
        os.utime(debug_file.parent, (old_time, old_time))

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        write_executable(
            bin_dir / "ps",
            """#!/usr/bin/env bash
printf '123 cargo test\\n'
""",
        )
        proc_dir = tmp_path / "proc" / "123"
        proc_dir.mkdir(parents=True)
        (proc_dir / "cwd").symlink_to(unrelated)

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["RUST_VERIFICATION_PROCESS_CWD_BASE"] = str(tmp_path / "proc")
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload["dry_run"] is not False or payload["refused"] is not False:
            raise AssertionError(payload)
        removed = {entry["relative_path"] for entry in payload["removed"]}
        if removed != {"debug"}:
            raise AssertionError(payload)
        if debug_file.parent.exists():
            raise AssertionError("visible unrelated cargo process blocked candidate deletion")


def assert_cache_prune_ignores_pattern_mentions_in_arguments() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, active_process_patterns=["cargo"])

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        debug_file.write_bytes(b"abc")
        old_time = time.time() - (15 * 24 * 60 * 60)
        os.utime(debug_file, (old_time, old_time))
        os.utime(debug_file.parent, (old_time, old_time))

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        write_executable(
            bin_dir / "ps",
            f"""#!/usr/bin/env bash
printf '123 grep cargo {repo}/notes.txt\\n'
""",
        )

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["RUST_VERIFICATION_PROCESS_CWD_BASE"] = str(tmp_path / "missing-proc")
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        removed = {entry["relative_path"] for entry in payload["removed"]}
        if removed != {"debug"}:
            raise AssertionError(payload)
        if debug_file.parent.exists():
            raise AssertionError("argument-only cargo mention blocked candidate deletion")


def assert_cache_prune_refuses_wrapped_active_processes_by_cwd() -> None:
    commands = [
        "sudo cargo build",
        "nice cargo build",
        "env -i cargo build",
        "rustup run stable cargo build",
        "bash -c 'cargo test'",
        "python3 -W ignore scripts/rust_verification.py cargo --repo . -- test",
        # Combined POSIX short-flag clusters: -c is "next-arg-required" but
        # bash/sh/zsh also accept it bundled with other short flags.
        "bash -lc 'cargo test'",
        "bash -ic 'cargo test'",
        "sh -ec 'cargo test'",
        "zsh -fc 'cargo test'",
        # `nice --` end-of-options marker followed by the command.
        "nice -- cargo build",
        # Legitimate stack of supported wrappers that exhausts the depth cap.
        "sudo nice env -i bash -c 'rustup run stable cargo test'",
        # Review-regression cases: supported wrapper flags must still expose
        # the wrapped cargo process so apply-prune refuses an active cache.
        "sudo --user root cargo build",
        "sudo --group wheel cargo build",
        "sudo --chdir /tmp cargo build",
        "sudo VAR=val cargo build",
        "env -v cargo build",
        "env --debug cargo build",
        "env -uLD_PRELOAD cargo build",
        "env -iS 'cargo build'",
        "env -S 'VAR=val cargo build'",
        "nice -n10 cargo build",
        "nice -n 10 -- cargo build",
        "nice -10 -- cargo build",
        "rustup run stable -- cargo build",
        "rustup run --install stable cargo build",
    ]
    for command in commands:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            repo = tmp_path / "repo"
            repo.mkdir()
            write_policy_with_cache(repo, active_process_patterns=["cargo", "rust_verification.py"])

            root_base = tmp_path / "rust-root"
            target = root_base / "bolt-v2" / "target"
            debug_file = target / "debug" / "old.bin"
            debug_file.parent.mkdir(parents=True)
            debug_file.write_bytes(b"abc")
            old_time = time.time() - (15 * 24 * 60 * 60)
            os.utime(debug_file, (old_time, old_time))
            os.utime(debug_file.parent, (old_time, old_time))

            bin_dir = tmp_path / "bin"
            bin_dir.mkdir()
            # Embed the command with single-quote escaping ('\'' inside '...'),
            # so that commands carrying inner single quotes (e.g. shell -c
            # payloads) survive the fake ps shim intact instead of being
            # split by printf into discarded extra format arguments.
            escaped_command = command.replace("'", "'\\''")
            write_executable(
                bin_dir / "ps",
                f"""#!/usr/bin/env bash
printf '123 {escaped_command}\\n'
""",
            )
            proc_dir = tmp_path / "proc" / "123"
            proc_dir.mkdir(parents=True)
            (proc_dir / "cwd").symlink_to(repo)

            env = os.environ.copy()
            env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
            env["RUST_VERIFICATION_PROCESS_CWD_BASE"] = str(tmp_path / "proc")
            env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

            result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
            if result.returncode != 2:
                raise AssertionError((command, result.returncode, result.stdout, result.stderr))
            payload = json.loads(result.stdout)
            if payload["refused"] is not True or payload["refusal_code"] != "active_process":
                raise AssertionError((command, payload))
            if not debug_file.exists():
                raise AssertionError(f"wrapped active process {command!r} did not protect cache")


def assert_cache_prune_ignores_bash_login_without_command_by_cwd() -> None:
    # Negative case: a bare login/interactive shell without -c does NOT carry
    # a cargo payload and must not produce a false-positive refusal even when
    # its cwd points at the repo.
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, active_process_patterns=["cargo", "rust_verification.py"])

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        debug_file.write_bytes(b"abc")
        old_time = time.time() - (15 * 24 * 60 * 60)
        os.utime(debug_file, (old_time, old_time))
        os.utime(debug_file.parent, (old_time, old_time))

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        write_executable(
            bin_dir / "ps",
            """#!/usr/bin/env bash
printf '123 bash -l\\n'
""",
        )
        proc_dir = tmp_path / "proc" / "123"
        proc_dir.mkdir(parents=True)
        (proc_dir / "cwd").symlink_to(repo)

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["RUST_VERIFICATION_PROCESS_CWD_BASE"] = str(tmp_path / "proc")
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        removed = {entry["relative_path"] for entry in payload["removed"]}
        if removed != {"debug"}:
            raise AssertionError(payload)
        if debug_file.parent.exists():
            raise AssertionError("bare `bash -l` produced false-positive refusal")


def assert_cache_prune_apply_waits_for_managed_cargo_lock() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, active_process_patterns=["cargo"], min_free_bytes=10**15)

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_dir = target / "debug"
        debug_file = debug_dir / "old.bin"
        debug_dir.mkdir(parents=True)
        debug_file.write_bytes(b"abc")
        old_time = time.time() - (15 * 24 * 60 * 60)
        os.utime(debug_file, (old_time, old_time))
        os.utime(debug_dir, (old_time, old_time))

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        marker = tmp_path / "started"
        write_executable(
            bin_dir / "fake-runner",
            """#!/usr/bin/env bash
printf started > "$MARKER"
sleep 1
test -d "$DEBUG_PARENT"
""",
        )
        write_executable(
            bin_dir / "ps",
            """#!/usr/bin/env bash
exit 0
""",
        )

        env = os.environ.copy()
        env["DEBUG_PARENT"] = str(debug_dir)
        env["MARKER"] = str(marker)
        env["RUST_VERIFICATION_REAL_CARGO"] = str(bin_dir / "fake-runner")
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        cargo_proc = subprocess.Popen(
            [sys.executable, str(SCRIPT), "cargo", "--repo", str(repo), "--", "test"],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        stdout = ""
        stderr = ""
        try:
            deadline = time.time() + 5
            while not marker.exists() and time.time() < deadline:
                time.sleep(0.05)
            if not marker.exists():
                raise AssertionError("fake managed cargo did not start")

            result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
            stdout, stderr = cargo_proc.communicate(timeout=5)
        finally:
            if cargo_proc.poll() is None:
                cargo_proc.kill()
                cargo_proc.communicate(timeout=5)

        if cargo_proc.returncode != 0:
            raise AssertionError((cargo_proc.returncode, stdout, stderr))
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        removed = {entry["relative_path"] for entry in payload["removed"]}
        if removed != {"debug"} or debug_dir.exists():
            raise AssertionError(payload)


def assert_cache_prune_apply_waits_for_managed_run_lock() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, active_process_patterns=["cargo"], min_free_bytes=10**15)
        (repo / "justfile").write_text("", encoding="utf-8")

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_dir = target / "debug"
        debug_file = debug_dir / "old.bin"
        debug_dir.mkdir(parents=True)
        debug_file.write_bytes(b"abc")
        old_time = time.time() - (15 * 24 * 60 * 60)
        os.utime(debug_file, (old_time, old_time))
        os.utime(debug_dir, (old_time, old_time))

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        marker = tmp_path / "started"
        write_executable(
            bin_dir / "just",
            """#!/usr/bin/env bash
printf started > "$MARKER"
sleep 1
test -d "$DEBUG_PARENT"
""",
        )
        write_executable(
            bin_dir / "ps",
            """#!/usr/bin/env bash
exit 0
""",
        )

        env = os.environ.copy()
        env["DEBUG_PARENT"] = str(debug_dir)
        env["MARKER"] = str(marker)
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        run_proc = subprocess.Popen(
            [sys.executable, str(SCRIPT), "run", "--repo", str(repo), "test"],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        stdout = ""
        stderr = ""
        try:
            deadline = time.time() + 5
            while not marker.exists() and time.time() < deadline:
                time.sleep(0.05)
            if not marker.exists():
                raise AssertionError("fake managed run did not start")

            result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
            stdout, stderr = run_proc.communicate(timeout=5)
        finally:
            if run_proc.poll() is None:
                run_proc.kill()
                run_proc.communicate(timeout=5)

        if run_proc.returncode != 0:
            raise AssertionError((run_proc.returncode, stdout, stderr))
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        removed = {entry["relative_path"] for entry in payload["removed"]}
        if removed != {"debug"} or debug_dir.exists():
            raise AssertionError(payload)


def assert_cache_prune_apply_checks_active_process_before_scan() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, active_process_patterns=["cache-prune-sentinel"], min_free_bytes=10**15)

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        debug_file.write_bytes(b"abc")

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        marker = tmp_path / "scanned"
        write_executable(
            bin_dir / "du",
            """#!/usr/bin/env bash
printf scanned > "$SCAN_MARKER"
printf '1 %s\\n' "$2"
""",
        )
        write_executable(
            bin_dir / "ps",
            """#!/usr/bin/env bash
printf '123 cache-prune-sentinel %s\\n' "$REPO_PATH"
""",
        )

        env = os.environ.copy()
        env["REPO_PATH"] = str(repo)
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["SCAN_MARKER"] = str(marker)
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload["refusal_code"] != "active_process":
            raise AssertionError(payload)
        if marker.exists():
            raise AssertionError("cache-prune scanned target before active-process check")


def assert_cache_prune_apply_rechecks_active_process_before_delete() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, active_process_patterns=["cache-prune-sentinel"], min_free_bytes=10**15)

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        debug_file.write_bytes(b"abc")
        old_time = time.time() - (15 * 24 * 60 * 60)
        os.utime(debug_file, (old_time, old_time))
        os.utime(debug_file.parent, (old_time, old_time))

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        count_file = tmp_path / "ps-count"
        write_executable(
            bin_dir / "ps",
            """#!/usr/bin/env bash
if [ ! -f "$COUNT_FILE" ]; then
  printf 1 > "$COUNT_FILE"
  exit 0
fi
printf '123 cache-prune-sentinel %s\\n' "$REPO_PATH"
""",
        )

        env = os.environ.copy()
        env["COUNT_FILE"] = str(count_file)
        env["REPO_PATH"] = str(repo)
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload["refusal_code"] != "active_process":
            raise AssertionError(payload)
        if not debug_file.exists():
            raise AssertionError("cache-prune deleted after active process appeared")


def assert_cache_prune_apply_fails_closed_when_process_visibility_missing() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, active_process_patterns=["cargo"])

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        debug_file.write_bytes(b"abc")
        old_time = time.time() - (15 * 24 * 60 * 60)
        os.utime(debug_file, (old_time, old_time))
        os.utime(debug_file.parent, (old_time, old_time))

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        write_executable(
            bin_dir / "ps",
            """#!/usr/bin/env bash
exit 1
""",
        )

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload["refused"] is not True or payload["refusal_code"] != "insufficient_process_visibility":
            raise AssertionError(payload)
        if not debug_file.exists():
            raise AssertionError("failed-closed apply deleted files")


def assert_cache_prune_apply_fails_closed_when_matching_process_scope_unknown() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, active_process_patterns=["cargo"])

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        debug_file.write_bytes(b"abc")
        old_time = time.time() - (15 * 24 * 60 * 60)
        os.utime(debug_file, (old_time, old_time))
        os.utime(debug_file.parent, (old_time, old_time))

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        write_executable(
            bin_dir / "ps",
            """#!/usr/bin/env bash
printf '123 cargo test\\n'
""",
        )

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload["refused"] is not True or payload["refusal_code"] != "insufficient_process_visibility":
            raise AssertionError(payload)
        if not debug_file.exists():
            raise AssertionError("failed-closed apply deleted files")


def assert_cache_prune_apply_fails_closed_when_policy_missing() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(pathlib.Path(tmp) / "rust-root")
        result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload["refused"] is not True or payload["refusal_code"] != "missing_policy":
            raise AssertionError(payload)


def assert_cache_prune_apply_fails_closed_when_cache_policy_malformed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        with (repo / "ci" / "rust-verification.toml").open("a", encoding="utf-8") as handle:
            handle.write(
                textwrap.dedent(
                    """\

                    [cache]
                    active_process_patterns = "cargo"

                    [cache.retention.debug]
                    prune_after_days = 14
                    prunable = true
                    """
                )
            )
        result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=os.environ.copy())
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload["refused"] is not True or payload["refusal_code"] != "invalid_policy":
            raise AssertionError(payload)


def assert_validate_policy_rejects_malformed_cache_policy() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        with (repo / "ci" / "rust-verification.toml").open("a", encoding="utf-8") as handle:
            handle.write(
                textwrap.dedent(
                    """\

                    [cache]
                    min_free_bytes = "10"
                    soft_limit_bytes = 100
                    active_process_patterns = ["cargo"]

                    [cache.retention.debug]
                    prune_after_days = 14
                    prunable = true
                    """
                )
            )

        result = run_owner(["validate-policy", "--repo", str(repo)], env=os.environ.copy())
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if "cache.min_free_bytes" not in result.stderr:
            raise AssertionError((result.returncode, result.stdout, result.stderr))


def assert_validate_policy_rejects_boolean_cache_numbers() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        with (repo / "ci" / "rust-verification.toml").open("a", encoding="utf-8") as handle:
            handle.write(
                textwrap.dedent(
                    """\

                    [cache]
                    min_free_bytes = true
                    soft_limit_bytes = 100
                    active_process_patterns = ["cargo"]

                    [cache.retention.debug]
                    prune_after_days = 14
                    prunable = true

                    [cache.retention.release]
                    prune_after_days = 30
                    prunable = true

                    [cache.retention.cross-target]
                    prune_after_days = 30
                    prunable = true

                    [cache.retention.tmp]
                    prune_after_days = 1
                    prunable = true

                    [cache.retention.other]
                    prunable = false
                    """
                )
            )
        result = run_owner(["validate-policy", "--repo", str(repo)], env=os.environ.copy())
        if result.returncode != 2 or "cache.min_free_bytes" not in result.stderr:
            raise AssertionError((result.returncode, result.stdout, result.stderr))

    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        with (repo / "ci" / "rust-verification.toml").open("a", encoding="utf-8") as handle:
            handle.write(
                textwrap.dedent(
                    """\

                    [cache]
                    min_free_bytes = 10
                    soft_limit_bytes = 100
                    active_process_patterns = ["cargo"]

                    [cache.retention.debug]
                    prune_after_days = true
                    prunable = true

                    [cache.retention.release]
                    prune_after_days = 30
                    prunable = true

                    [cache.retention.cross-target]
                    prune_after_days = 30
                    prunable = true

                    [cache.retention.tmp]
                    prune_after_days = 1
                    prunable = true

                    [cache.retention.other]
                    prunable = false
                    """
                )
            )
        result = run_owner(["validate-policy", "--repo", str(repo)], env=os.environ.copy())
        if result.returncode != 2 or "cache.retention.debug.prune_after_days" not in result.stderr:
            raise AssertionError((result.returncode, result.stdout, result.stderr))


def assert_cache_prune_apply_removes_only_candidates() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, active_process_patterns=["cargo"])

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        other_file = target / "keep-me" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        other_file.parent.mkdir()
        debug_file.write_bytes(b"abc")
        other_file.write_bytes(b"12345")
        old_time = time.time() - (15 * 24 * 60 * 60)
        os.utime(debug_file, (old_time, old_time))
        os.utime(debug_file.parent, (old_time, old_time))
        os.utime(other_file, (old_time, old_time))
        os.utime(other_file.parent, (old_time, old_time))
        expected_debug_bytes = disk_bytes(debug_file)

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        write_executable(
            bin_dir / "ps",
            """#!/usr/bin/env bash
exit 0
""",
        )

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload["dry_run"] is not False or payload["refused"] is not False:
            raise AssertionError(payload)
        if payload["reclaimable_bytes"] != expected_debug_bytes:
            raise AssertionError(payload)
        removed = {entry["relative_path"]: entry for entry in payload["removed"]}
        if set(removed) != {"debug"}:
            raise AssertionError(payload)
        if debug_file.parent.exists():
            raise AssertionError("candidate subtree still exists")
        if not other_file.exists() or not target.exists():
            raise AssertionError("apply removed non-candidate or target root")

        second = run_owner(["cache-prune", "--repo", str(repo), "--dry-run", "--json"], env=env)
        if second.returncode != 0:
            raise AssertionError((second.returncode, second.stdout, second.stderr))
        second_payload = json.loads(second.stdout)
        if second_payload["candidates"] or second_payload["reclaimable_bytes"] != 0:
            raise AssertionError(second_payload)


def assert_cache_prune_apply_preserves_subtree_when_scan_incomplete() -> None:
    if not hasattr(os, "mkfifo"):
        return
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, active_process_patterns=["cargo"], min_free_bytes=10**15)

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        debug_file = target / "debug" / "old.bin"
        debug_file.parent.mkdir(parents=True)
        debug_file.write_bytes(b"abc")
        os.mkfifo(debug_file.parent / "pipe")
        old_time = time.time() - (15 * 24 * 60 * 60)
        os.utime(debug_file, (old_time, old_time))
        os.utime(debug_file.parent, (old_time, old_time))

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        write_executable(
            bin_dir / "ps",
            """#!/usr/bin/env bash
exit 0
""",
        )

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload["removed"] or payload["candidates"]:
            raise AssertionError(payload)
        if not debug_file.exists():
            raise AssertionError("incomplete scan allowed subtree deletion")


def assert_cache_prune_rejects_conflicting_modes() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo)
        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(tmp_path / "rust-root")

        result = run_owner(["cache-prune", "--repo", str(repo), "--dry-run", "--apply", "--json"], env=env)
        if result.returncode == 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))


def assert_repo_policy_declares_cache_retention() -> None:
    text = (REPO_ROOT / "ci" / "rust-verification.toml").read_text(encoding="utf-8")
    required = (
        "[cache]",
        "min_free_bytes",
        "soft_limit_bytes",
        "active_process_patterns",
        "[cache.retention.debug]",
        "[cache.retention.release]",
        "[cache.retention.cross-target]",
        "[cache.retention.tmp]",
        "[cache.retention.other]",
        "prunable = false",
    )
    missing = [item for item in required if item not in text]
    if missing:
        raise AssertionError(f"missing cache policy fields: {missing}")


def main() -> int:
    assert_cache_status_reports_managed_target_tree()
    assert_cache_commands_require_json_flag()
    assert_cache_policy_syntax_works_without_external_toml()
    assert_cache_status_uses_allocated_disk_bytes_for_sparse_files()
    assert_cache_status_uses_single_scan_for_subtree_bytes()
    assert_cache_status_counts_hardlinked_files_once()
    assert_scan_cache_tree_handles_deep_tree_iteratively()
    assert_cache_status_classifies_subtrees_and_skips_special_files()
    assert_cache_status_ignores_broken_top_level_symlink()
    assert_cache_status_skips_permission_denied_top_level_child()
    assert_cache_status_skips_unreadable_subtree_when_du_fails()
    assert_cache_prune_dry_run_lists_stale_candidates_without_deleting()
    assert_cache_prune_dry_run_lists_stale_cross_target_candidates()
    assert_cache_prune_dry_run_preserves_stale_cache_below_thresholds()
    assert_cache_prune_apply_refuses_active_related_process()
    assert_cache_prune_apply_refuses_active_related_process_by_cwd()
    assert_cache_prune_active_process_scan_uses_portable_ps_columns()
    assert_cache_prune_apply_ignores_unrelated_process_by_lsof_cwd()
    assert_cache_prune_apply_ignores_visible_unrelated_process_by_cwd()
    assert_cache_prune_ignores_pattern_mentions_in_arguments()
    assert_cache_prune_refuses_wrapped_active_processes_by_cwd()
    assert_cache_prune_ignores_bash_login_without_command_by_cwd()
    assert_cache_prune_apply_waits_for_managed_cargo_lock()
    assert_cache_prune_apply_waits_for_managed_run_lock()
    assert_cache_prune_apply_checks_active_process_before_scan()
    assert_cache_prune_apply_rechecks_active_process_before_delete()
    assert_cache_prune_apply_fails_closed_when_process_visibility_missing()
    assert_cache_prune_apply_fails_closed_when_matching_process_scope_unknown()
    assert_cache_prune_apply_fails_closed_when_policy_missing()
    assert_cache_prune_apply_fails_closed_when_cache_policy_malformed()
    assert_validate_policy_rejects_malformed_cache_policy()
    assert_validate_policy_rejects_boolean_cache_numbers()
    assert_cache_prune_apply_removes_only_candidates()
    assert_cache_prune_apply_preserves_subtree_when_scan_incomplete()
    assert_cache_prune_rejects_conflicting_modes()
    assert_repo_policy_declares_cache_retention()
    print("OK: Rust verification cache retention self-tests passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
