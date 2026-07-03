#!/usr/bin/env python3
"""Self-tests for managed Rust cache retention commands."""

from __future__ import annotations

import contextlib
import io
import json
import importlib.util
import os
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import time
import types


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
    env = env.copy()
    env.setdefault("BOLT_ALLOW_LOCAL_RUST", "1")
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


def write_policy(repo: pathlib.Path, *, target_namespace: str = "bolt-v2") -> None:
    (repo / "ci").mkdir()
    (repo / "ci" / "rust-verification.toml").write_text(
        textwrap.dedent(
            f"""\
            schema_version = 2
            project_id = "bolt-v2"
            target_namespace = "{target_namespace}"

            [local_compile_policy]
            enabled = true
            allowed_ci_env = "GITHUB_ACTIONS"
            break_glass_env = "BOLT_ALLOW_LOCAL_RUST"
            refused_managed_commands = ["test", "clippy", "build"]
            refused_cargo_subcommands = ["b", "bench", "build", "c", "check", "clippy", "d", "doc", "fetch", "install", "nextest", "r", "run", "rustc", "t", "test", "zigbuild"]

            [local_lane_policy]
            enabled = true
            allowed_ci_env = "GITHUB_ACTIONS"
            lock_dir = "/tmp/rust-verification-lanes"
            acquire_timeout_seconds = 1800
            heartbeat_seconds = 15
            poll_interval_seconds = 1

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
    target_namespace: str = "bolt-v2",
) -> None:
    write_policy(repo, target_namespace=target_namespace)
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


def assert_cache_prune_age_only_apply_prunes_stale_candidates_without_pressure() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, min_free_bytes=1, soft_limit_bytes=10**12)

        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        old_debug_file = target / "debug" / "old.bin"
        recent_release_file = target / "release" / "recent.bin"
        old_debug_file.parent.mkdir(parents=True)
        recent_release_file.parent.mkdir()
        old_debug_file.write_bytes(b"old")
        recent_release_file.write_bytes(b"recent")
        old_time = time.time() - (15 * 24 * 60 * 60)
        os.utime(old_debug_file, (old_time, old_time))
        os.utime(old_debug_file.parent, (old_time, old_time))

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

        dry_run = run_owner(["cache-prune", "--repo", str(repo), "--age-only", "--dry-run", "--json"], env=env)
        if dry_run.returncode != 0:
            raise AssertionError((dry_run.returncode, dry_run.stdout, dry_run.stderr))
        dry_payload = json.loads(dry_run.stdout)
        if dry_payload["dry_run"] is not True or dry_payload.get("age_only") is not True:
            raise AssertionError(dry_payload)
        if dry_payload.get("pressure") is not False:
            raise AssertionError(dry_payload)
        dry_candidates = {entry["relative_path"] for entry in dry_payload["candidates"]}
        if dry_candidates != {"debug"}:
            raise AssertionError(dry_payload)
        if not old_debug_file.exists() or not recent_release_file.exists():
            raise AssertionError("age-only dry-run deleted files")

        apply = run_owner(["cache-prune", "--repo", str(repo), "--age-only", "--apply", "--json"], env=env)
        if apply.returncode != 0:
            raise AssertionError((apply.returncode, apply.stdout, apply.stderr))
        payload = json.loads(apply.stdout)
        if payload["dry_run"] is not False or payload.get("age_only") is not True:
            raise AssertionError(payload)
        removed = {entry["relative_path"] for entry in payload["removed"]}
        if removed != {"debug"}:
            raise AssertionError(payload)
        if old_debug_file.parent.exists():
            raise AssertionError("age-only apply kept stale debug subtree")
        if not recent_release_file.exists():
            raise AssertionError("age-only apply removed recent release subtree")


def assert_cache_prune_age_only_apply_refuses_active_related_process() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, active_process_patterns=["cargo"], min_free_bytes=1, soft_limit_bytes=10**12)

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
printf '123 cargo build\\n'
""",
        )
        proc_dir = tmp_path / "proc" / "123"
        proc_dir.mkdir(parents=True)
        (proc_dir / "cwd").symlink_to(target)

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["RUST_VERIFICATION_PROCESS_CWD_BASE"] = str(tmp_path / "proc")
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(["cache-prune", "--repo", str(repo), "--age-only", "--apply", "--json"], env=env)
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload["refused"] is not True or payload["refusal_code"] != "active_process":
            raise AssertionError(payload)
        if payload.get("age_only") is not True:
            raise AssertionError(payload)
        if not debug_file.exists():
            raise AssertionError("refused age-only apply deleted files")


def assert_cache_prune_age_only_error_refusals_report_age_only() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        result = run_owner(
            ["cache-prune", "--repo", str(repo), "--age-only", "--json"],
            env=os.environ.copy(),
        )
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload["refused"] is not True or payload["refusal_code"] != "missing_policy":
            raise AssertionError(payload)
        if payload.get("age_only") is not True:
            raise AssertionError(payload)


def assert_cache_prune_multiple_repos_attempts_all_namespaces_after_refusal() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        root_repo = tmp_path / "repo"
        bte_repo = tmp_path / "bte"
        root_repo.mkdir()
        bte_repo.mkdir()
        write_policy_with_cache(
            root_repo,
            active_process_patterns=["cargo"],
            min_free_bytes=1,
            soft_limit_bytes=10**12,
            target_namespace="bolt-v2",
        )
        write_policy_with_cache(
            bte_repo,
            active_process_patterns=["cargo"],
            min_free_bytes=1,
            soft_limit_bytes=10**12,
            target_namespace="backtesting-vertical-slice",
        )

        root_base = tmp_path / "rust-root"
        root_target = root_base / "bolt-v2" / "target"
        bte_target = root_base / "backtesting-vertical-slice" / "target"
        root_file = root_target / "debug" / "old.bin"
        bte_file = bte_target / "debug" / "old.bin"
        for path in (root_file, bte_file):
            path.parent.mkdir(parents=True)
            path.write_bytes(b"abc")
            old_time = time.time() - (15 * 24 * 60 * 60)
            os.utime(path, (old_time, old_time))
            os.utime(path.parent, (old_time, old_time))

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        write_executable(
            bin_dir / "ps",
            """#!/usr/bin/env bash
printf '123 cargo build\\n'
""",
        )
        proc_dir = tmp_path / "proc" / "123"
        proc_dir.mkdir(parents=True)
        (proc_dir / "cwd").symlink_to(root_target)

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["RUST_VERIFICATION_PROCESS_CWD_BASE"] = str(tmp_path / "proc")
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(
            [
                "cache-prune",
                "--repo",
                str(root_repo),
                "--repo",
                str(bte_repo),
                "--age-only",
                "--apply",
                "--json",
            ],
            env=env,
        )
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        payload = json.loads(result.stdout)
        if payload.get("age_only") is not True or payload.get("dry_run") is not False:
            raise AssertionError(payload)
        results = payload.get("results", [])
        if len(results) != 2:
            raise AssertionError(payload)
        root_result, bte_result = results
        if root_result["exit_code"] != 2 or root_result["payload"].get("refusal_code") != "active_process":
            raise AssertionError(payload)
        if bte_result["exit_code"] != 0:
            raise AssertionError(payload)
        removed = {entry["relative_path"] for entry in bte_result["payload"]["removed"]}
        if removed != {"debug"}:
            raise AssertionError(payload)
        if not root_file.exists():
            raise AssertionError("root refusal still deleted root cache")
        if bte_file.exists():
            raise AssertionError("multi-repo prune did not continue to BTE cache")


def assert_cache_prune_multiple_repos_attempts_all_after_unexpected_exception() -> None:
    module = load_owner_module()
    original = module.cache_prune_payload

    def fake_cache_prune_payload(repo: pathlib.Path, *, dry_run: bool, age_only: bool) -> dict[str, object]:
        if repo.name == "bad":
            raise RuntimeError("synthetic cache failure")
        return {
            "age_only": age_only,
            "candidates": [],
            "dry_run": dry_run,
            "pressure": False,
            "pressure_reasons": [],
            "reclaimable_bytes": 0,
            "refused": False,
            "removed": [],
            "target_dir": str(repo / "target"),
        }

    module.cache_prune_payload = fake_cache_prune_payload
    try:
        args = types.SimpleNamespace(
            age_only=True,
            apply=False,
            json=True,
            repo=["/tmp/bad", "/tmp/good"],
        )
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            exit_code = module.cmd_cache_prune(args)
    finally:
        module.cache_prune_payload = original

    if exit_code != 2:
        raise AssertionError((exit_code, stdout.getvalue()))
    payload = json.loads(stdout.getvalue())
    results = payload.get("results", [])
    if len(results) != 2:
        raise AssertionError(payload)
    first, second = results
    if first["exit_code"] != 2 or first["payload"].get("refusal_code") != "operation_failed":
        raise AssertionError(payload)
    if "RuntimeError: synthetic cache failure" not in first["payload"].get("refusal_reason", ""):
        raise AssertionError(payload)
    if second["exit_code"] != 0 or second["payload"].get("refused") is not False:
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
actual="$1|$2|$3|$4|$5|$6|$7|$8"
printf '%s' "$actual" > "$ARG_FILE"
test "$actual" = '-ww|-ax|-o|pid=|-o|ppid=|-o|command='
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
    failures: list[AssertionError] = []
    for _ in range(3):
        try:
            assert_cache_prune_apply_ignores_unrelated_process_by_lsof_cwd_once()
            return
        except AssertionError as exc:
            failures.append(exc)
            if "insufficient_process_visibility" not in str(exc):
                raise
            time.sleep(0.05)
    raise failures[-1]


def assert_cache_prune_apply_ignores_unrelated_process_by_lsof_cwd_once() -> None:
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
if [ "$3" != '123' ] && [ -x /usr/sbin/lsof ]; then
  exec /usr/sbin/lsof "$@"
fi
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


def assert_cache_prune_skips_unrelated_process_before_cwd_lookup() -> None:
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
        lsof_marker = tmp_path / "lsof-called"
        write_executable(
            bin_dir / "ps",
            """#!/usr/bin/env bash
printf '123 unrelated-daemon --idle\\n'
""",
        )
        write_executable(
            bin_dir / "lsof",
            f"""#!/usr/bin/env bash
touch {lsof_marker}
exit 1
""",
        )

        env = os.environ.copy()
        env["RUST_VERIFICATION_PROCESS_CWD_BASE"] = str(tmp_path / "missing-proc")
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if lsof_marker.exists():
            raise AssertionError("unrelated non-Rust process triggered cwd/lsof lookup")
        if debug_file.parent.exists():
            raise AssertionError("unrelated process prevented stale cache deletion")


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
        # Standard shell redirections can appear before or inside a simple
        # command without changing the executable that owns the active target.
        "> /dev/null cargo build",
        "< /dev/null cargo build",
        "cargo>out build",
        "< /dev/null no-mistakes run -- cargo build",
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
        write_policy_with_cache(repo, active_process_patterns=["cargo"], min_free_bytes=1, soft_limit_bytes=10**12)

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
            bin_dir / "cargo",
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
        env["BOLT_ALLOW_LOCAL_RUST"] = "1"
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

            policy_file = repo / "ci" / "rust-verification.toml"
            policy_file.write_text(policy_file.read_text().replace("soft_limit_bytes = 1000000000000", "soft_limit_bytes = 1"))
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
        write_policy_with_cache(repo, active_process_patterns=["cargo"], min_free_bytes=1, soft_limit_bytes=10**12)
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
        env["BOLT_ALLOW_LOCAL_RUST"] = "1"
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

            policy_file = repo / "ci" / "rust-verification.toml"
            policy_file.write_text(policy_file.read_text().replace("soft_limit_bytes = 1000000000000", "soft_limit_bytes = 1"))
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


def assert_all_managed_cache_policies_are_bounded_to_30_gib() -> None:
    expected = "soft_limit_bytes = 32212254720"
    policy_paths = (
        REPO_ROOT / "ci" / "rust-verification.toml",
        REPO_ROOT / "crates" / "backtesting-vertical-slice" / "ci" / "rust-verification.toml",
    )
    for path in policy_paths:
        text = path.read_text(encoding="utf-8")
        if expected not in text:
            raise AssertionError(f"{path.relative_to(REPO_ROOT)} does not declare {expected}")


def assert_cache_prune_recipe_sweeps_all_managed_cache_namespaces() -> None:
    source = (REPO_ROOT / "justfile").read_text(encoding="utf-8")
    recipe_start = source.index("cache-prune *args:")
    recipe_end = source.index("# clean-merged: print", recipe_start)
    recipe = source[recipe_start:recipe_end]
    required = (
        '--repo "{{repo_root}}"',
        '--repo "{{repo_root}}/crates/backtesting-vertical-slice"',
        "--age-only",
    )
    missing = [item for item in required if item not in recipe]
    if missing:
        raise AssertionError(f"cache-prune recipe missing {missing}: {recipe}")
    command_lines = [line for line in recipe.splitlines() if "cache-prune" in line and "--repo" in line]
    if len(command_lines) != 1:
        raise AssertionError(f"cache-prune recipe must sweep all namespaces in one command: {recipe}")


def cache_prune_for_visible_command(command: str, *, expose_cwd: bool = True) -> tuple[subprocess.CompletedProcess[str], bool]:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, active_process_patterns=["cargo", "rustc", "rust_verification.py"])

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
        command = command.replace("{repo}", str(repo)).replace("{target}", str(target))
        escaped_command = command.replace("'", "'\\''")
        write_executable(
            bin_dir / "ps",
            f"""#!/usr/bin/env bash
printf '123 {escaped_command}\\n'
""",
        )
        if expose_cwd:
            proc_dir = tmp_path / "proc" / "123"
            proc_dir.mkdir(parents=True)
            (proc_dir / "cwd").symlink_to(repo)

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["RUST_VERIFICATION_PROCESS_CWD_BASE"] = str(tmp_path / "proc")
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"

        result = run_owner(["cache-prune", "--repo", str(repo), "--apply", "--json"], env=env)
        return result, debug_file.exists()


def assert_v6_red_active_process_parser_gaps() -> None:
    commands = [
        "env -iuLD_PRELOAD cargo build",
        "env -iu LD_PRELOAD cargo build",
        "rustup run stable -- -- cargo build",
        "stdbuf -oL cargo build",
        "catchsegv cargo test",
        "command cargo build",
        "exec cargo build",
        "nohup cargo build",
        "time cargo build",
        "timeout 30 cargo build",
        "flock /tmp/bolt.lock cargo build",
        "flock -o /tmp/bolt.lock cargo build",
        "flock -c 'cargo build' /tmp/bolt.lock",
        "xargs cargo build",
        "setsid cargo build",
        "taskset -c 0 cargo build",
        "ionice -c2 cargo build",
        "chrt -r 10 cargo build",
        "make build",
        "python -c 'import os; os.system(\"cargo build\")'",
        "bash -c 'alias c=cargo; c build'",
        "bash -c 'cargo() { command cargo \"$@\"; }; cargo build'",
        "bash -c 'builtin command cargo build'",
        "bash -c 'VAR=val cargo build'",
        "bash -c 'eval cargo build'",
        "bash -c 'x=$(cargo build)'",
        "bash -c 'x=\"$(cargo build)\"'",
        "bash -c 'x=`cargo build`'",
        "> /dev/null cargo build",
        "< /dev/null cargo build",
        "cargo>out build",
        "< /dev/null no-mistakes run -- cargo build",
        "find . -name Cargo.toml -exec cargo build \\;",
        "su user -c 'cargo build'",
        "runuser -u user -- cargo build",
        "sg staff -c 'cargo build'",
        "sudo -EHu root cargo build",
        "/tmp/c clean",
        "/tmp/c test",
        "/tmp/c build",
        "/tmp/c clean --manifest-path {repo}/Cargo.toml",
        "/tmp/c test --manifest-path {repo}/Cargo.toml",
        "/tmp/c run --manifest-path {repo}/Cargo.toml",
        "mycargo build --manifest-path {repo}/Cargo.toml",
        "/tmp/rust-test --manifest-path {repo}/Cargo.toml",
        "/tmp/r --crate-name bolt_v2 --out-dir {target}/debug/deps --emit=dep-info,link",
        "/tmp/myrustc --out-dir {target}/debug/deps",
        "myrustc --out-dir {target}/debug/deps",
        "bash -c '/tmp/myrustc --out-dir {target}/debug/deps'",
        "eval /tmp/myrustc --out-dir {target}/debug/deps",
        "/tmp/rust-build build",
        "/tmp/repo/scripts/cargo-build-script",
        "docker exec bolt-dev cargo build /tmp/repo",
        "docker run -v /tmp/repo:/repo rust cargo build",
        "docker run --label my-label rust /tmp/c build",
        "docker run --unknown-opt=rust mycargo build",
        "podman run --unknown-opt=rust myrustc --out-dir {target}/debug/deps",
        "env >output.log /tmp/c build",
        "runuser -u user /tmp/c build",
        "npm run cargo-build",
        "python scripts/build.py",
        "sudo sudo sudo sudo sudo sudo sudo cargo test",
        "bash -c 'bash -c \"bash -c \\'bash -c \\\\\\'bash -c \\\\\\\\\\\\\\'bash -c \\\\\\\\\\\\\\\\\\\\\\\\\\\\\\'cargo build\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\'\\\\\\\\\\\\\\'\\'\"'",
    ]
    misses: list[str] = []
    for command in commands:
        result, debug_file_exists = cache_prune_for_visible_command(command)
        refused = False
        refusal_code = ""
        if result.stdout:
            try:
                payload = json.loads(result.stdout)
                refused = payload.get("refused") is True
                refusal_code = str(payload.get("refusal_code", ""))
            except json.JSONDecodeError:
                pass
        if result.returncode == 0 or not refused or refusal_code not in {
            "active_process",
            "process_parse_depth_exceeded",
            "unsupported_process_wrapper",
            "unclassified_process",
            "containerized_process",
            "script_launched_cargo",
        } or not debug_file_exists:
            misses.append(
                f"{command!r}: returncode={result.returncode} refusal_code={refusal_code!r} "
                f"target_preserved={debug_file_exists}"
            )
    if misses:
        raise AssertionError("active-process parser silently missed v6 cargo launch forms: " + "; ".join(misses))


def assert_v6_red_active_process_wrapper_options_expose_cargo_pattern() -> None:
    owner = load_owner_module()
    cases = [
        "sudo -R /tmp cargo build",
        "sudo -c staff cargo build",
        "sudo -a pam cargo build",
        "env --split-string 'cargo build'",
        "env --split-string='cargo build'",
        "env -S'cargo build'",
        "env -Scargo build",
        "env -iS timeout 30 cargo build",
        "env -S timeout 30 cargo build",
        "env -iuLD_PRELOAD cargo build",
        "env -iu LD_PRELOAD cargo build",
        "env --block-signal cargo build",
        "env --block-signal=PIPE cargo build",
        "nice --adjustment 10 cargo build",
        "nice --adjustment=10 cargo build",
        "timeout -- 30 cargo build",
        "stdbuf -oL cargo build",
        "taskset -- 0 cargo build",
        "catchsegv cargo test",
        "podman run --rm rust:latest cargo build",
        "chroot /mnt cargo build",
        "bash -c 'VAR=val cargo build'",
        "bash -c 'eval cargo build'",
        "bash -c 'x=$(cargo build)'",
        "bash -c 'x=\"$(cargo build)\"'",
        "bash -c 'x=`cargo build`'",
        "find . -name Cargo.toml -exec cargo build \\;",
        "su user -c 'cargo build'",
        "runuser -u user -- cargo build",
        "sg staff -c 'cargo build'",
        "sudo -EHu root cargo build",
        "flock --timeout 5 /tmp/bolt.lock cargo build",
        "flock --timeout=5 /tmp/bolt.lock cargo build",
        "flock -- -lockfile cargo build",
        "flock /tmp/bolt.lock -c 'cargo build'",
        "flock -xc 'cargo build' /tmp/bolt.lock",
    ]
    misses: list[str] = []
    for command in cases:
        names = owner.command_process_names(command)
        matched = owner.matching_process_pattern(command, ["cargo"])
        if "cargo" not in names or matched != "cargo":
            misses.append(f"{command!r}: names={sorted(names)!r} matched={matched!r}")
    if misses:
        raise AssertionError("wrapper option parsing must expose wrapped cargo process names: " + "; ".join(misses))


def assert_v6_red_wrapper_end_of_options_does_not_overconsume_command_words() -> None:
    owner = load_owner_module()
    command = "nice -- -5 cargo build"
    names = owner.command_process_names(command)
    matched = owner.matching_process_pattern(command, ["cargo"])
    if "cargo" in names or matched == "cargo":
        raise AssertionError(f"{command!r} treated post-separator command argument as nice adjustment: names={sorted(names)!r} matched={matched!r}")


def assert_v6_red_wrapped_renamed_cargo_launches_are_classified() -> None:
    owner = load_owner_module()
    commands = [
        "/tmp/mycargo build",
        "/tmp/mycargo --target-dir=/tmp/raw-target test",
        "/tmp/mycargo --config=build.target-dir=/tmp/raw-target test",
        "time /tmp/c build",
        "time -apv /tmp/c test",
        "command /tmp/c test",
        "exec /tmp/c clean",
        "exec -lc /tmp/c clean",
        "exec -cla name /tmp/c clean",
        "nohup /tmp/c build",
        "docker exec container /tmp/c build",
        "podman run --rm rust:latest /tmp/c build",
        "chroot /mnt /tmp/c build",
        "setsid /tmp/c build",
        "setsid -fw /tmp/c build",
        "taskset -c 0 /tmp/c build",
        "taskset -- 0 /tmp/c build",
        "ionice -c2 /tmp/c build",
        "ionice -tc2 /tmp/c build",
        "chrt -r 10 /tmp/c build",
        "xargs /tmp/c build",
        "env >output.log /tmp/c build",
        "runuser -u user /tmp/c build",
        "mycargo build",
        "docker run --label my-label rust /tmp/c build",
        "docker run --unknown-opt=rust mycargo build",
        "python -c \"import os; os.system('/tmp/c build')\"",
        "python -c \"import subprocess; subprocess.run(['/tmp/mycargo', '--target-dir=/tmp/raw-target', 'test'])\"",
        "python -c \"import subprocess; subprocess.run(['/tmp/c', 'build'])\"",
        "python -c \"import subprocess; subprocess.run(args=['/tmp/c', 'build'])\"",
        "env -S \"/tmp/mycargo --target-dir=/tmp/raw-target test\"",
        "bash -c 'sleep 10 && /tmp/c test'",
        "bash -c '/tmp/mycargo --target-dir=/tmp/raw-target test'",
        "bash -c 'echo ok ; /tmp/c build'",
    ]
    misses: list[str] = []
    for command in commands:
        if not owner.command_may_be_renamed_cargo(command) or not owner.command_may_launch_rust(command):
            misses.append(
                f"{command!r}: renamed={owner.command_may_be_renamed_cargo(command)!r} "
                f"may_launch={owner.command_may_launch_rust(command)!r} "
                f"names={sorted(owner.command_process_names(command))!r}"
            )
    with tempfile.TemporaryDirectory() as tmp:
        cargo_target = pathlib.Path(tmp) / "cargo"
        cargo_target.write_text("#!/bin/sh\n", encoding="utf-8")
        renamed = pathlib.Path(tmp) / "mycargo"
        renamed.symlink_to(cargo_target)
        command = f"{renamed} test"
        if not owner.command_may_be_renamed_cargo(command) or not owner.command_may_launch_rust(command):
            misses.append(
                f"{command!r}: renamed={owner.command_may_be_renamed_cargo(command)!r} "
                f"may_launch={owner.command_may_launch_rust(command)!r} "
                f"names={sorted(owner.command_process_names(command))!r}"
            )
    if misses:
        raise AssertionError("wrapped renamed cargo launches must be classified: " + "; ".join(misses))


def assert_v6_red_active_process_parser_resolves_relative_manifest_scope() -> None:
    owner = load_owner_module()
    original_run = owner.subprocess.run
    original_process_cwd = owner.process_cwd

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        repo = root / "bolt-v2"
        sibling = root / "runner"
        target = repo / "target"
        repo.mkdir()
        sibling.mkdir()
        target.mkdir()

        def fake_run(*args: object, **kwargs: object) -> subprocess.CompletedProcess[str]:
            return subprocess.CompletedProcess(
                args=args,
                returncode=0,
                stdout=(
                    "424242 cargo build --manifest-path ../bolt-v2/Cargo.toml\n"
                    f"424243 cargo test --manifest-path $(echo ; echo {repo}/Cargo.toml)\n"
                ),
                stderr="",
            )

        owner.subprocess.run = fake_run
        owner.process_cwd = lambda pid: sibling
        try:
            related = owner.active_related_processes(
                repo,
                target,
                {"cache": {"active_process_patterns": ["cargo", "rustc", "rust_verification.py"]}},
            )
        finally:
            owner.subprocess.run = original_run
            owner.process_cwd = original_process_cwd

    if len(related) < 2:
        raise AssertionError(f"relative/substituted --manifest-path cargo processes outside repo cwd were ignored: {related!r}")


def assert_v6_red_active_process_scan_ignores_current_process_ancestor() -> None:
    owner = load_owner_module()
    original_run = owner.subprocess.run

    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "bolt-v2"
        target = repo / "target"
        repo.mkdir()
        target.mkdir()
        ancestor_pid = os.getppid()
        external_pid = ancestor_pid + 100000

        def fake_run(*args: object, **kwargs: object) -> subprocess.CompletedProcess[str]:
            return subprocess.CompletedProcess(
                args=args,
                returncode=0,
                stdout=(
                    f"{ancestor_pid} bash -c 'python3 scripts/rust_verification.py cargo --repo {repo} -- cargo clean'\n"
                    f"{external_pid} cargo test --manifest-path {repo / 'Cargo.toml'}\n"
                ),
                stderr="",
            )

        owner.subprocess.run = fake_run
        try:
            related = owner.active_related_processes(
                repo,
                target,
                {"cache": {"active_process_patterns": ["cargo", "rustc", "rust_verification.py"]}},
            )
        finally:
            owner.subprocess.run = original_run

    ancestor_entries = [entry for entry in related if entry.get("pid") == ancestor_pid]
    if ancestor_entries:
        raise AssertionError(f"current process ancestor was reported as active related process: {ancestor_entries!r}")
    if not any(entry.get("pid") == external_pid for entry in related):
        raise AssertionError(f"external cargo process was not reported: {related!r}")


def assert_managed_cargo_preflight_errors_are_structured() -> None:
    owner = load_owner_module()
    original_cache_status_payload = owner.cache_status_payload

    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp)
        write_policy_with_cache(repo)

        def failing_cache_status(_repo: pathlib.Path) -> dict[str, object]:
            raise OSError("disk preflight unavailable")

        args = type("Args", (), {"repo": str(repo), "args": ["build"]})()
        stderr = io.StringIO()
        owner.cache_status_payload = failing_cache_status
        old_break_glass = os.environ.get("BOLT_ALLOW_LOCAL_RUST")
        try:
            os.environ["BOLT_ALLOW_LOCAL_RUST"] = "1"
            with contextlib.redirect_stderr(stderr):
                result = owner.cmd_cargo(args)
        except OSError as exc:
            raise AssertionError(f"managed cargo preflight leaked exception: {exc}") from exc
        finally:
            owner.cache_status_payload = original_cache_status_payload
            if old_break_glass is None:
                os.environ.pop("BOLT_ALLOW_LOCAL_RUST", None)
            else:
                os.environ["BOLT_ALLOW_LOCAL_RUST"] = old_break_glass

    if result != 2:
        raise AssertionError(f"preflight error should refuse with exit 2, got {result}")
    try:
        payload = json.loads(stderr.getvalue())
    except json.JSONDecodeError as exc:
        raise AssertionError(f"preflight error did not emit structured JSON: {stderr.getvalue()!r}") from exc
    if payload.get("refusal_code") != "preflight_error" or payload.get("refused") is not True:
        raise AssertionError(f"unexpected preflight refusal payload: {payload!r}")


def assert_managed_cargo_clean_target_errors_are_structured() -> None:
    owner = load_owner_module()
    original_target_dir = owner.target_dir

    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp)
        write_policy_with_cache(repo)

        def failing_target_dir(_repo: pathlib.Path, _policy: dict[str, object] | None = None) -> pathlib.Path:
            raise OSError("managed target unavailable")

        args = type("Args", (), {"repo": str(repo), "args": ["clean"]})()
        stderr = io.StringIO()
        owner.target_dir = failing_target_dir
        try:
            with contextlib.redirect_stderr(stderr):
                result = owner.cmd_cargo(args)
        except OSError as exc:
            raise AssertionError(f"managed cargo clean leaked target-dir exception: {exc}") from exc
        finally:
            owner.target_dir = original_target_dir

    if result != 2:
        raise AssertionError(f"clean target error should refuse with exit 2, got {result}")
    try:
        payload = json.loads(stderr.getvalue())
    except json.JSONDecodeError as exc:
        raise AssertionError(f"clean target error did not emit structured JSON: {stderr.getvalue()!r}") from exc
    if payload.get("refusal_code") != "preflight_error" or payload.get("refused") is not True:
        raise AssertionError(f"unexpected clean target refusal payload: {payload!r}")


def assert_v6_red_active_process_parser_resolves_config_target_scope() -> None:
    owner = load_owner_module()
    original_run = owner.subprocess.run
    original_process_cwd = owner.process_cwd

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        repo = root / "bolt-v2"
        sibling = root / "runner"
        target = repo / "target"
        repo.mkdir()
        sibling.mkdir()
        target.mkdir()

        def fake_run(*args: object, **kwargs: object) -> subprocess.CompletedProcess[str]:
            return subprocess.CompletedProcess(
                args=args,
                returncode=0,
                stdout="424242 cargo build --config build.target-dir=../bolt-v2/target\n",
                stderr="",
            )

        owner.subprocess.run = fake_run
        owner.process_cwd = lambda pid: sibling
        try:
            related = owner.active_related_processes(
                repo,
                target,
                {"cache": {"active_process_patterns": ["cargo", "rustc", "rust_verification.py"]}},
            )
        finally:
            owner.subprocess.run = original_run
            owner.process_cwd = original_process_cwd

    if not related:
        raise AssertionError("relative --config build.target-dir cargo process outside repo cwd was ignored")


def assert_v6_red_active_process_parser_resolves_nested_shell_manifest_scope() -> None:
    owner = load_owner_module()
    original_run = owner.subprocess.run
    original_process_cwd = owner.process_cwd

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        repo = root / "bolt-v2"
        sibling = root / "runner"
        target = repo / "target"
        repo.mkdir()
        sibling.mkdir()
        target.mkdir()

        def fake_run(*args: object, **kwargs: object) -> subprocess.CompletedProcess[str]:
            return subprocess.CompletedProcess(
                args=args,
                returncode=0,
                stdout="424242 bash -c 'cargo build --manifest-path ../bolt-v2/Cargo.toml'\n",
                stderr="",
            )

        owner.subprocess.run = fake_run
        owner.process_cwd = lambda pid: sibling
        try:
            related = owner.active_related_processes(
                repo,
                target,
                {"cache": {"active_process_patterns": ["cargo", "rustc", "rust_verification.py"]}},
            )
        finally:
            owner.subprocess.run = original_run
            owner.process_cwd = original_process_cwd

    if not related:
        raise AssertionError("nested shell relative --manifest-path cargo process outside repo cwd was ignored")


def assert_v6_red_active_process_parser_uses_command_scope_without_cwd() -> None:
    result, debug_file_exists = cache_prune_for_visible_command(
        "timeout 30 cargo build --manifest-path {repo}/Cargo.toml",
        expose_cwd=False,
    )
    refused = False
    refusal_code = ""
    if result.stdout:
        try:
            payload = json.loads(result.stdout)
            refused = payload.get("refused") is True
            refusal_code = str(payload.get("refusal_code", ""))
        except json.JSONDecodeError:
            pass
    if result.returncode == 0 or not refused or refusal_code not in {"active_process", "unclassified_process"} or not debug_file_exists:
        raise AssertionError(
            "active-process parser must refuse scoped Rust wrapper commands even when cwd is unavailable: "
            f"returncode={result.returncode} refusal_code={refusal_code!r} target_preserved={debug_file_exists} "
            f"stdout={result.stdout!r} stderr={result.stderr!r}"
        )


def assert_v6_red_active_process_parser_fails_closed_for_unscoped_wrapped_rust_without_cwd() -> None:
    result, debug_file_exists = cache_prune_for_visible_command(
        "timeout 30 cargo build",
        expose_cwd=False,
    )
    refused = False
    refusal_code = ""
    if result.stdout:
        try:
            payload = json.loads(result.stdout)
            refused = payload.get("refused") is True
            refusal_code = str(payload.get("refusal_code", ""))
        except json.JSONDecodeError:
            pass
    if result.returncode == 0 or not refused or refusal_code != "insufficient_process_visibility" or not debug_file_exists:
        raise AssertionError(
            "active-process parser must fail closed for wrapper-launched Rust when cwd and command scope are unavailable: "
            f"returncode={result.returncode} refusal_code={refusal_code!r} target_preserved={debug_file_exists} "
            f"stdout={result.stdout!r} stderr={result.stderr!r}"
        )


def assert_v6_red_active_process_fails_closed_for_attached_semicolon_shell_chains() -> None:
    owner = load_owner_module()
    original_run = owner.subprocess.run
    original_process_cwd = owner.process_cwd

    def fake_run(*args: object, **kwargs: object) -> subprocess.CompletedProcess[str]:
        return subprocess.CompletedProcess(
            args=args,
            returncode=0,
            stdout="424242 bash -c 'if true; then /tmp/c build; fi'\n",
            stderr="",
        )

    owner.subprocess.run = fake_run
    owner.process_cwd = lambda pid: None
    try:
        try:
            owner.active_related_processes(
                pathlib.Path("/tmp/repo"),
                pathlib.Path("/tmp/repo/target"),
                {"cache": {"active_process_patterns": ["cargo", "rustc", "rust_verification.py"]}},
            )
        except owner.ProcessVisibilityError:
            return
        raise AssertionError("attached-semicolon renamed Cargo shell chain returned clean process visibility")
    finally:
        owner.subprocess.run = original_run
        owner.process_cwd = original_process_cwd


def assert_active_process_parser_uses_single_cwd_snapshot_per_pid() -> None:
    owner = load_owner_module()
    original_run = owner.subprocess.run
    original_getpid = owner.os.getpid
    original_process_cwd = owner.process_cwd
    calls: list[int] = []

    def fake_run(*args: object, **kwargs: object) -> subprocess.CompletedProcess[str]:
        return subprocess.CompletedProcess(
            args=args,
            returncode=0,
            stdout="424242 cargo build\n",
            stderr="",
        )

    def fake_process_cwd(pid: int) -> pathlib.Path | None:
        calls.append(pid)
        return None

    owner.subprocess.run = fake_run
    owner.os.getpid = lambda: 999999
    owner.process_cwd = fake_process_cwd
    try:
        try:
            owner.active_related_processes(
                pathlib.Path("/tmp/repo"),
                pathlib.Path("/tmp/repo/target"),
                {"cache": {"active_process_patterns": ["cargo", "rustc", "rust_verification.py"]}},
            )
        except owner.ProcessVisibilityError:
            pass
        else:
            raise AssertionError("unscoped cargo without cwd must fail closed")
    finally:
        owner.subprocess.run = original_run
        owner.os.getpid = original_getpid
        owner.process_cwd = original_process_cwd
    if calls != [424242]:
        raise AssertionError(f"process_cwd must be sampled once per pid, got {calls!r}")


def assert_v6_red_active_process_parser_ignores_unscoped_opaque_build_without_cwd() -> None:
    failures: list[str] = []
    for command in (
        "make build",
        "python -m build",
        "/usr/bin/make build",
        "/tmp/build-tool test",
        "timeout 30 ./my-script.sh --out-dir /tmp/output",
    ):
        result, debug_file_exists = cache_prune_for_visible_command(command, expose_cwd=False)
        if result.returncode != 0 or debug_file_exists:
            failures.append(
                f"{command!r}: returncode={result.returncode} target_removed={not debug_file_exists} "
                f"stdout={result.stdout!r} stderr={result.stderr!r}"
            )
    if failures:
        raise AssertionError(
            "active-process parser must not fail closed for unscoped generic build commands without cwd: "
            + "; ".join(failures)
        )


def assert_v6_red_active_process_parser_does_not_treat_trustd_as_rust() -> None:
    failures: list[str] = []
    for command in ("trustd", "bash -c 'echo trust'", "python trust_report.py", "grep build Cargo.toml"):
        result, debug_file_exists = cache_prune_for_visible_command(command, expose_cwd=False)
        if result.returncode != 0 or debug_file_exists:
            failures.append(
                f"{command!r}: returncode={result.returncode} target_removed={not debug_file_exists} "
                f"stdout={result.stdout!r} stderr={result.stderr!r}"
            )
    if failures:
        raise AssertionError(
            "active-process parser must not classify unrelated executable names containing 'rust' as Rust work: "
            + "; ".join(failures)
        )


def assert_v6_red_active_process_parser_does_not_treat_rust_named_scripts_as_rust() -> None:
    failures: list[str] = []
    for command in (
        "/tmp/cargo-build.sh test",
        "/tmp/cargo-build.PY test",
        "tests/cargo-tests.py build",
        "./rust-tests.sh check",
        "tools/clippy.bash --dry-run",
    ):
        result, debug_file_exists = cache_prune_for_visible_command(command, expose_cwd=False)
        if result.returncode != 0 or debug_file_exists:
            failures.append(
                f"{command!r}: returncode={result.returncode} target_removed={not debug_file_exists} "
                f"stdout={result.stdout!r} stderr={result.stderr!r}"
            )
    if failures:
        raise AssertionError(
            "active-process parser must not classify Rust-named helper scripts as Rust work: "
            + "; ".join(failures)
        )


def assert_v6_regression_cargo_process_names_stay_visible() -> None:
    owner = load_owner_module()
    commands = [
        "cargo +stable build",
        "cargo install cargo-nextest --locked",
        "cargo-clippy --workspace",
        "cargo-fmt --all",
        "cargo-nextest run",
        "nextest run",
        "rustc --version",
    ]
    missing = [
        (command, sorted(owner.command_process_names(command)))
        for command in commands
        if pathlib.Path(command.split()[0]).name not in owner.command_process_names(command)
    ]
    if missing:
        raise AssertionError(f"direct Rust tool process names must stay visible: {missing!r}")


def assert_managed_env_scrubs_build_target_dir_and_routes_target_dir() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)
        root_base = tmp_path / "rust-root"
        leaky_env = {
            "BOLT_ALLOW_LOCAL_RUST": "1",
            "BOLT_MANAGED_JUST": "1",
            "CARGO_BUILD_TARGET_DIR": str(tmp_path / "leaked-build-target"),
            "CARGO_BUILD_RUSTFLAGS": "--out-dir /tmp/raw-out",
            "CARGO_ENCODED_RUSTFLAGS": "--out-dir\x1f/tmp/raw-out",
            "CARGO_HOME": str(tmp_path / "leaked-cargo-home"),
            "CARGO_INCREMENTAL": "1",
            "CARGO_INSTALL_ROOT": str(tmp_path / "leaked-install-root"),
            "CARGO_TARGET_DIR": str(tmp_path / "leaked-target"),
            "CARGO_TARGET_TMPDIR": str(tmp_path / "leaked-target-tmp"),
            "RUSTC_WORKSPACE_WRAPPER": str(tmp_path / "leaked-workspace-wrapper"),
            "RUSTC_WRAPPER": str(tmp_path / "leaked-wrapper"),
            "RUSTFLAGS": "--out-dir /tmp/raw-out",
            "RUSTUP_HOME": str(tmp_path / "leaked-rustup-home"),
        }
        old_values = {key: os.environ.get(key) for key in [*leaky_env, "RUST_VERIFICATION_ROOT_BASE"]}
        try:
            os.environ.update(leaky_env)
            os.environ["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
            env = owner.managed_env(repo)
        finally:
            for key, value in old_values.items():
                if value is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = value
        expected_target = str(root_base / "bolt-v2" / "target")
        leaked = sorted(key for key in leaky_env if key in env and key != "CARGO_TARGET_DIR")
        if leaked:
            raise AssertionError(f"managed_env must scrub env-based routing/output overrides: {leaked!r}")
        if env.get("CARGO_TARGET_DIR") != expected_target:
            raise AssertionError((env.get("CARGO_TARGET_DIR"), expected_target))


def assert_managed_cargo_ignores_real_cargo_env_override() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, min_free_bytes=10, soft_limit_bytes=10**18)
        root_base = tmp_path / "rust-root"
        (root_base / "bolt-v2" / "target").mkdir(parents=True)

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        path_marker = tmp_path / "path-cargo-started"
        override_marker = tmp_path / "override-started"
        write_executable(
            bin_dir / "cargo",
            f"""#!/usr/bin/env bash
touch {path_marker}
printf '%s\\n' "$@"
exit 0
""",
        )
        write_executable(
            bin_dir / "override-cargo",
            f"""#!/usr/bin/env bash
touch {override_marker}
printf 'override used\\n'
exit 0
""",
        )

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["RUST_VERIFICATION_REAL_CARGO"] = str(bin_dir / "override-cargo")
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
        result = run_owner(["cargo", "--repo", str(repo), "--", "test"], env=env)
        if result.returncode != 0 or not path_marker.exists() or override_marker.exists():
            raise AssertionError(
                "managed cargo must ignore caller-provided RUST_VERIFICATION_REAL_CARGO: "
                f"returncode={result.returncode} path_started={path_marker.exists()} "
                f"override_started={override_marker.exists()} stdout={result.stdout!r} stderr={result.stderr!r}"
            )


def assert_v6_red_managed_cargo_clean_refuses_active_process() -> None:
    cases = [
        ["clean"],
        ["+stable", "clean"],
        ["--config", "net.offline=true", "clean"],
        ["--manifest-path", "Cargo.toml", "clean"],
        ["--target", "aarch64-unknown-linux-gnu", "clean"],
    ]
    failures: list[str] = []
    for cargo_args in cases:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            repo = tmp_path / "repo"
            repo.mkdir()
            write_policy_with_cache(repo, active_process_patterns=["cargo", "rustc", "rust_verification.py"])
            root_base = tmp_path / "rust-root"
            target = root_base / "bolt-v2" / "target"
            protected_file = target / "debug" / "protected.bin"
            protected_file.parent.mkdir(parents=True)
            protected_file.write_bytes(b"abc")

            bin_dir = tmp_path / "bin"
            bin_dir.mkdir()
            write_executable(
                bin_dir / "cargo",
                """#!/usr/bin/env bash
for arg in "$@"; do
  if [[ "$arg" == "clean" ]]; then
    rm -rf "$CARGO_TARGET_DIR/debug"
  fi
done
exit 0
""",
            )
            escaped_command = f"cargo build --manifest-path {repo / 'Cargo.toml'}"
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
            result = run_owner(["cargo", "--repo", str(repo), "--", *cargo_args], env=env)
            if result.returncode == 0 or not protected_file.exists():
                failures.append(
                    f"{cargo_args!r}: returncode={result.returncode} protected_exists={protected_file.exists()} "
                    f"stdout={result.stdout!r} stderr={result.stderr!r}"
                )
    if failures:
        raise AssertionError("managed cargo clean must refuse before deletion when related Rust processes are active: " + "; ".join(failures))

def assert_v6_red_disk_preflight_before_managed_cargo_and_run() -> None:
    failures: list[str] = []
    cases = [
        ("cargo", ["cargo", "{repo}", "--", "build"]),
        ("cargo", ["cargo", "{repo}", "--", "+stable", "build"]),
        ("cargo", ["cargo", "{repo}", "--", "--config", "net.offline=true", "build"]),
        ("cargo", ["cargo", "{repo}", "--", "bench", "--locked"]),
        ("cargo", ["cargo", "{repo}", "--", "doc", "--locked"]),
        ("cargo", ["cargo", "{repo}", "--", "fetch", "--locked"]),
        ("cargo", ["cargo", "{repo}", "--", "install", "--path", "."]),
        ("cargo", ["cargo", "{repo}", "--", "nextest", "archive", "--locked"]),
        ("cargo", ["cargo", "{repo}", "--", "nextest", "run", "--locked"]),
        ("cargo", ["cargo", "{repo}", "--", "run", "--release", "--bin", "bolt-v2", "--", "secrets", "check"]),
        ("run", ["run", "{repo}", "build"]),
        ("run", ["run", "{repo}", "clippy"]),
        ("run", ["run", "{repo}", "test"]),
    ]
    for command_name, args_template in cases:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            repo = tmp_path / "repo"
            repo.mkdir()
            write_policy_with_cache(repo, min_free_bytes=10**18, soft_limit_bytes=1)
            root_base = tmp_path / "rust-root"
            target = root_base / "bolt-v2" / "target"
            cache_file = target / "debug" / "large.bin"
            cache_file.parent.mkdir(parents=True)
            cache_file.write_bytes(b"abc")
            legacy_root = repo / "target"
            legacy_root.mkdir()
            (legacy_root / "legacy.bin").write_bytes(b"legacy")

            bin_dir = tmp_path / "bin"
            bin_dir.mkdir()
            marker = tmp_path / "started"
            write_executable(
                bin_dir / "cargo",
                f"""#!/usr/bin/env bash
touch {marker}
exit 0
""",
            )
            write_executable(
                bin_dir / "just",
                f"""#!/usr/bin/env bash
touch {marker}
exit 0
""",
            )

            env = os.environ.copy()
            env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
            env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
            args = [
                str(repo) if token == "{repo}" else token
                for token in args_template
            ]
            if command_name in ("cargo", "run"):
                args.insert(1, "--repo")
            result = run_owner(args, env=env)
            combined = f"{result.stdout}\n{result.stderr}".lower()
            if (
                result.returncode == 0
                or marker.exists()
                or "free" not in combined
                or "managed" not in combined
                or "legacy" not in combined
            ):
                failures.append(
                    f"{args!r}: returncode={result.returncode} runner_started={marker.exists()} "
                    f"stdout={result.stdout!r} stderr={result.stderr!r}"
                )
    if failures:
        raise AssertionError("managed heavy Rust commands must run disk preflight before execution: " + "; ".join(failures))


def assert_v6_red_nextest_archive_extraction_uses_exclusive_cache_lock() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, min_free_bytes=10, soft_limit_bytes=10**18)
        owner = load_owner_module()
        lock_modes: list[bool] = []

        class FakeLock:
            def __enter__(self) -> None:
                return None

            def __exit__(self, _exc_type: object, _exc: object, _tb: object) -> None:
                return None

        def fake_cache_lock(_policy: dict[str, object], *, exclusive: bool) -> FakeLock:
            lock_modes.append(exclusive)
            return FakeLock()

        owner.cache_lock = fake_cache_lock
        owner.disk_preflight_refusal_payload = lambda _repo, _policy: None
        owner.run_process = lambda _argv, repo, env: 0

        archive_args = types.SimpleNamespace(
            repo=str(repo),
            args=[
                "nextest",
                "run",
                "--archive-file",
                ".nextest-archive/nextest-archive.tar.zst",
                "--extract-to",
                str(tmp_path / "managed-target-parent"),
                "--extract-overwrite",
                "--partition",
                "count:1/4",
            ],
        )
        old_break_glass = os.environ.get("BOLT_ALLOW_LOCAL_RUST")
        try:
            os.environ["BOLT_ALLOW_LOCAL_RUST"] = "1"
            archive_result = owner.cmd_cargo(archive_args)
            normal_args = types.SimpleNamespace(repo=str(repo), args=["nextest", "run", "--locked"])
            normal_result = owner.cmd_cargo(normal_args)
            configured_archive_args = types.SimpleNamespace(
                repo=str(repo),
                args=[
                    "nextest",
                    "--config-file",
                    ".config/nextest.toml",
                    "run",
                    "--archive-file",
                    ".nextest-archive/nextest-archive.tar.zst",
                    "--extract-to",
                    str(tmp_path / "configured-managed-target-parent"),
                ],
            )
            configured_archive_result = owner.cmd_cargo(configured_archive_args)
            manifest_archive_args = types.SimpleNamespace(
                repo=str(repo),
                args=[
                    "nextest",
                    "--manifest-path",
                    "Cargo.toml",
                    "run",
                    "--archive-file",
                    ".nextest-archive/nextest-archive.tar.zst",
                    "--extract-to",
                    str(tmp_path / "manifest-managed-target-parent"),
                ],
            )
            manifest_archive_result = owner.cmd_cargo(manifest_archive_args)
            separator_args = types.SimpleNamespace(
                repo=str(repo),
                args=[
                    "nextest",
                    "run",
                    "--",
                    "--archive-file",
                    ".nextest-archive/nextest-archive.tar.zst",
                    "--extract-to",
                    str(tmp_path / "test-binary-output"),
                ],
            )
            separator_result = owner.cmd_cargo(separator_args)
            managed_run_archive_args = types.SimpleNamespace(
                repo=str(repo),
                command="test",
                args=[
                    "--archive-file",
                    ".nextest-archive/nextest-archive.tar.zst",
                    "--extract-to",
                    str(tmp_path / "managed-target-parent"),
                    "--extract-overwrite",
                    "--partition",
                    "count:1/4",
                ],
                args_separator=False,
            )
            managed_run_archive_result = owner.cmd_run(managed_run_archive_args)
            managed_run_separator_args = types.SimpleNamespace(
                repo=str(repo),
                command="test",
                args=[
                    "--archive-file",
                    ".nextest-archive/nextest-archive.tar.zst",
                    "--extract-to",
                    str(tmp_path / "test-binary-output"),
                ],
                args_separator=True,
            )
            managed_run_separator_result = owner.cmd_run(managed_run_separator_args)
        finally:
            if old_break_glass is None:
                os.environ.pop("BOLT_ALLOW_LOCAL_RUST", None)
            else:
                os.environ["BOLT_ALLOW_LOCAL_RUST"] = old_break_glass
        if (
            archive_result != 0
            or normal_result != 0
            or configured_archive_result != 0
            or manifest_archive_result != 0
            or separator_result != 0
            or managed_run_archive_result != 0
            or managed_run_separator_result != 0
            or lock_modes != [True, False, True, True, False, True, False]
        ):
            raise AssertionError(
                "nextest archive extraction must serialize on the managed cache lock while ordinary nextest "
                "and post-separator test args remain shared: "
                f"archive_result={archive_result} normal_result={normal_result} "
                f"configured_archive_result={configured_archive_result} "
                f"manifest_archive_result={manifest_archive_result} "
                f"separator_result={separator_result} "
                f"managed_run_archive_result={managed_run_archive_result} "
                f"managed_run_separator_result={managed_run_separator_result} "
                f"lock_modes={lock_modes!r}"
            )


def assert_managed_cargo_clean_keeps_disk_pressure_escape_hatch() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(
            repo,
            active_process_patterns=["cache-prune-sentinel-never-present"],
            min_free_bytes=10**18,
            soft_limit_bytes=1,
        )
        root_base = tmp_path / "rust-root"
        target = root_base / "bolt-v2" / "target"
        (target / "debug").mkdir(parents=True)
        (target / "debug" / "large.bin").write_bytes(b"abc")
        legacy_root = repo / "target"
        legacy_root.mkdir()
        (legacy_root / "legacy.bin").write_bytes(b"legacy")

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        marker = tmp_path / "started-clean"
        write_executable(
            bin_dir / "cargo",
            f"""#!/usr/bin/env bash
touch {marker}
exit 0
""",
        )
        write_executable(
            bin_dir / "ps",
            """#!/usr/bin/env bash
exit 0
""",
        )

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
        result = run_owner(["cargo", "--repo", str(repo), "--", "clean"], env=env)
        if result.returncode != 0 or not marker.exists():
            raise AssertionError(
                "managed cargo clean must remain available under disk/cache pressure when no related process is active: "
                f"returncode={result.returncode} started={marker.exists()} stdout={result.stdout!r} stderr={result.stderr!r}"
            )


def assert_managed_cargo_rejects_alias_subcommands() -> None:
    failures: list[str] = []
    cases: list[tuple[str, str | None]] = [(alias, None) for alias in ["b", "c", "d", "r", "t"]]
    cases.append(("wipe", 'wipe = "clean"\n'))
    cases.append(("global-wipe", None))
    for alias, cargo_config_aliases in cases:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            repo = tmp_path / "repo"
            repo.mkdir()
            write_policy_with_cache(repo, min_free_bytes=10, soft_limit_bytes=10**18)
            if cargo_config_aliases is not None:
                (repo / ".cargo").mkdir()
                (repo / ".cargo" / "config.toml").write_text(
                    f"[alias]\n{cargo_config_aliases}",
                    encoding="utf-8",
                )
            cargo_home = tmp_path / "cargo-home"
            if alias == "global-wipe":
                cargo_home.mkdir()
                (cargo_home / "config.toml").write_text('[alias]\nglobal-wipe = "clean"\n', encoding="utf-8")
            root_base = tmp_path / "rust-root"
            (root_base / "bolt-v2" / "target").mkdir(parents=True)

            bin_dir = tmp_path / "bin"
            bin_dir.mkdir()
            marker = tmp_path / "started"
            write_executable(
                bin_dir / "cargo",
                f"""#!/usr/bin/env bash
touch {marker}
exit 0
""",
            )

            env = os.environ.copy()
            env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
            env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
            if alias == "global-wipe":
                env["CARGO_HOME"] = str(cargo_home)
            result = run_owner(["cargo", "--repo", str(repo), "--", alias], env=env)
            combined = f"{result.stdout}\n{result.stderr}".lower()
            if result.returncode == 0 or marker.exists() or "alias" not in combined or "managed" not in combined:
                failures.append(
                    f"{alias!r}: returncode={result.returncode} cargo_started={marker.exists()} "
                    f"stdout={result.stdout!r} stderr={result.stderr!r}"
                )
    if failures:
        raise AssertionError("managed cargo must reject alias subcommands before invoking Cargo: " + "; ".join(failures))


def assert_v6_red_managed_cargo_rejects_target_routing_overrides() -> None:
    failures: list[str] = []
    cases = [
        ["test", "--target-dir", "/tmp/raw-target"],
        ["test", "--target-dir=/tmp/raw-target"],
        ["test", "--config", "build.target-dir=/tmp/raw-target"],
        ["test", "--config=build.target-dir=/tmp/raw-target"],
        ["test", "--config", 'build = { target-dir = "/tmp/raw-target" }'],
        ["test", "--config", 'build = { "target\\u002Ddir" = "/tmp/raw-target" }'],
        ["test", "--config", '[build]\ntarget-dir = "/tmp/raw-target"'],
        ["test", "--config", 'build.rustflags = ["--out-dir", "/tmp/raw-out"]'],
        ["test", "--config", 'build = { rustflags = ["--artifact-dir", "/tmp/raw-artifacts"] }'],
        ["rustc", "--", "--out-dir", "/tmp/raw-out"],
        ["rustc", "--", "--artifact-dir", "/tmp/raw-artifacts"],
        ["install", "ripgrep", "--root", "/tmp/install-root"],
        ["install", "ripgrep", "--root=/tmp/install-root"],
    ]
    for cargo_args in cases:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            repo = tmp_path / "repo"
            repo.mkdir()
            write_policy_with_cache(repo, min_free_bytes=10, soft_limit_bytes=10**18)
            root_base = tmp_path / "rust-root"
            target = root_base / "bolt-v2" / "target"
            target.mkdir(parents=True)

            bin_dir = tmp_path / "bin"
            bin_dir.mkdir()
            marker = tmp_path / "started"
            captured = tmp_path / "captured-args"
            write_executable(
                bin_dir / "cargo",
                f"""#!/usr/bin/env bash
touch {marker}
printf '%s\\n' "$@" > {captured}
exit 0
""",
            )

            env = os.environ.copy()
            env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
            env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
            result = run_owner(["cargo", "--repo", str(repo), "--", *cargo_args], env=env)
            combined = f"{result.stdout}\n{result.stderr}".lower()
            if result.returncode == 0 or marker.exists() or "target" not in combined or "routing" not in combined:
                failures.append(
                    f"{cargo_args!r}: returncode={result.returncode} runner_started={marker.exists()} "
                    f"captured={captured.read_text() if captured.exists() else ''!r} "
                    f"stdout={result.stdout!r} stderr={result.stderr!r}"
                )
    if failures:
        raise AssertionError("managed cargo must reject target/output routing overrides before invoking Cargo: " + "; ".join(failures))


def assert_managed_cargo_rejects_config_file_target_routing_override() -> None:
    failures: list[str] = []
    cases = [
        ["test", "--config", "{config_file}"],
        ["test", "--config={config_file}"],
    ]
    for cargo_args_template in cases:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            repo = tmp_path / "repo"
            repo.mkdir()
            write_policy_with_cache(repo, min_free_bytes=10, soft_limit_bytes=10**18)
            root_base = tmp_path / "rust-root"
            (root_base / "bolt-v2" / "target").mkdir(parents=True)
            config_file = tmp_path / "cargo-config.toml"
            config_file.write_text('[build]\ntarget-dir = "/tmp/raw-target"\n', encoding="utf-8")

            bin_dir = tmp_path / "bin"
            bin_dir.mkdir()
            marker = tmp_path / "started"
            write_executable(
                bin_dir / "cargo",
                f"""#!/usr/bin/env bash
touch {marker}
exit 0
""",
            )

            env = os.environ.copy()
            env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
            env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
            cargo_args = [
                str(config_file) if token == "{config_file}" else token.replace("{config_file}", str(config_file))
                for token in cargo_args_template
            ]
            result = run_owner(["cargo", "--repo", str(repo), "--", *cargo_args], env=env)
            combined = f"{result.stdout}\n{result.stderr}".lower()
            if result.returncode == 0 or marker.exists() or "config" not in combined or "routing" not in combined:
                failures.append(
                    f"{cargo_args!r}: returncode={result.returncode} runner_started={marker.exists()} "
                    f"stdout={result.stdout!r} stderr={result.stderr!r}"
                )
    if failures:
        raise AssertionError("managed cargo must reject path-style cargo --config before invoking Cargo: " + "; ".join(failures))


def assert_v6_red_managed_run_rejects_target_routing_overrides() -> None:
    failures: list[str] = []
    cases = [
        ["test", "--target-dir", "/tmp/raw-target"],
        ["test", "--target-dir=/tmp/raw-target"],
        ["test", "--config", "build.target-dir=/tmp/raw-target"],
        ["test", "--config", 'build = { target-dir = "/tmp/raw-target" }'],
        ["test", "--config", '[build]\ntarget-dir = "/tmp/raw-target"'],
        ["test", "--config", 'build.rustflags = ["--out-dir", "/tmp/raw-out"]'],
    ]
    for run_args in cases:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            repo = tmp_path / "repo"
            repo.mkdir()
            write_policy_with_cache(repo, min_free_bytes=10, soft_limit_bytes=10**18)
            root_base = tmp_path / "rust-root"
            (root_base / "bolt-v2" / "target").mkdir(parents=True)

            bin_dir = tmp_path / "bin"
            bin_dir.mkdir()
            marker = tmp_path / "started"
            write_executable(
                bin_dir / "just",
                f"""#!/usr/bin/env bash
touch {marker}
exit 0
""",
            )

            env = os.environ.copy()
            env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
            env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
            result = run_owner(["run", "--repo", str(repo), *run_args], env=env)
            combined = f"{result.stdout}\n{result.stderr}".lower()
            if result.returncode == 0 or marker.exists() or "target" not in combined or "routing" not in combined:
                failures.append(
                    f"{run_args!r}: returncode={result.returncode} just_started={marker.exists()} "
                    f"stdout={result.stdout!r} stderr={result.stderr!r}"
                )
    if failures:
        raise AssertionError("managed run must reject target/output routing overrides before invoking just: " + "; ".join(failures))


def assert_managed_run_authorizes_private_just_recipes() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, min_free_bytes=10, soft_limit_bytes=10**18)
        root_base = tmp_path / "rust-root"
        (root_base / "bolt-v2" / "target").mkdir(parents=True)

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        marker = tmp_path / "managed-just-env"
        write_executable(
            bin_dir / "just",
            f"""#!/usr/bin/env bash
printf '%s\\n' "${{BOLT_MANAGED_JUST:-}}" > {marker}
exit 0
""",
        )

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
        result = run_owner(["run", "--repo", str(repo), "clippy"], env=env)
        if result.returncode != 0 or marker.read_text(encoding="utf-8").strip() != "1":
            raise AssertionError(
                "managed run must mark private just recipes as wrapper-authorized: "
                f"returncode={result.returncode} env={marker.read_text(encoding='utf-8') if marker.exists() else '<missing>'!r} "
                f"stdout={result.stdout!r} stderr={result.stderr!r}"
            )


def assert_direct_private_managed_just_recipes_require_wrapper_env() -> None:
    failures: list[str] = []
    recipes = ["managed-clippy", "managed-test", "managed-build"]
    for recipe in recipes:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            bin_dir = tmp_path / "bin"
            bin_dir.mkdir()
            marker = tmp_path / "cargo-started"
            write_executable(
                bin_dir / "cargo",
                f"""#!/usr/bin/env bash
touch {marker}
exit 0
""",
            )
            env = os.environ.copy()
            env.pop("BOLT_MANAGED_JUST", None)
            env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
            result = subprocess.run(
                [
                    "just",
                    "-f",
                    str(REPO_ROOT / "justfile"),
                    "--working-directory",
                    str(REPO_ROOT),
                    recipe,
                ],
                cwd=REPO_ROOT,
                env=env,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            combined = f"{result.stdout}\n{result.stderr}".lower()
            if result.returncode == 0 or marker.exists() or "managed" not in combined or "rust_verification.py" not in combined:
                failures.append(
                    f"{recipe}: returncode={result.returncode} cargo_started={marker.exists()} "
                    f"stdout={result.stdout!r} stderr={result.stderr!r}"
                )
    if failures:
        raise AssertionError("direct private managed just recipes must refuse outside the wrapper: " + "; ".join(failures))


def assert_v6_red_managed_cargo_allows_post_separator_binary_args() -> None:
    allowed_cases = [
        ["run", "--release", "--bin", "bolt-v2", "--", "--root", "/tmp/binary-arg"],
        ["test", "--locked", "--", "--out-dir", "/tmp/test-arg"],
        ["nextest", "run", "--", "--target-dir", "/tmp/test-arg"],
        ["--manifest-path", "Cargo.toml", "test", "--", "--target-dir", "/tmp/test-arg"],
        ["--profile", "dev", "test", "--", "--target-dir", "/tmp/test-arg"],
        ["bench", "--locked", "--", "--artifact-dir", "/tmp/bench-arg"],
    ]
    failures: list[str] = []
    for cargo_args in allowed_cases:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            repo = tmp_path / "repo"
            repo.mkdir()
            write_policy_with_cache(repo, min_free_bytes=10, soft_limit_bytes=10**18)
            root_base = tmp_path / "rust-root"
            (root_base / "bolt-v2" / "target").mkdir(parents=True)

            bin_dir = tmp_path / "bin"
            bin_dir.mkdir()
            marker = tmp_path / "started"
            captured = tmp_path / "captured-args"
            write_executable(
                bin_dir / "cargo",
                f"""#!/usr/bin/env bash
touch {marker}
printf '%s\\n' "$@" > {captured}
exit 0
""",
            )

            env = os.environ.copy()
            env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
            env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
            result = run_owner(["cargo", "--repo", str(repo), "--", *cargo_args], env=env)
            if result.returncode != 0 or not marker.exists():
                failures.append(
                    f"{cargo_args!r}: returncode={result.returncode} runner_started={marker.exists()} "
                    f"captured={captured.read_text() if captured.exists() else ''!r} "
                    f"stdout={result.stdout!r} stderr={result.stderr!r}"
                )
    if failures:
        raise AssertionError("managed cargo must not reject binary/test args after Cargo separator: " + "; ".join(failures))


def assert_v6_red_managed_run_allows_post_separator_binary_args() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy_with_cache(repo, min_free_bytes=10, soft_limit_bytes=10**18)
        root_base = tmp_path / "rust-root"
        (root_base / "bolt-v2" / "target").mkdir(parents=True)

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        marker = tmp_path / "just-started"
        captured = tmp_path / "just-args"
        write_executable(
            bin_dir / "just",
            f"""#!/usr/bin/env bash
touch {marker}
printf '%s\\n' "$@" > {captured}
exit 0
""",
        )

        env = os.environ.copy()
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
        result = run_owner(
            ["run", "--repo", str(repo), "test", "--", "--target-dir", "/tmp/valid-test-binary-arg"],
            env=env,
        )
        captured_args = captured.read_text().splitlines() if captured.exists() else []
        expected_tail = ["managed-test", "--", "--target-dir", "/tmp/valid-test-binary-arg"]
        if result.returncode != 0 or not marker.exists() or captured_args[-4:] != expected_tail:
            raise AssertionError(
                "managed run test must preserve Cargo separator semantics for test-binary args: "
                f"returncode={result.returncode} just_started={marker.exists()} "
                f"captured={captured_args!r} "
                f"stdout={result.stdout!r} stderr={result.stderr!r}"
            )


def assert_v6_red_policy_gaps() -> None:
    checks = [
        assert_v6_red_active_process_parser_gaps,
        assert_v6_red_active_process_wrapper_options_expose_cargo_pattern,
        assert_v6_red_wrapper_end_of_options_does_not_overconsume_command_words,
        assert_v6_red_wrapped_renamed_cargo_launches_are_classified,
        assert_v6_red_active_process_parser_resolves_relative_manifest_scope,
        assert_v6_red_active_process_scan_ignores_current_process_ancestor,
        assert_v6_red_active_process_parser_resolves_config_target_scope,
        assert_v6_red_active_process_parser_resolves_nested_shell_manifest_scope,
        assert_v6_red_active_process_parser_uses_command_scope_without_cwd,
        assert_v6_red_active_process_parser_fails_closed_for_unscoped_wrapped_rust_without_cwd,
        assert_v6_red_active_process_fails_closed_for_attached_semicolon_shell_chains,
        assert_active_process_parser_uses_single_cwd_snapshot_per_pid,
        assert_v6_red_active_process_parser_ignores_unscoped_opaque_build_without_cwd,
        assert_v6_red_active_process_parser_does_not_treat_trustd_as_rust,
        assert_v6_red_active_process_parser_does_not_treat_rust_named_scripts_as_rust,
        assert_v6_red_managed_cargo_clean_refuses_active_process,
        assert_v6_red_disk_preflight_before_managed_cargo_and_run,
        assert_v6_red_nextest_archive_extraction_uses_exclusive_cache_lock,
        assert_managed_cargo_clean_keeps_disk_pressure_escape_hatch,
        assert_managed_cargo_rejects_alias_subcommands,
        assert_v6_red_managed_cargo_rejects_target_routing_overrides,
        assert_managed_cargo_rejects_config_file_target_routing_override,
        assert_v6_red_managed_run_rejects_target_routing_overrides,
        assert_managed_run_authorizes_private_just_recipes,
        assert_direct_private_managed_just_recipes_require_wrapper_env,
        assert_v6_red_managed_cargo_allows_post_separator_binary_args,
        assert_v6_red_managed_run_allows_post_separator_binary_args,
        assert_managed_cargo_ignores_real_cargo_env_override,
    ]
    failures: list[str] = []
    for check in checks:
        try:
            check()
        except AssertionError as exc:
            failures.append(f"{check.__name__}: {exc}")
    if failures:
        raise AssertionError("v6 RED policy coverage failures: " + " | ".join(failures))


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
    assert_cache_prune_age_only_apply_prunes_stale_candidates_without_pressure()
    assert_cache_prune_age_only_apply_refuses_active_related_process()
    assert_cache_prune_age_only_error_refusals_report_age_only()
    assert_cache_prune_multiple_repos_attempts_all_namespaces_after_refusal()
    assert_cache_prune_multiple_repos_attempts_all_after_unexpected_exception()
    assert_cache_prune_apply_refuses_active_related_process()
    assert_cache_prune_apply_refuses_active_related_process_by_cwd()
    assert_cache_prune_active_process_scan_uses_portable_ps_columns()
    assert_managed_cargo_preflight_errors_are_structured()
    assert_managed_cargo_clean_target_errors_are_structured()
    assert_cache_prune_apply_ignores_unrelated_process_by_lsof_cwd()
    assert_cache_prune_skips_unrelated_process_before_cwd_lookup()
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
    assert_all_managed_cache_policies_are_bounded_to_30_gib()
    assert_cache_prune_recipe_sweeps_all_managed_cache_namespaces()
    assert_v6_regression_cargo_process_names_stay_visible()
    assert_managed_env_scrubs_build_target_dir_and_routes_target_dir()
    assert_v6_red_policy_gaps()
    print("OK: Rust verification cache retention self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
